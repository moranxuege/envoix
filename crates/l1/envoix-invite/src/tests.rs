use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::identifiers::{INVITE_PAYLOAD_VERSION, QR_OUTER_PREFIX, ROOM_CODE_NAMESPACE_PREFIX};
use crate::invite::{
    MAX_DECODED_PAYLOAD_LENGTH, MAX_ENCODED_PAYLOAD_LENGTH, MAX_INVITE_INPUT_LENGTH,
    MAX_RELAY_LENGTH,
};
use crate::{
    EntropyError, EntropySource, Invite, InviteError, InviteField, RecognizedInvalid, Role,
    encode_deep_link, encode_qr, generate_room_code, route_invite,
};

const CODE: &str = "123456-amber-comet";
const BROKER: &str = "node@test.example:9445";
const RELAY: &str = "https://relay.test.envoix.chkxwlyh.us:9444";
const PAYLOAD: &str = "eyJ2ZXJzaW9uIjozLCJjb2RlIjoiMTIzNDU2LWFtYmVyLWNvbWV0IiwiYnJva2VyIjoibm9kZUB0ZXN0LmV4YW1wbGU6OTQ0NSIsInJlbGF5IjoiaHR0cHM6Ly9yZWxheS50ZXN0LmVudm9peC5jaGt4d2x5aC51czo5NDQ0Iiwicm9sZSI6InNlbmQifQ";
const QR_LITERAL: &str = "envoix:eyJ2ZXJzaW9uIjozLCJjb2RlIjoiMTIzNDU2LWFtYmVyLWNvbWV0IiwiYnJva2VyIjoibm9kZUB0ZXN0LmV4YW1wbGU6OTQ0NSIsInJlbGF5IjoiaHR0cHM6Ly9yZWxheS50ZXN0LmVudm9peC5jaGt4d2x5aC51czo5NDQ0Iiwicm9sZSI6InNlbmQifQ";
const DEEP_LINK_LITERAL: &str = "envoix://invite/v3/eyJ2ZXJzaW9uIjozLCJjb2RlIjoiMTIzNDU2LWFtYmVyLWNvbWV0IiwiYnJva2VyIjoibm9kZUB0ZXN0LmV4YW1wbGU6OTQ0NSIsInJlbGF5IjoiaHR0cHM6Ly9yZWxheS50ZXN0LmVudm9peC5jaGt4d2x5aC51czo5NDQ0Iiwicm9sZSI6InNlbmQifQ";

struct ScriptedEntropy {
    values: Vec<u32>,
    next: usize,
}

impl ScriptedEntropy {
    fn new(values: impl Into<Vec<u32>>) -> Self {
        Self {
            values: values.into(),
            next: 0,
        }
    }
}

impl EntropySource for ScriptedEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), EntropyError> {
        let Some(value) = self.values.get(self.next) else {
            return Err(EntropyError::Unavailable);
        };
        if destination.len() != 4 {
            return Err(EntropyError::Unavailable);
        }
        destination.copy_from_slice(&value.to_le_bytes());
        self.next += 1;
        Ok(())
    }
}

#[test]
fn invite_v1_conformance() {
    let invite = Invite::new(CODE, BROKER, RELAY, Role::Send).unwrap();

    let qr = encode_qr(&invite).unwrap();
    let deep_link = encode_deep_link(&invite).unwrap();
    assert_eq!(qr, QR_LITERAL);
    assert_eq!(deep_link, DEEP_LINK_LITERAL);
    assert_eq!(qr, format!("{QR_OUTER_PREFIX}{PAYLOAD}"));
    assert_eq!(route_invite(&qr).unwrap(), invite);
    assert_eq!(route_invite(&deep_link).unwrap(), invite);
    assert_eq!(invite.role().opposite(), Role::Receive);

    let generated =
        generate_room_code(&mut ScriptedEntropy::new(vec![u32::MAX, 42, 0, 17])).unwrap();
    assert_eq!(generated.as_str(), "000042-amber-comet");
    let parts = generated.as_str().split('-').collect::<Vec<_>>();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0].len(), 6);
    assert!(parts[0].bytes().all(|byte| byte.is_ascii_digit()));
    assert!(
        parts[1..]
            .iter()
            .all(|word| { !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()) })
    );
    assert_eq!(
        generated.namespaced_key().as_str(),
        format!("{ROOM_CODE_NAMESPACE_PREFIX}000042")
    );
}

#[test]
fn recognized_invalid_invite_does_not_fallback() {
    let version_two = encoded_json(r#"{"version":2}"#);
    let cases = [
        format!("{QR_OUTER_PREFIX}{version_two}"),
        format!("envoix://invite/v3/{version_two}"),
        format!("envoix://invite/v2/{PAYLOAD}"),
        format!("envoix://invite/v03/{PAYLOAD}"),
        "envoix://pair/123456-amber-comet?broker=legacy".to_owned(),
        "123456".to_owned(),
        "123456-amber-comet".to_owned(),
        "envoix://other/value".to_owned(),
        "ENVOIX://invite/value".to_owned(),
    ];

    for input in cases {
        assert!(
            matches!(route_invite(&input), Err(InviteError::RecognizedInvalid(_))),
            "{input:?} must be terminally recognized-invalid"
        );
    }

    assert_eq!(
        route_invite("https://example.test/not-envoix"),
        Err(InviteError::NotEnvoixInvite)
    );
    assert_eq!(
        route_invite(&format!("{QR_OUTER_PREFIX}{version_two}")),
        Err(InviteError::RecognizedInvalid(
            RecognizedInvalid::UnsupportedPayloadVersion {
                found: 2,
                expected: INVITE_PAYLOAD_VERSION,
            }
        ))
    );
    assert_eq!(
        route_invite(&format!("envoix://invite/v2/{PAYLOAD}")),
        Err(InviteError::RecognizedInvalid(
            RecognizedInvalid::UnsupportedPayloadVersion {
                found: 2,
                expected: INVITE_PAYLOAD_VERSION,
            }
        ))
    );
    assert_eq!(
        route_invite("envoix://pair/123456-amber-comet"),
        Err(InviteError::RecognizedInvalid(
            RecognizedInvalid::LegacyPairDeepLink
        ))
    );
    assert_eq!(
        route_invite("123456-amber-comet"),
        Err(InviteError::RecognizedInvalid(
            RecognizedInvalid::BareRoomCode
        ))
    );
}

#[test]
fn current_dialect_is_bounded_typed_and_forward_additive() {
    assert_eq!(
        route_invite("envoix:not-base64!"),
        Err(InviteError::MalformedBase64)
    );
    assert_eq!(
        route_invite(&format!("envoix:{}", encoded_json("not json"))),
        Err(InviteError::MalformedPayload)
    );
    assert_eq!(
        route_invite(&format!("envoix:{}", URL_SAFE_NO_PAD.encode([0xff, 0xfe]))),
        Err(InviteError::MalformedPayload)
    );
    assert_eq!(
        route_invite(&format!(
            "envoix:{}",
            encoded_json(r#"{"version":3,"code":"123456-amber-comet"}"#)
        )),
        Err(InviteError::MalformedPayload)
    );
    assert_eq!(
        route_invite(&format!(
            "envoix:{}",
            encoded_json(
                r#"{"version":3,"code":"123456-amber-comet","broker":"b","relay":"r","role":"sideways"}"#
            )
        )),
        Err(InviteError::MalformedPayload)
    );
    assert_eq!(
        route_invite(&format!(
            "envoix:{}",
            encoded_json(r#"{"version":3,"code":"bad","broker":"b","relay":"r","role":"send"}"#)
        )),
        Err(InviteError::InvalidField(InviteField::Code))
    );

    let additive = encoded_json(
        r#"{"version":3,"code":"123456-amber-comet","broker":"node@test.example:9445","relay":"https://relay.test.envoix.chkxwlyh.us:9444","role":"send","future_hint":{"x":1}}"#,
    );
    assert_eq!(
        route_invite(&format!("envoix:{additive}")).unwrap(),
        Invite::new(CODE, BROKER, RELAY, Role::Send).unwrap()
    );

    let overlong_input = "x".repeat(MAX_INVITE_INPUT_LENGTH + 1);
    assert!(matches!(
        route_invite(&overlong_input),
        Err(InviteError::InputTooLong { .. })
    ));
    let overlong_whitespace = " ".repeat(MAX_INVITE_INPUT_LENGTH + 1);
    assert!(matches!(
        route_invite(&overlong_whitespace),
        Err(InviteError::InputTooLong { .. })
    ));
    let encoded_too_long = format!("envoix:{}", "A".repeat(MAX_ENCODED_PAYLOAD_LENGTH + 1));
    assert!(matches!(
        route_invite(&encoded_too_long),
        Err(InviteError::EncodedPayloadTooLong { .. })
    ));
    let decoded_too_long = format!(
        "envoix:{}",
        URL_SAFE_NO_PAD.encode(vec![b' '; MAX_DECODED_PAYLOAD_LENGTH + 1])
    );
    assert!(matches!(
        route_invite(&decoded_too_long),
        Err(InviteError::DecodedPayloadTooLong { .. })
    ));
    assert_eq!(
        Invite::new(
            CODE,
            BROKER,
            "r".repeat(MAX_RELAY_LENGTH + 1),
            Role::Receive
        ),
        Err(InviteError::InvalidField(InviteField::Relay))
    );

    assert_eq!(
        generate_room_code(&mut ScriptedEntropy::new(Vec::new())),
        Err(InviteError::EntropyUnavailable)
    );
    assert_eq!(
        generate_room_code(&mut ScriptedEntropy::new(vec![u32::MAX; 32])),
        Err(InviteError::UnusableEntropy)
    );
}

#[test]
fn room_codes_and_invites_redact_the_pairing_secret() {
    let invite = Invite::new(CODE, BROKER, RELAY, Role::Send).unwrap();
    let room_key = invite.code().namespaced_key();

    assert_eq!(
        room_key.as_str(),
        format!("{ROOM_CODE_NAMESPACE_PREFIX}123456")
    );
    assert!(!room_key.as_str().contains("amber"));
    assert!(!room_key.as_str().contains("comet"));
    assert!(!format!("{:?}", invite.code()).contains(CODE));
    assert!(!format!("{invite:?}").contains(CODE));
    assert!(!format!("{room_key:?}").contains(CODE));

    assert_eq!(
        crate::NamespacedRoomKey::parse(room_key.as_str())
            .unwrap()
            .as_str(),
        room_key.as_str()
    );
    assert!(crate::NamespacedRoomKey::parse("v2:123456-amber-comet").is_err());
    assert!(crate::NamespacedRoomKey::parse("123456").is_err());
}

fn encoded_json(json: &str) -> String {
    URL_SAFE_NO_PAD.encode(json.as_bytes())
}
