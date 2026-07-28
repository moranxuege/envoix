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
async fn concurrent_first_use_reuses_the_atomic_winner() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("identity.json");
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(16));
    let mut tasks = Vec::new();

    for _ in 0..16 {
        let path = path.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            load_secret_key(&IdentityConfig::Persistent(path)).await
        }));
    }

    let mut public_keys = Vec::new();
    for task in tasks {
        public_keys.push(task.await.unwrap().unwrap().public());
    }
    assert!(public_keys.windows(2).all(|keys| keys[0] == keys[1]));
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
