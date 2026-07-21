use super::*;

/// A complete, valid params object (a joined room receive). Tests mutate a
/// clone to exercise each rejection path.
fn valid() -> serde_json::Value {
    serde_json::json!({
        "direction": "receive",
        "path": "/tmp/out",
        "code": "123456-cobalt-flint",
        "broker": "id@1.2.3.4:5",
        "relay": "",
        "chunk_size": "",
        "data_stream_window": "",
        "candidates_allow": "",
        "candidates_deny": "",
        "receipt_server": "",
        "use_room": true,
        "use_mdns": false,
        "resume": false
    })
}

fn parse(
    v: &serde_json::Value,
    mode: CreateMode,
) -> Result<(SessionContext, Option<serde_json::Value>), String> {
    parse_create_params(&v.to_string(), mode)
}

#[test]
fn valid_params_build_a_context() {
    let (ctx, extras) = parse(&valid(), CreateMode::Normal).unwrap();
    assert!(matches!(ctx.params.direction, TransferDirection::Receive));
    assert_eq!(ctx.params.sources.len(), 1); // room only (use_mdns false)
    assert!(extras.is_none());
}

#[test]
fn a_missing_field_is_rejected_not_defaulted() {
    // The whole point: deleting a `put(...)` on the Kotlin side must error,
    // not silently become "".
    let mut v = valid();
    v.as_object_mut().unwrap().remove("data_stream_window");
    assert!(parse(&v, CreateMode::Normal).is_err());
}

#[test]
fn an_unknown_or_renamed_field_is_rejected() {
    let mut v = valid();
    v["chunkSize"] = serde_json::json!("64KB"); // camelCase typo of chunk_size
    assert!(parse(&v, CreateMode::Normal).is_err());
}

#[test]
fn a_typo_direction_is_rejected() {
    let mut v = valid();
    v["direction"] = serde_json::json!("recieve");
    assert!(parse(&v, CreateMode::Normal).is_err());
}

#[test]
fn an_empty_path_is_rejected() {
    let mut v = valid();
    v["path"] = serde_json::json!("");
    assert!(parse(&v, CreateMode::Normal).is_err());
}

#[test]
fn staging_requires_a_source_but_normal_does_not() {
    assert!(parse(&valid(), CreateMode::Staging).is_err());
    assert!(parse(&valid(), CreateMode::Normal).is_ok());
}

#[test]
fn staging_with_a_source_round_trips_extras() {
    let mut v = valid();
    v["direction"] = serde_json::json!("send");
    v["path"] = serde_json::json!("/tmp/staged");
    v["platform_extras"] = serde_json::json!({
        "source_uri": "content://x",
        "source_recoverable": true
    });
    let (_, extras) = parse(&v, CreateMode::Staging).unwrap();
    let extras = extras.unwrap();
    assert_eq!(extras["source_uri"], "content://x");
    assert_eq!(extras["source_recoverable"], true);
}

#[test]
fn publication_evidence_round_trips_extras() {
    let mut v = valid();
    v["platform_extras"] = serde_json::json!({
        "saved_uri": "content://downloads/1",
        "published_name": "photo.jpg",
        "published_size": 3,
        "published_sha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "publication_invalid": false
    });
    let (_, extras) = parse(&v, CreateMode::Normal).unwrap();
    let extras = extras.unwrap();
    assert_eq!(extras["published_size"], 3);
    assert_eq!(
        extras["published_sha256"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(extras["publication_invalid"], false);
}

#[test]
fn a_half_pair_of_source_fields_is_rejected() {
    let mut v = valid();
    v["platform_extras"] = serde_json::json!({ "source_uri": "content://x" });
    assert!(parse(&v, CreateMode::Normal).is_err());
}

#[test]
fn an_unknown_extras_key_is_rejected() {
    let mut v = valid();
    v["platform_extras"] = serde_json::json!({ "qrr": "x" }); // typo of qr
    assert!(parse(&v, CreateMode::Normal).is_err());
}
