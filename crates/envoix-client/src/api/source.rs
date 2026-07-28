//! How the two peers of a transfer find and authenticate each other.

use envoix_invite::InvitationBootstrap;
use envoix_protocol::PeerDescriptor;
use envoix_session::RememberedCredential;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The rendezvous mode selected for a Manifest v2 session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferMode {
    Manual,
    Invitation,
    Remembered,
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

/// Opaque handle to a developer token retained only for this process.
#[derive(Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SharedTokenRef(String);

impl std::fmt::Debug for SharedTokenRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SharedTokenRef(<redacted>)")
    }
}

/// Opaque process handle to a credential loaded from protected storage.
#[derive(Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RememberedCredentialRef(String);

impl RememberedCredentialRef {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_process_handle(value: String) -> Self {
        Self(value)
    }
}

impl std::fmt::Debug for RememberedCredentialRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RememberedCredentialRef(<redacted>)")
    }
}

/// A validated peer source. Invitation credentials remain behind an opaque
/// process-memory reference. Manual and mDNS developer tokens use the same
/// reference-only policy and therefore do not survive an app restart.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PeerSource {
    Manual {
        peer: PeerDescriptor,
        token_ref: SharedTokenRef,
    },
    Invitation {
        secret_ref: InviteSecretRef,
        room_id: String,
        broker: String,
    },
    Remembered {
        credential_ref: RememberedCredentialRef,
        generation: u64,
        previous_generation: Option<u64>,
        broker: String,
    },
    ShowManual {
        token_ref: Option<SharedTokenRef>,
    },
    Mdns {
        token_ref: Option<SharedTokenRef>,
    },
}

impl PeerSource {
    pub fn manual(peer: PeerDescriptor, token: String) -> Result<Self, super::TransferError> {
        Ok(Self::Manual {
            peer,
            token_ref: shared_token_store().insert(token)?,
        })
    }

    pub fn show_manual(token: Option<String>) -> Result<Self, super::TransferError> {
        Ok(Self::ShowManual {
            token_ref: token
                .map(|token| shared_token_store().insert(token))
                .transpose()?,
        })
    }

    pub fn mdns(token: Option<String>) -> Result<Self, super::TransferError> {
        Ok(Self::Mdns {
            token_ref: token
                .map(|token| shared_token_store().insert(token))
                .transpose()?,
        })
    }

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

    pub fn remembered(
        opaque_credential: &[u8],
        generation: u64,
        previous_generation: Option<u64>,
        broker: String,
    ) -> Result<Self, super::TransferError> {
        let credential_ref = register_remembered_credential(opaque_credential)?;
        Ok(Self::remembered_registered(
            credential_ref,
            generation,
            previous_generation,
            broker,
        ))
    }

    pub fn remembered_registered(
        credential_ref: RememberedCredentialRef,
        generation: u64,
        previous_generation: Option<u64>,
        broker: String,
    ) -> Self {
        Self::Remembered {
            credential_ref,
            generation,
            previous_generation: previous_generation.filter(|previous| *previous < generation),
            broker,
        }
    }

    pub fn mode(&self) -> TransferMode {
        match self {
            Self::Manual { .. } => TransferMode::Manual,
            Self::Invitation { .. } => TransferMode::Invitation,
            Self::Remembered { .. } => TransferMode::Remembered,
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
struct SharedTokenStore {
    entries: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

#[derive(Default)]
struct RememberedCredentialStore {
    entries: std::sync::Mutex<std::collections::HashMap<String, RememberedCredential>>,
}

impl RememberedCredentialStore {
    fn insert(
        &self,
        credential: RememberedCredential,
    ) -> Result<RememberedCredentialRef, super::TransferError> {
        let reference = random_reference("remembered credential")?;
        self.entries
            .lock()
            .map_err(|_| super::TransferError::input("remembered credential store is unavailable"))?
            .insert(reference.clone(), credential);
        Ok(RememberedCredentialRef(reference))
    }

    fn get(
        &self,
        credential_ref: &RememberedCredentialRef,
    ) -> Result<RememberedCredential, super::TransferError> {
        self.entries
            .lock()
            .map_err(|_| super::TransferError::input("remembered credential store is unavailable"))?
            .get(&credential_ref.0)
            .cloned()
            .ok_or_else(|| {
                super::TransferError::input(
                    "remembered credential is unavailable; reload it from protected storage",
                )
            })
    }
}

impl SharedTokenStore {
    fn insert(&self, token: String) -> Result<SharedTokenRef, super::TransferError> {
        let reference = random_reference("shared token")?;
        self.entries
            .lock()
            .map_err(|_| super::TransferError::input("shared token store is unavailable"))?
            .insert(reference.clone(), token);
        Ok(SharedTokenRef(reference))
    }

    fn get(&self, token_ref: &SharedTokenRef) -> Result<String, super::TransferError> {
        self.entries
            .lock()
            .map_err(|_| super::TransferError::input("shared token store is unavailable"))?
            .get(&token_ref.0)
            .cloned()
            .ok_or_else(|| {
                super::TransferError::input(
                    "developer token is unavailable; enter a new token or restart the transfer",
                )
            })
    }
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
        let value = random_reference("invitation")?;
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
            consumption: InvitationConsumption {
                consumed: Arc::new(AtomicBool::new(false)),
            },
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

fn random_reference(kind: &str) -> Result<String, super::TransferError> {
    use base64::Engine as _;

    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| super::TransferError::input(format!("failed to generate {kind} reference")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn invitation_store() -> &'static InviteSecretStore {
    static STORE: std::sync::OnceLock<InviteSecretStore> = std::sync::OnceLock::new();
    STORE.get_or_init(InviteSecretStore::default)
}

fn shared_token_store() -> &'static SharedTokenStore {
    static STORE: std::sync::OnceLock<SharedTokenStore> = std::sync::OnceLock::new();
    STORE.get_or_init(SharedTokenStore::default)
}

fn remembered_credential_store() -> &'static RememberedCredentialStore {
    static STORE: std::sync::OnceLock<RememberedCredentialStore> = std::sync::OnceLock::new();
    STORE.get_or_init(RememberedCredentialStore::default)
}

pub struct InvitationLease {
    secret_ref: InviteSecretRef,
    bootstrap: InvitationBootstrap,
    consumption: InvitationConsumption,
}

/// Shared one-way marker used to burn an invitation at authentication time.
#[derive(Clone)]
pub struct InvitationConsumption {
    consumed: Arc<AtomicBool>,
}

impl InvitationConsumption {
    /// Permanently mark the invitation consumed after data-plane authentication.
    pub fn consume(&self) {
        self.consumed.store(true, Ordering::Release);
    }
}

impl InvitationLease {
    pub fn bootstrap(&self) -> &InvitationBootstrap {
        &self.bootstrap
    }

    /// Obtain a marker that can be moved into an authentication callback.
    pub fn consumption(&self) -> InvitationConsumption {
        self.consumption.clone()
    }

    /// Permanently mark the invitation consumed.
    pub fn consume(&self) {
        self.consumption.consume();
    }
}

impl Drop for InvitationLease {
    fn drop(&mut self) {
        invitation_store().release(
            &self.secret_ref,
            self.consumption.consumed.load(Ordering::Acquire),
        );
    }
}

pub fn acquire_invitation(
    secret_ref: &InviteSecretRef,
) -> Result<InvitationLease, super::TransferError> {
    invitation_store().acquire(secret_ref)
}

pub fn acquire_shared_token(token_ref: &SharedTokenRef) -> Result<String, super::TransferError> {
    shared_token_store().get(token_ref)
}

pub fn acquire_remembered_credential(
    credential_ref: &RememberedCredentialRef,
) -> Result<RememberedCredential, super::TransferError> {
    remembered_credential_store().get(credential_ref)
}

pub fn register_remembered_credential(
    opaque_credential: &[u8],
) -> Result<RememberedCredentialRef, super::TransferError> {
    let credential = RememberedCredential::from_opaque(opaque_credential)
        .map_err(|error| super::TransferError::input(error.to_string()))?;
    remembered_credential_store().insert(credential)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use envoix_invite::{Capabilities, InviteV2, TransferRole};

    fn peer() -> PeerDescriptor {
        "2cfu7vzc7zhqv6w3k7m2kkwqvwzppmzvv53lmst6xm7ubjx5qnya@127.0.0.1:11204"
            .parse()
            .expect("test descriptor")
    }

    #[test]
    fn serialized_sources_never_contain_raw_developer_tokens() {
        let canary = "manual-token-canary-123";
        let sources = [
            PeerSource::manual(peer(), canary.into()).expect("manual source"),
            PeerSource::show_manual(Some(canary.into())).expect("manual receiver source"),
            PeerSource::mdns(Some(canary.into())).expect("mDNS source"),
        ];

        for source in sources {
            let encoded = serde_json::to_string(&source).expect("serialize source");
            assert!(!encoded.contains(canary));
            assert!(!format!("{source:?}").contains(canary));
        }
    }

    #[test]
    fn token_reference_resolves_only_through_the_process_store() {
        let source = PeerSource::mdns(Some("shared-token-123".into())).expect("mDNS source");
        let PeerSource::Mdns {
            token_ref: Some(token_ref),
        } = source
        else {
            panic!("expected mDNS token reference");
        };

        assert_eq!(
            acquire_shared_token(&token_ref).expect("stored token"),
            "shared-token-123"
        );
    }

    #[test]
    fn serialized_remembered_source_contains_only_a_random_reference() {
        let mut opaque = b"ENVR".to_vec();
        opaque.push(1);
        opaque.extend_from_slice(&[0xa5; 32]);
        let source = PeerSource::remembered(&opaque, 9, Some(8), "broker".into())
            .expect("remembered source");
        let encoded = serde_json::to_string(&source).expect("serialize source");

        assert!(!encoded.contains("a5"));
        assert!(
            !encoded.contains(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(opaque))
        );
        assert!(encoded.contains("\"generation\":9"));
    }

    #[test]
    fn invitation_lease_retries_before_consumption_and_burns_afterward() {
        let created = InviteV2::create(
            "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445"
                .into(),
            Vec::new(),
            TransferRole::Sender,
            Capabilities::current(),
            1_750_000_000,
        )
        .expect("create invitation");
        let source = PeerSource::invitation(created.into_bootstrap(), "broker".into())
            .expect("store invitation");
        let PeerSource::Invitation { secret_ref, .. } = source else {
            panic!("expected invitation source");
        };

        drop(acquire_invitation(&secret_ref).expect("first lease"));
        let lease = acquire_invitation(&secret_ref).expect("pre-authentication retry");
        lease.consumption().consume();
        drop(lease);

        assert!(acquire_invitation(&secret_ref).is_err());
    }
}
