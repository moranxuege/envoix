use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result as AnyResult, anyhow};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use envoix_error::CoreError;
use iroh::SecretKey;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::SessionError;

const IDENTITY_FILE_VERSION: u32 = 1;
static IDENTITY_TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

/// iroh endpoint identity policy.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum IdentityConfig {
    /// Generate a fresh endpoint identity for each endpoint bind.
    #[default]
    Ephemeral,
    /// Reuse one process-memory identity across endpoint rebinds.
    Memory(MemoryIdentity),
    /// Load an existing identity from this file, creating one if missing.
    Persistent(PathBuf),
}

/// Secret endpoint identity kept in memory without exposing key bytes through
/// the public API or `Debug` output.
#[derive(Clone, Eq, PartialEq)]
pub struct MemoryIdentity([u8; 32]);

impl MemoryIdentity {
    pub fn generate() -> Self {
        Self(SecretKey::generate().to_bytes())
    }
}

impl std::fmt::Debug for MemoryIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MemoryIdentity([REDACTED])")
    }
}

pub(crate) async fn load_secret_key(identity: &IdentityConfig) -> Result<SecretKey, SessionError> {
    match identity {
        IdentityConfig::Ephemeral => Ok(SecretKey::generate()),
        IdentityConfig::Memory(identity) => Ok(SecretKey::from_bytes(&identity.0)),
        IdentityConfig::Persistent(path) => load_or_create_identity(path)
            .await
            .map_err(|error| CoreError::InvalidInput(error.to_string())),
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct IdentityFile {
    version: u32,
    secret_key: String,
}

async fn load_or_create_identity(path: &Path) -> AnyResult<SecretKey> {
    if fs::try_exists(path)
        .await
        .context("failed to check persistent identity file")?
    {
        return read_identity(path).await;
    }

    let secret_key = SecretKey::generate();
    let file = IdentityFile {
        version: IDENTITY_FILE_VERSION,
        secret_key: URL_SAFE_NO_PAD.encode(secret_key.to_bytes()),
    };
    let text = serde_json::to_vec_pretty(&file).context("failed to encode identity file")?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .context("failed to create persistent identity directory")?;
    }
    match write_new_identity_file(path, &text).await {
        Ok(()) => Ok(secret_key),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another endpoint won the atomic first-use creation race. Reuse
            // that identity instead of failing one of the concurrent starts.
            read_identity(path)
                .await
                .context("failed to read concurrently created persistent identity")
        }
        Err(error) => Err(error).context("failed to create persistent identity file"),
    }
}

async fn read_identity(path: &Path) -> AnyResult<SecretKey> {
    let text = fs::read(path)
        .await
        .context("failed to read persistent identity file")?;
    let file: IdentityFile =
        serde_json::from_slice(&text).context("identity file is not valid JSON")?;
    if file.version != IDENTITY_FILE_VERSION {
        return Err(anyhow!(
            "unsupported identity file version {}",
            file.version
        ));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(file.secret_key.as_bytes())
        .context("identity secret is not valid base64url")?;
    let bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("identity secret must be 32 bytes"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

#[cfg(unix)]
async fn write_new_identity_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary_path = temporary_identity_path(path);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    publish_identity_file(path, &temporary_path, options, bytes).await
}

#[cfg(not(unix))]
async fn write_new_identity_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary_path = temporary_identity_path(path);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    publish_identity_file(path, &temporary_path, options, bytes).await
}

fn temporary_identity_path(path: &Path) -> PathBuf {
    let counter = IDENTITY_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("identity");
    path.with_file_name(format!(".{name}.{}.{counter}.tmp", std::process::id()))
}

async fn publish_identity_file(
    final_path: &Path,
    temporary_path: &Path,
    options: fs::OpenOptions,
    bytes: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let result = async {
        let mut file = options.open(temporary_path).await?;
        file.write_all(bytes).await?;
        file.sync_all().await?;
        drop(file);
        publish_identity_no_replace(temporary_path, final_path).await
    }
    .await;
    let _ = fs::remove_file(temporary_path).await;
    result
}

#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "visionos",
    target_os = "watchos",
    target_os = "redox",
))]
async fn publish_identity_no_replace(
    temporary_path: &Path,
    final_path: &Path,
) -> std::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;

    match renameat_with(CWD, temporary_path, CWD, final_path, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        // Older kernels/filesystems can lack rename-with-flags. Preserve the
        // prior no-replace fallback where hard links are permitted.
        Err(Errno::NOSYS | Errno::INVAL) => fs::hard_link(temporary_path, final_path).await,
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "visionos",
    target_os = "watchos",
    target_os = "redox",
)))]
async fn publish_identity_no_replace(
    temporary_path: &Path,
    final_path: &Path,
) -> std::io::Result<()> {
    // A hard link publishes the already-complete inode without replacing an
    // identity another concurrent endpoint has just installed.
    fs::hard_link(temporary_path, final_path).await
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;
