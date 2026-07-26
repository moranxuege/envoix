use envoix_invite::{Invite, InviteError, Role, encode_deep_link};
use serde::{Deserialize, Serialize};

/// The rendezvous channel a card is frozen to when it is created.
///
/// A send mints one; a join adopts the one its invite carried, endpoints
/// included (`SF06`). It is durable card truth rather than a live setting, so a
/// restarted app still shows the invite it published and no card can have its
/// channel changed underneath it mid-flow (`XR04`, `XS02`).
///
/// What is stored is the invite's own fields, not its encoded text: the
/// shareable form is derived by the same L1 encoder that produced it, so an
/// invite has exactly one spelling and there is no second copy to drift.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairingChannel {
    code: String,
    broker: String,
    relay: String,
    /// The role the invite DECLARES — its creator's. A joiner takes the
    /// opposite (`SF06`), which is why this is kept as received rather than as
    /// "mine": re-encoding has to yield the same invite text.
    role: Role,
}

impl PairingChannel {
    pub fn from_invite(invite: &Invite) -> Self {
        Self {
            code: invite.code().as_str().to_owned(),
            broker: invite.broker().to_owned(),
            relay: invite.relay().to_owned(),
            role: invite.role(),
        }
    }

    /// The invite these fields spell, re-validated by the grammar that owns it.
    pub fn invite(&self) -> Result<Invite, InviteError> {
        Invite::new(
            self.code.clone(),
            self.broker.clone(),
            self.relay.clone(),
            self.role,
        )
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub const fn role(&self) -> Role {
        self.role
    }

    /// The canonical shareable text, or `None` when the stored fields no longer
    /// form a valid invite. A record whose channel cannot be re-encoded shows
    /// no invite rather than a half-built one.
    pub fn shareable(&self) -> Option<String> {
        self.invite()
            .ok()
            .and_then(|invite| encode_deep_link(&invite).ok())
    }
}
