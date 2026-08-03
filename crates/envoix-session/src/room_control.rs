//! Foreground, direction-neutral room control over one authenticated QUIC stream.
//!
//! The room carries offers and decisions only. Every accepted offer still uses
//! a fresh directional InviteV2 and the unchanged Manifest data plane.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use envoix_auth::RememberedSession;
use envoix_error::CoreError;
use envoix_invite::{
    BootstrapKind, InvitationControlContext, InvitationSide, InviteV2, ROOM_CONTROL_LOCATOR_PREFIX,
    RoomCode, TransferRole,
};
use envoix_rendezvous::{Join, RENDEZVOUS_PROTOCOL_VERSION, Role};
use envoix_rendezvous_iroh::{RoomPairing, drive_pairing};
use iroh::endpoint::{Connection, RecvStream, SendStream, VarInt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::endpoint::build_endpoint;
use crate::room::join_broker_with_retry;
use crate::{
    BindAddrs, BoundEndpoint, RendezvousRetryPolicy, SessionConfig, SessionError,
    TransferCancelToken, parse_broker_addr,
};

pub const ROOM_CONTROL_ALPN: &[u8] = b"envoix-room-control/5";
const ROOM_CONTROL_VERSION: u16 = 5;
const ROOM_URI_PREFIX: &str = "envoix://room/";
const ROOM_INVITE_TTL: Duration = Duration::from_secs(300);
const ROOM_IDLE_TIMEOUT_MS: u64 = 15 * 60 * 1_000;
const PAIRING_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(8);
const CONTROL_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROL_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CONTROL_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONTROL_FRAME_BYTES: usize = 64 * 1024;
const MAX_TRANSFER_INVITE_BYTES: usize = 8 * 1024;
const MAX_OFFER_ID_BYTES: usize = 128;
const MAX_ROOT_NAME_BYTES: usize = 255;
const MAX_ROOT_PREVIEWS: usize = 3;
const MAX_SEEN_OFFER_IDS: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomControlInvite {
    code: String,
    broker: String,
    relay: Option<String>,
    expires_at_unix_secs: u64,
}

impl RoomControlInvite {
    pub fn generate(
        broker: impl Into<String>,
        relay: Option<String>,
    ) -> Result<Self, SessionError> {
        let room_code =
            RoomCode::generate().map_err(|error| CoreError::InvalidInput(error.to_string()))?;
        Self::from_parts(
            room_code.canonical().to_string(),
            broker.into(),
            relay,
            now_unix_secs()?.saturating_add(ROOM_INVITE_TTL.as_secs()),
        )
    }

    pub fn parse(
        input: &str,
        fallback_broker: impl Into<String>,
        fallback_relay: Option<String>,
    ) -> Result<Self, SessionError> {
        let input = input.trim();
        if let Some(rest) = input.strip_prefix(ROOM_URI_PREFIX) {
            let (encoded_code, query) = rest.split_once('?').unwrap_or((rest, ""));
            let code = percent_decode(encoded_code)?;
            let mut broker = None;
            let mut relay = None;
            let mut expires = None;
            for field in query.split('&').filter(|field| !field.is_empty()) {
                let (key, value) = field.split_once('=').unwrap_or((field, ""));
                match key {
                    "broker" => broker = Some(percent_decode(value)?),
                    "relay" => relay = Some(percent_decode(value)?),
                    "expires" => {
                        expires = Some(percent_decode(value)?.parse::<u64>().map_err(|_| {
                            CoreError::InvalidInput("invalid room invitation expiry".into())
                        })?)
                    }
                    _ => {}
                }
            }
            return Self::from_parts(
                code,
                broker.unwrap_or_else(|| fallback_broker.into()),
                relay.or(fallback_relay),
                expires.unwrap_or_else(|| {
                    now_unix_secs()
                        .unwrap_or_default()
                        .saturating_add(ROOM_INVITE_TTL.as_secs())
                }),
            );
        }
        Self::from_parts(
            input.to_string(),
            fallback_broker.into(),
            fallback_relay,
            now_unix_secs()?.saturating_add(ROOM_INVITE_TTL.as_secs()),
        )
    }

    fn from_parts(
        code: String,
        broker: String,
        relay: Option<String>,
        expires_at_unix_secs: u64,
    ) -> Result<Self, SessionError> {
        let code = normalize_room_code(&code)?;
        let broker = broker.trim().to_string();
        if broker.is_empty() {
            return Err(CoreError::InvalidInput(
                "room control invitation needs a broker".into(),
            ));
        }
        let relay = relay
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Ok(Self {
            code,
            broker,
            relay,
            expires_at_unix_secs,
        })
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn broker(&self) -> &str {
        &self.broker
    }

    pub fn relay(&self) -> Option<&str> {
        self.relay.as_deref()
    }

    pub fn expires_at_unix_secs(&self) -> u64 {
        self.expires_at_unix_secs
    }

    pub fn payload(&self) -> String {
        let mut payload = format!("{ROOM_URI_PREFIX}{}", percent_encode(&self.code));
        payload.push_str("?broker=");
        payload.push_str(&percent_encode(&self.broker));
        if let Some(relay) = &self.relay {
            payload.push_str("&relay=");
            payload.push_str(&percent_encode(relay));
        }
        payload.push_str("&expires=");
        payload.push_str(&self.expires_at_unix_secs.to_string());
        payload
    }

    fn room_id(&self) -> String {
        let room_code = RoomCode::parse(&self.code).expect("validated room code");
        format!("{ROOM_CONTROL_LOCATOR_PREFIX}{}", room_code.room_id())
    }

    fn ensure_fresh(&self) -> Result<(), SessionError> {
        if now_unix_secs()? >= self.expires_at_unix_secs {
            return Err(CoreError::InvalidInput(
                "room control invitation has expired".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomLifetimePolicy {
    Idle15Minutes,
    UntilForegroundEnds,
}

/// Transient rendezvous mechanics for reopening one remembered relationship.
///
/// This role is chosen for each connection attempt. It is not persisted as
/// room ownership and both peers are equal after authentication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RememberedRoomControlRole {
    Connector,
    Responder,
}

impl RememberedRoomControlRole {
    fn is_responder(self) -> bool {
        self == Self::Responder
    }
}

/// Failure from one remembered room-control generation attempt.
///
/// Callers may try a fallback generation only when the peer was not yet
/// authenticated. Once true, the remembered peer was proven by the PAKE even
/// if the subsequent QUIC or control Hello phase failed.
#[derive(Debug)]
pub struct RememberedRoomControlConnectError {
    error: SessionError,
    peer_authenticated: bool,
}

impl RememberedRoomControlConnectError {
    fn new(error: SessionError, peer_authenticated: bool) -> Self {
        Self {
            error,
            peer_authenticated,
        }
    }

    pub fn peer_authenticated(&self) -> bool {
        self.peer_authenticated
    }

    pub fn error(&self) -> &SessionError {
        &self.error
    }

    pub fn into_error(self) -> SessionError {
        self.error
    }
}

impl std::fmt::Display for RememberedRoomControlConnectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for RememberedRoomControlConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RoomLifetimeState {
    pub revision: u64,
    pub policy: RoomLifetimePolicy,
    pub idle_deadline_unix_ms: Option<u64>,
}

impl RoomLifetimeState {
    fn initial(now_unix_ms: u64) -> Self {
        Self {
            revision: 1,
            policy: RoomLifetimePolicy::Idle15Minutes,
            idle_deadline_unix_ms: Some(now_unix_ms.saturating_add(ROOM_IDLE_TIMEOUT_MS)),
        }
    }

    fn remembered() -> Self {
        Self {
            revision: 1,
            policy: RoomLifetimePolicy::UntilForegroundEnds,
            idle_deadline_unix_ms: None,
        }
    }

    fn validate(&self) -> Result<(), SessionError> {
        if self.revision == 0 {
            return Err(CoreError::Protocol(
                "room lifetime revision must be positive".into(),
            ));
        }
        if self.policy == RoomLifetimePolicy::UntilForegroundEnds
            && self.idle_deadline_unix_ms.is_some()
        {
            return Err(CoreError::Protocol(
                "foreground room lifetime cannot have an idle deadline".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomCloseReason {
    UserEnded,
    IdleExpired,
    InvitationExpired,
    PeerEnded,
    Backgrounded,
    NetworkLost,
    ProtocolFailure,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomOfferRejection {
    Declined,
    Busy,
    Expired,
    Invalid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RoomTransferOffer {
    pub offer_id: String,
    pub transfer_invite: String,
    pub root_names: Vec<String>,
    pub item_count: u32,
    pub directory_count: u32,
    pub total_bytes: u64,
}

impl RoomTransferOffer {
    fn validate(
        &self,
        expected_broker: &str,
        expected_relay: Option<&str>,
    ) -> Result<(), SessionError> {
        if self.offer_id.is_empty()
            || self.offer_id.len() > MAX_OFFER_ID_BYTES
            || !self
                .offer_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(CoreError::InvalidInput(
                "room offer id must be 1-128 ASCII letters, digits, '-' or '_'".into(),
            ));
        }
        if self.transfer_invite.len() > MAX_TRANSFER_INVITE_BYTES {
            return Err(CoreError::InvalidInput(
                "room offer needs a bounded directional sender invitation".into(),
            ));
        }
        let transfer_invite = InviteV2::parse_for_role(
            &self.transfer_invite,
            TransferRole::Receiver,
            now_unix_secs()?,
        )
        .map_err(|error| {
            CoreError::InvalidInput(format!(
                "room offer needs a valid directional InviteV2: {error}"
            ))
        })?;
        let route = &transfer_invite.invitation().public_context;
        let relay_matches = match expected_relay {
            Some(expected) => {
                route.relay_urls.len() == 1 && route.relay_urls[0].as_str() == expected
            }
            None => route.relay_urls.is_empty(),
        };
        if route.broker != expected_broker || !relay_matches {
            return Err(CoreError::InvalidInput(
                "room offer transfer route differs from room control route".into(),
            ));
        }
        if self.directory_count > self.item_count {
            return Err(CoreError::InvalidInput(
                "room offer directory count exceeds item count".into(),
            ));
        }
        if self.root_names.len() > MAX_ROOT_PREVIEWS {
            return Err(CoreError::InvalidInput(
                "room offer has more than three root previews".into(),
            ));
        }
        for name in &self.root_names {
            if name.is_empty()
                || name.len() > MAX_ROOT_NAME_BYTES
                || matches!(name.as_str(), "." | "..")
                || name
                    .chars()
                    .any(|character| character.is_control() || matches!(character, '/' | '\\'))
            {
                return Err(CoreError::InvalidInput(
                    "room offer contains an invalid root preview".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoomControlEvent {
    IncomingOffer(RoomTransferOffer),
    OfferAccepted {
        offer_id: String,
    },
    OfferRejected {
        offer_id: String,
        reason: RoomOfferRejection,
    },
    LifetimeChanged(RoomLifetimeState),
    PeerClosed(RoomCloseReason),
    Pong {
        nonce: u64,
    },
}

#[derive(Clone, Deserialize, Serialize)]
enum ControlMessage {
    Hello {
        protocol_version: u16,
        #[serde(default)]
        session_kind: ControlSessionKind,
        display_name: String,
        creator: bool,
        pairing_binding: Vec<u8>,
        lifetime: Option<RoomLifetimeState>,
    },
    TransferOffer(RoomTransferOffer),
    OfferAccepted {
        offer_id: String,
    },
    OfferRejected {
        offer_id: String,
        reason: RoomOfferRejection,
    },
    LifetimeChanged(RoomLifetimeState),
    Activity,
    TransferActive {
        active: bool,
    },
    Close(RoomCloseReason),
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ControlSessionKind {
    #[default]
    Invitation,
    Remembered,
}

#[derive(Default)]
struct OfferState {
    pending_local: Option<String>,
    pending_remote: Option<String>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
}

impl OfferState {
    fn remember(&mut self, offer_id: String) {
        if self.seen.insert(offer_id.clone()) {
            self.seen_order.push_back(offer_id);
        }
        while self.seen_order.len() > MAX_SEEN_OFFER_IDS {
            if let Some(expired) = self.seen_order.pop_front() {
                self.seen.remove(&expired);
            }
        }
    }
}

struct RoomLifetimeMachine {
    state: RoomLifetimeState,
    local_transfer_active: bool,
    peer_transfer_active: bool,
}

impl RoomLifetimeMachine {
    fn new(state: RoomLifetimeState) -> Self {
        Self {
            state,
            local_transfer_active: false,
            peer_transfer_active: false,
        }
    }

    fn note_activity(
        &mut self,
        now_unix_ms: u64,
    ) -> Result<Option<RoomLifetimeState>, SessionError> {
        if self.state.policy != RoomLifetimePolicy::Idle15Minutes || self.any_transfer_active() {
            return Ok(None);
        }
        self.advance(Some(now_unix_ms.saturating_add(ROOM_IDLE_TIMEOUT_MS)))
            .map(Some)
    }

    fn set_local_transfer_active(
        &mut self,
        active: bool,
        now_unix_ms: u64,
    ) -> Result<Option<RoomLifetimeState>, SessionError> {
        if self.local_transfer_active == active {
            return Ok(None);
        }
        let was_active = self.any_transfer_active();
        self.local_transfer_active = active;
        self.apply_transfer_edge(was_active, now_unix_ms)
    }

    fn set_peer_transfer_active(
        &mut self,
        active: bool,
        now_unix_ms: u64,
    ) -> Result<Option<RoomLifetimeState>, SessionError> {
        if self.peer_transfer_active == active {
            return Ok(None);
        }
        let was_active = self.any_transfer_active();
        self.peer_transfer_active = active;
        self.apply_transfer_edge(was_active, now_unix_ms)
    }

    fn set_policy(
        &mut self,
        policy: RoomLifetimePolicy,
        now_unix_ms: u64,
    ) -> Result<Option<RoomLifetimeState>, SessionError> {
        if self.state.policy == policy {
            return Ok(None);
        }
        self.state.policy = policy;
        let deadline = match policy {
            RoomLifetimePolicy::Idle15Minutes if !self.any_transfer_active() => {
                Some(now_unix_ms.saturating_add(ROOM_IDLE_TIMEOUT_MS))
            }
            RoomLifetimePolicy::Idle15Minutes | RoomLifetimePolicy::UntilForegroundEnds => None,
        };
        self.advance(deadline).map(Some)
    }

    fn apply_authoritative(
        &mut self,
        state: RoomLifetimeState,
    ) -> Result<RoomLifetimeState, SessionError> {
        state.validate()?;
        if state.revision <= self.state.revision {
            return Err(CoreError::Protocol(
                "room lifetime revision did not advance".into(),
            ));
        }
        self.state = state.clone();
        Ok(state)
    }

    fn apply_transfer_edge(
        &mut self,
        was_active: bool,
        now_unix_ms: u64,
    ) -> Result<Option<RoomLifetimeState>, SessionError> {
        let is_active = self.any_transfer_active();
        if was_active == is_active || self.state.policy != RoomLifetimePolicy::Idle15Minutes {
            return Ok(None);
        }
        let deadline = (!is_active).then(|| now_unix_ms.saturating_add(ROOM_IDLE_TIMEOUT_MS));
        self.advance(deadline).map(Some)
    }

    fn any_transfer_active(&self) -> bool {
        self.local_transfer_active || self.peer_transfer_active
    }

    fn advance(
        &mut self,
        idle_deadline_unix_ms: Option<u64>,
    ) -> Result<RoomLifetimeState, SessionError> {
        self.state.revision = self
            .state
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::Protocol("room lifetime revision exhausted".into()))?;
        self.state.idle_deadline_unix_ms = idle_deadline_unix_ms;
        Ok(self.state.clone())
    }
}

pub struct RoomControlSession {
    _endpoint: iroh::Endpoint,
    connection: Connection,
    send: Mutex<SendStream>,
    recv: Mutex<RecvStream>,
    offers: std::sync::Mutex<OfferState>,
    lifetime_updates: Mutex<()>,
    lifetime: std::sync::Mutex<RoomLifetimeMachine>,
    broker: String,
    relay: Option<String>,
    peer_name: String,
    mode: RoomControlSessionMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RoomControlSessionMode {
    Invitation { creator: bool },
    Remembered { generation: u64 },
}

impl RoomControlSessionMode {
    fn session_kind(self) -> ControlSessionKind {
        match self {
            Self::Invitation { .. } => ControlSessionKind::Invitation,
            Self::Remembered { .. } => ControlSessionKind::Remembered,
        }
    }

    fn is_creator(self) -> bool {
        matches!(self, Self::Invitation { creator: true })
    }

    fn is_remembered(self) -> bool {
        matches!(self, Self::Remembered { .. })
    }

    fn remembered_generation(self) -> Option<u64> {
        match self {
            Self::Invitation { .. } => None,
            Self::Remembered { generation } => Some(generation),
        }
    }
}

struct CloseOnIncompleteMutation {
    connection: Connection,
    armed: bool,
}

impl CloseOnIncompleteMutation {
    fn new(connection: &Connection) -> Self {
        Self {
            connection: connection.clone(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CloseOnIncompleteMutation {
    fn drop(&mut self) {
        if self.armed {
            self.connection
                .close(VarInt::from_u32(3), b"incomplete control mutation");
        }
    }
}

impl RoomControlSession {
    pub fn peer_name(&self) -> &str {
        &self.peer_name
    }

    pub fn is_creator(&self) -> bool {
        self.mode.is_creator()
    }

    pub fn is_remembered(&self) -> bool {
        self.mode.is_remembered()
    }

    pub fn remembered_generation(&self) -> Option<u64> {
        self.mode.remembered_generation()
    }

    pub fn lifetime_state(&self) -> RoomLifetimeState {
        self.lifetime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state
            .clone()
    }

    pub async fn offer_transfer(
        &self,
        offer: RoomTransferOffer,
    ) -> Result<Option<RoomLifetimeState>, SessionError> {
        offer.validate(&self.broker, self.relay.as_deref())?;
        {
            let mut state = self
                .offers
                .lock()
                .map_err(|_| CoreError::Transport("room offer state unavailable".into()))?;
            if state.pending_local.is_some() || state.pending_remote.is_some() {
                return Err(CoreError::InvalidInput(
                    "another room offer is waiting for a decision".into(),
                ));
            }
            if state.seen.contains(&offer.offer_id) {
                return Err(CoreError::InvalidInput(
                    "room offer id has already been used".into(),
                ));
            }
            state.pending_local = Some(offer.offer_id.clone());
            state.remember(offer.offer_id.clone());
        }
        let mut mutation = CloseOnIncompleteMutation::new(&self.connection);
        let lifetime = match self.note_activity().await {
            Ok(lifetime) => lifetime,
            Err(error) => {
                if let Ok(mut state) = self.offers.lock() {
                    state.pending_local = None;
                }
                return Err(error);
            }
        };
        if let Err(error) = self.send(ControlMessage::TransferOffer(offer)).await {
            if let Ok(mut state) = self.offers.lock() {
                state.pending_local = None;
            }
            return Err(error);
        }
        mutation.disarm();
        Ok(lifetime)
    }

    pub async fn accept_offer(
        &self,
        offer_id: &str,
    ) -> Result<Option<RoomLifetimeState>, SessionError> {
        self.resolve_remote_offer(offer_id)?;
        let mut mutation = CloseOnIncompleteMutation::new(&self.connection);
        let lifetime = self.note_activity().await?;
        self.send_offer_response(ControlMessage::OfferAccepted {
            offer_id: offer_id.to_string(),
        })
        .await?;
        mutation.disarm();
        Ok(lifetime)
    }

    pub async fn reject_offer(
        &self,
        offer_id: &str,
        reason: RoomOfferRejection,
    ) -> Result<Option<RoomLifetimeState>, SessionError> {
        self.resolve_remote_offer(offer_id)?;
        let mut mutation = CloseOnIncompleteMutation::new(&self.connection);
        let lifetime = self.note_activity().await?;
        self.send_offer_response(ControlMessage::OfferRejected {
            offer_id: offer_id.to_string(),
            reason,
        })
        .await?;
        mutation.disarm();
        Ok(lifetime)
    }

    pub async fn set_policy(
        &self,
        policy: RoomLifetimePolicy,
    ) -> Result<Option<RoomLifetimeState>, SessionError> {
        if self.mode.is_remembered() {
            return Err(CoreError::InvalidInput(
                "remembered room lifetime is fixed while connected".into(),
            ));
        }
        if !self.mode.is_creator() {
            return Err(CoreError::InvalidInput(
                "only the room creator can change its lifetime".into(),
            ));
        }
        self.update_creator_lifetime(|lifetime, now| lifetime.set_policy(policy, now))
            .await
    }

    pub async fn set_local_transfer_active(
        &self,
        active: bool,
    ) -> Result<Option<RoomLifetimeState>, SessionError> {
        if self.mode.is_remembered() {
            return Ok(None);
        }
        if self.mode.is_creator() {
            return self
                .update_creator_lifetime(|lifetime, now| {
                    lifetime.set_local_transfer_active(active, now)
                })
                .await;
        }

        let _update = self.lifetime_updates.lock().await;
        let changed = {
            let mut lifetime = self.lock_lifetime()?;
            if lifetime.local_transfer_active == active {
                false
            } else {
                lifetime.local_transfer_active = active;
                true
            }
        };
        if changed {
            let mut mutation = CloseOnIncompleteMutation::new(&self.connection);
            self.send(ControlMessage::TransferActive { active }).await?;
            mutation.disarm();
        }
        Ok(None)
    }

    pub async fn note_activity(&self) -> Result<Option<RoomLifetimeState>, SessionError> {
        if self.mode.is_remembered() {
            return Ok(None);
        }
        if self.mode.is_creator() {
            return self
                .update_creator_lifetime(RoomLifetimeMachine::note_activity)
                .await;
        }
        self.send(ControlMessage::Activity).await?;
        Ok(None)
    }

    pub async fn ping(&self, nonce: u64) -> Result<(), SessionError> {
        self.send(ControlMessage::Ping { nonce }).await
    }

    pub async fn close(&self, reason: RoomCloseReason) -> Result<(), SessionError> {
        let _lifetime_update = if reason == RoomCloseReason::IdleExpired {
            if self.mode.is_remembered() {
                return Err(CoreError::InvalidInput(
                    "remembered room control has no idle deadline".into(),
                ));
            }
            if !self.mode.is_creator() {
                return Err(CoreError::InvalidInput(
                    "only the room creator can expire its idle deadline".into(),
                ));
            }
            let update = self.lifetime_updates.lock().await;
            let now = now_unix_millis()?;
            let lifetime = self.lock_lifetime()?;
            let expired = lifetime.state.policy == RoomLifetimePolicy::Idle15Minutes
                && !lifetime.any_transfer_active()
                && lifetime
                    .state
                    .idle_deadline_unix_ms
                    .is_some_and(|deadline| now >= deadline);
            if !expired {
                return Err(CoreError::InvalidInput(
                    "room idle deadline has not expired".into(),
                ));
            }
            drop(lifetime);
            Some(update)
        } else {
            None
        };
        let result = match tokio::time::timeout(CONTROL_WRITE_TIMEOUT, async {
            let mut send = self.send.lock().await;
            let result = write_control_message(&mut *send, &ControlMessage::Close(reason)).await;
            if result.is_ok() {
                let _ = send.finish();
                let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
            }
            result
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(CoreError::Transport("room control close timed out".into())),
        };
        self.connection.close(VarInt::from_u32(0), b"room closed");
        result
    }

    pub async fn next_event(&self) -> Result<RoomControlEvent, SessionError> {
        loop {
            match self.receive().await? {
                ControlMessage::Hello { .. } => {
                    return Err(CoreError::Protocol("duplicate room control hello".into()));
                }
                ControlMessage::TransferOffer(offer) => {
                    if let Err(error) = offer.validate(&self.broker, self.relay.as_deref()) {
                        let _ = self
                            .send(ControlMessage::OfferRejected {
                                offer_id: offer.offer_id,
                                reason: RoomOfferRejection::Invalid,
                            })
                            .await;
                        return Err(error);
                    }
                    let busy = {
                        let mut state = self.offers.lock().map_err(|_| {
                            CoreError::Transport("room offer state unavailable".into())
                        })?;
                        if state.pending_local.is_some()
                            || state.pending_remote.is_some()
                            || state.seen.contains(&offer.offer_id)
                        {
                            true
                        } else {
                            state.pending_remote = Some(offer.offer_id.clone());
                            state.remember(offer.offer_id.clone());
                            false
                        }
                    };
                    if busy {
                        self.send(ControlMessage::OfferRejected {
                            offer_id: offer.offer_id,
                            reason: RoomOfferRejection::Busy,
                        })
                        .await?;
                        continue;
                    }
                    return Ok(RoomControlEvent::IncomingOffer(offer));
                }
                ControlMessage::OfferAccepted { offer_id } => {
                    self.resolve_local_offer(&offer_id)?;
                    return Ok(RoomControlEvent::OfferAccepted { offer_id });
                }
                ControlMessage::OfferRejected { offer_id, reason } => {
                    self.resolve_local_offer(&offer_id)?;
                    return Ok(RoomControlEvent::OfferRejected { offer_id, reason });
                }
                ControlMessage::LifetimeChanged(state) => {
                    if self.mode.is_remembered() {
                        return Err(CoreError::Protocol(
                            "remembered room lifetime cannot change".into(),
                        ));
                    }
                    if self.mode.is_creator() {
                        return Err(CoreError::Protocol(
                            "the room joiner cannot publish lifetime state".into(),
                        ));
                    }
                    let state = self.apply_authoritative_lifetime(state).await?;
                    return Ok(RoomControlEvent::LifetimeChanged(state));
                }
                ControlMessage::Activity => {
                    if self.mode.is_remembered() {
                        return Err(CoreError::Protocol(
                            "remembered room control has no activity authority".into(),
                        ));
                    }
                    if !self.mode.is_creator() {
                        return Err(CoreError::Protocol(
                            "the room creator cannot send an activity hint".into(),
                        ));
                    }
                    if let Some(state) = self
                        .update_creator_lifetime(RoomLifetimeMachine::note_activity)
                        .await?
                    {
                        return Ok(RoomControlEvent::LifetimeChanged(state));
                    }
                }
                ControlMessage::TransferActive { active } => {
                    if self.mode.is_remembered() {
                        return Err(CoreError::Protocol(
                            "remembered room control has no transfer activity authority".into(),
                        ));
                    }
                    if !self.mode.is_creator() {
                        return Err(CoreError::Protocol(
                            "the room creator cannot send a transfer activity hint".into(),
                        ));
                    }
                    if let Some(state) = self
                        .update_creator_lifetime(|lifetime, now| {
                            lifetime.set_peer_transfer_active(active, now)
                        })
                        .await?
                    {
                        return Ok(RoomControlEvent::LifetimeChanged(state));
                    }
                }
                ControlMessage::Close(reason) => {
                    if self.mode.is_remembered() && reason == RoomCloseReason::IdleExpired {
                        return Err(CoreError::Protocol(
                            "remembered room control peer cannot expire an idle deadline".into(),
                        ));
                    }
                    if self.mode.is_creator() && reason == RoomCloseReason::IdleExpired {
                        return Err(CoreError::Protocol(
                            "the room joiner cannot expire the idle deadline".into(),
                        ));
                    }
                    self.connection
                        .close(VarInt::from_u32(0), b"peer closed room");
                    return Ok(RoomControlEvent::PeerClosed(reason));
                }
                ControlMessage::Ping { nonce } => {
                    self.send(ControlMessage::Pong { nonce }).await?;
                }
                ControlMessage::Pong { nonce } => {
                    return Ok(RoomControlEvent::Pong { nonce });
                }
            }
        }
    }

    fn resolve_remote_offer(&self, offer_id: &str) -> Result<(), SessionError> {
        let mut state = self
            .offers
            .lock()
            .map_err(|_| CoreError::Transport("room offer state unavailable".into()))?;
        if state.pending_remote.as_deref() != Some(offer_id) {
            return Err(CoreError::InvalidInput(
                "this room offer is not waiting for a local decision".into(),
            ));
        }
        state.pending_remote = None;
        Ok(())
    }

    fn resolve_local_offer(&self, offer_id: &str) -> Result<(), SessionError> {
        let mut state = self
            .offers
            .lock()
            .map_err(|_| CoreError::Transport("room offer state unavailable".into()))?;
        if state.pending_local.as_deref() != Some(offer_id) {
            return Err(CoreError::Protocol(
                "peer responded to an unknown room offer".into(),
            ));
        }
        state.pending_local = None;
        Ok(())
    }

    fn lock_lifetime(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, RoomLifetimeMachine>, SessionError> {
        self.lifetime
            .lock()
            .map_err(|_| CoreError::Transport("room lifetime state unavailable".into()))
    }

    async fn update_creator_lifetime(
        &self,
        transition: impl FnOnce(
            &mut RoomLifetimeMachine,
            u64,
        ) -> Result<Option<RoomLifetimeState>, SessionError>,
    ) -> Result<Option<RoomLifetimeState>, SessionError> {
        debug_assert!(self.mode.is_creator());
        let _update = self.lifetime_updates.lock().await;
        let state = {
            let mut lifetime = self.lock_lifetime()?;
            transition(&mut lifetime, now_unix_millis()?)?
        };
        if let Some(state) = &state {
            let mut mutation = CloseOnIncompleteMutation::new(&self.connection);
            self.send(ControlMessage::LifetimeChanged(state.clone()))
                .await?;
            mutation.disarm();
        }
        Ok(state)
    }

    async fn apply_authoritative_lifetime(
        &self,
        state: RoomLifetimeState,
    ) -> Result<RoomLifetimeState, SessionError> {
        let _update = self.lifetime_updates.lock().await;
        self.lock_lifetime()?.apply_authoritative(state)
    }

    async fn send(&self, message: ControlMessage) -> Result<(), SessionError> {
        let result = tokio::time::timeout(CONTROL_WRITE_TIMEOUT, async {
            let mut send = self.send.lock().await;
            write_control_message(&mut *send, &message).await
        })
        .await;
        match result {
            Ok(result) => result,
            Err(_) => {
                self.connection
                    .close(VarInt::from_u32(3), b"control write timed out");
                Err(CoreError::Transport("room control write timed out".into()))
            }
        }
    }

    async fn send_offer_response(&self, message: ControlMessage) -> Result<(), SessionError> {
        terminate_on_response_send_failure(self.send(message).await, || {
            self.connection
                .close(VarInt::from_u32(2), b"offer response failed");
        })
    }

    async fn receive(&self) -> Result<ControlMessage, SessionError> {
        let mut recv = self.recv.lock().await;
        read_control_message(&mut *recv).await
    }
}

fn terminate_on_response_send_failure<T>(
    result: Result<T, SessionError>,
    terminate: impl FnOnce(),
) -> Result<T, SessionError> {
    if result.is_err() {
        terminate();
    }
    result
}

impl Drop for RoomControlSession {
    fn drop(&mut self) {
        self.connection
            .close(VarInt::from_u32(0), b"room session dropped");
    }
}

pub async fn connect_room_control(
    invite: RoomControlInvite,
    display_name: String,
    creator: bool,
    config: SessionConfig,
    cancel: &TransferCancelToken,
) -> Result<RoomControlSession, SessionError> {
    invite.ensure_fresh()?;
    let room_id = invite.room_id();
    let context = InvitationControlContext::room_control(room_id.clone())
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    let mut peer_authenticated = false;
    connect_room_control_inner(
        RoomControlConnectRequest {
            broker: invite.broker,
            relay: invite.relay,
            room_id,
            password: invite.code,
            context,
            pairing_responder: creator,
            bootstrap_kind: BootstrapKind::RoomCode,
            mode: RoomControlSessionMode::Invitation { creator },
        },
        display_name,
        config,
        cancel,
        &mut peer_authenticated,
    )
    .await
}

/// Reopens a remembered relationship as an equal-member room control session.
///
/// The role applies only to this rendezvous attempt. Callers must arrange one
/// connector and one responder for the same credential generation, and must
/// persist the next generation after this function authenticates.
#[allow(clippy::too_many_arguments)]
pub async fn connect_remembered_room_control(
    remembered: RememberedSession,
    broker: String,
    relay: Option<String>,
    display_name: String,
    role: RememberedRoomControlRole,
    mut config: SessionConfig,
    cancel: &TransferCancelToken,
) -> Result<RoomControlSession, RememberedRoomControlConnectError> {
    remembered.generation().checked_add(1).ok_or_else(|| {
        RememberedRoomControlConnectError::new(
            CoreError::InvalidInput("remembered credential generation is exhausted".into()),
            false,
        )
    })?;
    let broker = broker.trim().to_string();
    if broker.is_empty() {
        return Err(RememberedRoomControlConnectError::new(
            CoreError::InvalidInput("remembered room control needs a broker".into()),
            false,
        ));
    }
    let relay = relay
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let generation = remembered.generation();
    let room_id = remembered.room_id().to_string();
    let password = remembered.control_password();
    let context = remembered
        .control_context()
        .map_err(|error| RememberedRoomControlConnectError::new(error, false))?;
    config.rendezvous_retry = remembered_room_control_retry_policy(config.rendezvous_retry);
    let mut peer_authenticated = false;
    let result = connect_room_control_inner(
        RoomControlConnectRequest {
            broker,
            relay,
            room_id,
            password,
            context,
            pairing_responder: role.is_responder(),
            bootstrap_kind: BootstrapKind::FullTicket,
            mode: RoomControlSessionMode::Remembered { generation },
        },
        display_name,
        config,
        cancel,
        &mut peer_authenticated,
    )
    .await;
    result.map_err(|error| RememberedRoomControlConnectError::new(error, peer_authenticated))
}

fn remembered_room_control_retry_policy(
    mut policy: RendezvousRetryPolicy,
) -> RendezvousRetryPolicy {
    // A foreground scheduler owns role, generation, and time-based fallback.
    // Retrying here would be invisible to that scheduler and would multiply
    // one logical probe into several independently rate-limited broker joins.
    policy.pairing_attempts = 1;
    policy.server_retries = 0;
    policy
}

struct RoomControlConnectRequest {
    broker: String,
    relay: Option<String>,
    room_id: String,
    password: String,
    context: InvitationControlContext,
    pairing_responder: bool,
    bootstrap_kind: BootstrapKind,
    mode: RoomControlSessionMode,
}

async fn connect_room_control_inner(
    request: RoomControlConnectRequest,
    display_name: String,
    mut config: SessionConfig,
    cancel: &TransferCancelToken,
    peer_authenticated: &mut bool,
) -> Result<RoomControlSession, SessionError> {
    validate_display_name(&display_name)?;
    config.relay = request.relay.clone();
    let accepted_alpns = [ROOM_CONTROL_ALPN.to_vec()];
    let endpoint = build_endpoint(
        Some(BindAddrs::dual_stack(0)),
        &config.identity,
        &accepted_alpns,
        false,
        &config.relay,
        config.relay_only,
        &config.candidates,
        config.data_stream_window,
    )
    .await?;
    let bound = BoundEndpoint {
        local_endpoint: endpoint,
        candidates: config.candidates.clone(),
    };
    let broker = parse_broker_addr(&request.broker, request.relay.as_deref())?;
    let my_addr = bound
        .ready_endpoint_addr(!config.direct_only && request.relay.is_some())
        .await;
    let (role, pairing) = pair_control(
        &bound.local_endpoint,
        RoomControlPairingRequest {
            broker,
            room_id: request.room_id,
            password: request.password,
            context: request.context,
            responder: request.pairing_responder,
            bootstrap_kind: request.bootstrap_kind,
            local_endpoint: my_addr,
            retry_policy: config.rendezvous_retry,
        },
        cancel,
    )
    .await?;
    *peer_authenticated = true;
    let pairing_binding = pairing.control_transcript_hash.as_bytes().to_vec();
    let (connection, mut send, mut recv) =
        establish_control_connection(&bound.local_endpoint, role, pairing.peer, cancel).await?;
    let local_lifetime = match request.mode {
        RoomControlSessionMode::Invitation { creator: true } => {
            Some(RoomLifetimeState::initial(now_unix_millis()?))
        }
        RoomControlSessionMode::Invitation { creator: false }
        | RoomControlSessionMode::Remembered { .. } => None,
    };
    let hello = ControlMessage::Hello {
        protocol_version: ROOM_CONTROL_VERSION,
        session_kind: request.mode.session_kind(),
        display_name,
        creator: request.mode.is_creator(),
        pairing_binding: pairing_binding.clone(),
        lifetime: local_lifetime.clone(),
    };
    let peer_hello = run_control_phase(
        async {
            write_control_message(&mut send, &hello).await?;
            read_control_message(&mut recv).await
        },
        cancel,
        CONTROL_HANDSHAKE_TIMEOUT,
        "room control hello",
    )
    .await?;
    let (peer_name, lifetime) =
        validate_control_hello(peer_hello, request.mode, &pairing_binding, local_lifetime)?;
    Ok(RoomControlSession {
        _endpoint: bound.local_endpoint,
        connection,
        send: Mutex::new(send),
        recv: Mutex::new(recv),
        offers: std::sync::Mutex::new(OfferState::default()),
        lifetime_updates: Mutex::new(()),
        lifetime: std::sync::Mutex::new(RoomLifetimeMachine::new(lifetime)),
        broker: request.broker,
        relay: request.relay,
        peer_name,
        mode: request.mode,
    })
}

fn validate_control_hello(
    peer_hello: ControlMessage,
    mode: RoomControlSessionMode,
    expected_pairing_binding: &[u8],
    local_lifetime: Option<RoomLifetimeState>,
) -> Result<(String, RoomLifetimeState), SessionError> {
    let (peer_kind, peer_name, peer_creator, peer_binding, peer_lifetime) = match peer_hello {
        ControlMessage::Hello {
            protocol_version,
            session_kind,
            display_name,
            creator,
            pairing_binding,
            lifetime,
        } if protocol_version == ROOM_CONTROL_VERSION => {
            validate_display_name(&display_name)?;
            (
                session_kind,
                display_name,
                creator,
                pairing_binding,
                lifetime,
            )
        }
        ControlMessage::Hello {
            protocol_version, ..
        } => {
            return Err(CoreError::Protocol(format!(
                "unsupported room control version {protocol_version}"
            )));
        }
        _ => {
            return Err(CoreError::Protocol(
                "room control peer did not send hello first".into(),
            ));
        }
    };
    if peer_kind != mode.session_kind() {
        return Err(CoreError::Protocol(
            "room control peer selected a different session mode".into(),
        ));
    }
    if peer_binding != expected_pairing_binding {
        return Err(CoreError::Crypto(
            "room control channel is not bound to its pairing".into(),
        ));
    }
    let lifetime = match (mode, local_lifetime, peer_creator, peer_lifetime) {
        (RoomControlSessionMode::Invitation { creator: true }, Some(state), false, None) => state,
        (RoomControlSessionMode::Invitation { creator: false }, None, true, Some(state)) => {
            validate_initial_lifetime(&state)?;
            state
        }
        (RoomControlSessionMode::Remembered { .. }, None, false, None) => {
            RoomLifetimeState::remembered()
        }
        (RoomControlSessionMode::Remembered { .. }, _, _, _) => {
            return Err(CoreError::Protocol(
                "remembered room peers cannot claim creator or lifetime ownership".into(),
            ));
        }
        _ => {
            return Err(CoreError::Protocol(
                "only the room creator can establish its lifetime".into(),
            ));
        }
    };
    Ok((peer_name, lifetime))
}

struct RoomControlPairingRequest {
    broker: iroh::EndpointAddr,
    room_id: String,
    password: String,
    context: InvitationControlContext,
    responder: bool,
    bootstrap_kind: BootstrapKind,
    local_endpoint: iroh::EndpointAddr,
    retry_policy: RendezvousRetryPolicy,
}

async fn pair_control(
    endpoint: &iroh::Endpoint,
    request: RoomControlPairingRequest,
    cancel: &TransferCancelToken,
) -> Result<(Role, RoomPairing<iroh::EndpointAddr>), SessionError> {
    let invitation_side = if request.responder {
        InvitationSide::Creator
    } else {
        InvitationSide::Joiner
    };
    let transfer_role = if request.responder {
        TransferRole::Receiver
    } else {
        TransferRole::Sender
    };
    let bootstrap_methods = request
        .responder
        .then_some(vec![request.bootstrap_kind])
        .unwrap_or_default();
    let selected_bootstrap_method = (!request.responder).then_some(request.bootstrap_kind);
    let mut last = None;
    for _ in 0..request.retry_policy.pairing_attempts.max(1) {
        let join = Join {
            version: RENDEZVOUS_PROTOCOL_VERSION,
            room_id: request.room_id.clone(),
            invitation_side,
            transfer_role,
            bootstrap_methods: bootstrap_methods.clone(),
            selected_bootstrap_method,
        };
        let session = tokio::select! {
            result = join_broker_with_retry(
                endpoint,
                &request.broker,
                join,
                request.retry_policy,
            ) => result?,
            _ = cancel.cancelled() => return Err(CoreError::Cancelled),
        };
        let role = session.role;
        let pairing = tokio::select! {
            result = tokio::time::timeout(
                PAIRING_EXCHANGE_TIMEOUT,
                drive_pairing(
                    session,
                    &request.password,
                    &request.context,
                    &request.local_endpoint,
                    None,
                ),
            ) => result,
            _ = cancel.cancelled() => return Err(CoreError::Cancelled),
        };
        match pairing {
            Ok(Ok(pairing)) => return Ok((role, pairing)),
            Ok(Err(error)) => last = Some(CoreError::Transport(error.to_string())),
            Err(_) => last = Some(CoreError::Transport("room control pairing stalled".into())),
        }
    }
    Err(last.expect("at least one room control pairing attempt"))
}

async fn establish_control_connection(
    endpoint: &iroh::Endpoint,
    role: Role,
    peer: iroh::EndpointAddr,
    cancel: &TransferCancelToken,
) -> Result<(Connection, SendStream, RecvStream), SessionError> {
    let expected_peer = peer.id;
    match role {
        Role::Initiator => {
            let accepted = tokio::select! {
                result = tokio::time::timeout(CONTROL_CONNECT_TIMEOUT, async {
                    loop {
                        let incoming = endpoint.accept().await.ok_or_else(|| {
                            CoreError::Transport("room control endpoint closed".into())
                        })?;
                        let connection = incoming.await
                            .map_err(|error| CoreError::Transport(error.to_string()))?;
                        if connection.remote_id() == expected_peer
                            && connection.alpn() == ROOM_CONTROL_ALPN
                        {
                            break Ok::<_, SessionError>(connection);
                        }
                        connection.close(VarInt::from_u32(1), b"unexpected room peer");
                    }
                }) => result
                    .map_err(|_| CoreError::Transport("room control connection timed out".into()))??,
                _ = cancel.cancelled() => return Err(CoreError::Cancelled),
            };
            let (send, recv) = run_control_phase(
                async {
                    accepted
                        .accept_bi()
                        .await
                        .map_err(|error| CoreError::Transport(error.to_string()))
                },
                cancel,
                CONTROL_HANDSHAKE_TIMEOUT,
                "room control stream accept",
            )
            .await?;
            Ok((accepted, send, recv))
        }
        Role::Responder => {
            let connection = tokio::select! {
                result = tokio::time::timeout(
                    CONTROL_CONNECT_TIMEOUT,
                    endpoint.connect(peer, ROOM_CONTROL_ALPN),
                ) => result
                    .map_err(|_| CoreError::Transport("room control connection timed out".into()))?
                    .map_err(|error| CoreError::Transport(error.to_string()))?,
                _ = cancel.cancelled() => return Err(CoreError::Cancelled),
            };
            if connection.remote_id() != expected_peer {
                return Err(CoreError::Crypto(
                    "room control connected to an unexpected endpoint".into(),
                ));
            }
            let (send, recv) = run_control_phase(
                async {
                    connection
                        .open_bi()
                        .await
                        .map_err(|error| CoreError::Transport(error.to_string()))
                },
                cancel,
                CONTROL_HANDSHAKE_TIMEOUT,
                "room control stream open",
            )
            .await?;
            Ok((connection, send, recv))
        }
    }
}

async fn run_control_phase<T, F>(
    future: F,
    cancel: &TransferCancelToken,
    timeout: Duration,
    label: &'static str,
) -> Result<T, SessionError>
where
    F: std::future::Future<Output = Result<T, SessionError>>,
{
    tokio::select! {
        result = tokio::time::timeout(timeout, future) => result
            .map_err(|_| CoreError::Transport(format!("{label} timed out")))?,
        _ = cancel.cancelled() => Err(CoreError::Cancelled),
    }
}

async fn write_control_message<W>(
    writer: &mut W,
    message: &ControlMessage,
) -> Result<(), SessionError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let payload =
        serde_json::to_vec(message).map_err(|error| CoreError::Protocol(error.to_string()))?;
    if payload.len() > MAX_CONTROL_FRAME_BYTES {
        return Err(CoreError::InvalidInput(
            "room control message exceeds its allocation bound".into(),
        ));
    }
    writer.write_all(b"ENRC").await?;
    writer
        .write_all(&ROOM_CONTROL_VERSION.to_be_bytes())
        .await?;
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_control_message<R>(reader: &mut R) -> Result<ControlMessage, SessionError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut header = [0_u8; 10];
    reader.read_exact(&mut header).await?;
    if &header[..4] != b"ENRC" {
        return Err(CoreError::Protocol("bad room control frame magic".into()));
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    if version != ROOM_CONTROL_VERSION {
        return Err(CoreError::Protocol(format!(
            "unsupported room control frame version {version}"
        )));
    }
    let length = u32::from_be_bytes(header[6..10].try_into().expect("fixed header")) as usize;
    if length > MAX_CONTROL_FRAME_BYTES {
        return Err(CoreError::Protocol(
            "room control frame exceeds its allocation bound".into(),
        ));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload).map_err(|error| CoreError::Protocol(error.to_string()))
}

fn normalize_room_code(code: &str) -> Result<String, SessionError> {
    let code = code.trim();
    if code.starts_with('R') || code.starts_with('r') {
        return Err(CoreError::InvalidInput(
            "legacy R-prefixed room codes are not supported".into(),
        ));
    }
    let canonical =
        RoomCode::parse(code).map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    Ok(canonical.canonical().to_string())
}

fn validate_display_name(name: &str) -> Result<(), SessionError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 48 || trimmed.chars().any(char::is_control) {
        return Err(CoreError::InvalidInput(
            "room display name must contain 1-48 visible characters".into(),
        ));
    }
    Ok(())
}

fn validate_initial_lifetime(state: &RoomLifetimeState) -> Result<(), SessionError> {
    state.validate()?;
    if state.revision != 1
        || state.policy != RoomLifetimePolicy::Idle15Minutes
        || state.idle_deadline_unix_ms.is_none()
    {
        return Err(CoreError::Protocol(
            "room creator sent an invalid initial lifetime".into(),
        ));
    }
    Ok(())
}

fn now_unix_millis() -> Result<u64, SessionError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .map_err(|_| CoreError::InvalidInput("system clock precedes Unix epoch".into()))
}

fn now_unix_secs() -> Result<u64, SessionError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| CoreError::InvalidInput("system clock precedes Unix epoch".into()))
}

fn percent_encode(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

fn percent_decode(value: &str) -> Result<String, SessionError> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let encoded = bytes
                .get(index + 1..index + 3)
                .ok_or_else(|| CoreError::InvalidInput("truncated percent escape".into()))?;
            let encoded = std::str::from_utf8(encoded)
                .map_err(|_| CoreError::InvalidInput("invalid percent escape".into()))?;
            output.push(
                u8::from_str_radix(encoded, 16)
                    .map_err(|_| CoreError::InvalidInput("invalid percent escape".into()))?,
            );
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output)
        .map_err(|_| CoreError::InvalidInput("room invitation is not UTF-8".into()))
}

#[cfg(test)]
#[path = "room_control_tests.rs"]
mod tests;
