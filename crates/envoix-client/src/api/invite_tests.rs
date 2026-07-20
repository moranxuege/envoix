use super::*;

const BROKER: &str = "e946a31a@67.230.187.238:8445";
const RELAY: &str = "https://envoix.example:8444";

#[test]
fn room_generates_a_typeable_code() {
    let inv = Invite::room(BROKER.into(), Some(RELAY.into())).unwrap();
    // <digits>-<word>-<word>
    assert_eq!(inv.code().split('-').count(), 3);
    assert!(
        inv.code()
            .split('-')
            .next()
            .unwrap()
            .chars()
            .all(|c| c.is_ascii_digit())
    );
}

#[test]
fn payload_round_trips_through_parse() {
    let inv = Invite::room(BROKER.into(), Some(RELAY.into())).unwrap();
    let parsed = Invite::parse(&inv.payload()).unwrap();
    assert_eq!(parsed, inv);
    // The reserved-char values survive encoding.
    assert_eq!(parsed.broker.as_deref(), Some(BROKER));
    assert_eq!(parsed.relay(), Some(RELAY));
}

#[test]
fn typed_code_and_full_url_converge_on_the_code() {
    let inv = Invite::room("id@1.2.3.4:5".into(), None).unwrap();
    let code = inv.code().to_string();
    let from_url = Invite::parse(&inv.payload()).unwrap();
    let from_code = Invite::parse(&code).unwrap();
    assert_eq!(from_url.code(), from_code.code());
    // A bare code has no transport hints; they come from config/defaults.
    assert_eq!(from_code.broker, None);
    assert_eq!(from_url.broker.as_deref(), Some("id@1.2.3.4:5"));
}

#[test]
fn unknown_params_are_ignored_for_forward_compat() {
    let url =
        "envoix://pair/1234-amber-comet?broker=id%40h%3A1&direct=9.9.9.9%3A1&mdns=envoix-x";
    let inv = Invite::parse(url).unwrap();
    assert_eq!(inv.code(), "1234-amber-comet");
    assert_eq!(inv.broker.as_deref(), Some("id@h:1"));
}

#[test]
fn role_hint_round_trips_and_flips() {
    let inv = Invite::room("id@h:1".into(), None)
        .unwrap()
        .with_role(Role::Send);
    let parsed = Invite::parse(&inv.payload()).unwrap();
    assert_eq!(parsed.role(), Some(Role::Send));
    assert_eq!(parsed.role().unwrap().opposite(), Role::Receive);
    // A bare code carries no role.
    assert_eq!(Invite::parse("1234-amber-comet").unwrap().role(), None);
}

#[test]
fn empty_code_is_rejected() {
    assert!(Invite::parse("").is_err());
    assert!(Invite::parse("envoix://pair/").is_err());
}

#[test]
fn malformed_bare_codes_are_rejected() {
    for input in [
        "https://example.com/not-a-code",
        "amber-comet",
        "room-amber-comet",
        "123456-amber",
        "123456-amber-comet-extra",
        "123456-amber-comet!",
        "123-amber-comet",
        "1234567-amber-comet",
    ] {
        assert!(
            Invite::parse(input).is_err(),
            "accepted malformed code: {input}"
        );
    }
}

#[test]
fn legacy_and_current_nameplate_lengths_are_accepted() {
    assert!(Invite::parse("1234-amber-comet").is_ok());
    assert!(Invite::parse("123456-amber-comet").is_ok());
}

#[test]
fn peer_source_uses_fallback_broker_for_a_bare_code() {
    let inv = Invite::parse("1234-amber-comet").unwrap();
    assert!(inv.peer_source(None).is_err());
    let source = inv.peer_source(Some("id@h:1".into())).unwrap();
    assert!(matches!(source, PeerSource::Room { broker, .. } if broker == "id@h:1"));
}
