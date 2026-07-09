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
mod invite;
pub mod driver;
mod options;
pub mod machine;
pub mod receipt;
mod source;
mod transfer;

pub use envoix_session::CandidateFilter;
pub use envoix_types::{DataPath, PairingStep};
pub use error::{ErrorKind, Phase, TransferError};
pub use event::{FailureCode, StampedEvent, TransferEvent};
pub use invite::{Invite, Role};
pub use options::{PathPolicy, TransferOptions};
pub use source::{PeerSource, TransferMode};
pub use transfer::{Transfer, TransferStats};

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use envoix_qr::{QrInvitePayload, generate_token};
use envoix_session::{
    BindAddrs, DEFAULT_CHUNK_SIZE, SessionConfig, TransferCancelToken, TransferDirection,
    TransferSummary, parse_broker_addr, receive_file_enable_mdns, receive_file_via_room,
    receive_file_with_bound_peer, send_file_enable_mdns, send_file_manual, send_file_via_room,
};
use tracing::Instrument;

use crate::{IdentityConfig, PeerDescriptor, PublicError};
use envoix_auth::PairingConfig;
use transfer::{EventSender, SessionEventAdapter, StatsHandle};

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
    envoix_session::split_code(code).0
}

/// Run a transfer body and emit one structured summary line for it (in the
/// ambient transfer span, so it carries `room`/`transfer_id`) - a compact,
/// grep-able ledger entry per transfer that does not require `--json`.
async fn with_summary(
    fut: TransferFuture,
    stats: StatsHandle,
) -> Result<TransferSummary, PublicError> {
    let result = fut.await;
    let stats = stats.snapshot();
    // The full data-path history, e.g. "relay -> [2607:..]:.. -> 1.2.3.4:..".
    let paths = stats
        .paths
        .iter()
        .map(|path| path.to_string())
        .collect::<Vec<_>>()
        .join(" -> ");
    match &result {
        Ok(summary) => tracing::info!(
            bytes = summary.bytes_transferred,
            file = %summary.file_name,
            avg_bps = stats.avg_bytes_per_sec,
            peak_bps = stats.peak_bytes_per_sec,
            connect_ms = stats.connect_latency_ms.unwrap_or_default(),
            duration_ms = stats.duration_ms,
            paths = %paths,
            outcome = "completed",
            "transfer finished"
        ),
        Err(error) => tracing::warn!(
            avg_bps = stats.avg_bytes_per_sec,
            paths = %paths,
            outcome = "failed",
            %error,
            "transfer finished"
        ),
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

/// A transfer to run via [`Client::run`]: a direction, the file/output path, an
/// ordered list of peer sources to try (falling back to the next only on a
/// pre-connection failure), and the per-transfer options.
#[derive(Debug)]
pub struct TransferRequest {
    /// Send a file or receive into a directory.
    pub direction: TransferDirection,
    /// The file to send, or the directory to receive into.
    pub path: PathBuf,
    /// Rendezvous sources to attempt in order (e.g. Room, then mDNS).
    pub sources: Vec<PeerSource>,
    /// Per-transfer options (relay, resume, bind addrs, path policy).
    pub options: TransferOptions,
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
        let config = config_path
            .map(crate::RuntimeConfig::read)
            .transpose()
            .map_err(setup_error)?
            .unwrap_or_default();
        let candidates = config.candidates.unwrap_or_default();
        Self::from_config_fields(
            config.chunk_size.as_deref(),
            &candidates.allow,
            &candidates.deny,
        )
    }

    /// A client assembled from discrete config fields - the shape the Android
    /// FFI passes across the boundary - plus the `ENVOIX_CHUNK_SIZE` override.
    /// This is the shared assembler behind [`Self::from_runtime_sources`], which
    /// only adds reading the fields from a TOML file first.
    pub fn from_config_fields(
        chunk_size: Option<&str>,
        candidates_allow: &[String],
        candidates_deny: &[String],
    ) -> Result<Self, TransferError> {
        let mut client = Self::new();
        if let Some(chunk_size) = chunk_size {
            client.chunk_size = crate::parse_chunk_size(chunk_size).map_err(setup_error)?;
        }
        if !candidates_allow.is_empty() || !candidates_deny.is_empty() {
            client.candidates = CandidateFilter::from_lists(candidates_allow, candidates_deny)
                .map_err(setup_error)?;
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
        let events_stats = events.stats_handle();
        let cancel = TransferCancelToken::new();
        let (fut, span) = self.build_send(to, file, &options, &events, &cancel)?;
        let task = tokio::spawn(with_summary(fut, events_stats.clone()).instrument(span));
        Ok(Transfer::new(
            event_receiver,
            cancel,
            events_phase,
            events_stats,
            task,
        ))
    }

    /// Builds one send attempt's future + tracing span, emitting through the
    /// given event channel and honoring the given cancel token. Shared by
    /// [`Self::send`] (a single source) and [`Self::run`] (fallback across
    /// sources on one channel), so all attempts stream to the same caller.
    fn build_send(
        &self,
        to: PeerSource,
        file: PathBuf,
        options: &TransferOptions,
        events: &EventSender,
        cancel: &TransferCancelToken,
    ) -> Result<(TransferFuture, tracing::Span), TransferError> {
        let sink = Box::new(SessionEventAdapter(events.clone()));
        let resume = options.resume;
        let mode = to.mode();
        let span = transfer_span(TransferDirection::Send, mode);

        let fut: TransferFuture = match to {
            PeerSource::Manual { peer, token } => {
                let pairing = shared_token(&token)?;
                let config = self.session_config(options);
                let cancel = cancel.clone();
                let events = events.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                        mode,
                    });
                    send_file_manual(peer, file, resume, config, &pairing, sink, cancel).await
                })
            }
            PeerSource::Invite { invite } => {
                let (peer, token) = resolve_invite(&invite)?;
                let pairing = shared_token(&token)?;
                let config = self.session_config(options);
                let cancel = cancel.clone();
                let events = events.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                        mode,
                    });
                    send_file_manual(peer, file, resume, config, &pairing, sink, cancel).await
                })
            }
            PeerSource::Mdns { token: Some(token) } => {
                let pairing = shared_token(&token)?;
                let config = self.session_config(options);
                let cancel = cancel.clone();
                let events = events.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                        mode,
                    });
                    send_file_enable_mdns(file, resume, config, &pairing, sink, cancel).await
                })
            }
            PeerSource::Mdns { token: None } => {
                return Err(TransferError::input("sending over mDNS requires a token"));
            }
            PeerSource::Room { code, broker } => {
                span.record("room", room_id_of(&code));
                let broker =
                    parse_broker_addr(&broker, options.relay.as_deref()).map_err(setup_error)?;
                let config = self.session_config(options);
                let cancel = cancel.clone();
                let events = events.clone();
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
        Ok((fut, span))
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
        let events_stats = events.stats_handle();
        let cancel = TransferCancelToken::new();
        let (fut, span) = self.build_receive(from, into, &options, &events, &cancel)?;
        let task = tokio::spawn(with_summary(fut, events_stats.clone()).instrument(span));
        Ok(Transfer::new(
            event_receiver,
            cancel,
            events_phase,
            events_stats,
            task,
        ))
    }

    /// Builds one receive attempt's future + tracing span, emitting through the
    /// given event channel. The receive-side counterpart to [`Self::build_send`],
    /// shared by [`Self::receive`] and [`Self::run`].
    fn build_receive(
        &self,
        from: PeerSource,
        into: PathBuf,
        options: &TransferOptions,
        events: &EventSender,
        cancel: &TransferCancelToken,
    ) -> Result<(TransferFuture, tracing::Span), TransferError> {
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
                let pairing = shared_token(&token)?;
                let config = self.session_config(options);
                let cancel = cancel.clone();
                let on_bound = advertise(events.clone(), token, None);
                let events = events.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Receive,
                        mode,
                    });
                    receive_file_with_bound_peer(
                        listen, into, config, &pairing, sink, on_bound, cancel,
                    )
                    .await
                })
            }
            PeerSource::ShowInvite { ttl_secs } => {
                let token = new_token()?;
                let pairing = shared_token(&token)?;
                let config = self.session_config(options);
                let cancel = cancel.clone();
                let on_bound = advertise(events.clone(), token, Some(ttl_secs));
                let events = events.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Receive,
                        mode,
                    });
                    receive_file_with_bound_peer(
                        listen, into, config, &pairing, sink, on_bound, cancel,
                    )
                    .await
                })
            }
            PeerSource::Mdns { token } => {
                // A provided token is only displayed; a generated one also
                // yields an invite so the sender can be handed a QR.
                let (token, invite_ttl) = match token {
                    Some(token) => (token, None),
                    None => (new_token()?, Some(DEFAULT_INVITE_TTL_SECS)),
                };
                let pairing = shared_token(&token)?;
                let config = self.session_config(options);
                let cancel = cancel.clone();
                let on_bound = advertise(events.clone(), token, invite_ttl);
                let events = events.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Receive,
                        mode,
                    });
                    receive_file_enable_mdns(listen, into, config, &pairing, sink, on_bound, cancel)
                        .await
                })
            }
            PeerSource::Room { code, broker } => {
                span.record("room", room_id_of(&code));
                let broker =
                    parse_broker_addr(&broker, options.relay.as_deref()).map_err(setup_error)?;
                let config = self.session_config(options);
                let cancel = cancel.clone();
                let events = events.clone();
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
        Ok((fut, span))
    }

    /// Runs a transfer, trying each source in [`TransferRequest::sources`] in
    /// order and falling back to the next **only** on a pre-connection failure -
    /// so a transfer that reached a live connection is never re-attempted. The
    /// returned [`Transfer`]'s event stream carries every attempt, on one
    /// channel; `cancel` stops whichever attempt is in flight.
    pub fn run(&self, request: TransferRequest) -> Result<Transfer, TransferError> {
        let TransferRequest {
            direction,
            path,
            sources,
            options,
        } = request;
        if sources.is_empty() {
            return Err(TransferError::input(
                "a transfer needs at least one peer source",
            ));
        }
        crate::validate_chunk_size(self.chunk_size).map_err(setup_error)?;
        validate_path_policy(&options)?;
        let (events, event_receiver) = EventSender::channel();
        let events_phase = events.phase_cell();
        let events_stats = events.stats_handle();
        let cancel = TransferCancelToken::new();
        let client = self.clone();
        let stats = events_stats.clone();
        let loop_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let count = sources.len();
            let mut last: Result<TransferSummary, PublicError> = Err(PublicError::Transfer(
                "no peer source attempted".to_string(),
            ));
            for (i, source) in sources.into_iter().enumerate() {
                let built = match direction {
                    TransferDirection::Send => {
                        client.build_send(source, path.clone(), &options, &events, &loop_cancel)
                    }
                    TransferDirection::Receive => {
                        client.build_receive(source, path.clone(), &options, &events, &loop_cancel)
                    }
                };
                let (fut, span) = match built {
                    Ok(pair) => pair,
                    Err(error) => {
                        // A source that fails to even build (bad invite, missing
                        // token, unsupported role) is a pre-connection failure.
                        last = Err(PublicError::InvalidInput(error.to_string()));
                        if i + 1 == count {
                            break;
                        }
                        continue;
                    }
                };
                match with_summary(fut, stats.clone()).instrument(span).await {
                    Ok(summary) => return Ok(summary),
                    Err(error) => {
                        last = Err(error);
                        // Fall back only if we never reached a live connection.
                        if stats.connected() || i + 1 == count {
                            break;
                        }
                    }
                }
            }
            // The event stream must tell the whole story on its own: emit the
            // terminal Failed (with its typed reason_code) here, because the
            // lower layers only return the error - the one session-level Failed
            // event is the mDNS multi-peer loop's per-attempt report.
            if let Err(error) = &last {
                let reason = error.to_string();
                events.emit(TransferEvent::Failed {
                    direction,
                    reason_code: event::FailureCode::classify(&reason),
                    reason,
                });
            }
            last
        });
        Ok(Transfer::new(
            event_receiver,
            cancel,
            events_phase,
            events_stats,
            task,
        ))
    }

    fn session_config(&self, options: &TransferOptions) -> SessionConfig {
        SessionConfig {
            chunk_size: self.chunk_size,
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

/// An `on_bound` callback that advertises the bound peer with its token and,
/// when `invite_ttl` is `Some`, an encoded invite expiring after that many
/// seconds. Shared by every listening receive (show-manual, show-invite, mDNS).
fn advertise(
    events: EventSender,
    token: String,
    invite_ttl: Option<u64>,
) -> impl FnOnce(PeerDescriptor) + Send {
    move |peer: PeerDescriptor| {
        let invite = invite_ttl.map(|ttl| {
            QrInvitePayload::new(token.clone(), peer.clone(), unix_now() + ttl).encode()
        });
        events.emit(TransferEvent::Advertised {
            peer,
            token: Some(token),
            invite,
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
    fn config_fields_apply_chunk_size_and_candidate_cidrs() {
        // The FFI path passes discrete fields (no file) and must assemble the
        // same client as the equivalent config.toml above.
        let deny = vec!["10.0.0.0/8".to_string(), "fe80::/10".to_string()];
        let client = Client::from_config_fields(Some("1M"), &[], &deny).unwrap();

        assert_eq!(client.chunk_size, 1024 * 1024);
        let kept = client
            .candidates
            .apply(["10.0.0.5:1".parse().unwrap(), "1.2.3.4:2".parse().unwrap()]);
        assert_eq!(kept, vec!["1.2.3.4:2".parse().unwrap()]);
    }

    #[test]
    fn config_fields_reject_invalid_candidate_cidr() {
        assert!(Client::from_config_fields(None, &[], &["not-a-cidr".to_string()]).is_err());
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

    #[test]
    fn run_rejects_empty_sources() {
        let error = client()
            .run(TransferRequest {
                direction: TransferDirection::Send,
                path: "f.txt".into(),
                sources: vec![],
                options: TransferOptions::default(),
            })
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
    }

    #[test]
    fn run_validates_chunk_size_up_front() {
        let mut client = Client::new();
        client.chunk_size = 0;
        let error = client
            .run(TransferRequest {
                direction: TransferDirection::Send,
                path: "f.txt".into(),
                sources: vec![PeerSource::Mdns {
                    token: Some("abcdefghijkl".into()),
                }],
                options: TransferOptions::default(),
            })
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
    }

    #[tokio::test]
    async fn run_surfaces_a_source_failure_through_wait() {
        // A lone unbuildable source (garbage invite) has no fallback, so the
        // error surfaces on the returned handle rather than synchronously.
        let error = client()
            .run(TransferRequest {
                direction: TransferDirection::Send,
                path: "f.txt".into(),
                sources: vec![PeerSource::Invite {
                    invite: "not-an-invite".into(),
                }],
                options: TransferOptions::default(),
            })
            .expect("run spawns the transfer")
            .wait()
            .await
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
    }

    #[tokio::test]
    async fn run_emits_a_terminal_failed_event_with_reason_code() {
        // The event stream must tell the whole story on its own: a failed run
        // ends with a Failed event carrying the typed reason_code (frontends
        // branch on it; the operation's Result is a separate channel).
        let mut transfer = client()
            .run(TransferRequest {
                direction: TransferDirection::Send,
                path: "f.txt".into(),
                sources: vec![PeerSource::Invite {
                    invite: "not-an-invite".into(),
                }],
                options: TransferOptions::default(),
            })
            .expect("run spawns the transfer");
        let mut terminal = None;
        while let Some(stamped) = transfer.next_event().await {
            if let TransferEvent::Failed { reason_code, .. } = stamped.event {
                terminal = Some(reason_code);
            }
        }
        assert_eq!(terminal, Some(FailureCode::Other));
    }
}
