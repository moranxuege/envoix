use super::*;
use envoix_types::TransferId;

fn receipt() -> TransferReceipt {
    TransferReceipt {
        transfer_id: TransferId::new("transfer-aa11"),
        file_name: "photo.jpg".into(),
        file_size: 42,
        file_hash: "abc123".into(),
    }
}

#[test]
fn seal_open_round_trips() {
    let blob = seal_receipt("transfer-aa11", "123456-kelp-coral", &receipt()).unwrap();
    let opened = open_receipt("transfer-aa11", "123456-kelp-coral", &blob).unwrap();
    assert_eq!(opened, receipt());
}

#[test]
fn wrong_code_or_transfer_id_fails_to_open() {
    let blob = seal_receipt("transfer-aa11", "123456-kelp-coral", &receipt()).unwrap();
    assert!(open_receipt("transfer-aa11", "123456-kelp-CORAL", &blob).is_err());
    assert!(open_receipt("transfer-bb22", "123456-kelp-coral", &blob).is_err());
}

#[test]
fn mailbox_key_is_stable_hex_and_id_bound() {
    let key = receipt_mailbox_key("transfer-aa11");
    assert_eq!(key.len(), 64);
    assert_eq!(key, receipt_mailbox_key("transfer-aa11"));
    assert_ne!(key, receipt_mailbox_key("transfer-bb22"));
}

#[test]
fn verify_against_fact_checks_size_and_hash() {
    let mut receipt = receipt();
    receipt.transfer_id = TransferId::new("t-1");
    let blob = seal_receipt("t-1", "1-a-b", &receipt).unwrap();
    assert!(verify_receipt_against_fact("t-1", "1-a-b", &blob, "abc123", 42).is_ok());
    // Wrong committed hash or size: an authenticated mismatch.
    assert!(verify_receipt_against_fact("t-1", "1-a-b", &blob, "abc124", 42).is_err());
    assert!(verify_receipt_against_fact("t-1", "1-a-b", &blob, "abc123", 43).is_err());
    // Wrong seal (stale key from another attempt) fails to open at all.
    assert!(verify_receipt_against_fact("t-2", "1-a-b", &blob, "abc123", 42).is_err());
}

#[tokio::test]
async fn verify_checks_size_and_hash_against_the_file() {
    let dir = std::env::temp_dir().join(format!("envoix-receipt-test-{}", std::process::id()));
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let path = dir.join("v.bin");
    tokio::fs::write(&path, b"verified bytes").await.unwrap();
    let real = TransferReceipt {
        transfer_id: TransferId::new("t-1"),
        file_name: "v.bin".into(),
        file_size: 14,
        file_hash: blake3::hash(b"verified bytes").to_hex().to_string(),
    };
    let blob = seal_receipt("t-1", "1-a-b", &real).unwrap();
    assert!(
        verify_receipt_against_file("t-1", "1-a-b", &blob, &path)
            .await
            .is_ok()
    );

    // Same size, different bytes: must fail on the hash.
    tokio::fs::write(&path, b"verifieD bytes").await.unwrap();
    assert!(
        verify_receipt_against_file("t-1", "1-a-b", &blob, &path)
            .await
            .is_err()
    );
}
