//! A pairing invite: the receiver's advertisement of how to reach it, plus the
//! pairing code that authenticates any transport. One side builds an invite and
//! shows the [`code`](Invite::code) and/or a QR of its [`payload`](Invite::payload);
//! the other side [`parse`](Invite::parse)s a typed code or scanned payload.
//!
//! First cut carries the **room** (rendezvous) method only; `direct`/`mdns`/`node`
//! are additive later - unknown payload params are ignored so newer invites stay
//! parseable. See `docs/design/invite.md`.

use envoix_session::generate_code;

use super::{PeerSource, TransferError};

/// URL scheme + path prefix for an invite payload.
const SCHEME: &str = "envoix://pair/";
/// Word count in a generated code (`<digits>-<word>-<word>`).
const CODE_WORDS: usize = 2;
/// Older clients used four-digit numeric nameplates; current clients use six.
const MIN_NAMEPLATE_DIGITS: usize = 4;
const MAX_NAMEPLATE_DIGITS: usize = 6;

/// The role the invite's creator will take; a peer that scans/opens it should
/// take the [`opposite`](Role::opposite). A hint only - the transfer still runs
/// whichever command each side chooses; this just lets a scanner avoid the
/// two-senders / two-receivers mistake.
///
/// The invite-layer mirror of `envoix_types::PeerRole` (`Send` == `Sender`),
/// kept separate so the public invite API and its QR wiring (`send`/`receive`)
/// can evolve without disturbing the data-plane wire enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Send,
    Receive,
}

impl Role {
    /// The complementary role - what a peer joining this invite should take.
    pub fn opposite(self) -> Role {
        match self {
            Role::Send => Role::Receive,
            Role::Receive => Role::Send,
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Role::Send => "send",
            Role::Receive => "receive",
        }
    }

    fn from_wire(s: &str) -> Option<Role> {
        match s {
            "send" => Some(Role::Send),
            "receive" => Some(Role::Receive),
            _ => None,
        }
    }
}

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
    /// The role the creator will take; a joiner takes the opposite. Hint only.
    role: Option<Role>,
}

impl Invite {
    /// Build a room invite with a freshly generated code.
    pub fn room(broker: String, relay: Option<String>) -> Result<Self, TransferError> {
        Ok(Self {
            code: generate_code(CODE_WORDS).map_err(TransferError::input)?,
            broker: Some(broker),
            relay,
            role: None,
        })
    }

    /// Advertise the role the creator will take, so a peer that scans this can
    /// auto-select the opposite and avoid a two-senders / two-receivers mistake.
    pub fn with_role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    /// The pairing code, for display or typed entry.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The role the creator advertised, if any; a joiner should take its
    /// [`opposite`](Role::opposite).
    pub fn role(&self) -> Option<Role> {
        self.role
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
        if let Some(role) = self.role {
            params.push(format!("role={}", role.wire()));
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
                "role" => invite.role = Role::from_wire(&decode(value)),
                _ => {} // reserved for future methods (direct, mdns, node)
            }
        }
        Ok(invite)
    }

    /// The [`PeerSource`] this invite drives. `fallback_broker` is used when the
    /// invite carried no broker (a bare typed code), e.g. from CLI `--rendezvous`.
    pub fn peer_source(
        &self,
        fallback_broker: Option<String>,
    ) -> Result<PeerSource, TransferError> {
        let broker = self.broker.clone().or(fallback_broker).ok_or_else(|| {
            TransferError::input(
                "room pairing needs a broker (pass --rendezvous or scan a full invite)",
            )
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

    /// The broker this invite advertised, if any.
    pub fn broker(&self) -> Option<&str> {
        self.broker.as_deref()
    }

    fn from_code(code: &str) -> Result<Self, TransferError> {
        let code = code.trim();
        if code.is_empty() {
            return Err(TransferError::input("empty pairing code"));
        }
        if !is_pairing_code(code) {
            return Err(TransferError::input(
                "pairing code must have the form <digits>-<word>-<word>",
            ));
        }
        Ok(Self {
            code: code.to_string(),
            broker: None,
            relay: None,
            role: None,
        })
    }
}

fn is_pairing_code(code: &str) -> bool {
    let mut parts = code.split('-');
    let Some(nameplate) = parts.next() else {
        return false;
    };
    let Some(first_word) = parts.next() else {
        return false;
    };
    let Some(second_word) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }

    (MIN_NAMEPLATE_DIGITS..=MAX_NAMEPLATE_DIGITS).contains(&nameplate.len())
        && nameplate.bytes().all(|byte| byte.is_ascii_digit())
        && [first_word, second_word]
            .into_iter()
            .all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_alphabetic()))
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
#[path = "invite_tests.rs"]
mod tests;
