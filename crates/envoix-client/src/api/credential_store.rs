//! Minimal protected-credential backend for desktop development.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// Owner-only file storage for versioned opaque credentials.
///
/// Mobile applications use their platform Keychain/Keystore implementations;
/// this backend deliberately has no CLI surface.
pub struct DesktopCredentialStore {
    directory: PathBuf,
}

impl DesktopCredentialStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn put(&self, credential_ref: &str, opaque_credential: &[u8]) -> io::Result<()> {
        let target = self.path(credential_ref)?;
        fs::create_dir_all(&self.directory)?;
        #[cfg(unix)]
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;

        let temporary = self
            .directory
            .join(format!(".{credential_ref}.{}.tmp", std::process::id()));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&temporary)?;
            file.write_all(opaque_credential)?;
            file.sync_all()?;
            replace_file(&temporary, &target)?;
            #[cfg(unix)]
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn get(&self, credential_ref: &str) -> io::Result<Option<Vec<u8>>> {
        let path = self.path(credential_ref)?;
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        #[cfg(unix)]
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "credential file is not owner-only",
            ));
        }
        #[cfg(not(unix))]
        let _ = metadata;
        Ok(Some(fs::read(path)?))
    }

    pub fn delete(&self, credential_ref: &str) -> io::Result<()> {
        match fs::remove_file(self.path(credential_ref)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn path(&self, credential_ref: &str) -> io::Result<PathBuf> {
        if credential_ref.is_empty()
            || credential_ref.len() > 128
            || !credential_ref
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "credential reference is invalid",
            ));
        }
        Ok(self.directory.join(Path::new(credential_ref)))
    }
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, target: &Path) -> io::Result<()> {
    fs::rename(temporary, target)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, target: &Path) -> io::Result<()> {
    tempfile::TempPath::try_from_path(temporary.to_path_buf())?
        .persist(target)
        .map_err(io::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_delete_and_reject_path_traversal() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = DesktopCredentialStore::new(directory.path());

        store.put("reference_1", b"opaque").expect("store");
        assert_eq!(
            store.get("reference_1").expect("load").as_deref(),
            Some(b"opaque".as_slice())
        );
        store
            .put("reference_1", b"rotated")
            .expect("replace credential");
        assert_eq!(
            store
                .get("reference_1")
                .expect("load replacement")
                .as_deref(),
            Some(b"rotated".as_slice())
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(directory.path().join("reference_1"))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        store.delete("reference_1").expect("delete");
        assert_eq!(store.get("reference_1").expect("missing"), None);
        assert!(store.put("../outside", b"no").is_err());
    }
}
