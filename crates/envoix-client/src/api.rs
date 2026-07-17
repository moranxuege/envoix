//! The unified client API (see `docs/design/client-api.md`).
//!
//! One entry point per operation: a transfer is described by *what* to move,
//! *who* to move it with ([`PeerSource`]), and *how* to connect
//! ([`TransferOptions`]); it is observed through one event stream
//! ([`TransferEvent`]) and controlled through a [`Transfer`] handle.
//!
//! Binding-friendly by construction: no generics, closures, or lifetimes in
//! public signatures, so the surface can be exposed through UniFFI later.

pub mod driver;
mod error;
mod event;
mod invite;
pub mod machine;
pub mod manifest_activity;
pub mod manifest_driver;
mod options;
pub mod receipt;
pub mod record;
mod source;
mod transfer;
mod transport;

pub use envoix_protocol::{
    ManifestEntryKind, ManifestEntryResultStatus, ManifestEntryV1, ManifestHashAlgorithm,
    ManifestId, ManifestV1,
};
pub use envoix_session::{
    CandidateFilter, ManifestSendRequest, ManifestTransferSummary, SessionTransferSummary,
};
pub use envoix_types::{DataPath, PairingStep};
pub use error::{
    ErrorKind, FailureCategory, FailureCode, FailureOrigin, FailurePhase, Phase, RecoveryAction,
    TransferError, TransferFailure,
};
pub use event::{SessionFailureCode, StampedEvent, TransferEvent};
pub use invite::{Invite, Role};
pub use options::{PathPolicy, TransferOptions};
pub use source::{PeerSource, TransferMode};
pub use transfer::{Transfer, TransferSet, TransferStats};
pub use transport::{
    TransportAvailability, TransportCandidate, TransportPreference, TransportProvider,
    TransportSelection, TransportSelectionError, TransportSelectionReason, TransportSelector,
};

use transport::BUILT_IN_TRANSPORT_CANDIDATES;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use envoix_qr::{QrInvitePayload, generate_token};
use envoix_session::{
    BindAddrs, DEFAULT_CHUNK_SIZE, DEFAULT_DATA_STREAM_WINDOW, EndpointAddr, SessionConfig,
    TransferCancelToken, TransferDirection, TransferSummary, parse_broker_addr,
    receive_file_enable_mdns, receive_file_via_room, receive_file_with_bound_peer,
    receive_transfer_enable_mdns, receive_transfer_via_room, receive_transfer_with_bound_peer,
    send_file_enable_mdns, send_file_manual, send_file_to_endpoint_addr, send_file_via_room,
    send_manifest_enable_mdns, send_manifest_manual, send_manifest_to_endpoint_addr,
    send_manifest_via_room,
};
use tracing::Instrument;

use crate::{IdentityConfig, PeerDescriptor, PublicError};
use envoix_auth::PairingConfig;
use transfer::{EventSender, SessionEventAdapter, StatsHandle};

/// The transfer body each dispatch arm produces, run under the correlation span.
type TransferFuture = Pin<Box<dyn Future<Output = Result<TransferSummary, PublicError>> + Send>>;

/// An additive transfer body whose negotiated result may be one file or a
/// Manifest set.
type TransferSetFuture =
    Pin<Box<dyn Future<Output = Result<SessionTransferSummary, PublicError>> + Send>>;

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
        session_id = tracing::field::Empty,
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

/// Run a negotiated transfer body and log its protocol-specific aggregate
/// outcome without changing the compatible single-file summary surface.
async fn with_transfer_set_summary(
    fut: TransferSetFuture,
    stats: StatsHandle,
) -> Result<SessionTransferSummary, PublicError> {
    let result = fut.await;
    let stats = stats.snapshot();
    let paths = stats
        .paths
        .iter()
        .map(|path| path.to_string())
        .collect::<Vec<_>>()
        .join(" -> ");
    match &result {
        Ok(SessionTransferSummary::SingleFile(summary)) => tracing::info!(
            protocol = "single_file_v1",
            bytes = summary.bytes_transferred,
            file = %summary.file_name,
            avg_bps = stats.avg_bytes_per_sec,
            peak_bps = stats.peak_bytes_per_sec,
            connect_ms = stats.connect_latency_ms.unwrap_or_default(),
            duration_ms = stats.duration_ms,
            paths = %paths,
            outcome = "completed",
            "transfer set finished"
        ),
        Ok(SessionTransferSummary::Manifest(summary)) => tracing::info!(
            protocol = "manifest_v1",
            manifest_id = %summary.manifest_id,
            files = summary.file_count,
            directories = summary.directory_count,
            bytes = summary.total_bytes,
            avg_bps = stats.avg_bytes_per_sec,
            peak_bps = stats.peak_bytes_per_sec,
            connect_ms = stats.connect_latency_ms.unwrap_or_default(),
            duration_ms = stats.duration_ms,
            paths = %paths,
            outcome = "completed",
            "transfer set finished"
        ),
        Err(error) => tracing::warn!(
            avg_bps = stats.avg_bytes_per_sec,
            paths = %paths,
            outcome = "failed",
            %error,
            "transfer set finished"
        ),
    }
    result
}

/// Invite lifetime when the source does not specify one (mDNS listener with
/// a generated token).
const DEFAULT_INVITE_TTL_SECS: u64 = 300;
/// A sender that found no Room peer must eventually try the next configured
/// rendezvous source. Receivers intentionally have no deadline: waiting for a
/// sender is their normal steady state.
const ROOM_SEND_PRECONNECT_TIMEOUT: Duration = Duration::from_secs(60);

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
    /// Per-stream QUIC flow-control window (bytes) for the data endpoints.
    pub data_stream_window: u32,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            identity: IdentityConfig::Ephemeral,
            candidates: CandidateFilter::default(),
            data_stream_window: DEFAULT_DATA_STREAM_WINDOW,
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

/// One Manifest send run with ordered rendezvous fallback.
///
/// This is additive to [`TransferRequest`], whose established fields and
/// single-file behavior remain unchanged.
#[derive(Clone, Debug)]
pub struct ManifestTransferRequest {
    /// Validated transfer-set description plus its local source mapping.
    pub request: ManifestSendRequest,
    /// Rendezvous sources to attempt in order.
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
            config.data_stream_window.as_deref(),
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
        data_stream_window: Option<&str>,
    ) -> Result<Self, TransferError> {
        let mut client = Self::new();
        if let Some(chunk_size) = chunk_size {
            client.chunk_size = crate::parse_chunk_size(chunk_size).map_err(setup_error)?;
        }
        if !candidates_allow.is_empty() || !candidates_deny.is_empty() {
            client.candidates = CandidateFilter::from_lists(candidates_allow, candidates_deny)
                .map_err(setup_error)?;
        }
        if let Some(window) = data_stream_window {
            client.data_stream_window = crate::parse_window(window).map_err(setup_error)?;
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

    /// Sends one validated Manifest transfer set to `to` without changing the
    /// compatible single-file [`Self::send`] API.
    ///
    /// The receiving peer must listen through [`Self::receive_transfer`], which
    /// negotiates either the existing single-file protocol or Manifest v1.
    pub fn send_manifest(
        &self,
        request: ManifestSendRequest,
        to: PeerSource,
        options: TransferOptions,
    ) -> Result<TransferSet, TransferError> {
        crate::validate_chunk_size(self.chunk_size).map_err(setup_error)?;
        validate_path_policy(&options)?;
        request
            .manifest
            .validate_structure()
            .map_err(|error| TransferError::input(error.to_string()))?;
        let (events, event_receiver) = EventSender::channel();
        let events_phase = events.phase_cell();
        let events_stats = events.stats_handle();
        let cancel = TransferCancelToken::new();
        let (fut, span) = self.build_manifest_send(to, request, &options, &events, &cancel)?;
        let task =
            tokio::spawn(with_transfer_set_summary(fut, events_stats.clone()).instrument(span));
        Ok(TransferSet::new(
            event_receiver,
            cancel,
            events_phase,
            events_stats,
            task,
        ))
    }

    fn build_manifest_send(
        &self,
        to: PeerSource,
        request: ManifestSendRequest,
        options: &TransferOptions,
        events: &EventSender,
        cancel: &TransferCancelToken,
    ) -> Result<(TransferSetFuture, tracing::Span), TransferError> {
        match self.select_transport(options)?.provider {
            TransportProvider::Iroh => {
                self.build_iroh_manifest_send(to, request, options, events, cancel)
            }
            provider => Err(unregistered_transport_error(provider)),
        }
    }

    /// Existing iroh Manifest adapter. Provider selection happens in
    /// [`Self::build_manifest_send`], while iroh direct/relay policy remains
    /// inside the [`SessionConfig`] assembled below.
    fn build_iroh_manifest_send(
        &self,
        to: PeerSource,
        request: ManifestSendRequest,
        options: &TransferOptions,
        events: &EventSender,
        cancel: &TransferCancelToken,
    ) -> Result<(TransferSetFuture, tracing::Span), TransferError> {
        let sink = Box::new(SessionEventAdapter(events.clone()));
        let resume = options.resume;
        let mode = to.mode();
        let span = transfer_span(TransferDirection::Send, mode);

        let fut: TransferSetFuture = match to {
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
                    send_manifest_manual(peer, request, resume, config, &pairing, sink, cancel)
                        .await
                        .map(SessionTransferSummary::Manifest)
                })
            }
            PeerSource::Invite { invite } => {
                let (peer_addr, token) = resolve_invite(&invite, options.continuation)?;
                let pairing = shared_token(&token)?;
                let config = self.session_config(options);
                let cancel = cancel.clone();
                let events = events.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                        mode,
                    });
                    send_manifest_to_endpoint_addr(
                        peer_addr, request, resume, config, &pairing, sink, cancel,
                    )
                    .await
                    .map(SessionTransferSummary::Manifest)
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
                    send_manifest_enable_mdns(request, resume, config, &pairing, sink, cancel)
                        .await
                        .map(SessionTransferSummary::Manifest)
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
                    send_manifest_via_room(broker, &code, request, resume, config, sink, cancel)
                        .await
                        .map(SessionTransferSummary::Manifest)
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
        match self.select_transport(options)?.provider {
            TransportProvider::Iroh => self.build_iroh_send(to, file, options, events, cancel),
            provider => Err(unregistered_transport_error(provider)),
        }
    }

    /// Existing iroh single-file send adapter.
    fn build_iroh_send(
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
        if let Some(sid) = options.session_id {
            span.record("session_id", sid);
        }

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
                let (peer_addr, token) = resolve_invite(&invite, options.continuation)?;
                let pairing = shared_token(&token)?;
                let config = self.session_config(options);
                let cancel = cancel.clone();
                let events = events.clone();
                Box::pin(async move {
                    events.emit(TransferEvent::Binding {
                        direction: TransferDirection::Send,
                        mode,
                    });
                    send_file_to_endpoint_addr(
                        peer_addr, file, resume, config, &pairing, sink, cancel,
                    )
                    .await
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

    /// Receives one authenticated transfer into `into`, negotiating either the
    /// compatible single-file protocol or Manifest v1 after connection setup.
    ///
    /// The returned [`TransferSet`] reports the negotiated result without
    /// changing [`Self::receive`] or [`Transfer::wait`].
    pub fn receive_transfer(
        &self,
        into: PathBuf,
        from: PeerSource,
        options: TransferOptions,
    ) -> Result<TransferSet, TransferError> {
        crate::validate_chunk_size(self.chunk_size).map_err(setup_error)?;
        validate_path_policy(&options)?;
        let (events, event_receiver) = EventSender::channel();
        let events_phase = events.phase_cell();
        let events_stats = events.stats_handle();
        let cancel = TransferCancelToken::new();
        let (fut, span) = self.build_receive_transfer(from, into, &options, &events, &cancel)?;
        let task =
            tokio::spawn(with_transfer_set_summary(fut, events_stats.clone()).instrument(span));
        Ok(TransferSet::new(
            event_receiver,
            cancel,
            events_phase,
            events_stats,
            task,
        ))
    }

    fn build_receive_transfer(
        &self,
        from: PeerSource,
        into: PathBuf,
        options: &TransferOptions,
        events: &EventSender,
        cancel: &TransferCancelToken,
    ) -> Result<(TransferSetFuture, tracing::Span), TransferError> {
        match self.select_transport(options)?.provider {
            TransportProvider::Iroh => {
                self.build_iroh_receive_transfer(from, into, options, events, cancel)
            }
            provider => Err(unregistered_transport_error(provider)),
        }
    }

    /// Existing iroh negotiated receive adapter.
    fn build_iroh_receive_transfer(
        &self,
        from: PeerSource,
        into: PathBuf,
        options: &TransferOptions,
        events: &EventSender,
        cancel: &TransferCancelToken,
    ) -> Result<(TransferSetFuture, tracing::Span), TransferError> {
        let sink = Box::new(SessionEventAdapter(events.clone()));
        let listen = options
            .listen_addrs
            .clone()
            .unwrap_or_else(|| BindAddrs::dual_stack(0));
        let mode = from.mode();
        let span = transfer_span(TransferDirection::Receive, mode);

        let fut: TransferSetFuture = match from {
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
                    receive_transfer_with_bound_peer(
                        listen, into, config, &pairing, sink, on_bound, cancel,
                    )
                    .await
                })
            }
            PeerSource::ShowInvite { ttl_secs, token } => {
                let token = token.map_or_else(new_token, Ok)?;
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
                    receive_transfer_with_bound_peer(
                        listen, into, config, &pairing, sink, on_bound, cancel,
                    )
                    .await
                })
            }
            PeerSource::Mdns { token } => {
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
                    receive_transfer_enable_mdns(
                        listen, into, config, &pairing, sink, on_bound, cancel,
                    )
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
                    receive_transfer_via_room(broker, &code, listen, into, config, sink, cancel)
                        .await
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
        match self.select_transport(options)?.provider {
            TransportProvider::Iroh => self.build_iroh_receive(from, into, options, events, cancel),
            provider => Err(unregistered_transport_error(provider)),
        }
    }

    /// Existing iroh single-file receive adapter.
    fn build_iroh_receive(
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
        if let Some(sid) = options.session_id {
            span.record("session_id", sid);
        }

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
            PeerSource::ShowInvite { ttl_secs, token } => {
                let token = token.map_or_else(new_token, Ok)?;
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
                let source_mode = source.mode();
                let preconnect_timeout =
                    preconnect_timeout_for_source(direction, source_mode, i + 1 < count);
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
                let fut = with_preconnection_timeout(
                    fut,
                    stats.clone(),
                    preconnect_timeout,
                    direction,
                    source_mode,
                );
                match with_summary(Box::pin(fut), stats.clone())
                    .instrument(span)
                    .await
                {
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
                    reason_code: event::SessionFailureCode::classify(&reason),
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

    /// Runs one Manifest send, falling back to the next source only when the
    /// previous source failed before selecting a live data path.
    pub fn run_manifest(
        &self,
        request: ManifestTransferRequest,
    ) -> Result<TransferSet, TransferError> {
        let ManifestTransferRequest {
            request,
            sources,
            options,
        } = request;
        if sources.is_empty() {
            return Err(TransferError::input(
                "a Manifest transfer needs at least one peer source",
            ));
        }
        crate::validate_chunk_size(self.chunk_size).map_err(setup_error)?;
        validate_path_policy(&options)?;
        request
            .manifest
            .validate_structure()
            .map_err(|error| TransferError::input(error.to_string()))?;

        let (events, event_receiver) = EventSender::channel();
        let events_phase = events.phase_cell();
        let events_stats = events.stats_handle();
        let cancel = TransferCancelToken::new();
        let client = self.clone();
        let stats = events_stats.clone();
        let loop_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let count = sources.len();
            let mut last: Result<SessionTransferSummary, PublicError> = Err(PublicError::Transfer(
                "no peer source attempted".to_string(),
            ));
            for (index, source) in sources.into_iter().enumerate() {
                let mode = source.mode();
                let timeout =
                    preconnect_timeout_for_source(TransferDirection::Send, mode, index + 1 < count);
                let (fut, span) = match client.build_manifest_send(
                    source,
                    request.clone(),
                    &options,
                    &events,
                    &loop_cancel,
                ) {
                    Ok(pair) => pair,
                    Err(error) => {
                        last = Err(PublicError::InvalidInput(error.to_string()));
                        if index + 1 == count {
                            break;
                        }
                        continue;
                    }
                };
                let fut = with_preconnection_timeout(
                    fut,
                    stats.clone(),
                    timeout,
                    TransferDirection::Send,
                    mode,
                );
                match with_transfer_set_summary(Box::pin(fut), stats.clone())
                    .instrument(span)
                    .await
                {
                    Ok(summary) => return Ok(summary),
                    Err(error) => {
                        last = Err(error);
                        if stats.connected() || index + 1 == count {
                            break;
                        }
                    }
                }
            }
            emit_terminal_transfer_set_failure(&events, TransferDirection::Send, &last);
            last
        });
        Ok(TransferSet::new(
            event_receiver,
            cancel,
            events_phase,
            events_stats,
            task,
        ))
    }

    /// Runs an ALPN-negotiated receive across ordered rendezvous sources.
    /// The request must be receive-direction; senders use [`Self::run_manifest`].
    pub fn run_receive_transfer(
        &self,
        request: TransferRequest,
    ) -> Result<TransferSet, TransferError> {
        let TransferRequest {
            direction,
            path,
            sources,
            options,
        } = request;
        if direction != TransferDirection::Receive {
            return Err(TransferError::input(
                "run_receive_transfer requires receive direction",
            ));
        }
        if sources.is_empty() {
            return Err(TransferError::input(
                "a negotiated receive needs at least one peer source",
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
            let mut last: Result<SessionTransferSummary, PublicError> = Err(PublicError::Transfer(
                "no peer source attempted".to_string(),
            ));
            for (index, source) in sources.into_iter().enumerate() {
                let mode = source.mode();
                let timeout = preconnect_timeout_for_source(
                    TransferDirection::Receive,
                    mode,
                    index + 1 < count,
                );
                let (fut, span) = match client.build_receive_transfer(
                    source,
                    path.clone(),
                    &options,
                    &events,
                    &loop_cancel,
                ) {
                    Ok(pair) => pair,
                    Err(error) => {
                        last = Err(PublicError::InvalidInput(error.to_string()));
                        if index + 1 == count {
                            break;
                        }
                        continue;
                    }
                };
                let fut = with_preconnection_timeout(
                    fut,
                    stats.clone(),
                    timeout,
                    TransferDirection::Receive,
                    mode,
                );
                match with_transfer_set_summary(Box::pin(fut), stats.clone())
                    .instrument(span)
                    .await
                {
                    Ok(summary) => return Ok(summary),
                    Err(error) => {
                        last = Err(error);
                        if stats.connected() || index + 1 == count {
                            break;
                        }
                    }
                }
            }
            emit_terminal_transfer_set_failure(&events, TransferDirection::Receive, &last);
            last
        });
        Ok(TransferSet::new(
            event_receiver,
            cancel,
            events_phase,
            events_stats,
            task,
        ))
    }

    fn select_transport(
        &self,
        options: &TransferOptions,
    ) -> Result<TransportSelection, TransferError> {
        let selection =
            TransportSelector::select(options.transport, &BUILT_IN_TRANSPORT_CANDIDATES)
                .map_err(|error| setup_error(PublicError::Transport(error.to_string())))?;
        tracing::debug!(
            provider = %selection.provider,
            selection_reason = %selection.reason,
            iroh_path_policy = ?options.path,
            "transport provider selected"
        );
        Ok(selection)
    }

    fn session_config(&self, options: &TransferOptions) -> SessionConfig {
        SessionConfig {
            chunk_size: self.chunk_size,
            identity: self.identity.clone(),
            relay: options.relay.clone(),
            relay_only: options.path == PathPolicy::RelayOnly,
            direct_only: options.path == PathPolicy::DirectOnly,
            candidates: self.candidates.clone(),
            data_stream_window: self.data_stream_window,
        }
    }
}

fn unregistered_transport_error(provider: TransportProvider) -> TransferError {
    setup_error(PublicError::Transport(format!(
        "transport provider {provider} has no registered adapter"
    )))
}

fn preconnect_timeout_for_source(
    direction: TransferDirection,
    mode: TransferMode,
    has_fallback: bool,
) -> Option<Duration> {
    (direction == TransferDirection::Send && mode == TransferMode::Room && has_fallback)
        .then_some(ROOM_SEND_PRECONNECT_TIMEOUT)
}

async fn with_preconnection_timeout<R>(
    fut: Pin<Box<dyn Future<Output = Result<R, PublicError>> + Send>>,
    stats: StatsHandle,
    timeout: Option<Duration>,
    direction: TransferDirection,
    mode: TransferMode,
) -> Result<R, PublicError> {
    let Some(timeout) = timeout else {
        return fut.await;
    };
    let deadline = tokio::time::Instant::now() + timeout;
    tokio::pin!(fut);
    loop {
        if stats.connected() {
            return fut.await;
        }
        tokio::select! {
            result = &mut fut => return result,
            _ = tokio::time::sleep_until(deadline) => {
                if stats.connected() {
                    return fut.await;
                }
                return Err(PublicError::Transfer(format!(
                    "{direction:?} via {mode:?} timed out before connecting; trying fallback"
                )));
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

fn emit_terminal_transfer_set_failure(
    events: &EventSender,
    direction: TransferDirection,
    result: &Result<SessionTransferSummary, PublicError>,
) {
    if let Err(error) = result {
        let reason = error.to_string();
        events.emit(TransferEvent::Failed {
            direction,
            reason_code: event::SessionFailureCode::classify(&reason),
            reason,
        });
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
) -> impl FnOnce(PeerDescriptor, Vec<String>) + Send {
    move |peer: PeerDescriptor, relay_urls: Vec<String>| {
        let invite = invite_ttl.map(|ttl| {
            QrInvitePayload::new_with_relay_urls(
                token.clone(),
                peer.clone(),
                relay_urls,
                unix_now() + ttl,
            )
            .encode()
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

/// Decodes and validates an invite, returning the endpoint address to dial and the token.
fn resolve_invite(
    invite: &str,
    continuation: bool,
) -> Result<(EndpointAddr, String), TransferError> {
    let to_err = |e| TransferError::input(format!("invalid invite: {e}"));
    let payload = QrInvitePayload::decode(invite).map_err(to_err)?;
    if continuation {
        payload.validate_for_resume().map_err(to_err)?;
    } else {
        payload.validate(unix_now()).map_err(to_err)?;
    }
    let peer_addr = payload.endpoint_addr().map_err(to_err)?;
    Ok((peer_addr, payload.token))
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

    fn manifest_send_request() -> ManifestSendRequest {
        ManifestSendRequest::new(
            ManifestV1 {
                manifest_id: ManifestId::new("client-run-manifest"),
                entries: vec![ManifestEntryV1 {
                    entry_id: 0,
                    relative_path: "file.bin".into(),
                    kind: ManifestEntryKind::RegularFile,
                    size: 1,
                    hash: Some([7; 32]),
                    modified_at_unix_ms: None,
                }],
                file_count: 1,
                directory_count: 0,
                root_count: 1,
                total_bytes: 1,
                hash_algorithm: ManifestHashAlgorithm::Blake3_256,
            },
            [(0, PathBuf::from("file.bin"))],
        )
        .unwrap()
    }

    fn client() -> Client {
        Client::new()
    }

    #[test]
    fn current_client_delegates_automatic_transport_to_iroh() {
        let selection = client()
            .select_transport(&TransferOptions::default())
            .unwrap();

        assert_eq!(selection.provider, TransportProvider::Iroh);
        assert_eq!(selection.reason, TransportSelectionReason::Automatic);
    }

    #[test]
    fn required_pending_provider_fails_during_setup_before_network_activity() {
        let options = TransferOptions {
            transport: TransportPreference::Require(TransportProvider::WifiAware),
            ..Default::default()
        };

        let error = client().select_transport(&options).unwrap_err();

        assert_eq!(error.phase, Phase::Setup);
        assert_eq!(error.kind, ErrorKind::Transport);
        assert!(error.message.contains("implementation_pending"));
    }

    #[test]
    fn preferred_pending_provider_falls_back_to_iroh() {
        let options = TransferOptions {
            transport: TransportPreference::Prefer(TransportProvider::WifiAware),
            ..Default::default()
        };

        let selection = client().select_transport(&options).unwrap();

        assert_eq!(selection.provider, TransportProvider::Iroh);
        assert_eq!(
            selection.reason,
            TransportSelectionReason::Fallback {
                preferred: TransportProvider::WifiAware,
                preferred_availability: Some(TransportAvailability::ImplementationPending),
            }
        );
    }

    #[test]
    fn send_rejects_producer_sources() {
        for source in [
            PeerSource::ShowManual { token: None },
            PeerSource::ShowInvite {
                ttl_secs: 300,
                token: None,
            },
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
        let client = Client::from_config_fields(Some("1M"), &[], &deny, None).unwrap();

        assert_eq!(client.chunk_size, 1024 * 1024);
        let kept = client
            .candidates
            .apply(["10.0.0.5:1".parse().unwrap(), "1.2.3.4:2".parse().unwrap()]);
        assert_eq!(kept, vec!["1.2.3.4:2".parse().unwrap()]);
    }

    #[test]
    fn config_fields_reject_invalid_candidate_cidr() {
        assert!(Client::from_config_fields(None, &[], &["not-a-cidr".to_string()], None).is_err());
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
    fn run_manifest_rejects_empty_sources() {
        let error = client()
            .run_manifest(ManifestTransferRequest {
                request: manifest_send_request(),
                sources: vec![],
                options: TransferOptions::default(),
            })
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
    }

    #[test]
    fn negotiated_receive_rejects_send_direction() {
        let error = client()
            .run_receive_transfer(TransferRequest {
                direction: TransferDirection::Send,
                path: "file.bin".into(),
                sources: vec![PeerSource::Mdns { token: None }],
                options: TransferOptions::default(),
            })
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Input);
    }

    #[tokio::test]
    async fn run_manifest_advances_past_a_source_that_fails_to_build() {
        let mut transfer = client()
            .run_manifest(ManifestTransferRequest {
                request: manifest_send_request(),
                sources: vec![
                    PeerSource::Invite {
                        invite: "not-an-invite".into(),
                    },
                    PeerSource::Mdns { token: None },
                ],
                options: TransferOptions::default(),
            })
            .unwrap();

        let event = transfer.next_event().await.expect("terminal failure event");
        assert!(matches!(
            event.event,
            TransferEvent::Failed {
                direction: TransferDirection::Send,
                ..
            }
        ));
        let error = transfer.wait().await.unwrap_err();
        assert!(
            error.message.contains("mDNS requires a token"),
            "the final error must come from the second source: {error:?}"
        );
    }

    #[test]
    fn only_room_senders_with_a_fallback_get_a_preconnect_deadline() {
        assert_eq!(
            preconnect_timeout_for_source(TransferDirection::Send, TransferMode::Room, true),
            Some(ROOM_SEND_PRECONNECT_TIMEOUT),
        );
        assert_eq!(
            preconnect_timeout_for_source(TransferDirection::Receive, TransferMode::Room, true),
            None,
        );
        assert_eq!(
            preconnect_timeout_for_source(TransferDirection::Send, TransferMode::Room, false),
            None,
        );
        assert_eq!(
            preconnect_timeout_for_source(TransferDirection::Send, TransferMode::Mdns, true),
            None,
        );
    }

    #[tokio::test]
    async fn preconnect_deadline_ends_a_stuck_attempt() {
        let pending: TransferFuture = Box::pin(std::future::pending());
        let (events, _receiver) = EventSender::channel();
        let error = with_preconnection_timeout(
            pending,
            events.stats_handle(),
            Some(Duration::from_millis(5)),
            TransferDirection::Send,
            TransferMode::Room,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("timed out before connecting"));
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
        assert_eq!(terminal, Some(SessionFailureCode::Other));
    }
}
