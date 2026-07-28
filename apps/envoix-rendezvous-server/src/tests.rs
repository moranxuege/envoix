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

#[test]
fn broker_cli_values_populate_and_validate_policy() {
    let cli = Cli::try_parse_from([
        "envoix-rendezvous-server",
        "--room-attempt-limit",
        "9",
        "--max-connections-per-endpoint",
        "3",
        "--subnet-rate-burst",
        "77",
        "--max-retry-after",
        "12",
    ])
    .unwrap();
    let config = cli.broker_config().unwrap();
    assert_eq!(config.room_attempt_limit, 9);
    assert_eq!(config.max_connections_per_endpoint, 3);
    assert_eq!(config.subnet_join_rate.burst, 77);
    assert_eq!(config.max_retry_after, Duration::from_secs(12));
}

#[test]
fn invalid_zero_rate_is_rejected() {
    let cli =
        Cli::try_parse_from(["envoix-rendezvous-server", "--endpoint-rate-events", "0"]).unwrap();
    assert!(cli.broker_config().is_err());
}
