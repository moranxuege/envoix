use super::*;
use tempfile::TempDir;

#[tokio::test]
async fn ephemeral_identity_generates_distinct_keys() {
    let a = load_secret_key(&IdentityConfig::Ephemeral).await.unwrap();
    let b = load_secret_key(&IdentityConfig::Ephemeral).await.unwrap();
    assert_ne!(a.public(), b.public());
}

#[tokio::test]
async fn memory_identity_survives_endpoint_rebinds() {
    let identity = IdentityConfig::Memory(MemoryIdentity::generate());
    let first = load_secret_key(&identity).await.unwrap();
    let second = load_secret_key(&identity).await.unwrap();

    assert_eq!(first.public(), second.public());
    assert!(!format!("{identity:?}").contains(&URL_SAFE_NO_PAD.encode(first.to_bytes())));
}

#[tokio::test]
async fn persistent_identity_is_created_and_reused() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("identity.json");

    let first = load_secret_key(&IdentityConfig::Persistent(path.clone()))
        .await
        .unwrap();
    let second = load_secret_key(&IdentityConfig::Persistent(path))
        .await
        .unwrap();

    assert_eq!(first.public(), second.public());
}

#[tokio::test]
async fn invalid_identity_file_errors() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("identity.json");
    fs::write(&path, b"{\"version\":1,\"secret_key\":\"bad\"}")
        .await
        .unwrap();

    let error = load_secret_key(&IdentityConfig::Persistent(path))
        .await
        .unwrap_err();

    assert!(matches!(error, CoreError::InvalidInput(_)));
}
