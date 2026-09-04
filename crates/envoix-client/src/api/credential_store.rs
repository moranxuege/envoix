//! Desktop protected-credential backend.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(windows)]
use zeroize::{Zeroize, Zeroizing};

use crate::ports::{MAX_VAULT_SECRET_BYTES, PlatformPortError, SecretBytes, SecureVaultPort};
use crate::storage::VaultReference;

const MAX_CREDENTIAL_FILE_BYTES: u64 = 1024 * 1024;
#[cfg(windows)]
const WINDOWS_CREDENTIAL_MAGIC: &[u8] = b"ENVW\x01";
#[cfg(windows)]
const WINDOWS_CREDENTIAL_ENTROPY_DOMAIN: &[u8] = b"envoix/windows-credential/v1\0";
#[cfg(windows)]
const WINDOWS_CREDENTIAL_DIGEST_DOMAIN: &[u8] = b"envoix/windows-credential-integrity/v1\0";
#[cfg(windows)]
const WINDOWS_CREDENTIAL_DIGEST_BYTES: usize = 32;

/// Storage for versioned opaque credentials.
///
/// Windows persists user-scoped DPAPI ciphertext. Unix desktop development and
/// WSL use owner-only files. This backend deliberately has no CLI surface.
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
        if opaque_credential.len() > MAX_VAULT_SECRET_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "credential data exceeds the supported size",
            ));
        }
        #[cfg(windows)]
        let protected = protect_windows_credential(credential_ref, opaque_credential)?;
        #[cfg(windows)]
        let opaque_credential = protected.as_slice();
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
        if metadata.len() > MAX_CREDENTIAL_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "credential file exceeds the supported size",
            ));
        }
        let bytes = fs::read(path)?;
        #[cfg(windows)]
        let bytes = unprotect_windows_credential(credential_ref, &bytes)?;
        Ok(Some(bytes))
    }

    pub fn contains(&self, credential_ref: &str) -> io::Result<bool> {
        let path = self.path(credential_ref)?;
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
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
        Ok(true)
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

#[cfg(windows)]
fn protect_windows_credential(credential_ref: &str, credential: &[u8]) -> io::Result<Vec<u8>> {
    use std::ptr;

    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };

    let mut plaintext = Zeroizing::new(Vec::with_capacity(
        credential.len() + WINDOWS_CREDENTIAL_DIGEST_BYTES,
    ));
    plaintext.extend_from_slice(credential);
    plaintext.extend_from_slice(windows_credential_digest(credential_ref, credential).as_bytes());
    let input = windows_data_blob(&plaintext)?;
    let entropy_bytes = windows_credential_entropy(credential_ref);
    let entropy = windows_data_blob(&entropy_bytes)?;
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // SAFETY: input and entropy point to immutable live slices for the call;
    // output points to writable DATA_BLOB storage. UI is explicitly disabled.
    if unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            &entropy,
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let output = LocalDataBlob::new(output)?;
    let output_len = u64::try_from(output.as_slice().len())
        .map_err(|_| io::Error::other("protected credential is too large"))?;
    if output_len + WINDOWS_CREDENTIAL_MAGIC.len() as u64 > MAX_CREDENTIAL_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "protected credential exceeds the supported size",
        ));
    }
    let mut protected =
        Vec::with_capacity(WINDOWS_CREDENTIAL_MAGIC.len() + output.as_slice().len());
    protected.extend_from_slice(WINDOWS_CREDENTIAL_MAGIC);
    protected.extend_from_slice(output.as_slice());
    Ok(protected)
}

#[cfg(windows)]
fn unprotect_windows_credential(credential_ref: &str, protected: &[u8]) -> io::Result<Vec<u8>> {
    use std::ptr;

    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    let ciphertext = protected
        .strip_prefix(WINDOWS_CREDENTIAL_MAGIC)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows credential is not a supported DPAPI blob",
            )
        })?;
    let input = windows_data_blob(ciphertext)?;
    let entropy_bytes = windows_credential_entropy(credential_ref);
    let entropy = windows_data_blob(&entropy_bytes)?;
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    // SAFETY: input and entropy point to immutable live slices for the call;
    // output points to writable DATA_BLOB storage. No description is requested
    // and UI is explicitly disabled.
    if unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            &entropy,
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let output = LocalDataBlob::new(output)?;
    let plaintext = output.as_slice();
    let credential_len = plaintext
        .len()
        .checked_sub(WINDOWS_CREDENTIAL_DIGEST_BYTES)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows credential integrity envelope is truncated",
            )
        })?;
    let (credential, actual_digest) = plaintext.split_at(credential_len);
    if credential.len() > MAX_VAULT_SECRET_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows credential data exceeds the supported size",
        ));
    }
    let expected_digest = windows_credential_digest(credential_ref, credential);
    if actual_digest != expected_digest.as_bytes() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows credential integrity check failed",
        ));
    }
    Ok(credential.to_vec())
}

impl SecureVaultPort for DesktopCredentialStore {
    fn contains(&self, reference: &VaultReference) -> Result<bool, PlatformPortError> {
        self.contains(reference.as_str()).map_err(port_error)
    }

    fn store(
        &self,
        reference: &VaultReference,
        secret: &SecretBytes,
    ) -> Result<(), PlatformPortError> {
        self.put(reference.as_str(), secret.expose())
            .map_err(port_error)
    }

    fn load(&self, reference: &VaultReference) -> Result<Option<SecretBytes>, PlatformPortError> {
        self.get(reference.as_str())
            .map_err(port_error)?
            .map(|bytes| SecretBytes::new(bytes).map_err(|_| PlatformPortError::CorruptData))
            .transpose()
    }

    fn delete(&self, reference: &VaultReference) -> Result<(), PlatformPortError> {
        self.delete(reference.as_str()).map_err(port_error)
    }
}

fn port_error(error: io::Error) -> PlatformPortError {
    match error.kind() {
        io::ErrorKind::InvalidInput => PlatformPortError::InvalidRequest,
        io::ErrorKind::InvalidData => PlatformPortError::CorruptData,
        io::ErrorKind::PermissionDenied => PlatformPortError::PermissionDenied,
        io::ErrorKind::Interrupted => PlatformPortError::Canceled,
        io::ErrorKind::WouldBlock => PlatformPortError::Limited,
        _ => PlatformPortError::Unavailable,
    }
}

#[cfg(windows)]
fn windows_credential_entropy(credential_ref: &str) -> Vec<u8> {
    let mut entropy =
        Vec::with_capacity(WINDOWS_CREDENTIAL_ENTROPY_DOMAIN.len() + credential_ref.len());
    entropy.extend_from_slice(WINDOWS_CREDENTIAL_ENTROPY_DOMAIN);
    entropy.extend_from_slice(credential_ref.as_bytes());
    entropy
}

#[cfg(windows)]
fn windows_credential_digest(credential_ref: &str, credential: &[u8]) -> blake3::Hash {
    let mut digest = blake3::Hasher::new();
    digest.update(WINDOWS_CREDENTIAL_DIGEST_DOMAIN);
    digest.update(credential_ref.as_bytes());
    digest.update(&[0]);
    digest.update(credential);
    digest.finalize()
}

#[cfg(windows)]
fn windows_data_blob(
    bytes: &[u8],
) -> io::Result<windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB> {
    Ok(
        windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(bytes.len()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "credential data is too large")
            })?,
            pbData: bytes.as_ptr().cast_mut(),
        },
    )
}

#[cfg(windows)]
struct LocalDataBlob(windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB);

#[cfg(windows)]
impl LocalDataBlob {
    fn new(
        blob: windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB,
    ) -> io::Result<Self> {
        if blob.pbData.is_null() {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "DPAPI returned an empty data pointer",
            ))
        } else {
            Ok(Self(blob))
        }
    }

    fn as_slice(&self) -> &[u8] {
        // SAFETY: DPAPI initialized `pbData` with `cbData` bytes and this
        // LocalAlloc-owned allocation remains live for the returned borrow.
        unsafe { std::slice::from_raw_parts(self.0.pbData, self.0.cbData as usize) }
    }
}

#[cfg(windows)]
impl Drop for LocalDataBlob {
    fn drop(&mut self) {
        // SAFETY: DPAPI returned a LocalAlloc-owned buffer with `cbData`
        // initialized bytes. It is zeroed and released exactly once here.
        unsafe {
            std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).zeroize();
            windows_sys::Win32::Foundation::LocalFree(self.0.pbData.cast());
        }
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
        assert!(store.contains("reference_1").expect("contains"));
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
        assert!(!store.contains("reference_1").expect("missing"));
        assert_eq!(store.get("reference_1").expect("missing"), None);
        assert!(store.put("../outside", b"no").is_err());
    }

    #[test]
    fn oversized_credential_files_are_rejected_before_reading() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("reference_1");
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_CREDENTIAL_FILE_BYTES + 1).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let store = DesktopCredentialStore::new(directory.path());
        let error = store.get("reference_1").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn oversized_credentials_are_rejected_before_writing() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = DesktopCredentialStore::new(directory.path());
        let error = store
            .put("reference_1", &vec![0; MAX_VAULT_SECRET_BYTES + 1])
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!directory.path().join("reference_1").exists());
    }

    #[test]
    fn vault_port_classifies_invalid_stored_secret_without_exposing_it() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("reference_1"), []).unwrap();
        #[cfg(unix)]
        fs::set_permissions(
            directory.path().join("reference_1"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let store = DesktopCredentialStore::new(directory.path());
        let reference = VaultReference::parse("reference_1").unwrap();

        assert_eq!(
            SecureVaultPort::load(&store, &reference).unwrap_err(),
            PlatformPortError::CorruptData
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_credentials_are_encrypted_and_bound_to_their_reference() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = DesktopCredentialStore::new(directory.path());
        store.put("reference_1", b"opaque-secret").unwrap();
        store.put("reference_2", b"another-secret").unwrap();

        let protected = fs::read(directory.path().join("reference_1")).unwrap();
        assert!(protected.starts_with(WINDOWS_CREDENTIAL_MAGIC));
        assert!(
            !protected
                .windows(b"opaque-secret".len())
                .any(|window| window == b"opaque-secret")
        );

        fs::copy(
            directory.path().join("reference_1"),
            directory.path().join("reference_2"),
        )
        .unwrap();
        assert!(store.get("reference_2").is_err());

        let mut corrupted = protected;
        *corrupted.last_mut().unwrap() ^= 1;
        fs::write(directory.path().join("reference_1"), corrupted).unwrap();
        assert!(store.get("reference_1").is_err());
    }
}
