use std::path::PathBuf;
use std::sync::Arc;

use envoix_client::api::{
    Client, IdentityConfig, RememberedCredentialRef, RememberedRoomControlConnectError,
    RememberedRoomControlRole, RendezvousCause, RoomCloseReason, RoomControlEvent,
    RoomControlInvite, RoomControlSession, RoomLifetimePolicy, RoomLifetimeState,
    RoomOfferRejection, RoomTransferOffer, SessionError, TransferCancelToken, TransferOptions,
    acquire_remembered_credential, connect_remembered_room_control, connect_room_control,
};
use envoix_error::CoreError;

use crate::{
    DEFAULT_RELAY_URL, DEFAULT_RENDEZVOUS_BROKER, EnvoixError, FfiFailureCode,
    FfiRememberedCredentialVault, non_empty, op_err, spawn_on_ffi_runtime,
};

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiRoomControlInvite {
    pub code: String,
    pub payload: String,
    pub broker: String,
    pub relay: String,
    pub expires_at_epoch_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiRoomConnectMode {
    Host,
    Join,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiRememberedRoomConnectMode {
    Connector,
    Responder,
}

/// A failed single-generation attempt. Generation fallback is safe only when
/// `peer_authenticated` is false.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiRememberedRoomConnectError {
    #[error("{reason}")]
    Failed {
        reason: String,
        peer_authenticated: bool,
        /// Stable broker cause for recovery policy. Diagnostics must not be parsed.
        failure_code: Option<FfiFailureCode>,
        /// Broker-supplied delay, when present. RoomExpired is identified by
        /// `failure_code`; the broker currently does not transmit its tombstone TTL.
        retry_after_seconds: Option<u64>,
    },
}

/// Stable classifications for operations on an authenticated Room session.
/// Platform adapters must not inspect `reason` to decide whether the Room is
/// still usable.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiRoomControlError {
    #[error("{reason}")]
    Rejected { reason: String },
    #[error("{reason}")]
    NetworkLost { reason: String },
    #[error("{reason}")]
    Canceled { reason: String },
    #[error("{reason}")]
    Failed { reason: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiRoomLifetimePolicy {
    Idle15Minutes,
    UntilForegroundEnds,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiRoomLifetimeState {
    pub revision: u64,
    pub policy: FfiRoomLifetimePolicy,
    pub idle_deadline_epoch_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiRoomCloseReason {
    UserEnded,
    IdleExpired,
    InvitationExpired,
    PeerEnded,
    Backgrounded,
    NetworkLost,
    ProtocolFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiRoomOfferRejection {
    Declined,
    Busy,
    Expired,
    Invalid,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiRoomTransferOffer {
    pub offer_id: String,
    pub transfer_invite: String,
    pub root_names: Vec<String>,
    pub item_count: u32,
    pub directory_count: u32,
    pub total_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiRoomControlEventKind {
    VerificationRequested,
    VerificationSucceeded,
    VerificationFailed,
    IncomingOffer,
    OfferAccepted,
    OfferRejected,
    LifetimeChanged,
    PeerClosed,
    Pong,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiRoomControlEvent {
    pub kind: FfiRoomControlEventKind,
    pub offer: Option<FfiRoomTransferOffer>,
    pub offer_id: String,
    pub rejection: Option<FfiRoomOfferRejection>,
    pub lifetime: Option<FfiRoomLifetimeState>,
    pub close_reason: Option<FfiRoomCloseReason>,
    pub nonce: u64,
}

#[derive(Clone, Eq, PartialEq, uniffi::Record)]
pub struct FfiRoomControlSnapshot {
    pub peer_name: String,
    pub creator: bool,
    pub remembered_generation: Option<u64>,
    pub lifetime: FfiRoomLifetimeState,
}

#[derive(uniffi::Object)]
pub struct FfiRoomControlCancellation {
    token: TransferCancelToken,
}

#[uniffi::export]
impl FfiRoomControlCancellation {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            token: TransferCancelToken::new(),
        })
    }

    pub fn cancel(&self) {
        self.token.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

#[derive(uniffi::Object)]
pub struct FfiRoomControlSession {
    session: Arc<RoomControlSession>,
}

#[uniffi::export]
impl FfiRoomControlSession {
    pub fn snapshot(&self) -> FfiRoomControlSnapshot {
        FfiRoomControlSnapshot {
            peer_name: self.session.peer_name().to_string(),
            creator: self.session.is_creator(),
            remembered_generation: self.session.remembered_generation(),
            lifetime: ffi_lifetime(self.session.lifetime_state()),
        }
    }

    /// Stores a newly verified pairing credential without returning it in a
    /// presentation-facing snapshot.
    pub fn store_pairing_credential(&self, vault: Arc<dyn FfiRememberedCredentialVault>) -> bool {
        self.session
            .pairing_credential()
            .map(|credential| vault.store_remembered_credential(credential.to_opaque(), 0))
            .unwrap_or(false)
    }

    pub fn lifetime_snapshot(&self) -> FfiRoomLifetimeState {
        ffi_lifetime(self.session.lifetime_state())
    }

    pub async fn next_event(&self) -> Result<FfiRoomControlEvent, FfiRoomControlError> {
        let session = self.session.clone();
        spawn_on_ffi_runtime(async move {
            session
                .next_event()
                .await
                .map(project_event)
                .map_err(room_control_err)
        })
        .await
    }

    pub async fn request_verification(
        &self,
        expected_code: String,
    ) -> Result<(), FfiRoomControlError> {
        let session = self.session.clone();
        spawn_on_ffi_runtime(async move {
            session
                .request_verification(&expected_code)
                .await
                .map_err(room_control_err)
        })
        .await
    }

    pub async fn submit_verification_code(&self, code: String) -> Result<(), FfiRoomControlError> {
        let session = self.session.clone();
        spawn_on_ffi_runtime(async move {
            session
                .submit_verification_code(&code)
                .await
                .map_err(room_control_err)
        })
        .await
    }

    pub async fn offer_transfer(
        &self,
        offer: FfiRoomTransferOffer,
    ) -> Result<Option<FfiRoomLifetimeState>, FfiRoomControlError> {
        let session = self.session.clone();
        spawn_on_ffi_runtime(async move {
            session
                .offer_transfer(core_offer(offer))
                .await
                .map(|state| state.map(ffi_lifetime))
                .map_err(room_control_err)
        })
        .await
    }

    pub async fn accept_offer(
        &self,
        offer_id: String,
    ) -> Result<Option<FfiRoomLifetimeState>, FfiRoomControlError> {
        let session = self.session.clone();
        spawn_on_ffi_runtime(async move {
            session
                .accept_offer(&offer_id)
                .await
                .map(|state| state.map(ffi_lifetime))
                .map_err(room_control_err)
        })
        .await
    }

    pub async fn reject_offer(
        &self,
        offer_id: String,
        reason: FfiRoomOfferRejection,
    ) -> Result<Option<FfiRoomLifetimeState>, FfiRoomControlError> {
        let session = self.session.clone();
        spawn_on_ffi_runtime(async move {
            session
                .reject_offer(&offer_id, core_rejection(reason))
                .await
                .map(|state| state.map(ffi_lifetime))
                .map_err(room_control_err)
        })
        .await
    }

    pub async fn set_policy(
        &self,
        policy: FfiRoomLifetimePolicy,
    ) -> Result<Option<FfiRoomLifetimeState>, FfiRoomControlError> {
        let session = self.session.clone();
        spawn_on_ffi_runtime(async move {
            session
                .set_policy(core_policy(policy))
                .await
                .map(|state| state.map(ffi_lifetime))
                .map_err(room_control_err)
        })
        .await
    }

    pub async fn set_local_transfer_active(
        &self,
        active: bool,
    ) -> Result<Option<FfiRoomLifetimeState>, FfiRoomControlError> {
        let session = self.session.clone();
        spawn_on_ffi_runtime(async move {
            session
                .set_local_transfer_active(active)
                .await
                .map(|state| state.map(ffi_lifetime))
                .map_err(room_control_err)
        })
        .await
    }

    pub async fn ping(&self, nonce: u64) -> Result<(), FfiRoomControlError> {
        let session = self.session.clone();
        spawn_on_ffi_runtime(async move { session.ping(nonce).await.map_err(room_control_err) })
            .await
    }

    pub async fn close(&self, reason: FfiRoomCloseReason) -> Result<(), FfiRoomControlError> {
        let session = self.session.clone();
        spawn_on_ffi_runtime(async move {
            session
                .close(core_close_reason(reason))
                .await
                .map_err(room_control_err)
        })
        .await
    }
}

fn room_control_err(error: CoreError) -> FfiRoomControlError {
    match error {
        CoreError::InvalidInput(reason) => FfiRoomControlError::Rejected { reason },
        CoreError::Io(reason) | CoreError::Transport(reason) => {
            FfiRoomControlError::NetworkLost { reason }
        }
        CoreError::Cancelled => FfiRoomControlError::Canceled {
            reason: "room-control operation canceled".into(),
        },
        CoreError::InvitationConsumed(source) => room_control_err(*source),
        other => FfiRoomControlError::Failed {
            reason: other.to_string(),
        },
    }
}

#[uniffi::export]
pub fn make_room_control_invite(
    broker: String,
    relay: String,
) -> Result<FfiRoomControlInvite, EnvoixError> {
    let broker = non_empty(&broker).unwrap_or(DEFAULT_RENDEZVOUS_BROKER);
    let relay = non_empty(&relay)
        .or(Some(DEFAULT_RELAY_URL))
        .map(str::to_string);
    RoomControlInvite::generate(broker, relay)
        .map(project_invite)
        .map_err(op_err)
}

#[uniffi::export]
pub fn parse_room_control_invite(
    input: String,
    fallback_broker: String,
    fallback_relay: String,
) -> Result<FfiRoomControlInvite, EnvoixError> {
    let broker = non_empty(&fallback_broker).unwrap_or(DEFAULT_RENDEZVOUS_BROKER);
    let relay = non_empty(&fallback_relay)
        .or(Some(DEFAULT_RELAY_URL))
        .map(str::to_string);
    RoomControlInvite::parse(&input, broker, relay)
        .map(project_invite)
        .map_err(op_err)
}

#[allow(clippy::too_many_arguments)]
#[uniffi::export]
pub async fn connect_room_control_session(
    input: String,
    display_name: String,
    mode: FfiRoomConnectMode,
    verified_pairing: bool,
    identity_path: String,
    fallback_broker: String,
    fallback_relay: String,
    cancellation: Arc<FfiRoomControlCancellation>,
) -> Result<Arc<FfiRoomControlSession>, EnvoixError> {
    spawn_on_ffi_runtime(async move {
        let broker = non_empty(&fallback_broker).unwrap_or(DEFAULT_RENDEZVOUS_BROKER);
        let relay = non_empty(&fallback_relay)
            .or(Some(DEFAULT_RELAY_URL))
            .map(str::to_string);
        let invite = RoomControlInvite::parse(&input, broker, relay).map_err(op_err)?;
        let mut client = Client::default();
        if let Some(path) = non_empty(&identity_path) {
            client.identity = IdentityConfig::Persistent(PathBuf::from(path));
        }
        let mut options = TransferOptions::default();
        options.relay = invite.relay().map(str::to_string);
        let session = connect_room_control(
            invite,
            display_name,
            mode == FfiRoomConnectMode::Host,
            verified_pairing,
            client.session_config(&options),
            &cancellation.token,
        )
        .await
        .map_err(op_err)?;
        Ok(Arc::new(FfiRoomControlSession {
            session: Arc::new(session),
        }))
    })
    .await
}

/// Derives a generation-scoped, URL-safe presence tag for an untrusted local
/// discovery carrier. The tag is not a room identifier or ownership claim.
#[uniffi::export]
pub fn derive_remembered_presence_tag(
    remembered_credential_ref: String,
    remembered_generation: u64,
) -> Result<String, EnvoixError> {
    let credential_ref = non_empty(&remembered_credential_ref)
        .ok_or_else(|| op_err("remembered credential reference is missing"))?;
    acquire_remembered_credential(&RememberedCredentialRef::from_process_handle(
        credential_ref.to_string(),
    ))
    .map(|credential| credential.derive_presence_tag(remembered_generation))
    .map_err(op_err)
}

/// Authenticates one remembered generation and opens an equal-member room.
///
/// Availability, current/previous generation scheduling, generation rotation,
/// and simultaneous-attempt deduplication remain caller responsibilities.
#[allow(clippy::too_many_arguments)]
#[uniffi::export]
pub async fn connect_remembered_room_control_session(
    remembered_credential_ref: String,
    remembered_generation: u64,
    display_name: String,
    mode: FfiRememberedRoomConnectMode,
    identity_path: String,
    broker: String,
    relay: String,
    cancellation: Arc<FfiRoomControlCancellation>,
) -> Result<Arc<FfiRoomControlSession>, FfiRememberedRoomConnectError> {
    spawn_on_ffi_runtime(async move {
        let credential_ref = non_empty(&remembered_credential_ref).ok_or_else(|| {
            remembered_connect_err("remembered credential reference is missing", false)
        })?;
        let credential = acquire_remembered_credential(
            &RememberedCredentialRef::from_process_handle(credential_ref.to_string()),
        )
        .map_err(|error| remembered_connect_err(error, false))?;
        let broker = non_empty(&broker)
            .unwrap_or(DEFAULT_RENDEZVOUS_BROKER)
            .to_string();
        let relay = non_empty(&relay)
            .or(Some(DEFAULT_RELAY_URL))
            .map(str::to_string);
        let mut client = Client::default();
        if let Some(path) = non_empty(&identity_path) {
            client.identity = IdentityConfig::Persistent(PathBuf::from(path));
        }
        let mut options = TransferOptions::default();
        options.relay = relay.clone();
        let role = match mode {
            FfiRememberedRoomConnectMode::Connector => RememberedRoomControlRole::Connector,
            FfiRememberedRoomConnectMode::Responder => RememberedRoomControlRole::Responder,
        };
        let session = connect_remembered_room_control(
            credential.derive_session(remembered_generation),
            broker,
            relay,
            display_name,
            role,
            client.session_config(&options),
            &cancellation.token,
        )
        .await
        .map_err(remembered_session_connect_err)?;
        Ok(Arc::new(FfiRoomControlSession {
            session: Arc::new(session),
        }))
    })
    .await
}

fn remembered_connect_err(
    error: impl std::fmt::Display,
    peer_authenticated: bool,
) -> FfiRememberedRoomConnectError {
    FfiRememberedRoomConnectError::Failed {
        reason: error.to_string(),
        peer_authenticated,
        failure_code: None,
        retry_after_seconds: None,
    }
}

fn remembered_session_connect_err(
    error: RememberedRoomControlConnectError,
) -> FfiRememberedRoomConnectError {
    let peer_authenticated = error.peer_authenticated();
    let (failure_code, retry_after_seconds) = remembered_connect_metadata(error.error());
    FfiRememberedRoomConnectError::Failed {
        reason: error.to_string(),
        peer_authenticated,
        failure_code,
        retry_after_seconds,
    }
}

fn remembered_connect_metadata(error: &SessionError) -> (Option<FfiFailureCode>, Option<u64>) {
    match error {
        SessionError::Rendezvous { cause, retry_after } => (
            Some(match cause {
                RendezvousCause::RoomNotFound => FfiFailureCode::RoomNotFound,
                RendezvousCause::RoomExpired => FfiFailureCode::RoomExpired,
                RendezvousCause::RoomFull => FfiFailureCode::RoomFull,
                RendezvousCause::RoomRateLimited => FfiFailureCode::RoomRateLimited,
                RendezvousCause::RoomUnderAttack => FfiFailureCode::RoomUnderAttack,
                RendezvousCause::EndpointRateLimited => FfiFailureCode::EndpointRateLimited,
                RendezvousCause::IpRateLimited => FfiFailureCode::IpRateLimited,
                RendezvousCause::ServerBusy => FfiFailureCode::ServerBusy,
                RendezvousCause::MalformedJoin => FfiFailureCode::MalformedJoin,
                RendezvousCause::UnsupportedVersion => FfiFailureCode::UnsupportedRendezvousVersion,
            }),
            *retry_after,
        ),
        SessionError::InvitationConsumed(source) => remembered_connect_metadata(source),
        _ => (None, None),
    }
}

fn project_invite(invite: RoomControlInvite) -> FfiRoomControlInvite {
    FfiRoomControlInvite {
        code: invite.code().to_string(),
        payload: invite.payload(),
        broker: invite.broker().to_string(),
        relay: invite.relay().unwrap_or_default().to_string(),
        expires_at_epoch_ms: invite.expires_at_unix_secs().saturating_mul(1_000),
    }
}

fn core_offer(offer: FfiRoomTransferOffer) -> RoomTransferOffer {
    RoomTransferOffer {
        offer_id: offer.offer_id,
        transfer_invite: offer.transfer_invite,
        root_names: offer.root_names,
        item_count: offer.item_count,
        directory_count: offer.directory_count,
        total_bytes: offer.total_bytes,
    }
}

fn ffi_offer(offer: RoomTransferOffer) -> FfiRoomTransferOffer {
    FfiRoomTransferOffer {
        offer_id: offer.offer_id,
        transfer_invite: offer.transfer_invite,
        root_names: offer.root_names,
        item_count: offer.item_count,
        directory_count: offer.directory_count,
        total_bytes: offer.total_bytes,
    }
}

fn project_event(event: RoomControlEvent) -> FfiRoomControlEvent {
    let mut projected = FfiRoomControlEvent {
        kind: FfiRoomControlEventKind::Pong,
        offer: None,
        offer_id: String::new(),
        rejection: None,
        lifetime: None,
        close_reason: None,
        nonce: 0,
    };
    match event {
        RoomControlEvent::VerificationRequested => {
            projected.kind = FfiRoomControlEventKind::VerificationRequested;
        }
        RoomControlEvent::VerificationSucceeded => {
            projected.kind = FfiRoomControlEventKind::VerificationSucceeded;
        }
        RoomControlEvent::VerificationFailed => {
            projected.kind = FfiRoomControlEventKind::VerificationFailed;
        }
        RoomControlEvent::IncomingOffer(offer) => {
            projected.kind = FfiRoomControlEventKind::IncomingOffer;
            projected.offer = Some(ffi_offer(offer));
        }
        RoomControlEvent::OfferAccepted { offer_id } => {
            projected.kind = FfiRoomControlEventKind::OfferAccepted;
            projected.offer_id = offer_id;
        }
        RoomControlEvent::OfferRejected { offer_id, reason } => {
            projected.kind = FfiRoomControlEventKind::OfferRejected;
            projected.offer_id = offer_id;
            projected.rejection = Some(ffi_rejection(reason));
        }
        RoomControlEvent::LifetimeChanged(lifetime) => {
            projected.kind = FfiRoomControlEventKind::LifetimeChanged;
            projected.lifetime = Some(ffi_lifetime(lifetime));
        }
        RoomControlEvent::PeerClosed(reason) => {
            projected.kind = FfiRoomControlEventKind::PeerClosed;
            projected.close_reason = Some(ffi_close_reason(reason));
        }
        RoomControlEvent::Pong { nonce } => {
            projected.nonce = nonce;
        }
    }
    projected
}

fn ffi_lifetime(state: RoomLifetimeState) -> FfiRoomLifetimeState {
    FfiRoomLifetimeState {
        revision: state.revision,
        policy: ffi_policy(state.policy),
        idle_deadline_epoch_ms: state.idle_deadline_unix_ms,
    }
}

fn core_policy(policy: FfiRoomLifetimePolicy) -> RoomLifetimePolicy {
    match policy {
        FfiRoomLifetimePolicy::Idle15Minutes => RoomLifetimePolicy::Idle15Minutes,
        FfiRoomLifetimePolicy::UntilForegroundEnds => RoomLifetimePolicy::UntilForegroundEnds,
    }
}

fn ffi_policy(policy: RoomLifetimePolicy) -> FfiRoomLifetimePolicy {
    match policy {
        RoomLifetimePolicy::Idle15Minutes => FfiRoomLifetimePolicy::Idle15Minutes,
        RoomLifetimePolicy::UntilForegroundEnds => FfiRoomLifetimePolicy::UntilForegroundEnds,
    }
}

fn core_rejection(reason: FfiRoomOfferRejection) -> RoomOfferRejection {
    match reason {
        FfiRoomOfferRejection::Declined => RoomOfferRejection::Declined,
        FfiRoomOfferRejection::Busy => RoomOfferRejection::Busy,
        FfiRoomOfferRejection::Expired => RoomOfferRejection::Expired,
        FfiRoomOfferRejection::Invalid => RoomOfferRejection::Invalid,
    }
}

fn ffi_rejection(reason: RoomOfferRejection) -> FfiRoomOfferRejection {
    match reason {
        RoomOfferRejection::Declined => FfiRoomOfferRejection::Declined,
        RoomOfferRejection::Busy => FfiRoomOfferRejection::Busy,
        RoomOfferRejection::Expired => FfiRoomOfferRejection::Expired,
        RoomOfferRejection::Invalid => FfiRoomOfferRejection::Invalid,
    }
}

fn core_close_reason(reason: FfiRoomCloseReason) -> RoomCloseReason {
    match reason {
        FfiRoomCloseReason::UserEnded => RoomCloseReason::UserEnded,
        FfiRoomCloseReason::IdleExpired => RoomCloseReason::IdleExpired,
        FfiRoomCloseReason::InvitationExpired => RoomCloseReason::InvitationExpired,
        FfiRoomCloseReason::PeerEnded => RoomCloseReason::PeerEnded,
        FfiRoomCloseReason::Backgrounded => RoomCloseReason::Backgrounded,
        FfiRoomCloseReason::NetworkLost => RoomCloseReason::NetworkLost,
        FfiRoomCloseReason::ProtocolFailure => RoomCloseReason::ProtocolFailure,
    }
}

fn ffi_close_reason(reason: RoomCloseReason) -> FfiRoomCloseReason {
    match reason {
        RoomCloseReason::UserEnded => FfiRoomCloseReason::UserEnded,
        RoomCloseReason::IdleExpired => FfiRoomCloseReason::IdleExpired,
        RoomCloseReason::InvitationExpired => FfiRoomCloseReason::InvitationExpired,
        RoomCloseReason::PeerEnded => FfiRoomCloseReason::PeerEnded,
        RoomCloseReason::Backgrounded => FfiRoomCloseReason::Backgrounded,
        RoomCloseReason::NetworkLost => FfiRoomCloseReason::NetworkLost,
        RoomCloseReason::ProtocolFailure => FfiRoomCloseReason::ProtocolFailure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_projection_uses_epoch_milliseconds() {
        let invite = RoomControlInvite::parse(
            "envoix://room/123456-a1b2-c3d4?broker=test&expires=42",
            "fallback",
            None,
        )
        .unwrap();
        assert_eq!(project_invite(invite).expires_at_epoch_ms, 42_000);
    }

    #[test]
    fn human_room_code_uses_configured_fallback_endpoints() {
        let invite = parse_room_control_invite(
            "123456-a1b2-c3d4".into(),
            "https://broker.example.test".into(),
            "https://relay.example.test".into(),
        )
        .unwrap();

        assert_eq!(invite.broker, "https://broker.example.test");
        assert_eq!(invite.relay, "https://relay.example.test");
        assert!(
            invite
                .payload
                .contains("broker=https%3A%2F%2Fbroker.example.test")
        );
    }

    #[test]
    fn legacy_prefixed_room_control_codes_are_rejected() {
        for input in [
            "R123456-a1b2-c3d4",
            "r123456-a1b2-c3d4",
            "envoix://room/R123456-a1b2-c3d4",
        ] {
            assert!(
                parse_room_control_invite(input.into(), "broker".into(), String::new()).is_err(),
                "accepted legacy Room-Control code {input:?}"
            );
        }
    }

    #[test]
    fn event_projection_preserves_offer_id() {
        let event = project_event(RoomControlEvent::OfferAccepted {
            offer_id: "opaque_7".into(),
        });
        assert_eq!(event.kind, FfiRoomControlEventKind::OfferAccepted);
        assert_eq!(event.offer_id, "opaque_7");
    }

    #[test]
    fn verification_events_cross_the_ffi_boundary() {
        assert_eq!(
            project_event(RoomControlEvent::VerificationRequested).kind,
            FfiRoomControlEventKind::VerificationRequested
        );
        assert_eq!(
            project_event(RoomControlEvent::VerificationSucceeded).kind,
            FfiRoomControlEventKind::VerificationSucceeded
        );
        assert_eq!(
            project_event(RoomControlEvent::VerificationFailed).kind,
            FfiRoomControlEventKind::VerificationFailed
        );
    }

    #[test]
    fn offer_projection_preserves_directory_count() {
        let offer = FfiRoomTransferOffer {
            offer_id: "opaque_7".into(),
            transfer_invite: "opaque_invite".into(),
            root_names: vec!["Photos".into()],
            item_count: 7,
            directory_count: 3,
            total_bytes: 42,
        };
        assert_eq!(ffi_offer(core_offer(offer.clone())), offer);
    }

    #[test]
    fn lifetime_projection_preserves_revision_policy_and_nullable_deadline() {
        let state = RoomLifetimeState {
            revision: 7,
            policy: RoomLifetimePolicy::Idle15Minutes,
            idle_deadline_unix_ms: Some(123_456),
        };
        let event = project_event(RoomControlEvent::LifetimeChanged(state));
        assert_eq!(event.kind, FfiRoomControlEventKind::LifetimeChanged);
        assert_eq!(
            event.lifetime,
            Some(FfiRoomLifetimeState {
                revision: 7,
                policy: FfiRoomLifetimePolicy::Idle15Minutes,
                idle_deadline_epoch_ms: Some(123_456),
            })
        );

        let paused = ffi_lifetime(RoomLifetimeState {
            revision: 8,
            policy: RoomLifetimePolicy::Idle15Minutes,
            idle_deadline_unix_ms: None,
        });
        assert_eq!(paused.idle_deadline_epoch_ms, None);
    }

    #[test]
    fn remembered_connect_error_preserves_authenticated_phase() {
        assert!(matches!(
            remembered_connect_err("control hello failed", true),
            FfiRememberedRoomConnectError::Failed {
                reason,
                peer_authenticated: true,
                failure_code: None,
                retry_after_seconds: None,
            } if reason == "control hello failed"
        ));
    }

    #[test]
    fn remembered_connect_error_exposes_typed_rendezvous_cause_and_retry_delay() {
        let expired = SessionError::Rendezvous {
            cause: RendezvousCause::RoomExpired,
            retry_after: None,
        };
        assert_eq!(
            remembered_connect_metadata(&expired),
            (Some(FfiFailureCode::RoomExpired), None)
        );

        let busy = SessionError::Rendezvous {
            cause: RendezvousCause::ServerBusy,
            retry_after: Some(17),
        };
        assert_eq!(
            remembered_connect_metadata(&busy),
            (Some(FfiFailureCode::ServerBusy), Some(17))
        );
    }

    #[test]
    fn remembered_presence_tag_uses_registered_credential_without_exposing_it() {
        let mut opaque = b"ENVR".to_vec();
        opaque.push(1);
        opaque.extend_from_slice(&[0x6a; 32]);
        let reference =
            crate::register_protected_remembered_credential(opaque).expect("register credential");

        let current =
            derive_remembered_presence_tag(reference.clone(), 7).expect("current presence tag");
        let next = derive_remembered_presence_tag(reference, 8).expect("next presence tag");

        assert_eq!(
            current.len(),
            envoix_client::api::REMEMBERED_PRESENCE_TAG_LEN
        );
        assert_ne!(current, next);
    }

    #[test]
    fn room_control_errors_have_stable_session_dispositions() {
        assert!(matches!(
            room_control_err(CoreError::InvalidInput("offer rejected".into())),
            FfiRoomControlError::Rejected { reason } if reason == "offer rejected"
        ));
        assert!(matches!(
            room_control_err(CoreError::Io("socket closed".into())),
            FfiRoomControlError::NetworkLost { reason } if reason == "socket closed"
        ));
        assert!(matches!(
            room_control_err(CoreError::Transport("relay unavailable".into())),
            FfiRoomControlError::NetworkLost { reason } if reason == "relay unavailable"
        ));
        assert!(matches!(
            room_control_err(CoreError::InvitationConsumed(Box::new(
                CoreError::Transport("peer disconnected".into()),
            ))),
            FfiRoomControlError::NetworkLost { reason } if reason == "peer disconnected"
        ));
        assert!(matches!(
            room_control_err(CoreError::Cancelled),
            FfiRoomControlError::Canceled { .. }
        ));
        assert!(matches!(
            room_control_err(CoreError::Protocol("invalid event".into())),
            FfiRoomControlError::Failed { reason } if reason == "protocol error: invalid event"
        ));
    }

    #[test]
    fn core_info_advertises_room_control_v5_ffi_v25() {
        let info = crate::envoix_core_info();
        assert_eq!(info.ffi_api_version, 25);
        assert!(
            info.capabilities
                .iter()
                .any(|capability| capability == "foreground_room_control_v5")
        );
        assert!(
            info.capabilities
                .iter()
                .any(|capability| capability == "remembered_room_control_v1")
        );
        assert!(
            info.capabilities
                .iter()
                .any(|capability| capability == "structured_stage_timing_v1")
        );
        assert!(
            info.capabilities
                .iter()
                .any(|capability| capability == "canonical_failure_projection_v1")
        );
        assert!(
            info.capabilities
                .iter()
                .any(|capability| capability == "typed_room_control_errors_v1")
        );
        assert!(
            info.capabilities
                .iter()
                .any(|capability| capability == "typed_remembered_credential_vault_v1")
        );
    }
}
