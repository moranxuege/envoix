use super::*;

const TOKEN: &str = "abcdefghijkl"; // exactly MIN_SHARED_TOKEN_LEN bytes

fn valid_peer() -> PeerDescriptor {
    PeerDescriptor::new(
        iroh::SecretKey::generate().public().to_string(),
        vec!["127.0.0.1:9000".parse().unwrap()],
    )
    .unwrap()
}

fn valid_payload(now: u64) -> QrInvitePayload {
    QrInvitePayload::new(TOKEN.into(), valid_peer(), now + 300)
}

// --- encode / decode ---

#[test]
fn round_trip_encode_decode() {
    let payload = valid_payload(0);
    let encoded = payload.encode();
    let decoded = QrInvitePayload::decode(&encoded).unwrap();
    assert_eq!(payload, decoded);
}

#[test]
fn decode_rejects_missing_prefix() {
    let err = QrInvitePayload::decode("badstring").unwrap_err();
    assert!(matches!(err, QrError::DecodeError(_)));
}

#[test]
fn decode_rejects_invalid_base64() {
    let err = QrInvitePayload::decode("envoix:!!!").unwrap_err();
    assert!(matches!(err, QrError::DecodeError(_)));
}

#[test]
fn decode_rejects_invalid_json() {
    let b64 = URL_SAFE_NO_PAD.encode(b"not json");
    let err = QrInvitePayload::decode(&format!("envoix:{b64}")).unwrap_err();
    assert!(matches!(err, QrError::DecodeError(_)));
}

// Invite strings copied from a terminal or QR scanner often carry a
// trailing newline or leading space.
#[test]
fn decode_tolerates_surrounding_whitespace() {
    let invite = format!("  {}\n", valid_payload(0).encode());
    QrInvitePayload::decode(&invite).unwrap();
}

// --- validate ---

#[test]
fn valid_payload_passes_validation() {
    let now = 1_000_000_u64;
    valid_payload(now).validate(now).unwrap();
}

// expires_at == now satisfies the `<=` condition and must be rejected.
#[test]
fn expired_payload_is_rejected() {
    let payload = valid_payload(0); // expires_at = 300
    let err = payload.validate(300).unwrap_err(); // now == expires_at -> expired
    assert_eq!(err, QrError::Expired);
}

#[test]
fn expired_payload_can_only_continue_an_already_accepted_transfer() {
    let payload = valid_payload(0);
    assert_eq!(payload.validate(300), Err(QrError::Expired));
    payload.validate_for_resume().unwrap();

    let mut incompatible = payload;
    incompatible.protocol_version += 1;
    assert!(matches!(
        incompatible.validate_for_resume(),
        Err(QrError::ProtocolVersionMismatch { .. })
    ));
}

// expires_at == now + 1 is the tightest value that must pass.
#[test]
fn payload_expiring_in_one_second_passes() {
    let now = 1_000_000_u64;
    let mut payload = valid_payload(0);
    payload.expires_at = now + 1;
    payload.validate(now).unwrap();
}

#[test]
fn version_mismatch_is_rejected() {
    let mut payload = valid_payload(0);
    payload.version = 99;
    let err = payload.validate(0).unwrap_err();
    assert!(matches!(err, QrError::VersionMismatch { found: 99, .. }));
}

#[test]
fn protocol_version_mismatch_is_rejected() {
    let mut payload = valid_payload(0);
    payload.protocol_version = 999;
    let err = payload.validate(0).unwrap_err();
    assert!(matches!(
        err,
        QrError::ProtocolVersionMismatch { found: 999, .. }
    ));
}

#[test]
fn nonzero_flags_are_rejected() {
    let mut payload = valid_payload(0);
    payload.flags = 1;
    let err = payload.validate(0).unwrap_err();
    assert!(matches!(err, QrError::UnsupportedFlags(1)));
}

#[test]
fn empty_direct_addresses_are_rejected() {
    let mut payload = valid_payload(0);
    payload.peer.direct_addrs.clear();
    let err = payload.validate(0).unwrap_err();
    assert_eq!(err, QrError::NoDirectAddresses);
}

// Token exactly one byte short of the minimum must be rejected.
#[test]
fn token_one_byte_short_of_minimum_is_rejected() {
    let mut payload = valid_payload(0);
    payload.token = "a".repeat(MIN_SHARED_TOKEN_LEN - 1);
    assert_eq!(payload.validate(0).unwrap_err(), QrError::WeakToken);
}

#[test]
fn non_ascii_token_is_rejected() {
    let mut payload = valid_payload(0);
    payload.token = "abcdefghijklé".into(); // non-ASCII suffix, still ≥12 bytes
    assert_eq!(payload.validate(0).unwrap_err(), QrError::WeakToken);
}

#[test]
fn malformed_endpoint_id_is_rejected() {
    let mut payload = valid_payload(0);
    payload.peer.endpoint_id = "not-an-endpoint-id".into();
    let err = payload.validate(0).unwrap_err();
    assert!(matches!(err, QrError::MalformedEndpointId(_)));
}

#[test]
fn ipv6_direct_address_is_accepted() {
    let mut payload = valid_payload(0);
    payload.peer.direct_addrs = vec!["[::1]:9000".parse().unwrap()];
    payload.validate(0).unwrap();
}

// --- peer_descriptor ---

#[test]
fn peer_descriptor_returns_descriptor() {
    let mut payload = valid_payload(0);
    payload.peer.direct_addrs = vec![
        "1.2.3.4:1000".parse().unwrap(),
        "5.6.7.8:2000".parse().unwrap(),
    ];
    let peer = payload.peer_descriptor().unwrap();
    assert_eq!(peer.direct_addrs, payload.peer.direct_addrs);
}

#[test]
fn peer_descriptor_on_empty_direct_addresses_returns_error() {
    let mut payload = valid_payload(0);
    payload.peer.direct_addrs.clear();
    assert_eq!(
        payload.peer_descriptor().unwrap_err(),
        QrError::NoDirectAddresses
    );
}

// --- endpoint_addr / relay URLs ---

#[test]
fn missing_relay_urls_decodes_as_legacy_direct_only_payload() {
    let payload = valid_payload(0);
    let mut value = serde_json::to_value(&payload).unwrap();
    value.as_object_mut().unwrap().remove("relay_urls");
    let json = serde_json::to_vec(&value).unwrap();
    let decoded =
        QrInvitePayload::decode(&format!("envoix:{}", URL_SAFE_NO_PAD.encode(json))).unwrap();

    decoded.validate(0).unwrap();
    assert!(decoded.relay_urls.is_empty());
}

#[test]
fn endpoint_addr_includes_relay_urls_when_present() {
    let payload = QrInvitePayload::new_with_relay_urls(
        TOKEN.into(),
        valid_peer(),
        vec!["https://relay.example:8444".into()],
        300,
    );

    payload.validate(0).unwrap();
    let endpoint_addr = payload.endpoint_addr().unwrap();

    assert_eq!(
        endpoint_addr.ip_addrs().copied().collect::<Vec<_>>(),
        payload.peer.direct_addrs
    );
    assert_eq!(endpoint_addr.relay_urls().count(), 1);
}

#[test]
fn relay_urls_round_trip_through_invite_encoding() {
    let payload = QrInvitePayload::new_with_relay_urls(
        TOKEN.into(),
        valid_peer(),
        vec!["https://relay.example:8444".into()],
        300,
    );

    let decoded = QrInvitePayload::decode(&payload.encode()).unwrap();

    assert_eq!(decoded.relay_urls, payload.relay_urls);
    assert_eq!(decoded.endpoint_addr().unwrap().relay_urls().count(), 1);
}

#[test]
fn malformed_relay_url_is_rejected() {
    let mut payload = valid_payload(0);
    payload.relay_urls = vec!["not-a-url".into()];

    assert!(matches!(
        payload.validate(0).unwrap_err(),
        QrError::MalformedRelayUrl(_)
    ));
    assert!(matches!(
        payload.endpoint_addr().unwrap_err(),
        QrError::MalformedRelayUrl(_)
    ));
}

// --- generate_token ---

// Verify all structural requirements in a single test: length, charset,
// and SPAKE2 minimum - these are the same property viewed from three angles.
#[test]
fn generated_token_is_valid_hex_and_meets_spake2_minimum() {
    let token = generate_token().unwrap();
    assert_eq!(token.len(), TOKEN_RANDOM_BYTES * 2);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(token.len() >= MIN_SHARED_TOKEN_LEN);
}

#[test]
fn generated_token_passes_payload_validation() {
    let token = generate_token().unwrap();
    let payload = QrInvitePayload::new(token, valid_peer(), 999);
    payload.validate(0).unwrap();
}

// --- render_terminal_qr ---

#[test]
fn render_output_contains_only_block_chars_and_newlines() {
    let qr = render_terminal_qr("test").unwrap();
    for ch in qr.chars() {
        assert!(
            matches!(ch, '█' | '▀' | '▄' | ' ' | '\n'),
            "unexpected character: {ch:?}"
        );
    }
}

// All lines must be the same width so the QR matrix is square.
#[test]
fn render_all_lines_have_equal_width() {
    let qr = render_terminal_qr("envoix test payload").unwrap();
    let lines: Vec<&str> = qr.trim_end_matches('\n').split('\n').collect();
    let widths: Vec<usize> = lines.iter().map(|l| l.chars().count()).collect();
    assert!(
        widths.windows(2).all(|w| w[0] == w[1]),
        "lines have different widths: {widths:?}"
    );
}

// A real invite string must encode without hitting the QR data limit.
#[test]
fn render_invite_string_produces_scannable_qr() {
    let payload = valid_payload(0);
    let invite = payload.encode();
    assert!(render_terminal_qr(&invite).is_some());
}
