use super::*;

#[test]
fn put_get_round_trips_and_expires() {
    let store = ReceiptStore::new(Duration::from_millis(50));
    assert_eq!(
        store.put("k1".into(), b"blob".to_vec()),
        StatusCode::NO_CONTENT
    );
    assert_eq!(store.get("k1").as_deref(), Some(b"blob".as_slice()));
    assert_eq!(store.get("nope"), None);
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(store.get("k1"), None, "expired receipts are not served");
}
