//! How the two peers of a transfer find and authenticate each other.

use envoix_invite::InvitationBootstrap;
use envoix_protocol::PeerDescriptor;
use serde::{Deserialize, Serialize};

/// The rendezvous mode selected for a Manifest v2 session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferMode {
    Manual,
    Invitation,
    ShowManual,
    Mdns,
}

/// Opaque handle to private one-use invitation state.
#[derive(Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct InviteSecretRef(String);

impl std::fmt::Debug for InviteSecretRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InviteSecretRef(<redacted>)")
    }
}

/// A validated peer source. Invitation credentials remain behind an opaque
/// process-memory reference and therefore do not survive an app restart.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PeerSource {
    Manual {
        peer: PeerDescriptor,
        token: String,
    },
    Invitation {
        secret_ref: InviteSecretRef,
        room_id: String,
        broker: String,
    },
    ShowManual {
        token: Option<String>,
    },
    Mdns {
        token: Option<String>,
    },
}

impl PeerSource {
    pub fn invitation(
        bootstrap: InvitationBootstrap,
        broker: String,
    ) -> Result<Self, super::TransferError> {
        let room_id = bootstrap.room_id().to_string();
        Ok(Self::Invitation {
            secret_ref: invitation_store().insert(bootstrap)?,
            room_id,
            broker,
        })
    }

    pub fn mode(&self) -> TransferMode {
        match self {
            Self::Manual { .. } => TransferMode::Manual,
            Self::Invitation { .. } => TransferMode::Invitation,
            Self::ShowManual { .. } => TransferMode::ShowManual,
            Self::Mdns { .. } => TransferMode::Mdns,
        }
    }
}

#[derive(Clone)]
enum StoredState {
    Available(InvitationBootstrap),
    InProgress(InvitationBootstrap),
    Consumed,
}

#[derive(Default)]
struct InviteSecretStore {
    entries: std::sync::Mutex<std::collections::HashMap<String, StoredState>>,
}

impl InviteSecretStore {
    fn insert(
        &self,
        bootstrap: InvitationBootstrap,
    ) -> Result<InviteSecretRef, super::TransferError> {
        use base64::Engine as _;

        let mut bytes = [0_u8; 16];
        getrandom::fill(&mut bytes)
            .map_err(|_| super::TransferError::input("failed to generate invitation reference"))?;
        let value = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        self.entries
            .lock()
            .map_err(|_| super::TransferError::input("invitation secret store is unavailable"))?
            .insert(value.clone(), StoredState::Available(bootstrap));
        Ok(InviteSecretRef(value))
    }

    fn acquire(
        &self,
        secret_ref: &InviteSecretRef,
    ) -> Result<InvitationLease, super::TransferError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| super::TransferError::input("invitation secret store is unavailable"))?;
        let state = entries.get_mut(&secret_ref.0).ok_or_else(|| {
            super::TransferError::input("invitation private state is unavailable")
        })?;
        let bootstrap = match state {
            StoredState::Available(bootstrap) => bootstrap.clone(),
            StoredState::InProgress(_) | StoredState::Consumed => {
                return Err(super::TransferError::input(
                    "invitation replay: private state is already in use or consumed",
                ));
            }
        };
        *state = StoredState::InProgress(bootstrap.clone());
        Ok(InvitationLease {
            secret_ref: secret_ref.clone(),
            bootstrap,
            consumed: false,
        })
    }

    fn release(&self, secret_ref: &InviteSecretRef, consumed: bool) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        let Some(state) = entries.get_mut(&secret_ref.0) else {
            return;
        };
        if matches!(state, StoredState::InProgress(_)) {
            if consumed {
                *state = StoredState::Consumed;
            } else if let StoredState::InProgress(bootstrap) = state {
                *state = StoredState::Available(bootstrap.clone());
            }
        }
    }
}

fn invitation_store() -> &'static InviteSecretStore {
    static STORE: std::sync::OnceLock<InviteSecretStore> = std::sync::OnceLock::new();
    STORE.get_or_init(InviteSecretStore::default)
}

pub struct InvitationLease {
    secret_ref: InviteSecretRef,
    bootstrap: InvitationBootstrap,
    consumed: bool,
}

impl InvitationLease {
    pub fn bootstrap(&self) -> &InvitationBootstrap {
        &self.bootstrap
    }

    pub fn consume(&mut self) {
        self.consumed = true;
    }
}

impl Drop for InvitationLease {
    fn drop(&mut self) {
        invitation_store().release(&self.secret_ref, self.consumed);
    }
}

pub fn acquire_invitation(
    secret_ref: &InviteSecretRef,
) -> Result<InvitationLease, super::TransferError> {
    invitation_store().acquire(secret_ref)
}
