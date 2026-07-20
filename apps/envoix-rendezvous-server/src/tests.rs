use super::*;
use tempfile::TempDir;

#[test]
fn secret_key_is_created_then_reused() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("secret.key");

    let first = load_or_create_secret_key(&path).expect("create");
    assert!(path.exists(), "key file should be created");
    let second = load_or_create_secret_key(&path).expect("reuse");

    assert_eq!(first.public(), second.public(), "key must be stable");
}

#[test]
fn wrong_length_key_file_errors() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bad.key");
    std::fs::write(&path, b"too short").unwrap();
    assert!(load_or_create_secret_key(&path).is_err());
}
