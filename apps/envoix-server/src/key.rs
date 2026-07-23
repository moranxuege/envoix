use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use iroh::SecretKey;

use crate::{KeyError, KeyOperation};

pub(crate) fn load_or_create_node_key(path: &Path) -> Result<SecretKey, KeyError> {
    match read_node_key(path) {
        Ok(key) => Ok(key),
        Err(KeyError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            create_node_key(path)
        }
        Err(error) => Err(error),
    }
}

fn read_node_key(path: &Path) -> Result<SecretKey, KeyError> {
    let bytes = fs::read(path).map_err(|source| KeyError::Io {
        operation: KeyOperation::Read,
        source,
    })?;
    let bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| KeyError::InvalidLength {
            actual: bytes.len(),
        })?;
    Ok(SecretKey::from_bytes(&bytes))
}

fn create_node_key(path: &Path) -> Result<SecretKey, KeyError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|source| KeyError::Io {
            operation: KeyOperation::CreateParent,
            source,
        })?;
    }

    let mut file = match open_new(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
            return read_node_key(path);
        }
        Err(source) => {
            return Err(KeyError::Io {
                operation: KeyOperation::Create,
                source,
            });
        }
    };
    let key = SecretKey::generate();
    if let Err(source) = file.write_all(&key.to_bytes()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(KeyError::Io {
            operation: KeyOperation::Write,
            source,
        });
    }
    if let Err(source) = file.sync_all() {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(KeyError::Io {
            operation: KeyOperation::Sync,
            source,
        });
    }
    Ok(key)
}

#[cfg(unix)]
fn open_new(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_new(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}
