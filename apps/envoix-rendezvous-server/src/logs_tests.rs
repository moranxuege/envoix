use super::*;

#[test]
fn timeline_epoch_parses_envelope_and_rejects_raw() {
    assert_eq!(
        timeline_epoch("12\t1\t1783875675799\t8045\t3\t\tprotocol\tcomplete_ack\tsent"),
        Some(1783875675799),
    );
    // raw iroh line, header line, and a too-short epoch all yield None.
    assert_eq!(timeline_epoch("13:00:47  DEBUG  data path: direct"), None);
    assert_eq!(timeline_epoch("═════ send ═════"), None);
    assert_eq!(timeline_epoch("1\t1\t123\tx"), None);
}

#[test]
fn merge_interleaves_sources_by_epoch() {
    let store = RoomLogs::new(Duration::from_secs(60));
    store.push_rdz("r", 100, "INFO  paired".to_string());
    // 13-digit epochs; receive (200) predates send (300).
    assert!(store.upload(
        "r",
        "send",
        "0\t1\t0000000000300\t9\t3\t\tmachine\ttransition\t\n".to_string()
    ));
    assert!(store.upload(
        "r",
        "receive",
        "0\t1\t0000000000200\t9\t5\t\tsession\tcreated\t\n".to_string(),
    ));
    let merged = store.merge_view("r").unwrap();
    let rdz = merged.find("[rdz").unwrap();
    let recv = merged.find("[receive").unwrap();
    let send = merged.find("[send").unwrap();
    assert!(
        rdz < recv && recv < send,
        "interleaved by epoch across sources"
    );
}

#[test]
fn per_room_side_cap_rejects_new_but_allows_replace() {
    let store = RoomLogs::new(Duration::from_secs(60));
    for i in 0..MAX_CLIENTS_PER_ROOM {
        assert!(
            store.upload("r", &format!("s{i}"), "x".to_string()),
            "side {i} within cap"
        );
    }
    assert!(
        !store.upload("r", "overflow", "x".to_string()),
        "a new side past the cap is refused"
    );
    assert!(
        store.upload("r", "s0", "y".to_string()),
        "re-upload of an existing side still accepted"
    );
}

#[test]
fn constant_time_eq_matches_and_rejects() {
    assert!(constant_time_eq(b"operator-token", b"operator-token"));
    assert!(!constant_time_eq(b"operator-token", b"operator-toke")); // length differs
    assert!(!constant_time_eq(b"operator-token", b"operator-tokeX")); // last byte differs
    assert!(!constant_time_eq(b"", b"x"));
}
