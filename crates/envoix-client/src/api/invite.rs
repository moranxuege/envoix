//! A pairing invite: the receiver's advertisement of how to reach it, plus the
//! pairing code that authenticates any transport. One side builds an invite and
//! shows the [`code`](Invite::code) and/or a QR of its [`payload`](Invite::payload);
//! the other side [`parse`](Invite::parse)s a typed code or scanned payload.
//!
//! First cut carries the **room** (rendezvous) method only; `direct`/`mdns`/`node`
//! are additive later - unknown payload params are ignored so newer invites stay
//! parseable. See `docs/design/invite.md`.

use envoix_rendezvous_iroh::generate_code;

use super::{PeerSource, TransferError};

/// URL scheme + path prefix for an invite payload.
const SCHEME: &str = "envoix://pair/";
/// Word count in a generated code (`<digits>-<word>-<word>`).
const CODE_WORDS: usize = 2;

/// A pairing invite. Auth is SPAKE2 with [`code`](Invite::code) over whatever
/// transport connects, so the same invite works direct, over a relay, or through
/// a broker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invite {
    /// The pairing code, e.g. `144055-cobalt-flint`: the SPAKE2 password, whose
    /// digit prefix is the broker room id.
    code: String,
    /// Broker `<endpoint-id>@<ip:port>` for the room method, when advertised.
    broker: Option<String>,
    /// Relay URL for WAN/NAT reachability, when advertised.
    relay: Option<String>,
}

impl Invite {
    /// Build a room invite with a freshly generated code.
    pub fn room(broker: String, relay: Option<String>) -> Result<Self, TransferError> {
        Ok(Self {
            code: generate_code(CODE_WORDS).map_err(TransferError::input)?,
            broker: Some(broker),
            relay,
        })
    }

    /// The pairing code, for display or typed entry.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// A shareable `envoix://` URL carrying every advertised method - the QR
    /// payload. The typed [`code`](Invite::code) is the subset without transports.
    pub fn payload(&self) -> String {
        let mut params = Vec::new();
        if let Some(broker) = &self.broker {
            params.push(format!("broker={}", encode(broker)));
        }
        if let Some(relay) = &self.relay {
            params.push(format!("relay={}", encode(relay)));
        }
        let mut url = format!("{SCHEME}{}", self.code);
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        url
    }

    /// Parse a typed code (`144055-cobalt-flint`) or a scanned `envoix://` URL.
    /// Unknown query params are ignored, so payloads from newer versions (extra
    /// transports) still parse - they just contribute no method this side knows.
    pub fn parse(input: &str) -> Result<Self, TransferError> {
        let input = input.trim();
        let Some(rest) = input.strip_prefix(SCHEME) else {
            return Self::from_code(input); // bare code; transports come from config
        };
        let (code, query) = rest.split_once('?').unwrap_or((rest, ""));
        let mut invite = Self::from_code(&decode(code))?;
        for pair in query.split('&').filter(|s| !s.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match key {
                "broker" => invite.broker = Some(decode(value)),
                "relay" => invite.relay = Some(decode(value)),
                _ => {} // reserved for future methods (direct, mdns, node)
            }
        }
        Ok(invite)
    }

    /// The [`PeerSource`] this invite drives. `fallback_broker` is used when the
    /// invite carried no broker (a bare typed code), e.g. from CLI `--rendezvous`.
    pub fn peer_source(&self, fallback_broker: Option<String>) -> Result<PeerSource, TransferError> {
        let broker = self.broker.clone().or(fallback_broker).ok_or_else(|| {
            TransferError::input("room pairing needs a broker (pass --rendezvous or scan a full invite)")
        })?;
        Ok(PeerSource::Room {
            code: self.code.clone(),
            broker,
        })
    }

    /// The relay this invite advertised, if any.
    pub fn relay(&self) -> Option<&str> {
        self.relay.as_deref()
    }

    fn from_code(code: &str) -> Result<Self, TransferError> {
        let code = code.trim();
        if code.is_empty() {
            return Err(TransferError::input("empty pairing code"));
        }
        Ok(Self {
            code: code.to_string(),
            broker: None,
            relay: None,
        })
    }
}

/// Percent-encode a query-parameter value, keeping only RFC 3986 unreserved bytes.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Reverse [`encode`]; malformed `%` escapes are left as-is.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 3 <= bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BROKER: &str = "e946a31a@67.230.187.238:8445";
    const RELAY: &str = "https://envoix.example:8444";

    #[test]
    fn room_generates_a_typeable_code() {
        let inv = Invite::room(BROKER.into(), Some(RELAY.into())).unwrap();
        // <digits>-<word>-<word>
        assert_eq!(inv.code().split('-').count(), 3);
        assert!(inv.code().split('-').next().unwrap().chars().all(|c| c.is_ascii_digit()));
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
        let url = "envoix://pair/1234-amber-comet?broker=id%40h%3A1&direct=9.9.9.9%3A1&mdns=envoix-x";
        let inv = Invite::parse(url).unwrap();
        assert_eq!(inv.code(), "1234-amber-comet");
        assert_eq!(inv.broker.as_deref(), Some("id@h:1"));
    }

    #[test]
    fn empty_code_is_rejected() {
        assert!(Invite::parse("").is_err());
        assert!(Invite::parse("envoix://pair/").is_err());
    }

    #[test]
    fn peer_source_uses_fallback_broker_for_a_bare_code() {
        let inv = Invite::parse("1234-amber-comet").unwrap();
        assert!(inv.peer_source(None).is_err());
        let source = inv.peer_source(Some("id@h:1".into())).unwrap();
        assert!(matches!(source, PeerSource::Room { broker, .. } if broker == "id@h:1"));
    }
}
