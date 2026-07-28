//! Foreground invitation delivery over an mDNS-resolved, authenticated iroh
//! endpoint.
//!
//! mDNS supplies addressing only. Iroh's endpoint ID pins the encrypted QUIC
//! connection, while the existing Room invitation remains the authentication
//! bootstrap for the subsequent room-control session.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use envoix_error::CoreError;
use iroh::endpoint::{Incoming, VarInt};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc};
use tokio::task::{JoinHandle, JoinSet};

use crate::endpoint::build_endpoint;
use crate::room_control::RoomControlInvite;
use crate::{
    BindAddrs, BoundEndpoint, CandidateFilter, DEFAULT_DATA_STREAM_WINDOW, IdentityConfig,
    SessionError,
};

/// QUIC ALPN reserved for the nearby Room-invitation inbox.
pub const NEARBY_INVITE_ALPN: &[u8] = b"envoix-nearby-invite/1";
/// Maximum accepted UTF-8 Room invitation size.
pub const MAX_NEARBY_INVITE_BYTES: usize = 2_048;

const NEARBY_INVITE_WIRE_VERSION: u16 = 1;
const NEARBY_INVITE_WIRE_MAGIC: &[u8; 4] = b"ENNI";
const MAX_NEARBY_INVITE_FRAME_BYTES: usize = 8 * 1_024;
const NEARBY_INVITE_QUEUE_CAPACITY: usize = 8;
const MAX_RECENT_INVITES: usize = 32;
const MAX_CONCURRENT_CONNECTIONS: usize = 8;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(6);
const ACK_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_NEARBY_ENDPOINT_ID_BYTES: usize = 128;
const MAX_NEARBY_RELAY_URL_BYTES: usize = 512;
const MAX_NEARBY_DIRECT_ADDRESSES: usize = 8;
const MAX_NEARBY_DIRECT_ADDRESS_BYTES: usize = 128;

/// One encrypted Room invitation accepted by a foreground inbox.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NearbyInvite {
    pub request_id: u64,
    pub sender_endpoint_id: String,
    pub sender_peer_key: String,
    pub sender_display_name: String,
    pub invite: String,
    pub expires_at_unix_secs: u64,
}

/// Explicit, authenticated routing material for one nearby invitation inbox.
///
/// Native discovery transports this bounded record. The endpoint ID pins the
/// encrypted QUIC connection; direct and relay routes are addressing hints only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NearbyInviteEndpoint {
    pub endpoint_id: String,
    pub relay_url: Option<String>,
    pub direct_addresses: Vec<String>,
}

impl NearbyInviteEndpoint {
    fn from_endpoint_addr(address: &EndpointAddr) -> Result<Self, SessionError> {
        let relay_urls = address
            .relay_urls()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if relay_urls.len() > 1 {
            return Err(CoreError::InvalidInput(
                "nearby invitation endpoint has more than one relay route".into(),
            ));
        }
        let endpoint = Self {
            endpoint_id: address.id.to_string(),
            relay_url: relay_urls.into_iter().next(),
            direct_addresses: address.ip_addrs().map(ToString::to_string).collect(),
        };
        endpoint.to_endpoint_addr()?;
        Ok(endpoint)
    }

    fn to_endpoint_addr(&self) -> Result<EndpointAddr, SessionError> {
        let endpoint_id = self.endpoint_id.as_str();
        if endpoint_id.is_empty()
            || endpoint_id.len() > MAX_NEARBY_ENDPOINT_ID_BYTES
            || endpoint_id.trim() != endpoint_id
        {
            return Err(CoreError::InvalidInput(
                "invalid nearby invitation endpoint id".into(),
            ));
        }
        let endpoint_id = endpoint_id.parse::<EndpointId>().map_err(|error| {
            CoreError::InvalidInput(format!("invalid nearby invitation endpoint id: {error}"))
        })?;

        if self.direct_addresses.len() > MAX_NEARBY_DIRECT_ADDRESSES {
            return Err(CoreError::InvalidInput(format!(
                "nearby invitation endpoint has more than {MAX_NEARBY_DIRECT_ADDRESSES} direct routes"
            )));
        }
        let mut direct_addresses = Vec::with_capacity(self.direct_addresses.len());
        for encoded in &self.direct_addresses {
            if encoded.is_empty()
                || encoded.len() > MAX_NEARBY_DIRECT_ADDRESS_BYTES
                || encoded.trim() != encoded
            {
                return Err(CoreError::InvalidInput(
                    "invalid nearby invitation direct route".into(),
                ));
            }
            let address = encoded.parse::<SocketAddr>().map_err(|_| {
                CoreError::InvalidInput(format!(
                    "invalid nearby invitation direct route {encoded:?}"
                ))
            })?;
            if address.port() == 0 || address.ip().is_unspecified() || address.ip().is_multicast() {
                return Err(CoreError::InvalidInput(format!(
                    "nearby invitation direct route is not connectable: {encoded}"
                )));
            }
            if address.to_string() != *encoded {
                return Err(CoreError::InvalidInput(format!(
                    "nearby invitation direct route is not canonical: {encoded}"
                )));
            }
            if direct_addresses.contains(&address) {
                return Err(CoreError::InvalidInput(
                    "nearby invitation endpoint repeats a direct route".into(),
                ));
            }
            direct_addresses.push(address);
        }

        let relay = self
            .relay_url
            .as_deref()
            .map(|encoded| {
                if encoded.is_empty()
                    || encoded.len() > MAX_NEARBY_RELAY_URL_BYTES
                    || encoded.trim() != encoded
                {
                    return Err(CoreError::InvalidInput(
                        "invalid nearby invitation relay route".into(),
                    ));
                }
                encoded.parse::<RelayUrl>().map_err(|error| {
                    CoreError::InvalidInput(format!(
                        "invalid nearby invitation relay route: {error}"
                    ))
                })
            })
            .transpose()?;
        if direct_addresses.is_empty() && relay.is_none() {
            return Err(CoreError::InvalidInput(
                "nearby invitation endpoint has no connectable route".into(),
            ));
        }

        Ok(EndpointAddr::from_parts(
            endpoint_id,
            direct_addresses
                .into_iter()
                .map(TransportAddr::Ip)
                .chain(relay.into_iter().map(TransportAddr::Relay)),
        ))
    }
}

/// A foreground endpoint that accepts bounded Room invitations.
pub struct NearbyInviteInbox {
    endpoint: Endpoint,
    advertised_endpoint: NearbyInviteEndpoint,
    incoming: Mutex<mpsc::Receiver<NearbyInvite>>,
    accept_task: StdMutex<Option<JoinHandle<()>>>,
    sender_peer_key: String,
    sender_display_name: String,
    next_request_id: AtomicU64,
    closed: AtomicBool,
}

/// Start a dedicated ephemeral nearby-invitation endpoint.
pub async fn start_nearby_invite_inbox(
    relay: Option<String>,
    peer_key: String,
    display_name: String,
) -> Result<NearbyInviteInbox, SessionError> {
    let sender_peer_key = normalize_peer_key(&peer_key)?;
    let sender_display_name = normalize_display_name(&display_name)?;
    let relay = relay
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let accepted_alpns = [NEARBY_INVITE_ALPN.to_vec()];
    let candidates = CandidateFilter::default();
    let endpoint = build_endpoint(
        Some(BindAddrs::dual_stack(0)),
        &IdentityConfig::Ephemeral,
        &accepted_alpns,
        false,
        &relay,
        false,
        &candidates,
        DEFAULT_DATA_STREAM_WINDOW,
    )
    .await?;
    let bound = BoundEndpoint {
        local_endpoint: endpoint,
        candidates,
    };
    let endpoint_addr = bound.ready_endpoint_addr(relay.is_some()).await;
    let advertised_endpoint = NearbyInviteEndpoint::from_endpoint_addr(&endpoint_addr)?;
    let endpoint = bound.local_endpoint;

    let (incoming_send, incoming_recv) = mpsc::channel(NEARBY_INVITE_QUEUE_CAPACITY);
    let recent = Arc::new(StdMutex::new(RecentInvites::default()));
    let accept_endpoint = endpoint.clone();
    let accept_task = tokio::spawn(async move {
        run_accept_loop(accept_endpoint, incoming_send, recent).await;
    });

    Ok(NearbyInviteInbox {
        endpoint,
        advertised_endpoint,
        incoming: Mutex::new(incoming_recv),
        accept_task: StdMutex::new(Some(accept_task)),
        sender_peer_key,
        sender_display_name,
        next_request_id: AtomicU64::new(1),
        closed: AtomicBool::new(false),
    })
}

impl NearbyInviteInbox {
    /// The exact self-authenticating route native presence advertising must
    /// associate with this foreground inbox.
    pub fn endpoint(&self) -> NearbyInviteEndpoint {
        self.advertised_endpoint.clone()
    }

    /// Wait for the next non-expired Room invitation.
    pub async fn next_invite(&self) -> Result<NearbyInvite, SessionError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(CoreError::Cancelled);
        }
        let mut incoming = self.incoming.lock().await;
        loop {
            let invite = incoming.recv().await.ok_or(CoreError::Cancelled)?;
            if self.closed.load(Ordering::Acquire) {
                return Err(CoreError::Cancelled);
            }
            if invite.expires_at_unix_secs > now_unix_secs()? {
                return Ok(invite);
            }
        }
    }

    /// Deliver one Room invitation to the exact endpoint ID selected by the
    /// native discovery layer. Success means the remote bounded inbox queued
    /// the invitation (or had already queued the same request).
    pub async fn send_invite(
        &self,
        target_endpoint: &NearbyInviteEndpoint,
        invite: &str,
    ) -> Result<(), SessionError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(CoreError::Cancelled);
        }
        validate_invite(invite).map_err(InviteValidationError::into_core_error)?;
        let target = target_endpoint.to_endpoint_addr()?;
        let target_id = target.id;
        if target_id == self.endpoint.id() {
            return Err(CoreError::InvalidInput(
                "cannot deliver a nearby invitation to this endpoint".into(),
            ));
        }

        let request_id = self.next_request_id();
        let connection = tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.endpoint.connect(target, NEARBY_INVITE_ALPN),
        )
        .await
        .map_err(|_| CoreError::Transport("nearby invitation connection timed out".into()))?
        .map_err(|error| CoreError::Transport(error.to_string()))?;
        if connection.remote_id() != target_id {
            connection.close(VarInt::from_u32(1), b"unexpected nearby endpoint");
            return Err(CoreError::Crypto(
                "nearby invitation connected to an unexpected endpoint".into(),
            ));
        }

        let result = tokio::time::timeout(EXCHANGE_TIMEOUT, async {
            let (mut send, mut recv) = connection
                .open_bi()
                .await
                .map_err(|error| CoreError::Transport(error.to_string()))?;
            write_wire_message(
                &mut send,
                &WireMessage::Request {
                    request_id,
                    sender_peer_key: self.sender_peer_key.clone(),
                    sender_display_name: self.sender_display_name.clone(),
                    invite: invite.to_string(),
                },
            )
            .await?;
            let _ = send.finish();
            match read_wire_message(&mut recv).await? {
                WireMessage::Ack {
                    request_id: acknowledged,
                    status,
                } if acknowledged == request_id => match status {
                    AckStatus::Queued | AckStatus::Duplicate => Ok(()),
                    AckStatus::Busy => Err(CoreError::Transport(
                        "nearby invitation inbox is busy".into(),
                    )),
                    AckStatus::Invalid => Err(CoreError::InvalidInput(
                        "nearby peer rejected the room invitation".into(),
                    )),
                    AckStatus::Expired => Err(CoreError::InvalidInput(
                        "nearby peer rejected the expired room invitation".into(),
                    )),
                },
                WireMessage::Ack { .. } => Err(CoreError::Protocol(
                    "nearby invitation acknowledgement did not match its request".into(),
                )),
                WireMessage::Request { .. } => Err(CoreError::Protocol(
                    "nearby invitation peer returned a request instead of an acknowledgement"
                        .into(),
                )),
            }
        })
        .await
        .map_err(|_| CoreError::Transport("nearby invitation exchange timed out".into()))?;
        connection.close(VarInt::from_u32(0), b"nearby invitation complete");
        result
    }

    /// Stop advertising, accepting, and resolving nearby invitation endpoints.
    pub async fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.endpoint.close().await;
        }
        let task = self
            .accept_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            let _ = task.await;
        }
        let mut incoming = self.incoming.lock().await;
        incoming.close();
        while incoming.try_recv().is_ok() {}
    }

    fn next_request_id(&self) -> u64 {
        loop {
            let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
            if request_id != 0 {
                return request_id;
            }
        }
    }
}

impl Drop for NearbyInviteInbox {
    fn drop(&mut self) {
        if let Some(task) = self
            .accept_task
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum WireMessage {
    Request {
        request_id: u64,
        sender_peer_key: String,
        sender_display_name: String,
        invite: String,
    },
    Ack {
        request_id: u64,
        status: AckStatus,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AckStatus {
    Queued,
    Duplicate,
    Busy,
    Invalid,
    Expired,
}

#[derive(Clone)]
struct RecentInvite {
    sender: EndpointId,
    request_id: u64,
    invite: String,
    expires_at_unix_secs: u64,
}

#[derive(Default)]
struct RecentInvites {
    entries: VecDeque<RecentInvite>,
}

impl RecentInvites {
    fn enqueue(
        &mut self,
        sender: EndpointId,
        event: NearbyInvite,
        incoming: &mpsc::Sender<NearbyInvite>,
    ) -> AckStatus {
        let now = now_unix_secs().unwrap_or(u64::MAX);
        self.entries
            .retain(|entry| entry.expires_at_unix_secs > now);
        if self.entries.iter().any(|entry| {
            entry.sender == sender
                && (entry.request_id == event.request_id || entry.invite == event.invite)
        }) {
            return AckStatus::Duplicate;
        }

        let recent = RecentInvite {
            sender,
            request_id: event.request_id,
            invite: event.invite.clone(),
            expires_at_unix_secs: event.expires_at_unix_secs,
        };
        match incoming.try_send(event) {
            Ok(()) => {
                self.entries.push_back(recent);
                while self.entries.len() > MAX_RECENT_INVITES {
                    self.entries.pop_front();
                }
                AckStatus::Queued
            }
            Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {
                AckStatus::Busy
            }
        }
    }
}

async fn run_accept_loop(
    endpoint: Endpoint,
    incoming: mpsc::Sender<NearbyInvite>,
    recent: Arc<StdMutex<RecentInvites>>,
) {
    let mut handlers = JoinSet::new();
    loop {
        if handlers.len() >= MAX_CONCURRENT_CONNECTIONS {
            let _ = handlers.join_next().await;
            continue;
        }
        tokio::select! {
            next = endpoint.accept() => {
                let Some(next) = next else { break };
                let incoming = incoming.clone();
                let recent = recent.clone();
                handlers.spawn(async move {
                    if let Err(error) = handle_incoming(next, incoming, recent).await {
                        tracing::debug!(%error, "nearby invitation connection failed");
                    }
                });
            }
            Some(_) = handlers.join_next(), if !handlers.is_empty() => {}
        }
    }
    handlers.abort_all();
    while handlers.join_next().await.is_some() {}
}

async fn handle_incoming(
    incoming_connection: Incoming,
    incoming_invites: mpsc::Sender<NearbyInvite>,
    recent: Arc<StdMutex<RecentInvites>>,
) -> Result<(), SessionError> {
    let connection = tokio::time::timeout(EXCHANGE_TIMEOUT, incoming_connection)
        .await
        .map_err(|_| CoreError::Transport("nearby invitation handshake timed out".into()))?
        .map_err(|error| CoreError::Transport(error.to_string()))?;
    if connection.alpn() != NEARBY_INVITE_ALPN {
        connection.close(VarInt::from_u32(1), b"unexpected nearby ALPN");
        return Err(CoreError::Protocol(
            "nearby invitation negotiated an unexpected ALPN".into(),
        ));
    }
    let sender = connection.remote_id();

    let result = tokio::time::timeout(EXCHANGE_TIMEOUT, async {
        let (mut send, mut recv) = connection
            .accept_bi()
            .await
            .map_err(|error| CoreError::Transport(error.to_string()))?;
        let request = read_wire_message(&mut recv).await?;
        let WireMessage::Request {
            request_id,
            sender_peer_key,
            sender_display_name,
            invite,
        } = request
        else {
            return Err(CoreError::Protocol(
                "nearby invitation peer sent an acknowledgement as its request".into(),
            ));
        };
        let normalized_sender = match (
            request_id,
            normalize_peer_key(&sender_peer_key),
            normalize_display_name(&sender_display_name),
        ) {
            (0, _, _) | (_, Err(_), _) | (_, _, Err(_)) => None,
            (_, Ok(peer_key), Ok(display_name)) => Some((peer_key, display_name)),
        };
        let status = match normalized_sender {
            Some((sender_peer_key, sender_display_name)) => match validate_invite(&invite) {
                Ok(validated) => {
                    let event = NearbyInvite {
                        request_id,
                        sender_endpoint_id: sender.to_string(),
                        sender_peer_key,
                        sender_display_name,
                        invite,
                        expires_at_unix_secs: validated.expires_at_unix_secs,
                    };
                    recent
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .enqueue(sender, event, &incoming_invites)
                }
                Err(InviteValidationError::Expired) => AckStatus::Expired,
                Err(InviteValidationError::TooLarge | InviteValidationError::Invalid) => {
                    AckStatus::Invalid
                }
            },
            None => AckStatus::Invalid,
        };
        write_wire_message(&mut send, &WireMessage::Ack { request_id, status }).await?;
        let _ = send.finish();
        let _ = tokio::time::timeout(ACK_DRAIN_TIMEOUT, connection.closed()).await;
        Ok(())
    })
    .await
    .map_err(|_| CoreError::Transport("nearby invitation exchange timed out".into()))?;
    connection.close(VarInt::from_u32(0), b"nearby invitation handled");
    result
}

async fn write_wire_message<W>(writer: &mut W, message: &WireMessage) -> Result<(), SessionError>
where
    W: AsyncWrite + Unpin,
{
    let payload =
        serde_json::to_vec(message).map_err(|error| CoreError::Protocol(error.to_string()))?;
    if payload.len() > MAX_NEARBY_INVITE_FRAME_BYTES {
        return Err(CoreError::InvalidInput(
            "nearby invitation frame exceeds its allocation bound".into(),
        ));
    }
    writer.write_all(NEARBY_INVITE_WIRE_MAGIC).await?;
    writer
        .write_all(&NEARBY_INVITE_WIRE_VERSION.to_be_bytes())
        .await?;
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_wire_message<R>(reader: &mut R) -> Result<WireMessage, SessionError>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0_u8; 10];
    reader.read_exact(&mut header).await?;
    if &header[..4] != NEARBY_INVITE_WIRE_MAGIC {
        return Err(CoreError::Protocol(
            "bad nearby invitation frame magic".into(),
        ));
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    if version != NEARBY_INVITE_WIRE_VERSION {
        return Err(CoreError::Protocol(format!(
            "unsupported nearby invitation frame version {version}"
        )));
    }
    let length = u32::from_be_bytes(header[6..10].try_into().expect("fixed header")) as usize;
    if length > MAX_NEARBY_INVITE_FRAME_BYTES {
        return Err(CoreError::Protocol(
            "nearby invitation frame exceeds its allocation bound".into(),
        ));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload).map_err(|error| CoreError::Protocol(error.to_string()))
}

#[derive(Debug)]
struct ValidatedInvite {
    expires_at_unix_secs: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InviteValidationError {
    TooLarge,
    Invalid,
    Expired,
}

impl InviteValidationError {
    fn into_core_error(self) -> CoreError {
        match self {
            Self::TooLarge => CoreError::InvalidInput(format!(
                "nearby Room invitation exceeds {MAX_NEARBY_INVITE_BYTES} bytes"
            )),
            Self::Invalid => {
                CoreError::InvalidInput("nearby invitation must be a valid Room invitation".into())
            }
            Self::Expired => CoreError::InvalidInput("nearby Room invitation has expired".into()),
        }
    }
}

fn validate_invite(invite: &str) -> Result<ValidatedInvite, InviteValidationError> {
    if invite.len() > MAX_NEARBY_INVITE_BYTES {
        return Err(InviteValidationError::TooLarge);
    }
    if !invite.starts_with("envoix://room/") {
        return Err(InviteValidationError::Invalid);
    }
    let parsed = RoomControlInvite::parse(invite, String::new(), None)
        .map_err(|_| InviteValidationError::Invalid)?;
    let now = now_unix_secs().map_err(|_| InviteValidationError::Invalid)?;
    if parsed.expires_at_unix_secs() <= now {
        return Err(InviteValidationError::Expired);
    }
    Ok(ValidatedInvite {
        expires_at_unix_secs: parsed.expires_at_unix_secs(),
    })
}

fn normalize_peer_key(peer_key: &str) -> Result<String, SessionError> {
    let peer_key = peer_key.trim().to_ascii_lowercase();
    if peer_key.len() != 16 || !peer_key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CoreError::InvalidInput(
            "nearby presence key must be 16 hexadecimal characters".into(),
        ));
    }
    Ok(peer_key)
}

fn normalize_display_name(display_name: &str) -> Result<String, SessionError> {
    let display_name = display_name.trim();
    if display_name.is_empty()
        || display_name.chars().count() > 48
        || display_name.chars().any(char::is_control)
    {
        return Err(CoreError::InvalidInput(
            "nearby display name must contain 1-48 visible characters".into(),
        ));
    }
    Ok(display_name.to_string())
}

fn now_unix_secs() -> Result<u64, SessionError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| CoreError::InvalidInput("system clock precedes Unix epoch".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    #[tokio::test]
    async fn wire_request_round_trips() {
        let expected = WireMessage::Request {
            request_id: 7,
            sender_peer_key: "0011223344556677".into(),
            sender_display_name: "Sender".into(),
            invite: "envoix://room/example".into(),
        };
        let (mut writer, mut reader) = tokio::io::duplex(16 * 1_024);
        write_wire_message(&mut writer, &expected)
            .await
            .expect("write request");
        assert_eq!(
            read_wire_message(&mut reader).await.expect("read request"),
            expected
        );
    }

    #[tokio::test]
    async fn oversized_wire_length_is_rejected_before_payload_allocation() {
        let (mut writer, mut reader) = tokio::io::duplex(32);
        writer
            .write_all(NEARBY_INVITE_WIRE_MAGIC)
            .await
            .expect("write magic");
        writer
            .write_all(&NEARBY_INVITE_WIRE_VERSION.to_be_bytes())
            .await
            .expect("write version");
        writer
            .write_all(&((MAX_NEARBY_INVITE_FRAME_BYTES + 1) as u32).to_be_bytes())
            .await
            .expect("write length");

        assert!(matches!(
            read_wire_message(&mut reader).await,
            Err(CoreError::Protocol(_))
        ));
    }

    #[test]
    fn room_invite_validation_rejects_non_room_expired_and_oversized_input() {
        assert_eq!(
            validate_invite("envoix://invite/v2/example").unwrap_err(),
            InviteValidationError::Invalid
        );
        assert_eq!(
            validate_invite(&"x".repeat(MAX_NEARBY_INVITE_BYTES + 1)).unwrap_err(),
            InviteValidationError::TooLarge
        );

        let live = RoomControlInvite::generate("broker", None)
            .expect("generate Room invitation")
            .payload();
        assert!(validate_invite(&live).is_ok());
        let before_expiry = live
            .split_once("&expires=")
            .expect("generated invitation has expiry")
            .0;
        let expired = format!("{before_expiry}&expires=0");
        assert_eq!(
            validate_invite(&expired).unwrap_err(),
            InviteValidationError::Expired
        );
    }

    #[tokio::test]
    async fn recent_invites_are_deduplicated_and_queue_is_bounded() {
        let sender = SecretKey::generate().public();
        let (events, mut received) = mpsc::channel(1);
        let mut recent = RecentInvites::default();
        assert_eq!(
            recent.enqueue(
                sender,
                NearbyInvite {
                    request_id: 1,
                    sender_endpoint_id: sender.to_string(),
                    sender_peer_key: "0011223344556677".into(),
                    sender_display_name: "Sender".into(),
                    invite: "first".into(),
                    expires_at_unix_secs: u64::MAX,
                },
                &events,
            ),
            AckStatus::Queued
        );
        assert_eq!(
            recent.enqueue(
                sender,
                NearbyInvite {
                    request_id: 1,
                    sender_endpoint_id: sender.to_string(),
                    sender_peer_key: "0011223344556677".into(),
                    sender_display_name: "Sender".into(),
                    invite: "first".into(),
                    expires_at_unix_secs: u64::MAX,
                },
                &events,
            ),
            AckStatus::Duplicate
        );
        assert_eq!(
            recent.enqueue(
                sender,
                NearbyInvite {
                    request_id: 2,
                    sender_endpoint_id: sender.to_string(),
                    sender_peer_key: "0011223344556677".into(),
                    sender_display_name: "Sender".into(),
                    invite: "second".into(),
                    expires_at_unix_secs: u64::MAX,
                },
                &events,
            ),
            AckStatus::Busy
        );

        let event = received.recv().await.expect("queued invite");
        assert_eq!(event.request_id, 1);
        assert_eq!(event.sender_peer_key, "0011223344556677");
        assert_eq!(event.sender_display_name, "Sender");
        assert_eq!(event.invite, "first");
    }

    #[test]
    fn nearby_identity_is_normalized_and_bounded() {
        assert_eq!(
            normalize_peer_key(" 001122334455AAbb ").expect("valid peer key"),
            "001122334455aabb"
        );
        assert!(normalize_peer_key("0011").is_err());
        assert_eq!(
            normalize_display_name("  Sender device  ").expect("valid display name"),
            "Sender device"
        );
        assert!(normalize_display_name("").is_err());
        assert!(normalize_display_name(&"x".repeat(49)).is_err());
    }

    #[test]
    fn explicit_endpoint_route_round_trips_id_relay_and_direct_addresses() {
        let endpoint_id = SecretKey::generate().public();
        let direct_v4 = "192.0.2.1:4433".parse::<SocketAddr>().unwrap();
        let direct_v6 = "[2001:db8::1]:4433".parse::<SocketAddr>().unwrap();
        let relay = "https://relay.example.test"
            .parse::<RelayUrl>()
            .expect("relay URL");
        let address = EndpointAddr::from_parts(
            endpoint_id,
            [
                TransportAddr::Ip(direct_v4),
                TransportAddr::Ip(direct_v6),
                TransportAddr::Relay(relay.clone()),
            ],
        );

        let projected =
            NearbyInviteEndpoint::from_endpoint_addr(&address).expect("project endpoint route");
        assert_eq!(projected.endpoint_id, endpoint_id.to_string());
        assert_eq!(projected.relay_url, Some(relay.to_string()));
        assert_eq!(
            projected.direct_addresses,
            [direct_v4.to_string(), direct_v6.to_string()]
        );

        let reparsed = projected.to_endpoint_addr().expect("parse endpoint route");
        assert_eq!(reparsed.id, endpoint_id);
        assert_eq!(
            reparsed.ip_addrs().copied().collect::<Vec<_>>(),
            [direct_v4, direct_v6]
        );
        assert_eq!(
            reparsed
                .relay_urls()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            [relay.to_string()]
        );
    }

    #[test]
    fn explicit_endpoint_route_rejects_unbounded_or_unconnectable_hints() {
        let endpoint_id = SecretKey::generate().public().to_string();
        let route =
            |direct_addresses: Vec<String>, relay_url: Option<String>| NearbyInviteEndpoint {
                endpoint_id: endpoint_id.clone(),
                relay_url,
                direct_addresses,
            };

        for address in [
            "0.0.0.0:4433",
            "[::]:4433",
            "224.0.0.1:4433",
            "[ff02::1]:4433",
            "192.0.2.1:0",
        ] {
            assert!(
                route(vec![address.into()], None)
                    .to_endpoint_addr()
                    .is_err(),
                "{address} must not be accepted"
            );
        }
        assert!(route(Vec::new(), None).to_endpoint_addr().is_err());
        assert!(
            route(
                vec!["192.0.2.1:4433".into(); MAX_NEARBY_DIRECT_ADDRESSES + 1],
                None
            )
            .to_endpoint_addr()
            .is_err()
        );
        assert!(
            route(vec!["192.0.2.1:4433".into(), "192.0.2.1:4433".into()], None,)
                .to_endpoint_addr()
                .is_err()
        );
        assert!(
            route(vec!["x".repeat(MAX_NEARBY_DIRECT_ADDRESS_BYTES + 1)], None,)
                .to_endpoint_addr()
                .is_err()
        );
        assert!(
            route(Vec::new(), Some("not a relay URL".into()))
                .to_endpoint_addr()
                .is_err()
        );
        assert!(
            route(Vec::new(), Some("x".repeat(MAX_NEARBY_RELAY_URL_BYTES + 1)),)
                .to_endpoint_addr()
                .is_err()
        );
    }
}
