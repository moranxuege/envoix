use super::*;

const BROKER: &str =
    "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445";
const RELAY: &str = "https://envoix.example:8444";
const NOW: u64 = 1_750_000_000;

fn created(role: TransferRole) -> CreatedInvitation {
    InviteV2::create(
        BROKER.into(),
        vec![RELAY.into()],
        role,
        Capabilities::current(),
        NOW,
    )
    .unwrap()
}

fn deterministic_invitation() -> InviteV2 {
    let ticket = TicketSecret([0x42; TICKET_LEN]);
    let public_context = InvitationPublicContext {
        version: INVITE_VERSION,
        invite_id: InviteId([0x11; INVITE_ID_LEN]),
        protocol_version: TRANSFER_PROTOCOL_VERSION,
        creator_transfer_role: TransferRole::Sender,
        joiner_transfer_role: TransferRole::Receiver,
        broker: BROKER.into(),
        relay_urls: vec![RELAY.into()],
        capabilities: Capabilities::current(),
        expires_at: NOW + INVITE_TTL_SECS,
        bootstrap_methods: vec![
            BootstrapMethod::FullTicket {
                ticket_commitment: ticket.commitment(),
            },
            BootstrapMethod::RoomCode {
                room_id: "123456".into(),
            },
        ],
    };
    let context_commitment = commitment_for_context(&public_context).unwrap();
    InviteV2 {
        public_context,
        context_commitment,
        ticket,
    }
}

#[test]
fn golden_payload_is_stable() {
    const GOLDEN: &str = "envoix://invite/v2/eyJib290c3RyYXBfbWV0aG9kcyI6W3siaWQiOiJmdWxsLXRpY2tldC12MSIsInBha2UiOiJzcGFrZTItZWQyNTUxOS1zaGEyNTYtaGtkZi1obWFjIiwidGlja2V0X2NvbW1pdG1lbnQiOiJRbDdVNUtOck1Pb2h1UTRoeHhMR1NlZ2hUQ20zNnZhQWlkRURuRzVWT0V3In0seyJpZCI6InJvb20tY29kZS12MSIsInBha2UiOiJzcGFrZTItZWQyNTUxOS1zaGEyNTYtaGtkZi1obWFjIiwicm9vbV9pZCI6IjEyMzQ1NiJ9XSwiYnJva2VyIjoiZTk0NmEzMWEyMjA3ZWZjZDY4YjlkYmY0MDljNGJmMjQxYWEwMmEwY2JjMDAyOGFmMmUxZWQxMTQ3MjA2NGVmZkA2Ny4yMzAuMTg3LjIzODo4NDQ1IiwiY2FwYWJpbGl0aWVzIjp7Im9wdGlvbmFsIjpbIm1hbmlmZXN0LXYxIl0sInJlcXVpcmVkIjpbXX0sImNvbnRleHRfY29tbWl0bWVudCI6ImFuRS1kQ3dweHNFYzFfZjB6VjZOTUVSSm8tVnVRd3BYRWRBTUYzV3NrMjAiLCJjcmVhdG9yX3RyYW5zZmVyX3JvbGUiOiJzZW5kZXIiLCJleHBpcmVzX2F0IjoxNzUwMDAwMzAwLCJpbnZpdGVfaWQiOiJFUkVSRVJFUkVSRVJFUkVSRVJFUkVRIiwiam9pbmVyX3RyYW5zZmVyX3JvbGUiOiJyZWNlaXZlciIsInByZXNlbnRlZF9jcmVkZW50aWFsIjp7Im1ldGhvZCI6ImZ1bGwtdGlja2V0LXYxIiwidGlja2V0IjoiUWtKQ1FrSkNRa0pDUWtKQ1FrSkNRa0pDUWtKQ1FrSkNRa0pDUWtKQ1FrSSJ9LCJwcm90b2NvbF92ZXJzaW9uIjoxLCJyZWxheV91cmxzIjpbImh0dHBzOi8vZW52b2l4LmV4YW1wbGU6ODQ0NCJdLCJ2ZXJzaW9uIjoyfQ";

    let payload = deterministic_invitation().encode().unwrap();
    assert_eq!(payload, GOLDEN);
    let parsed = InviteV2::parse(&payload, NOW).unwrap();
    assert_eq!(parsed.joiner_role(), TransferRole::Receiver);
}

#[test]
fn room_code_forms_normalize_identically() {
    let canonical = RoomCode::parse("123456-k7m4-9v2d").unwrap();
    let compact = RoomCode::parse("123456k7m49v2d").unwrap();
    let uppercase = RoomCode::parse("123456-K7M4-9V2D").unwrap();

    assert_eq!(canonical, compact);
    assert_eq!(canonical, uppercase);
    assert_eq!(canonical.canonical(), "123456-k7m4-9v2d");
    assert_eq!(canonical.room_id(), "123456");
}

#[test]
fn malformed_room_codes_fail_closed() {
    for input in [
        "",
        " 123456-k7m4-9v2d",
        "123456-k7m4-9v2d ",
        "123456 k7m4 9v2d",
        "123456_k7m4_9v2d",
        "123456-k7m4-9v2",
        "123456-k7m4-9v2dd",
        "12345-k7m4-9v2d",
        "123456-k7m4-9v2!",
        "123456-k7m4-9v2é",
        "123456--k7m49v2d",
    ] {
        assert!(RoomCode::parse(input).is_err(), "accepted {input:?}");
    }
}

#[test]
fn generated_room_codes_have_the_required_shape() {
    for _ in 0..100 {
        let code = RoomCode::generate().unwrap();
        assert_eq!(code.canonical().len(), 16);
        assert_eq!(&code.canonical()[6..7], "-");
        assert_eq!(&code.canonical()[11..12], "-");
        assert!(RoomCode::parse(code.canonical()).is_ok());
    }
}

#[test]
fn complete_invitation_round_trips_and_routes() {
    let created = created(TransferRole::Receiver);
    let parsed = InviteV2::parse(&created.payload, NOW).unwrap();

    assert_eq!(parsed.joiner_role(), TransferRole::Sender);
    assert_eq!(parsed.invitation(), created.invitation());
    assert_eq!(created.expires_at, NOW + INVITE_TTL_SECS);
    assert_eq!(
        created
            .invitation()
            .public_context
            .bootstrap_methods
            .iter()
            .map(BootstrapMethod::id)
            .collect::<Vec<_>>(),
        vec![FULL_TICKET_METHOD, ROOM_CODE_METHOD]
    );
}

#[test]
fn wrong_local_role_is_rejected() {
    let created = created(TransferRole::Receiver);
    let error =
        InviteV2::parse_for_role(&created.payload, TransferRole::Receiver, NOW).unwrap_err();
    assert_eq!(error.code(), InvitationErrorCode::RoleConflict);
}

#[test]
fn expiry_is_strict() {
    let created = created(TransferRole::Sender);
    assert!(InviteV2::parse(&created.payload, created.expires_at - 1).is_ok());
    let error = InviteV2::parse(&created.payload, created.expires_at).unwrap_err();
    assert_eq!(error.code(), InvitationErrorCode::Expired);
}

#[test]
fn creator_and_full_ticket_joiner_recheck_expiry_during_authentication() {
    let created = created(TransferRole::Sender);
    let expires_at = created.expires_at;
    let public_context = created
        .invitation()
        .public_context
        .canonical_json()
        .unwrap();
    let creator = created.clone().into_bootstrap();
    let joiner = InviteV2::parse(&created.payload, NOW)
        .unwrap()
        .into_bootstrap();

    assert!(
        creator
            .validate_control_context(BootstrapKind::FullTicket, None, expires_at - 1)
            .is_ok()
    );
    assert_eq!(
        creator
            .validate_control_context(BootstrapKind::FullTicket, None, expires_at)
            .unwrap_err()
            .code(),
        InvitationErrorCode::Expired
    );
    assert!(
        joiner
            .validate_control_context(
                BootstrapKind::FullTicket,
                Some(&public_context),
                expires_at - 1,
            )
            .is_ok()
    );
    assert_eq!(
        joiner
            .validate_control_context(BootstrapKind::FullTicket, Some(&public_context), expires_at,)
            .unwrap_err()
            .code(),
        InvitationErrorCode::Expired
    );
}

#[test]
fn debug_redacts_every_secret() {
    let created = created(TransferRole::Sender);
    let debug = format!("{created:?} {:?}", created.invitation());
    let encoded = created.payload.strip_prefix(INVITE_V2_PREFIX).unwrap();
    let json = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
    let document: InviteDocument = serde_json::from_str(&json).unwrap();

    assert!(!debug.contains(&created.payload));
    assert!(!debug.contains(created.room_code.canonical()));
    assert!(!debug.contains(&document.presented_credential.ticket));
    assert!(!debug.contains(&document.context_commitment));
    assert!(
        !debug.contains(
            document.bootstrap_methods[0]
                .ticket_commitment
                .as_deref()
                .unwrap()
        )
    );
}

#[test]
fn generated_json_is_jcs_and_unpadded_base64url() {
    let created = created(TransferRole::Sender);
    let encoded = created.payload.strip_prefix(INVITE_V2_PREFIX).unwrap();
    assert!(!encoded.contains('='));
    let decoded = URL_SAFE_NO_PAD.decode(encoded).unwrap();
    let document: InviteDocument = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(decoded, jcs_document(&document).unwrap());
    let text = String::from_utf8(decoded).unwrap();
    assert!(text.starts_with("{\"bootstrap_methods\":"));
}

#[test]
fn legacy_payloads_are_typed_unsupported() {
    for legacy in [
        "envoix:eyJ2ZXJzaW9uIjoyfQ",
        "envoix://pair/123456-amber-comet",
    ] {
        let error = InviteV2::parse(legacy, NOW).unwrap_err();
        assert_eq!(error.code(), InvitationErrorCode::UnsupportedVersion);
    }
}

#[test]
fn rejects_oversized_encoded_and_decoded_payloads() {
    let encoded = format!(
        "{INVITE_V2_PREFIX}{}",
        "a".repeat(MAX_ENCODED_PAYLOAD_LEN + 1)
    );
    assert_eq!(
        InviteV2::parse(&encoded, NOW).unwrap_err().code(),
        InvitationErrorCode::Oversized
    );

    let decoded = vec![b' '; MAX_DECODED_PAYLOAD_LEN + 1];
    let payload = format!("{INVITE_V2_PREFIX}{}", URL_SAFE_NO_PAD.encode(decoded));
    assert_eq!(
        InviteV2::parse(&payload, NOW).unwrap_err().code(),
        InvitationErrorCode::Oversized
    );
}

#[test]
fn rejects_unknown_and_duplicate_fields() {
    let created = created(TransferRole::Sender);
    let encoded = created.payload.strip_prefix(INVITE_V2_PREFIX).unwrap();
    let mut text = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
    text.insert_str(1, "\"unknown\":true,");
    let payload = format!("{INVITE_V2_PREFIX}{}", URL_SAFE_NO_PAD.encode(text));
    assert_eq!(
        InviteV2::parse(&payload, NOW).unwrap_err().code(),
        InvitationErrorCode::Malformed
    );

    let mut text = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
    text.insert_str(1, "\"version\":2,");
    let payload = format!("{INVITE_V2_PREFIX}{}", URL_SAFE_NO_PAD.encode(text));
    assert_eq!(
        InviteV2::parse(&payload, NOW).unwrap_err().code(),
        InvitationErrorCode::Malformed
    );
}

#[test]
fn rejects_noncanonical_json() {
    let created = created(TransferRole::Sender);
    let encoded = created.payload.strip_prefix(INVITE_V2_PREFIX).unwrap();
    let mut decoded = URL_SAFE_NO_PAD.decode(encoded).unwrap();
    decoded.insert(1, b' ');
    let payload = format!("{INVITE_V2_PREFIX}{}", URL_SAFE_NO_PAD.encode(decoded));
    assert_eq!(
        InviteV2::parse(&payload, NOW).unwrap_err().code(),
        InvitationErrorCode::Malformed
    );
}

#[test]
fn rejects_tampered_ticket_and_context() {
    let created = created(TransferRole::Sender);
    let encoded = created.payload.strip_prefix(INVITE_V2_PREFIX).unwrap();
    let decoded = URL_SAFE_NO_PAD.decode(encoded).unwrap();
    let mut document: InviteDocument = serde_json::from_slice(&decoded).unwrap();
    document.presented_credential.ticket = URL_SAFE_NO_PAD.encode([0x55_u8; TICKET_LEN]);
    let payload = format!(
        "{INVITE_V2_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(jcs_document(&document).unwrap())
    );
    assert_eq!(
        InviteV2::parse(&payload, NOW).unwrap_err().code(),
        InvitationErrorCode::AuthenticationFailed
    );

    let mut document: InviteDocument = serde_json::from_slice(&decoded).unwrap();
    std::mem::swap(
        &mut document.creator_transfer_role,
        &mut document.joiner_transfer_role,
    );
    let payload = format!(
        "{INVITE_V2_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(jcs_document(&document).unwrap())
    );
    assert_eq!(
        InviteV2::parse(&payload, NOW).unwrap_err().code(),
        InvitationErrorCode::AuthenticationFailed
    );
}

#[test]
fn validates_capability_policy() {
    let invalid = Capabilities {
        required: vec!["z-cap".into(), "a-cap".into()],
        optional: Vec::new(),
    };
    assert_eq!(
        invalid.validate().unwrap_err().code(),
        InvitationErrorCode::Malformed
    );

    let unknown_required = Capabilities {
        required: vec!["future-v1".into()],
        optional: Vec::new(),
    };
    assert_eq!(
        unknown_required.validate().unwrap_err().code(),
        InvitationErrorCode::UnsupportedCapability
    );

    let unknown_optional = Capabilities {
        required: Vec::new(),
        optional: vec!["future-v1".into()],
    };
    unknown_optional.validate().unwrap();

    for invalid in [
        Capabilities {
            required: Vec::new(),
            optional: vec!["manifest-v1".into(), "manifest-v1".into()],
        },
        Capabilities {
            required: Vec::new(),
            optional: vec!["Manifest-v1".into()],
        },
        Capabilities {
            required: vec!["manifest-v1".into()],
            optional: vec!["manifest-v1".into()],
        },
    ] {
        assert_eq!(
            invalid.validate().unwrap_err().code(),
            InvitationErrorCode::Malformed
        );
    }

    let mut invitation = deterministic_invitation();
    invitation.public_context.capabilities.optional =
        vec!["future-v1".into(), "manifest-v1".into()];
    invitation.context_commitment = commitment_for_context(&invitation.public_context).unwrap();
    let payload = invitation.encode().unwrap();
    InviteV2::parse(&payload, NOW).unwrap();
}

#[test]
fn rejects_duplicate_relay_urls() {
    assert_eq!(
        validate_relays(&[RELAY.into(), RELAY.into()])
            .unwrap_err()
            .code(),
        InvitationErrorCode::Malformed
    );
}

#[test]
fn password_derivations_are_domain_separated() {
    let created = created(TransferRole::Sender);
    let invitation = created.invitation();
    let full_control = invitation
        .ticket()
        .control_pake_password(&invitation.context_commitment);
    let full_data = invitation.ticket().data_auth_password(
        &invitation.context_commitment,
        &invitation.public_context.invite_id,
    );
    let room_control = created.room_code.control_pake_password();
    let room_data = derive_room_data_auth_password(
        b"control shared key",
        &invitation.context_commitment,
        &invitation.public_context.invite_id,
    );

    assert_ne!(full_control, full_data);
    assert_ne!(full_control, room_control);
    assert_ne!(room_control, room_data);
}
