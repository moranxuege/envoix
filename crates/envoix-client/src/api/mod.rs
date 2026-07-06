//! The unified client API (see `docs/design/client-api.md`).
//!
//! One entry point per operation: a transfer is described by *what* to move,
//! *who* to move it with ([`PeerSource`]), and *how* to connect
//! ([`TransferOptions`]); it is observed through one event stream
//! ([`TransferEvent`]) and controlled through a [`Transfer`] handle.
//!
//! Binding-friendly by construction: no generics, closures, or lifetimes in
//! public signatures, so the surface can be exposed through UniFFI later.

mod error;
mod event;
mod options;
mod source;
mod transfer;

pub use envoix_session::CandidateFilter;
pub use envoix_types::{DataPath, PairingStep};
pub use error::{ErrorKind, Phase, TransferError};
pub use event::{StampedEvent, TransferEvent};
pub use options::{PathPolicy, TransferOptions};
pub use source::{PeerSource, TransferMode};
pub use transfer::Transfer;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use envoix_qr::{QrInvitePayload, generate_token};
use envoix_session::{
    BindAddrs, DEFAULT_CHUNK_SIZE, SessionConfig, TransferCancelToken, TransferDirection,
    TransferSummary, bind_iroh_endpoint_enable_mdns, parse_broker_addr, receive_file_via_room,
    receive_file_with_bound_peer, receive_with_auth_retries, send_file_enable_mdns,
    send_file_manual, send_file_via_room,
};
use tracing::Instrument;

use crate::{IdentityConfig, PeerDescriptor, PublicError};
use envoix_auth::PairingConfig;
use transfer::{EventSender, SessionEventAdapter};

/// Placeholder pairing for room transfers: the room flow derives the real
/// token from the SPAKE2 exchange and overrides this before authentication;
/// it exists only because `SessionConfig` requires a pairing.
const ROOM_PLACEHOLDER_TOKEN: &str = "envoix-room-unused-placeholder";

/// The transfer body each dispatch arm produces, run under the correlation span.
type TransferFuture = Pin<Box<dyn Future<Output = Result<TransferSummary, PublicError>> + Send>>;

/// The correlation span a transfer runs in: `room`/`transfer_id` are recorded
/// once known (the room id up front for room transfers, the transfer id when
/// the transfer starts), so every client log line correlates by the same ids
/// the broker (`room`) and the peer (`transfer_id`) use.
fn transfer_span(direction: TransferDirection, mode: TransferMode) -> tracing::Span {
    tracing::info_span!(
        "transfer",
        ?direction,
        ?mode,
        room = tracing::field::Empty,
        transfer_id = tracing::field::Empty,
    )
}

/// The rendezvous room id (the part the broker sees) of a room code, i.e.
/// everything before the first `-`; the remainder is the SPAKE2 password and
/// must never reach a log.
fn room_id_of(code: &str) -> &str {
    code.split('-').next().unwrap_or(code)
}

/// Run a transfer body and emit one structured summary line for it (in the
/// ambient transfer span, so it carries `room`/`transfer_id`) - a compact,
/// grep-able ledger entry per transfer that does not require `--json`.
async fn with_summary(fut: TransferFuture) -> Result<TransferSummary, PublicError> {
    let result = fut.await;
    match &result {
        Ok(summary) => tracing::info!(
            bytes = summary.bytes_transferred,
            file = %summary.file_name,
            outcome = "completed",
            "transfer finished"
        ),
        Err(error) => tracing::warn!(outcome = "failed", %error, "transfer finished"),
    }
    result
}

/// Invite lifetime when the source does not specify one (mDNS listener with
/// a generated token).
const DEFAULT_INVITE_TTL_SECS: u64 = 300;

/// The client: local policy (identity, chunk size) shared by transfers.
///
/// Construct with [`Client::new`] and adjust fields as needed; start
/// transfers with [`Client::send`] and [`Client::receive`]. Both must be
/// called within a tokio runtime.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Client {
    /// Maximum chunk payload size used for transfers.
    pub chunk_size: usize,
    /// iroh endpoint identity policy.
    pub identity: IdentityConfig,
    /// CIDR filter over the candidate addresses advertised to a peer.
    pub candidates: CandidateFilter,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            identity: IdentityConfig::Ephemeral,
            candidates: CandidateFilter::default(),
        }
    }
}

impl Client {
    /// A client with the default chunk size and an ephemeral identity.
    pub fn new() -> Self {
        Self::default()
    }

    /// A client configured from an optional TOML config file and environment
    /// overrides (`ENVOIX_CHUNK_SIZE`) - the runtime sources the CLI reads,
    /// without the legacy requirement of supplying a pairing up front.
    pub fn from_runtime_sources(config_path: Option<&Path>) -> Result<Self, TransferError> {
        let mut client = Self::new();
        if let Some(path) = config_path {
            let config = crate::RuntimeConfig::read(path).map_err(setup_error)?;
            if let Some(chunk_size) = config.chunk_size {
                client.chunk_size = crate::parse_chunk_size(&chunk_size).map_err(setup_error)?;
            }
            if let Some(candidates) = config.candidates {
                client.candidates =
                    CandidateFilter::from_lists(&candidates.allow, &candidates.deny)
                        .map_err(setup_error)?;
            }
        }
        if let Some(value) = std::env::var_os(crate::ENVOIX_CHUNK_SIZE) {
            let value = value.into_string().map_err(|_| {
                TransferError::input(format!("{} is not UTF-8", crate::ENVOIX_CHUNK_SIZE))
            })?;
            client.chunk_size = crate::parse_chunk_size(&value).map_err(setup_error)?;
        }
        crate::validate_chunk_size(client.chunk_size).map_err(setup_error)?;
        Ok(client)
    }

    /// Sends `file` to the peer described by `to`.
    ///
    /// Fails fast (before any network activity) on invalid input or a peer
    /// source that cannot be sent to yet; everything later is reported
    /// through the returned [`Transfer`].
    pub fn send(
        &self,
        file: PathBuf,
        to: PeerSource,
        options: TransferOptions,
    ) -> Result<Transfer, TransferError> {
        crate::validate_chunk_size(self.chunk_size).map_err(setup_error)?;
        validate_path_policy(&options)?;
        let (events, event_receiver) = EventSender::channel();
        let events_phase = events.phase_cell();
        let cancel = TransferCancelToken::new();
        let sink = Box::new(SessionEventAdapter(events.clone()));
        let resume = options.resume;
        let mode = to.mode();
        let span = transfer_span(TransferDirection::Send, mode);

        let fut: TransferFuture = match to {
            PeerSource::Manual { peer, token } => {
                let config = self.session_config(shared_token(&token)?, &options);
                let cancel = cancel.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                        mode,
                    });
                    send_file_manual(peer, file, resume, config, sink, cancel).await
                })
            }
            PeerSource::Invite { invite } => {
                let (peer, token) = resolve_invite(&invite)?;
                let config = self.session_config(shared_token(&token)?, &options);
                let cancel = cancel.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                        mode,
                    });
                    send_file_manual(peer, file, resume, config, sink, cancel).await
                })
            }
            PeerSource::Mdns { token: Some(token) } => {
                let config = self.session_config(shared_token(&token)?, &options);
                let cancel = cancel.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                        mode,
                    });
                    send_file_enable_mdns(file, resume, config, sink, cancel).await
                })
            }
            PeerSource::Mdns { token: None } => {
                return Err(TransferError::input("sending over mDNS requires a token"));
            }
            PeerSource::Room { code, broker } => {
                span.record("room", room_id_of(&code));
                let broker =
                    parse_broker_addr(&broker, options.relay.as_deref()).map_err(setup_error)?;
                let config = self.session_config(shared_token(ROOM_PLACEHOLDER_TOKEN)?, &options);
                let cancel = cancel.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                        mode,
                    });
                    send_file_via_room(broker, &code, file, resume, config, sink, cancel).await
                })
            }
            PeerSource::ShowManual { .. } | PeerSource::ShowInvite { .. } => {
                return Err(TransferError::input(
                    "this peer source listens for a dialer; sending toward it needs \
                     protocol role negotiation and is not supported yet",
                ));
            }
        };
        let task = tokio::spawn(with_summary(fut).instrument(span));
        Ok(Transfer::new(event_receiver, cancel, events_phase, task))
    }

    /// Receives one file into the `into` directory from the peer described
    /// by `from`.
    ///
    /// Listening sources report our address (and token/invite) through a
    /// [`TransferEvent::Advertised`] event for the user to hand to the peer.
    /// Fails fast on invalid input or a peer source that cannot be received
    /// from yet.
    pub fn receive(
        &self,
        into: PathBuf,
        from: PeerSource,
        options: TransferOptions,
    ) -> Result<Transfer, TransferError> {
        crate::validate_chunk_size(self.chunk_size).map_err(setup_error)?;
        validate_path_policy(&options)?;
        let (events, event_receiver) = EventSender::channel();
        let events_phase = events.phase_cell();
        let cancel = TransferCancelToken::new();
        let sink = Box::new(SessionEventAdapter(events.clone()));
        let listen = options
            .listen_addrs
            .clone()
            .unwrap_or_else(|| BindAddrs::dual_stack(0));
        let mode = from.mode();
        let span = transfer_span(TransferDirection::Receive, mode);

        let fut: TransferFuture = match from {
            PeerSource::ShowManual { token } => {
                let token = token.map_or_else(new_token, Ok)?;
                let config = self.session_config(shared_token(&token)?, &options);
                let cancel = cancel.clone();
                let on_bound = {
                    let events = events.clone();
                    move |peer: PeerDescriptor| {
                        events.emit(TransferEvent::Advertised {
                            peer,
                            token: Some(token),
                            invite: None,
                        });
                    }
                };
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Receive,
                        mode,
                    });
                    receive_file_with_bound_peer(listen, into, config, sink, on_bound, cancel).await
                })
            }
            PeerSource::ShowInvite { ttl_secs } => {
                let token = new_token()?;
                let config = self.session_config(shared_token(&token)?, &options);
                let cancel = cancel.clone();
                let on_bound = advertise_with_invite(events.clone(), token, ttl_secs);
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Receive,
                        mode,
                    });
                    receive_file_with_bound_peer(listen, into, config, sink, on_bound, cancel).await
                })
            }
            PeerSource::Mdns { token } => {
                // A provided token is only displayed; a generated one also
                // yields an invite so the sender can be handed a QR.
                let (token, invite_ttl) = match token {
                    Some(token) => (token, None),
                    None => (new_token()?, Some(DEFAULT_INVITE_TTL_SECS)),
                };
                let config = self.session_config(shared_token(&token)?, &options);
                let identity = self.identity.clone();
                let cancel = cancel.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Receive,
                        mode,
                    });
                    let endpoint = bind_iroh_endpoint_enable_mdns(listen, &identity)
                        .await?
                        .with_candidate_filter(config.candidates.clone());
                    let peer = endpoint.peer_descriptor()?;
                    let invite = invite_ttl.map(|ttl| {
                        QrInvitePayload::new(token.clone(), peer.clone(), unix_now() + ttl).encode()
                    });
                    events.emit(TransferEvent::Advertised {
                        peer,
                        token: Some(token),
                        invite,
                    });
                    receive_with_auth_retries(endpoint, into, config, sink, cancel).await
                })
            }
            PeerSource::Room { code, broker } => {
                span.record("room", room_id_of(&code));
                let broker =
                    parse_broker_addr(&broker, options.relay.as_deref()).map_err(setup_error)?;
                let config = self.session_config(shared_token(ROOM_PLACEHOLDER_TOKEN)?, &options);
                let cancel = cancel.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Receive,
                        mode,
                    });
                    receive_file_via_room(broker, &code, listen, into, config, sink, cancel).await
                })
            }
            PeerSource::Manual { .. } | PeerSource::Invite { .. } => {
                return Err(TransferError::input(
                    "this peer source dials a listener; receiving from it needs \
                     protocol role negotiation and is not supported yet",
                ));
            }
        };
        let task = tokio::spawn(with_summary(fut).instrument(span));
        Ok(Transfer::new(event_receiver, cancel, events_phase, task))
    }

    fn session_config(&self, pairing: PairingConfig, options: &TransferOptions) -> SessionConfig {
        SessionConfig {
            chunk_size: self.chunk_size,
            pairing,
            identity: self.identity.clone(),
            relay: options.relay.clone(),
            relay_only: options.path == PathPolicy::RelayOnly,
            direct_only: options.path == PathPolicy::DirectOnly,
            candidates: self.candidates.clone(),
        }
    }
}

fn shared_token(token: &str) -> Result<PairingConfig, TransferError> {
    PairingConfig::spake2_shared_token(token).map_err(setup_error)
}

/// Classifies an internal error that happened before the transfer started.
fn setup_error(error: PublicError) -> TransferError {
    TransferError::from_core(error, Phase::Setup)
}

/// Generates a fresh pairing token for listening sources given none.
fn new_token() -> Result<String, TransferError> {
    generate_token().map_err(|e| {
        setup_error(PublicError::Crypto(format!(
            "failed to generate token: {e}"
        )))
    })
}

/// An `on_bound` callback that advertises the peer together with an encoded
/// invite expiring after `ttl_secs`.
fn advertise_with_invite(
    events: EventSender,
    token: String,
    ttl_secs: u64,
) -> impl FnOnce(PeerDescriptor) + Send {
    move |peer: PeerDescriptor| {
        let invite =
            QrInvitePayload::new(token.clone(), peer.clone(), unix_now() + ttl_secs).encode();
        events.emit(TransferEvent::Advertised {
            peer,
            token: Some(token),
            invite: Some(invite),
        });
    }
}

fn validate_path_policy(options: &TransferOptions) -> Result<(), TransferError> {
    if options.path == PathPolicy::RelayOnly && options.relay.is_none() {
        return Err(TransferError::input(
            "PathPolicy::RelayOnly requires a relay",
        ));
    }
    Ok(())
}

/// Decodes and validates an invite, returning the peer to dial and the token.
fn resolve_invite(invite: &str) -> Result<(PeerDescriptor, String), TransferError> {
    let to_err = |e| TransferError::input(format!("invalid invite: {e}"));
    let payload = QrInvitePayload::decode(invite).map_err(to_err)?;
    payload.validate(unix_now()).map_err(to_err)?;
    let peer = payload.peer_descriptor().map_err(to_err)?;
    Ok((peer, payload.token))
}

/// Current Unix time in whole seconds.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client::new()
    }

    #[test]
    fn send_rejects_producer_sources() {
        for source in [
            PeerSource::ShowManual { token: None },
            PeerSource::ShowInvite { ttl_secs: 300 },
        ] {
            let error = client()
                .send("f.txt".into(), source, TransferOptions::default())
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Input);
        }
    }

    #[test]
    fn send_rejects_invalid_chunk_size() {
        let mut client = Client::new();
        client.chunk_size = 0;
        let error = client
            .send(
                "f.txt".into(),
                PeerSource::Mdns {
                    token: Some("abcdefghijkl".into()),
                },
                TransferOptions::default(),
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
    }

    #[test]
    fn send_over_mdns_requires_token() {
        let error = client()
            .send(
                "f.txt".into(),
                PeerSource::Mdns { token: None },
                TransferOptions::default(),
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
    }

    #[test]
    fn relay_only_requires_relay() {
        let options = TransferOptions {
            path: PathPolicy::RelayOnly,
            ..Default::default()
        };
        let error = client()
            .send(
                "f.txt".into(),
                PeerSource::Room {
                    code: "123456-a-b".into(),
                    broker: "unused".into(),
                },
                options,
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
    }

    #[test]
    fn receive_rejects_consumer_sources() {
        let peer = PeerDescriptor::new("peer", vec!["[::1]:9000".parse().unwrap()]).unwrap();
        for source in [
            PeerSource::Manual {
                peer,
                token: "abcdefghijkl".into(),
            },
            PeerSource::Invite {
                invite: "envoix:whatever".into(),
            },
        ] {
            let error = client()
                .receive("out".into(), source, TransferOptions::default())
                .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Input);
        }
    }

    #[test]
    fn runtime_sources_read_candidate_cidrs_from_config_file() {
        let path = std::env::temp_dir().join(format!(
            "envoix-api-config-{}-candidates.toml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "chunk_size = \"1M\"\n[candidates]\ndeny = [\"10.0.0.0/8\", \"fe80::/10\"]\n",
        )
        .unwrap();

        let client = Client::from_runtime_sources(Some(&path)).unwrap();

        assert_eq!(client.chunk_size, 1024 * 1024);
        // The deny list scopes addresses: a LAN address is dropped, a public one kept.
        let kept = client
            .candidates
            .apply(["10.0.0.5:1".parse().unwrap(), "1.2.3.4:2".parse().unwrap()]);
        assert_eq!(kept, vec!["1.2.3.4:2".parse().unwrap()]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn runtime_sources_reject_invalid_candidate_cidr() {
        let path = std::env::temp_dir().join(format!(
            "envoix-api-config-{}-badcidr.toml",
            std::process::id()
        ));
        std::fs::write(&path, "[candidates]\ndeny = [\"not-a-cidr\"]\n").unwrap();
        assert!(Client::from_runtime_sources(Some(&path)).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn runtime_sources_read_chunk_size_from_config_file() {
        let path = std::env::temp_dir().join(format!(
            "envoix-api-config-{}-chunk.toml",
            std::process::id()
        ));
        std::fs::write(&path, "chunk_size = \"1M\"\n").unwrap();

        let client = Client::from_runtime_sources(Some(&path)).unwrap();

        assert_eq!(client.chunk_size, 1024 * 1024);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn send_rejects_garbage_invite() {
        let error = client()
            .send(
                "f.txt".into(),
                PeerSource::Invite {
                    invite: "not-an-invite".into(),
                },
                TransferOptions::default(),
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
    }
}
