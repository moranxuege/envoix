//! Foreground, direction-neutral room control over one authenticated QUIC stream.
//!
//! The room carries offers and decisions only. Every accepted offer still uses
//! a fresh legacy pairing invitation and the unchanged Manifest data plane.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use envoix_error::CoreError;
use envoix_rendezvous::Role;
use envoix_rendezvous_iroh::{RoomPairing, drive_pairing, generate_code, join_room_with_intent};
use iroh::endpoint::{Connection, RecvStream, SendStream, VarInt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::endpoint::build_endpoint;
use crate::{
    BindAddrs, BoundEndpoint, SessionConfig, SessionError, TransferCancelToken, parse_broker_addr,
};

pub const ROOM_CONTROL_ALPN: &[u8] = b"envoix-room-control/1";
const ROOM_CONTROL_VERSION: u16 = 1;
const ROOM_CODE_PREFIX: char = 'R';
const ROOM_URI_PREFIX: &str = "envoix://room/";
const ROOM_BROKER_NAMESPACE: &str = "control-v1:";
const ROOM_INVITE_TTL: Duration = Duration::from_secs(300);
const PAIRING_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(8);
const CONTROL_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROL_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
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
        let legacy_code = generate_code(2).map_err(CoreError::InvalidInput)?;
        let code = format!("{ROOM_CODE_PREFIX}{legacy_code}");
        Self::from_parts(
            code,
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
        if broker.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "room control invitation needs a broker".into(),
            ));
        }
        Ok(Self {
            code,
            broker,
            relay: relay.filter(|value| !value.trim().is_empty()),
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
        let nameplate = self
            .code
            .strip_prefix(ROOM_CODE_PREFIX)
            .and_then(|code| code.split('-').next())
            .expect("validated room code");
        format!("{ROOM_BROKER_NAMESPACE}{nameplate}")
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
    pub total_bytes: u64,
}

impl RoomTransferOffer {
    fn validate(&self) -> Result<(), SessionError> {
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
        if self.transfer_invite.len() > MAX_TRANSFER_INVITE_BYTES
            || !self.transfer_invite.starts_with("envoix://pair/")
            || !self
                .transfer_invite
                .split_once('?')
                .is_some_and(|(_, query)| query.split('&').any(|part| part == "role=send"))
        {
            return Err(CoreError::InvalidInput(
                "room offer needs a bounded directional sender invitation".into(),
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
    PolicyChanged(RoomLifetimePolicy),
    PeerClosed(RoomCloseReason),
    Pong {
        nonce: u64,
    },
}

#[derive(Clone, Deserialize, Serialize)]
enum ControlMessage {
    Hello {
        protocol_version: u16,
        display_name: String,
        creator: bool,
        pairing_binding: String,
    },
    TransferOffer(RoomTransferOffer),
    OfferAccepted {
        offer_id: String,
    },
    OfferRejected {
        offer_id: String,
        reason: RoomOfferRejection,
    },
    PolicyChanged(RoomLifetimePolicy),
    Close(RoomCloseReason),
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
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

pub struct RoomControlSession {
    _endpoint: iroh::Endpoint,
    connection: Connection,
    send: Mutex<SendStream>,
    recv: Mutex<RecvStream>,
    offers: std::sync::Mutex<OfferState>,
    peer_name: String,
    creator: bool,
}

impl RoomControlSession {
    pub fn peer_name(&self) -> &str {
        &self.peer_name
    }

    pub fn is_creator(&self) -> bool {
        self.creator
    }

    pub async fn offer_transfer(&self, offer: RoomTransferOffer) -> Result<(), SessionError> {
        offer.validate()?;
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
        if let Err(error) = self.send(ControlMessage::TransferOffer(offer)).await {
            if let Ok(mut state) = self.offers.lock() {
                state.pending_local = None;
            }
            return Err(error);
        }
        Ok(())
    }

    pub async fn accept_offer(&self, offer_id: &str) -> Result<(), SessionError> {
        self.resolve_remote_offer(offer_id)?;
        self.send_offer_response(ControlMessage::OfferAccepted {
            offer_id: offer_id.to_string(),
        })
        .await
    }

    pub async fn reject_offer(
        &self,
        offer_id: &str,
        reason: RoomOfferRejection,
    ) -> Result<(), SessionError> {
        self.resolve_remote_offer(offer_id)?;
        self.send_offer_response(ControlMessage::OfferRejected {
            offer_id: offer_id.to_string(),
            reason,
        })
        .await
    }

    pub async fn set_policy(&self, policy: RoomLifetimePolicy) -> Result<(), SessionError> {
        if !self.creator {
            return Err(CoreError::InvalidInput(
                "only the room creator can change its lifetime".into(),
            ));
        }
        self.send(ControlMessage::PolicyChanged(policy)).await
    }

    pub async fn ping(&self, nonce: u64) -> Result<(), SessionError> {
        self.send(ControlMessage::Ping { nonce }).await
    }

    pub async fn close(&self, reason: RoomCloseReason) -> Result<(), SessionError> {
        let mut send = self.send.lock().await;
        let result = write_control_message(&mut *send, &ControlMessage::Close(reason)).await;
        if result.is_ok() {
            let _ = send.finish();
            let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
        }
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
                    if let Err(error) = offer.validate() {
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
                ControlMessage::PolicyChanged(policy) => {
                    if self.creator {
                        return Err(CoreError::Protocol(
                            "the room joiner cannot change its lifetime".into(),
                        ));
                    }
                    return Ok(RoomControlEvent::PolicyChanged(policy));
                }
                ControlMessage::Close(reason) => {
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

    async fn send(&self, message: ControlMessage) -> Result<(), SessionError> {
        let mut send = self.send.lock().await;
        write_control_message(&mut *send, &message).await
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
    mut config: SessionConfig,
    cancel: &TransferCancelToken,
) -> Result<RoomControlSession, SessionError> {
    invite.ensure_fresh()?;
    validate_display_name(&display_name)?;
    config.relay = invite.relay.clone();
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
    let broker = parse_broker_addr(&invite.broker, invite.relay.as_deref())?;
    let my_addr = bound
        .ready_endpoint_addr(!config.direct_only && invite.relay.is_some())
        .await;
    let (role, pairing) = pair_control(
        &bound.local_endpoint,
        broker,
        &invite.room_id(),
        invite.code(),
        &my_addr,
        cancel,
    )
    .await?;
    let (connection, mut send, mut recv) =
        establish_control_connection(&bound.local_endpoint, role, pairing.peer, cancel).await?;
    let hello = ControlMessage::Hello {
        protocol_version: ROOM_CONTROL_VERSION,
        display_name,
        creator,
        pairing_binding: pairing.token.clone(),
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
    let (peer_name, peer_creator, peer_binding) = match peer_hello {
        ControlMessage::Hello {
            protocol_version,
            display_name,
            creator,
            pairing_binding,
        } if protocol_version == ROOM_CONTROL_VERSION => {
            validate_display_name(&display_name)?;
            (display_name, creator, pairing_binding)
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
    if peer_creator == creator {
        return Err(CoreError::Protocol(
            "room control needs exactly one creator".into(),
        ));
    }
    if peer_binding != pairing.token {
        return Err(CoreError::Crypto(
            "room control channel is not bound to its pairing".into(),
        ));
    }
    Ok(RoomControlSession {
        _endpoint: bound.local_endpoint,
        connection,
        send: Mutex::new(send),
        recv: Mutex::new(recv),
        offers: std::sync::Mutex::new(OfferState::default()),
        peer_name,
        creator,
    })
}

async fn pair_control(
    endpoint: &iroh::Endpoint,
    broker: iroh::EndpointAddr,
    room_id: &str,
    password: &str,
    my_addr: &iroh::EndpointAddr,
    cancel: &TransferCancelToken,
) -> Result<(Role, RoomPairing<iroh::EndpointAddr>), SessionError> {
    const ATTEMPTS: usize = 4;
    let mut last = None;
    for _ in 0..ATTEMPTS {
        let session = tokio::select! {
            result = join_room_with_intent(endpoint, broker.clone(), room_id, None) => {
                result.map_err(|error| CoreError::Transport(error.to_string()))?
            }
            _ = cancel.cancelled() => return Err(CoreError::Cancelled),
        };
        let role = session.role;
        let pairing = tokio::select! {
            result = tokio::time::timeout(
                PAIRING_EXCHANGE_TIMEOUT,
                drive_pairing(session, password, my_addr),
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
    let Some(prefix) = code.chars().next() else {
        return Err(CoreError::InvalidInput(
            "room code must start with 'R'".into(),
        ));
    };
    if !prefix.eq_ignore_ascii_case(&ROOM_CODE_PREFIX) {
        return Err(CoreError::InvalidInput(
            "room code must start with 'R'".into(),
        ));
    }
    let legacy = &code[prefix.len_utf8()..];
    let mut parts = legacy.split('-');
    let nameplate = parts.next().unwrap_or_default();
    let first = parts.next().unwrap_or_default();
    let second = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || nameplate.len() != 6
        || !nameplate.bytes().all(|byte| byte.is_ascii_digit())
        || [first, second]
            .into_iter()
            .any(|word| word.is_empty() || !word.bytes().all(|byte| byte.is_ascii_alphabetic()))
    {
        return Err(CoreError::InvalidInput(
            "room code must have the form R<6 digits>-<word>-<word>".into(),
        ));
    }
    Ok(format!(
        "{ROOM_CODE_PREFIX}{nameplate}-{}-{}",
        first.to_ascii_lowercase(),
        second.to_ascii_lowercase()
    ))
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
