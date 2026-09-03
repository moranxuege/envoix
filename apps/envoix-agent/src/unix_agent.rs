use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::future::Future;
use std::io;
use std::net::IpAddr;
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use clap::Parser;
use envoix_client::api::{
    self, DataPath, DestinationDecisionV2, DestinationRequestV2, EventSink,
    PendingManifestV2Receive, RememberedCredential, RememberedRoomControlRole, RoomControlEvent,
    RoomControlInvite, RoomControlSession, RoomOfferRejection, RoomTransferOffer, TransferEvent,
    TransferOptions, TransferRole,
};
use envoix_client::model::{
    ContentId, FailureCode, FailureOutcome, FailurePhase, RecoveryAction, RememberedAttemptOutcome,
    RememberedGenerationRole, Transfer, TransferDirection, TransferFailure, TransferId,
    TransferRejection, remembered_generation_attempts,
};
use envoix_client::ports::{PlatformPortError, SecureVaultPort};
use envoix_client::product::{
    AGENT_PROTOCOL_VERSION, AgentControlTransport, AgentCredentialProtection, AgentDiagnostics,
    AgentEvent, AgentEventCursor, AgentEventEnvelope, AgentOfferDecision, AgentPathKind,
    AgentPendingOffer, AgentRelationshipChange, AgentRequest, AgentRequestEnvelope, AgentResponse,
    AgentResponseEnvelope, AgentSettings, AgentSnapshot, AgentStatus, AgentTransferPath,
    DeviceSummary, InboxItem, InboxRoot, MAX_AGENT_ACTIVE_PATHS, MAX_AGENT_EVENT_BATCH,
    MAX_AGENT_PENDING_OFFERS, MAX_AGENT_REQUEST_BYTES, MAX_AGENT_RESPONSE_BYTES, PairingInvitation,
    PreparedRememberedDevice, ProductStore, RememberedDeviceRecord, default_agent_control_endpoint,
    default_agent_state_directory, is_valid_agent_request_id,
};
#[cfg(windows)]
use envoix_client::product::{current_windows_user_sid, windows_process_user_sid};
use envoix_client::storage::{ENGINE_STATE_SCHEMA_VERSION, EngineStoreError};
use envoix_client::{
    DEFAULT_RELAY_URL, DEFAULT_RENDEZVOUS_BROKER, IdentityConfig, TransferCancelToken,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
#[cfg(windows)]
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, oneshot, watch};
use tokio::task::JoinHandle;

const REMEMBERED_FALLBACK_TIMEOUT: Duration = Duration::from_secs(35);
const REMEMBERED_CONNECT_CANCEL_GRACE_PERIOD: Duration = Duration::from_secs(1);
const RECEIVER_RETRY_DELAY: Duration = Duration::from_secs(3);
const RECEIVER_RETRY_MAX_DELAY: Duration = Duration::from_secs(60);
const RECEIVER_PARKED_ATTEMPT: Duration = Duration::from_secs(30);
const INITIAL_PAIRING_RETRY_DELAY: Duration = Duration::from_millis(750);
const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(5);
const MAX_RETAINED_AGENT_EVENTS: usize = 1_024;
const OUTGOING_PROGRESS_CHECKPOINT_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "envoix-agent",
    version,
    about = "Persistent Envoix receiver and local Inbox service"
)]
struct Cli {
    /// Managed Agent settings written by `envoix agent install`.
    #[arg(long)]
    settings: Option<PathBuf>,
    /// Product metadata, protected credentials, and transfer state.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Directory where completed incoming roots are saved.
    #[arg(long)]
    inbox: Option<PathBuf>,
    /// Unix socket or Windows Named Pipe used by local controllers.
    #[arg(long, visible_aliases = ["socket", "pipe"])]
    control_endpoint: Option<PathBuf>,
    /// Human-readable name for this Agent host.
    #[arg(long)]
    device_name: Option<String>,
    /// Rendezvous broker; overrides the managed settings file.
    #[arg(long)]
    broker: Option<String>,
    /// Relay URL; overrides managed settings. Use `none` to disable it.
    #[arg(long)]
    relay: Option<String>,
    /// Optional transport-only runtime TOML.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Increase logging verbosity.
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
    verbose: u8,
}

/// Filesystem, identity, and routing settings owned by one Agent host.
#[derive(Clone, Debug)]
pub struct AgentHostConfiguration {
    pub state_directory: PathBuf,
    pub inbox_directory: PathBuf,
    pub control_endpoint: PathBuf,
    pub device_name: String,
    pub broker: String,
    pub relay: Option<String>,
}

impl AgentHostConfiguration {
    fn validate(&self) -> Result<(), AgentHostError> {
        if !self.state_directory.is_absolute() || !self.inbox_directory.is_absolute() {
            return Err(AgentHostError::invalid_configuration(
                "Agent state and Inbox directories must be absolute",
            ));
        }
        #[cfg(unix)]
        if !self.control_endpoint.is_absolute() {
            return Err(AgentHostError::invalid_configuration(
                "Agent control endpoint must be absolute",
            ));
        }
        #[cfg(windows)]
        validate_windows_pipe_endpoint(&self.control_endpoint)
            .map_err(AgentHostError::invalid_configuration)?;
        let device_name = self.device_name.trim();
        if device_name != self.device_name
            || device_name.is_empty()
            || device_name.chars().count() > 64
            || device_name.chars().any(char::is_control)
        {
            return Err(AgentHostError::invalid_configuration(
                "Agent device name must contain 1 to 64 visible characters without surrounding whitespace",
            ));
        }
        api::parse_broker_addr(&self.broker, self.relay.as_deref())
            .map_err(AgentHostError::invalid_configuration)?;
        Ok(())
    }
}

/// Stable startup and runtime failure categories for an embedded Agent host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentHostErrorCode {
    InvalidConfiguration,
    StateAlreadyOwned,
    UnsupportedPersistentState,
    StateCorrupt,
    VaultUnavailable,
    VaultInteractionRequired,
    VaultPermissionDenied,
    VaultCorrupt,
    VaultCanceled,
    IoFailure,
    Internal,
}

/// A typed, secret-free Agent host failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentHostFailure {
    pub code: AgentHostErrorCode,
    pub reason: String,
}

/// Error returned by [`AgentHost::run`].
#[derive(Debug, thiserror::Error)]
#[error("{reason}")]
pub struct AgentHostError {
    code: AgentHostErrorCode,
    reason: String,
}

impl AgentHostError {
    pub fn code(&self) -> AgentHostErrorCode {
        self.code
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    fn invalid_configuration(reason: impl std::fmt::Display) -> Self {
        Self {
            code: AgentHostErrorCode::InvalidConfiguration,
            reason: reason.to_string(),
        }
    }

    fn from_runtime(error: anyhow::Error) -> Self {
        let code = error
            .chain()
            .find_map(|cause| {
                cause
                    .downcast_ref::<EngineStoreError>()
                    .map(engine_store_host_error_code)
                    .or_else(|| {
                        cause
                            .downcast_ref::<PlatformPortError>()
                            .map(platform_host_error_code)
                    })
                    .or_else(|| {
                        cause
                            .downcast_ref::<io::Error>()
                            .map(|_| AgentHostErrorCode::IoFailure)
                    })
            })
            .unwrap_or(AgentHostErrorCode::Internal);
        Self {
            code,
            reason: format!("{error:#}"),
        }
    }

    fn failure(&self) -> AgentHostFailure {
        AgentHostFailure {
            code: self.code,
            reason: self.reason.clone(),
        }
    }
}

fn engine_store_host_error_code(error: &EngineStoreError) -> AgentHostErrorCode {
    match error {
        EngineStoreError::AlreadyOwned { .. } => AgentHostErrorCode::StateAlreadyOwned,
        EngineStoreError::UnsupportedSchema { .. }
        | EngineStoreError::UnsupportedLegacyState { .. } => {
            AgentHostErrorCode::UnsupportedPersistentState
        }
        EngineStoreError::StateTooLarge { .. }
        | EngineStoreError::InvalidState(_)
        | EngineStoreError::Decode(_) => AgentHostErrorCode::StateCorrupt,
        EngineStoreError::MissingVaultCredential => AgentHostErrorCode::VaultUnavailable,
        EngineStoreError::PlatformPort(error) => platform_host_error_code(error),
        EngineStoreError::Io(_) => AgentHostErrorCode::IoFailure,
    }
}

fn platform_host_error_code(error: &PlatformPortError) -> AgentHostErrorCode {
    match error {
        PlatformPortError::Unavailable | PlatformPortError::Limited => {
            AgentHostErrorCode::VaultUnavailable
        }
        PlatformPortError::PermissionDenied => AgentHostErrorCode::VaultPermissionDenied,
        PlatformPortError::InteractionRequired => AgentHostErrorCode::VaultInteractionRequired,
        PlatformPortError::InvalidRequest => AgentHostErrorCode::InvalidConfiguration,
        PlatformPortError::CorruptData => AgentHostErrorCode::VaultCorrupt,
        PlatformPortError::Canceled => AgentHostErrorCode::VaultCanceled,
    }
}

/// Observable lifecycle of one embedded Agent host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentHostLifecycleState {
    Starting,
    Ready,
    Stopping,
    Stopped,
    Failed { failure: AgentHostFailure },
}

/// A cloneable observer for Agent readiness and terminal state.
#[derive(Clone, Debug)]
pub struct AgentHostLifecycleHandle {
    receiver: watch::Receiver<AgentHostLifecycleState>,
}

impl AgentHostLifecycleHandle {
    pub fn state(&self) -> AgentHostLifecycleState {
        self.receiver.borrow().clone()
    }

    pub async fn changed(&mut self) -> Option<AgentHostLifecycleState> {
        self.receiver.changed().await.ok()?;
        Some(self.state())
    }
}

/// A cloneable request handle for an orderly Agent shutdown.
#[derive(Clone, Debug)]
pub struct AgentShutdownHandle {
    token: TransferCancelToken,
    lifecycle: watch::Sender<AgentHostLifecycleState>,
}

impl AgentShutdownHandle {
    /// Requests shutdown. The operation is idempotent.
    pub fn shutdown(&self) {
        self.lifecycle.send_modify(|state| {
            if matches!(
                state,
                AgentHostLifecycleState::Starting | AgentHostLifecycleState::Ready
            ) {
                *state = AgentHostLifecycleState::Stopping;
            }
        });
        self.token.cancel();
    }

    pub fn is_shutdown_requested(&self) -> bool {
        self.token.is_cancelled()
    }
}

/// One durable Agent owner backed by an injected secure-vault adapter.
pub struct AgentHost {
    config: AgentHostConfiguration,
    client: api::Client,
    vault: Arc<dyn SecureVaultPort>,
    credential_protection: AgentCredentialProtection,
    shutdown: AgentShutdownHandle,
    lifecycle: watch::Sender<AgentHostLifecycleState>,
}

impl AgentHost {
    /// Creates an Agent using the supplied vault. The protection classification
    /// is returned through Agent diagnostics and must describe that adapter.
    pub fn new(
        config: AgentHostConfiguration,
        client: api::Client,
        vault: Arc<dyn SecureVaultPort>,
        credential_protection: AgentCredentialProtection,
    ) -> Self {
        let (lifecycle, _) = watch::channel(AgentHostLifecycleState::Starting);
        Self {
            config,
            client,
            vault,
            credential_protection,
            shutdown: AgentShutdownHandle {
                token: TransferCancelToken::new(),
                lifecycle: lifecycle.clone(),
            },
            lifecycle,
        }
    }

    /// Uses the existing owner-only Unix store or per-user Windows DPAPI store.
    pub fn with_desktop_vault(config: AgentHostConfiguration, client: api::Client) -> Self {
        let vault = Arc::new(api::DesktopCredentialStore::new(
            config.state_directory.join("vault"),
        ));
        Self::new(config, client, vault, desktop_credential_protection())
    }

    /// Returns a handle that remains valid while [`Self::run`] is active.
    pub fn shutdown_handle(&self) -> AgentShutdownHandle {
        self.shutdown.clone()
    }

    pub fn lifecycle_handle(&self) -> AgentHostLifecycleHandle {
        AgentHostLifecycleHandle {
            receiver: self.lifecycle.subscribe(),
        }
    }

    /// Runs until shutdown is requested or the local control transport fails.
    pub async fn run(self) -> Result<(), AgentHostError> {
        let lifecycle = self.lifecycle.clone();
        let result = if let Err(error) = self.config.validate() {
            Err(error)
        } else {
            self.run_inner().await.map_err(AgentHostError::from_runtime)
        };
        match &result {
            Ok(()) => {
                lifecycle.send_replace(AgentHostLifecycleState::Stopped);
            }
            Err(error) => {
                lifecycle.send_replace(AgentHostLifecycleState::Failed {
                    failure: error.failure(),
                });
            }
        }
        result
    }

    async fn run_inner(self) -> Result<()> {
        if self.shutdown.is_shutdown_requested() {
            return Ok(());
        }
        let lifecycle = self.lifecycle.clone();
        create_private_directory(&self.config.state_directory)?;
        create_directory(&self.config.inbox_directory)?;
        let store = ProductStore::open_with_vault(&self.config.state_directory, self.vault)?;
        let runtime = Arc::new(AgentRuntime {
            config: self.config,
            client: self.client,
            store: Mutex::new(store),
            credential_protection: self.credential_protection,
            active_receivers: Mutex::new(HashMap::new()),
            active_rooms: Mutex::new(HashMap::new()),
            active_outgoing: Mutex::new(HashMap::new()),
            active_pairings: Mutex::new(HashSet::new()),
            active_paths: Mutex::new(Vec::new()),
            pending_offers: Mutex::new(HashMap::new()),
            events: Mutex::new(AgentEventLog::new()?),
            shutdown: self.shutdown.token,
            background_tasks: Mutex::new(Vec::new()),
        });

        let device_ids = lock(&runtime.store)?
            .device_records()
            .into_iter()
            .map(|device| device.id().to_string())
            .collect::<Vec<_>>();
        for device_id in device_ids {
            spawn_remembered_receiver(runtime.clone(), device_id);
        }

        let result = serve(runtime.clone(), &lifecycle).await;
        shutdown_background_tasks(&runtime).await;
        result
    }
}

fn desktop_credential_protection() -> AgentCredentialProtection {
    #[cfg(unix)]
    return AgentCredentialProtection::OwnerOnlyFile;
    #[cfg(windows)]
    return AgentCredentialProtection::WindowsDpapi;
}

struct AgentRuntime {
    config: AgentHostConfiguration,
    client: api::Client,
    store: Mutex<ProductStore>,
    credential_protection: AgentCredentialProtection,
    active_receivers: Mutex<HashMap<String, TransferCancelToken>>,
    active_rooms: Mutex<HashMap<String, Arc<Notify>>>,
    active_outgoing: Mutex<HashMap<String, ActiveOutgoingTransfer>>,
    active_pairings: Mutex<HashSet<String>>,
    active_paths: Mutex<Vec<AgentTransferPath>>,
    pending_offers: Mutex<HashMap<String, PendingOfferControl>>,
    events: Mutex<AgentEventLog>,
    shutdown: TransferCancelToken,
    background_tasks: Mutex<Vec<JoinHandle<()>>>,
}

struct ActiveOutgoingTransfer {
    relationship_id: String,
    cancel: TransferCancelToken,
}

struct PreparedOutgoingTransfer {
    transfer: Transfer,
    job: api::CanonicalTransferJob,
    bootstrap: api::InvitationBootstrap,
    broker: String,
    relay: Option<String>,
    offer: RoomTransferOffer,
}

struct PendingOfferControl {
    offer: AgentPendingOffer,
    decision: Option<oneshot::Sender<AgentOfferDecision>>,
}

struct PendingIncomingOffer {
    offer: RoomTransferOffer,
    decision: oneshot::Receiver<AgentOfferDecision>,
    runtime: Arc<AgentRuntime>,
}

struct ActivePathCleanup {
    runtime: Arc<AgentRuntime>,
    transfer_id: String,
    direction: TransferDirection,
}

impl ActivePathCleanup {
    fn new(
        runtime: Arc<AgentRuntime>,
        transfer_id: impl Into<String>,
        direction: TransferDirection,
    ) -> Self {
        Self {
            runtime,
            transfer_id: transfer_id.into(),
            direction,
        }
    }
}

impl Drop for ActivePathCleanup {
    fn drop(&mut self) {
        if let Err(error) = clear_active_path(&self.runtime, &self.transfer_id, self.direction) {
            tracing::error!(
                transfer_id = %self.transfer_id,
                %error,
                "active transfer path could not be cleared"
            );
        }
    }
}

impl Drop for PendingIncomingOffer {
    fn drop(&mut self) {
        clear_pending_offer(&self.runtime, &self.offer.offer_id);
    }
}

struct AgentEventLog {
    instance_id: String,
    sequence: u64,
    events: VecDeque<AgentEventEnvelope>,
}

enum AgentEventRead {
    Events {
        cursor: AgentEventCursor,
        events: Vec<AgentEventEnvelope>,
    },
    SnapshotRequired(AgentEventCursor),
}

impl AgentEventLog {
    fn new() -> Result<Self> {
        let mut random = [0_u8; 12];
        getrandom::fill(&mut random)
            .map_err(|error| anyhow!("Agent instance entropy unavailable: {error}"))?;
        Ok(Self {
            instance_id: format!("agent_{}", URL_SAFE_NO_PAD.encode(random)),
            sequence: 0,
            events: VecDeque::new(),
        })
    }

    fn cursor(&self) -> AgentEventCursor {
        AgentEventCursor {
            instance_id: self.instance_id.clone(),
            sequence: self.sequence,
        }
    }

    fn record(&mut self, event: AgentEvent) -> Result<AgentEventEnvelope> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| anyhow!("Agent event sequence is exhausted"))?;
        let envelope = AgentEventEnvelope::new(self.instance_id.clone(), self.sequence, event)?;
        self.events.push_back(envelope.clone());
        while self.events.len() > MAX_RETAINED_AGENT_EVENTS {
            self.events.pop_front();
        }
        Ok(envelope)
    }

    fn read_after(&self, after: &AgentEventCursor, limit: usize) -> AgentEventRead {
        let current = self.cursor();
        if after.instance_id != self.instance_id || after.sequence > self.sequence {
            return AgentEventRead::SnapshotRequired(current);
        }
        let oldest = self
            .events
            .front()
            .map(|event| event.sequence)
            .unwrap_or_else(|| self.sequence.saturating_add(1));
        if after.sequence.saturating_add(1) < oldest {
            return AgentEventRead::SnapshotRequired(current);
        }
        let limit = limit.min(MAX_AGENT_EVENT_BATCH);
        if limit == 0 {
            return AgentEventRead::Events {
                cursor: after.clone(),
                events: Vec::new(),
            };
        }
        let events = self
            .events
            .iter()
            .filter(|event| event.sequence > after.sequence)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let cursor = AgentEventCursor {
            instance_id: self.instance_id.clone(),
            sequence: events
                .last()
                .map(|event| event.sequence)
                .unwrap_or(self.sequence),
        };
        AgentEventRead::Events { cursor, events }
    }
}

impl AgentRuntime {
    fn status(&self) -> Result<AgentStatus> {
        let paired_devices = lock(&self.store)?.devices().len();
        let active_receivers = lock(&self.active_receivers)?.len();
        let active_pairings = lock(&self.active_pairings)?.len();
        let active_paths = lock(&self.active_paths)?.len();
        let pending_offers = lock(&self.pending_offers)?
            .values()
            .filter(|pending| pending.decision.is_some())
            .count();
        Ok(AgentStatus {
            protocol_version: AGENT_PROTOCOL_VERSION,
            pid: std::process::id(),
            device_name: self.config.device_name.clone(),
            state_directory: self.config.state_directory.display().to_string(),
            inbox_directory: self.config.inbox_directory.display().to_string(),
            broker: self.config.broker.clone(),
            relay: self.config.relay.clone(),
            paired_devices,
            active_receivers,
            active_pairings,
            active_paths,
            pending_offers,
        })
    }

    fn session_config(&self, relay: Option<&str>) -> api::SessionConfig {
        let mut options = TransferOptions::default();
        options.relay = relay.map(str::to_string);
        self.client.session_config(&options)
    }

    fn snapshot(&self, inbox_limit: usize) -> Result<AgentSnapshot> {
        let status = self.status()?;
        let active_paths = self.active_paths()?;
        let pending_offers = self.pending_offers()?;
        let store = lock(&self.store)?;
        let event_cursor = lock(&self.events)?.cursor();
        Ok(AgentSnapshot {
            status,
            engine: store.engine_snapshot(),
            inbox: store.inbox(inbox_limit),
            active_paths,
            pending_offers,
            event_cursor,
        })
    }

    fn diagnostics(&self) -> Result<AgentDiagnostics> {
        let store = lock(&self.store)?;
        let engine = store.engine_snapshot();
        #[cfg(unix)]
        let control_transport = AgentControlTransport::UnixSocket;
        #[cfg(windows)]
        let control_transport = AgentControlTransport::WindowsNamedPipe;
        Ok(AgentDiagnostics {
            agent_protocol_version: AGENT_PROTOCOL_VERSION,
            application_contract_version: envoix_client::APPLICATION_CONTRACT_VERSION,
            engine_schema_version: ENGINE_STATE_SCHEMA_VERSION,
            platform: std::env::consts::OS.to_string(),
            control_transport,
            credential_protection: self.credential_protection,
            engine_sequence: engine.last_sequence,
            relationships: engine.relationships.len(),
            transfers: engine.transfers.len(),
            inbox_items: store.inbox(usize::MAX).len(),
            active_paths: lock(&self.active_paths)?.len(),
            pending_offers: lock(&self.pending_offers)?
                .values()
                .filter(|pending| pending.decision.is_some())
                .count(),
        })
    }

    fn pending_offers(&self) -> Result<Vec<AgentPendingOffer>> {
        let mut offers = lock(&self.pending_offers)?
            .values()
            .filter(|pending| pending.decision.is_some())
            .map(|pending| pending.offer.clone())
            .collect::<Vec<_>>();
        offers.sort_by(|left, right| left.offer_id.cmp(&right.offer_id));
        Ok(offers)
    }

    fn active_paths(&self) -> Result<Vec<AgentTransferPath>> {
        let mut paths = lock(&self.active_paths)?.clone();
        paths.sort_by(|left, right| {
            left.transfer_id.cmp(&right.transfer_id).then_with(|| {
                transfer_direction_order(left.direction)
                    .cmp(&transfer_direction_order(right.direction))
            })
        });
        Ok(paths)
    }
}

fn record_agent_event(runtime: &AgentRuntime, event: AgentEvent) -> Result<()> {
    lock(&runtime.events)?.record(event)?;
    Ok(())
}

fn stage_pending_offer(
    runtime: &Arc<AgentRuntime>,
    device_id: &str,
    device_label: &str,
    offer: RoomTransferOffer,
    allocatable_bytes: u64,
) -> Result<PendingIncomingOffer> {
    let summary = AgentPendingOffer {
        offer_id: offer.offer_id.clone(),
        from_device_id: device_id.to_string(),
        from_device_label: device_label.to_string(),
        root_names: offer.root_names.clone(),
        item_count: offer.item_count,
        directory_count: offer.directory_count,
        total_bytes: offer.total_bytes,
        allocatable_bytes,
    };
    let (sender, receiver) = oneshot::channel();
    {
        let mut pending = lock(&runtime.pending_offers)?;
        if pending.len() >= MAX_AGENT_PENDING_OFFERS {
            bail!("Agent already has the maximum number of pending offers");
        }
        if pending.contains_key(&offer.offer_id) {
            bail!("offer {} is already pending", offer.offer_id);
        }
        pending.insert(
            offer.offer_id.clone(),
            PendingOfferControl {
                offer: summary,
                decision: Some(sender),
            },
        );
    }
    if let Err(error) = record_agent_event(
        runtime,
        AgentEvent::PendingOfferChanged {
            offer_id: offer.offer_id.clone(),
            pending: true,
        },
    ) {
        lock(&runtime.pending_offers)?.remove(&offer.offer_id);
        return Err(error);
    }
    Ok(PendingIncomingOffer {
        offer,
        decision: receiver,
        runtime: runtime.clone(),
    })
}

fn decide_pending_offer(
    runtime: &AgentRuntime,
    offer_id: &str,
    decision: AgentOfferDecision,
) -> Result<AgentPendingOffer> {
    let (offer, sender) = {
        let mut pending = lock(&runtime.pending_offers)?;
        let pending = pending
            .get_mut(offer_id)
            .ok_or_else(|| anyhow!("pending offer {offer_id} does not exist"))?;
        let sender = pending
            .decision
            .take()
            .ok_or_else(|| anyhow!("pending offer {offer_id} already has a decision"))?;
        (pending.offer.clone(), sender)
    };
    if sender.send(decision).is_err() {
        clear_pending_offer(runtime, offer_id);
        bail!("pending offer {offer_id} is no longer active");
    }
    Ok(offer)
}

fn clear_pending_offer(runtime: &AgentRuntime, offer_id: &str) {
    let removed = match lock(&runtime.pending_offers) {
        Ok(mut pending) => pending.remove(offer_id).is_some(),
        Err(error) => {
            tracing::error!(offer_id, %error, "pending offer state could not be locked");
            false
        }
    };
    if removed
        && let Err(error) = record_agent_event(
            runtime,
            AgentEvent::PendingOfferChanged {
                offer_id: offer_id.to_string(),
                pending: false,
            },
        )
    {
        tracing::error!(offer_id, %error, "pending offer event could not be recorded");
    }
}

fn transfer_direction_order(direction: TransferDirection) -> u8 {
    match direction {
        TransferDirection::Send => 0,
        TransferDirection::Receive => 1,
    }
}

fn project_agent_path(path: &DataPath) -> AgentPathKind {
    match path {
        DataPath::Direct { addr } if is_lan_ip(addr.ip()) => AgentPathKind::Lan,
        DataPath::Direct { .. } => AgentPathKind::Direct,
        DataPath::Relay { .. } => AgentPathKind::Relay,
        DataPath::WifiAware => AgentPathKind::WifiAware,
        DataPath::Other { .. } => AgentPathKind::Other,
    }
}

fn is_lan_ip(ip: IpAddr) -> bool {
    if is_tailscale_ip(ip) {
        return false;
    }
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local() || ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local() || ip.is_loopback(),
    }
}

fn is_tailscale_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

fn set_active_path(
    runtime: &AgentRuntime,
    transfer_id: &str,
    direction: TransferDirection,
    path: &DataPath,
) -> Result<()> {
    let path = project_agent_path(path);
    {
        let mut active = lock(&runtime.active_paths)?;
        if let Some(current) = active
            .iter_mut()
            .find(|current| current.transfer_id == transfer_id && current.direction == direction)
        {
            if current.path == path {
                return Ok(());
            }
            current.path = path;
        } else {
            if active.len() >= MAX_AGENT_ACTIVE_PATHS {
                bail!("Agent already has the maximum number of active transfer paths");
            }
            active.push(AgentTransferPath {
                transfer_id: transfer_id.to_string(),
                direction,
                path,
            });
        }
    }
    record_agent_event(
        runtime,
        AgentEvent::TransferPathChanged {
            transfer_id: transfer_id.to_string(),
            direction,
            path: Some(path),
        },
    )
}

fn clear_active_path(
    runtime: &AgentRuntime,
    transfer_id: &str,
    direction: TransferDirection,
) -> Result<()> {
    let removed = {
        let mut active = lock(&runtime.active_paths)?;
        active
            .iter()
            .position(|current| {
                current.transfer_id == transfer_id && current.direction == direction
            })
            .map(|index| active.swap_remove(index))
            .is_some()
    };
    if removed {
        record_agent_event(
            runtime,
            AgentEvent::TransferPathChanged {
                transfer_id: transfer_id.to_string(),
                direction,
                path: None,
            },
        )?;
    }
    Ok(())
}

fn requires_explicit_offer_approval(total_bytes: u64, allocatable_bytes: u64) -> bool {
    total_bytes > api::AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES || total_bytes > allocatable_bytes / 2
}

fn prepare_inbox_destination(runtime: &AgentRuntime) -> Result<(PathBuf, u64)> {
    create_directory(&runtime.config.inbox_directory)?;
    let target_directory = fs::canonicalize(&runtime.config.inbox_directory)?;
    let allocatable_bytes = api::local_allocatable_bytes(&target_directory)?;
    Ok((target_directory, allocatable_bytes))
}

fn wake_active_room(runtime: &AgentRuntime, relationship_id: &str) -> Result<bool> {
    let wake = lock(&runtime.active_rooms)?.get(relationship_id).cloned();
    if let Some(wake) = wake {
        wake.notify_one();
        return Ok(true);
    }
    Ok(false)
}

fn relationship_has_active_outgoing(runtime: &AgentRuntime, relationship_id: &str) -> Result<bool> {
    Ok(lock(&runtime.active_outgoing)?
        .values()
        .any(|active| active.relationship_id == relationship_id))
}

fn cancel_outgoing_for_relationship(runtime: &AgentRuntime, relationship_id: &str) -> Result<()> {
    for active in lock(&runtime.active_outgoing)?.values() {
        if active.relationship_id == relationship_id {
            active.cancel.cancel();
        }
    }
    Ok(())
}

fn relationship_has_active_transfer(runtime: &AgentRuntime, relationship_id: &str) -> Result<bool> {
    if relationship_has_active_outgoing(runtime, relationship_id)?
        || lock(&runtime.pending_offers)?
            .values()
            .any(|pending| pending.offer.from_device_id == relationship_id)
    {
        return Ok(true);
    }
    let transfer_ids = lock(&runtime.active_paths)?
        .iter()
        .map(|path| path.transfer_id.clone())
        .collect::<Vec<_>>();
    let store = lock(&runtime.store)?;
    for transfer_id in transfer_ids {
        if store
            .transfer(&transfer_id)?
            .is_some_and(|transfer| transfer.relationship_id.as_str() == relationship_id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    let settings = cli
        .settings
        .as_deref()
        .map(read_agent_settings)
        .transpose()?;
    let state_directory = match cli.state_dir {
        Some(path) => path,
        None => default_agent_state_directory()?,
    };
    let inbox_directory = cli
        .inbox
        .or_else(|| {
            settings
                .as_ref()
                .map(|settings| settings.inbox_directory.clone())
        })
        .unwrap_or_else(|| state_directory.join("inbox"));
    let device_name = cli
        .device_name
        .or_else(|| {
            settings
                .as_ref()
                .map(|settings| settings.device_name.clone())
        })
        .unwrap_or_else(|| "WSL".into());
    let control_endpoint = cli
        .control_endpoint
        .map(Ok)
        .unwrap_or_else(default_agent_control_endpoint)?;
    let broker = cli
        .broker
        .or_else(|| settings.as_ref().map(|settings| settings.broker.clone()))
        .unwrap_or_else(|| DEFAULT_RENDEZVOUS_BROKER.to_string());
    let relay = match cli.relay {
        Some(value) => parse_relay_override(&value),
        None => settings
            .as_ref()
            .map(|settings| settings.relay.clone())
            .unwrap_or_else(|| Some(DEFAULT_RELAY_URL.to_string())),
    };

    let client = agent_client(cli.config.as_deref())?;
    let host = AgentHost::with_desktop_vault(
        AgentHostConfiguration {
            state_directory: state_directory.clone(),
            inbox_directory,
            control_endpoint,
            device_name,
            broker,
            relay,
        },
        client,
    );
    let shutdown = host.shutdown_handle();
    let running = host.run();
    tokio::pin!(running);
    tokio::select! {
        result = &mut running => result.map_err(Into::into),
        signal = termination_signal() => {
            shutdown.shutdown();
            match signal {
                Ok(()) => {
                    tracing::info!("shutting down");
                    running.await.map_err(Into::into)
                }
                Err(error) => {
                    let _ = running.await;
                    Err(error.into())
                }
            }
        }
    }
}

fn parse_relay_override(value: &str) -> Option<String> {
    match value.trim() {
        "" | "none" | "off" => None,
        value => Some(value.to_string()),
    }
}

fn init_tracing(verbosity: u8) {
    use tracing_subscriber::EnvFilter;
    let default = match verbosity {
        0 => "envoix_agent=info,warn",
        1 => "envoix_agent=debug,envoix=debug,warn",
        _ => "envoix_agent=trace,envoix=trace,iroh=debug,warn",
    };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| default.into()))
        .with_target(false)
        .init();
}

fn agent_client(config_path: Option<&Path>) -> Result<api::Client> {
    let mut client = api::Client::from_runtime_sources(config_path)?;
    client.identity = IdentityConfig::Ephemeral;
    Ok(client)
}

fn read_agent_settings(path: &Path) -> Result<AgentSettings> {
    let bytes =
        fs::read(path).with_context(|| format!("read Agent settings {}", path.display()))?;
    let settings: AgentSettings = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse Agent settings {}", path.display()))?;
    settings
        .validate()
        .with_context(|| format!("validate Agent settings {}", path.display()))?;
    Ok(settings)
}

#[cfg(unix)]
async fn serve(
    runtime: Arc<AgentRuntime>,
    lifecycle: &watch::Sender<AgentHostLifecycleState>,
) -> Result<()> {
    prepare_socket(&runtime.config.control_endpoint).await?;
    let listener = UnixListener::bind(&runtime.config.control_endpoint)
        .with_context(|| format!("bind {}", runtime.config.control_endpoint.display()))?;
    fs::set_permissions(
        &runtime.config.control_endpoint,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )?;
    let socket_uid = fs::metadata(&runtime.config.control_endpoint)?.uid();
    let _cleanup = SocketCleanup(runtime.config.control_endpoint.clone());
    tracing::info!(
        endpoint = %runtime.config.control_endpoint.display(),
        inbox = %runtime.config.inbox_directory.display(),
        "Envoix Agent ready"
    );
    if runtime.shutdown.is_cancelled() {
        return Ok(());
    }
    lifecycle.send_replace(AgentHostLifecycleState::Ready);

    loop {
        tokio::select! {
            _ = runtime.shutdown.cancelled() => return Ok(()),
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                if let Err(error) = validate_unix_peer(&stream, socket_uid) {
                    tracing::warn!(%error, "rejected local Agent peer");
                    continue;
                }
                let connection_runtime = runtime.clone();
                if let Err(error) = spawn_background_task(&runtime, async move {
                    if let Err(error) = serve_connection(connection_runtime, stream).await {
                        tracing::warn!(%error, "local Agent request failed");
                    }
                }) {
                    tracing::warn!(%error, "local Agent request could not start");
                }
            }
        }
    }
}

#[cfg(windows)]
async fn serve(
    runtime: Arc<AgentRuntime>,
    lifecycle: &watch::Sender<AgentHostLifecycleState>,
) -> Result<()> {
    let endpoint = validate_windows_pipe_endpoint(&runtime.config.control_endpoint)?.to_string();
    let owner_sid = current_windows_user_sid()?;
    let mut server = create_windows_pipe(&endpoint, &owner_sid, true)?;
    tracing::info!(
        endpoint,
        inbox = %runtime.config.inbox_directory.display(),
        "Envoix Agent ready"
    );
    if runtime.shutdown.is_cancelled() {
        return Ok(());
    }
    lifecycle.send_replace(AgentHostLifecycleState::Ready);

    loop {
        tokio::select! {
            _ = runtime.shutdown.cancelled() => return Ok(()),
            connected = server.connect() => {
                if let Err(error) = connected {
                    tracing::warn!(%error, "Windows Agent pipe connection failed");
                    server = create_windows_pipe(&endpoint, &owner_sid, false)?;
                    continue;
                }
                if let Err(error) = validate_windows_peer(&server, &owner_sid) {
                    tracing::warn!(%error, "rejected local Agent peer");
                    server = create_windows_pipe(&endpoint, &owner_sid, false)?;
                    continue;
                }
                let connected = server;
                server = create_windows_pipe(&endpoint, &owner_sid, false)?;
                let connection_runtime = runtime.clone();
                if let Err(error) = spawn_background_task(&runtime, async move {
                    if let Err(error) = serve_connection(connection_runtime, connected).await {
                        tracing::warn!(%error, "local Agent request failed");
                    }
                }) {
                    tracing::warn!(%error, "local Agent request could not start");
                }
            }
        }
    }
}

#[cfg(any(windows, test))]
fn validate_windows_pipe_endpoint(endpoint: &Path) -> io::Result<&str> {
    const PREFIX: &str = "\\\\.\\pipe\\";
    let endpoint = endpoint.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows Agent pipe name must be valid UTF-8",
        )
    })?;
    let Some(name) = endpoint.strip_prefix(PREFIX) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            r"Windows Agent endpoint must start with \\.\pipe\",
        ));
    };
    if name.is_empty()
        || name.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
        })
        || endpoint.encode_utf16().count() > 256
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows Agent pipe name is empty, nested, invalid, or too long",
        ));
    }
    Ok(endpoint)
}

#[cfg(windows)]
fn create_windows_pipe(endpoint: &str, owner_sid: &str, first: bool) -> Result<NamedPipeServer> {
    use std::ffi::c_void;

    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

    let descriptor = WindowsSecurityDescriptor::new(owner_sid)?;
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>())
            .expect("SECURITY_ATTRIBUTES size fits u32"),
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true);
    // SAFETY: `attributes` and its LocalAlloc-owned security descriptor remain
    // alive for the complete CreateNamedPipeW call. Tokio copies the descriptor
    // into the kernel object before returning and never retains this pointer.
    let server = unsafe {
        options.create_with_security_attributes_raw(
            endpoint,
            (&mut attributes as *mut SECURITY_ATTRIBUTES).cast::<c_void>(),
        )
    }?;
    Ok(server)
}

#[cfg(windows)]
struct WindowsSecurityDescriptor(*mut std::ffi::c_void);

#[cfg(windows)]
impl WindowsSecurityDescriptor {
    fn new(owner_sid: &str) -> io::Result<Self> {
        use std::os::windows::ffi::OsStrExt as _;
        use std::ptr;

        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };

        let sddl = std::ffi::OsStr::new(&format!("D:P(A;;GA;;;{owner_sid})"))
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut descriptor = ptr::null_mut();
        // SAFETY: `sddl` is a live, null-terminated UTF-16 string and
        // `descriptor` points to writable PSECURITY_DESCRIPTOR storage.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor.cast()))
    }
}

#[cfg(windows)]
impl Drop for WindowsSecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: The descriptor was allocated by
        // ConvertStringSecurityDescriptorToSecurityDescriptorW and is released
        // exactly once with LocalFree.
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.0);
        }
    }
}

#[cfg(windows)]
fn validate_windows_peer(server: &NamedPipeServer, owner_sid: &str) -> Result<()> {
    use std::os::windows::io::AsRawHandle as _;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;

    let mut client_process_id = 0_u32;
    // SAFETY: `server` owns a connected Named Pipe server handle and the PID
    // output points to initialized writable storage for the duration of the call.
    if unsafe {
        GetNamedPipeClientProcessId(
            server.as_raw_handle().cast::<std::ffi::c_void>() as HANDLE,
            &mut client_process_id,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    let peer_sid = windows_process_user_sid(client_process_id)?;
    if peer_sid != owner_sid {
        bail!("Agent peer SID does not match the pipe owner");
    }
    Ok(())
}

#[cfg(unix)]
fn validate_unix_peer(stream: &UnixStream, socket_uid: u32) -> Result<()> {
    let peer_uid = stream.peer_cred()?.uid();
    if peer_uid != socket_uid {
        bail!("Agent peer UID {peer_uid} does not match socket owner {socket_uid}");
    }
    Ok(())
}

#[cfg(unix)]
async fn termination_signal() -> io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        signal = tokio::signal::ctrl_c() => signal,
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(windows)]
async fn termination_signal() -> io::Result<()> {
    tokio::signal::ctrl_c().await
}

#[cfg(unix)]
async fn prepare_socket(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Agent socket needs a parent directory"))?;
    create_directory(parent)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_socket() {
        bail!(
            "{} exists and is not a Unix socket; refusing to remove it",
            path.display()
        );
    }
    if UnixStream::connect(path).await.is_ok() {
        bail!(
            "another Envoix Agent is already listening at {}",
            path.display()
        );
    }
    fs::remove_file(path).with_context(|| format!("remove stale socket {}", path.display()))?;
    Ok(())
}

async fn serve_connection<S>(runtime: Arc<AgentRuntime>, stream: S) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut request_bytes = Vec::new();
    let mut limited = BufReader::new(read).take(MAX_AGENT_REQUEST_BYTES + 1);
    limited.read_until(b'\n', &mut request_bytes).await?;
    let (request_id, response) = if request_bytes.len() as u64 > MAX_AGENT_REQUEST_BYTES {
        (
            "invalid".to_string(),
            AgentResponse::error("request_too_large", "Agent request exceeds 64 KiB"),
        )
    } else {
        match decode_request(&request_bytes) {
            Ok(envelope) => {
                let request_id = envelope.request_id;
                let response = handle_request(runtime, envelope.request).await;
                (request_id, response)
            }
            Err(error) => (
                error.request_id,
                AgentResponse::error(error.code, error.message),
            ),
        }
    };
    let mut envelope = AgentResponseEnvelope::new(request_id.clone(), response)?;
    let mut response_bytes = serde_json::to_vec(&envelope)?;
    if response_bytes.len() as u64 > MAX_AGENT_RESPONSE_BYTES {
        envelope = AgentResponseEnvelope::new(
            request_id,
            AgentResponse::error(
                "response_too_large",
                "Agent response exceeds the control message limit",
            ),
        )?;
        response_bytes = serde_json::to_vec(&envelope)?;
    }
    response_bytes.push(b'\n');
    write.write_all(&response_bytes).await?;
    write.shutdown().await?;
    Ok(())
}

#[derive(Debug)]
struct RequestDecodeError {
    request_id: String,
    code: &'static str,
    message: String,
}

fn decode_request(bytes: &[u8]) -> Result<AgentRequestEnvelope, RequestDecodeError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| RequestDecodeError {
            request_id: "invalid".into(),
            code: "invalid_request",
            message: error.to_string(),
        })?;
    let request_id = value
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .filter(|request_id| is_valid_agent_request_id(request_id))
        .unwrap_or("invalid")
        .to_string();
    let protocol_version = value
        .get("protocol_version")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| value.get("command").map(|_| 3));
    if protocol_version != Some(u64::from(AGENT_PROTOCOL_VERSION)) {
        return Err(RequestDecodeError {
            request_id,
            code: "unsupported_protocol_version",
            message: format!(
                "Agent protocol {} is unsupported; expected {}",
                protocol_version
                    .map(|version| version.to_string())
                    .unwrap_or_else(|| "missing".into()),
                AGENT_PROTOCOL_VERSION
            ),
        });
    }
    let envelope: AgentRequestEnvelope =
        serde_json::from_value(value).map_err(|error| RequestDecodeError {
            request_id: request_id.clone(),
            code: "invalid_request",
            message: error.to_string(),
        })?;
    envelope.validate().map_err(|error| RequestDecodeError {
        request_id,
        code: "invalid_request",
        message: error.to_string(),
    })?;
    Ok(envelope)
}

async fn handle_request(runtime: Arc<AgentRuntime>, request: AgentRequest) -> AgentResponse {
    match handle_request_result(runtime, request).await {
        Ok(response) => response,
        Err(error) => AgentResponse::error("operation_failed", format!("{error:#}")),
    }
}

async fn handle_request_result(
    runtime: Arc<AgentRuntime>,
    request: AgentRequest,
) -> Result<AgentResponse> {
    match request {
        AgentRequest::Status => Ok(AgentResponse::Status {
            status: runtime.status()?,
        }),
        AgentRequest::Snapshot { inbox_limit } => Ok(AgentResponse::Snapshot {
            snapshot: Box::new(runtime.snapshot(inbox_limit)?),
        }),
        AgentRequest::Events { after, limit } => {
            match lock(&runtime.events)?.read_after(&after, limit) {
                AgentEventRead::Events { cursor, events } => {
                    Ok(AgentResponse::Events { cursor, events })
                }
                AgentEventRead::SnapshotRequired(cursor) => {
                    Ok(AgentResponse::SnapshotRequired { cursor })
                }
            }
        }
        AgentRequest::ListDevices => Ok(AgentResponse::Devices {
            devices: lock(&runtime.store)?.devices(),
        }),
        AgentRequest::RevokeDevice { device } => {
            let forgotten = lock(&runtime.store)?.forget_device(&device)?;
            tracing::info!(
                relationship_id = %forgotten.id,
                device = %forgotten.label,
                "device revoked by local Agent control client"
            );
            if let Some(cancel) = lock(&runtime.active_receivers)?.get(&forgotten.id) {
                cancel.cancel();
            }
            cancel_outgoing_for_relationship(&runtime, &forgotten.id)?;
            record_agent_event(
                &runtime,
                AgentEvent::RelationshipChanged {
                    relationship_id: forgotten.id.clone(),
                    change: AgentRelationshipChange::Revoked,
                },
            )?;
            Ok(AgentResponse::DeviceRevoked { device: forgotten })
        }
        AgentRequest::UpdateDeviceRoute {
            device,
            broker,
            relay,
        } => {
            let current = lock(&runtime.store)?.resolve_device(&device)?;
            if relationship_has_active_transfer(&runtime, current.id())? {
                bail!(
                    "device route cannot change while it has an active transfer or pending offer"
                );
            }
            let unchanged = current.broker() == broker && current.relay() == relay.as_deref();
            let updated = lock(&runtime.store)?.update_device_route(
                current.id(),
                &broker,
                relay.as_deref(),
            )?;
            tracing::info!(
                relationship_id = %updated.id,
                device = %updated.label,
                broker = %updated.broker,
                relay = updated.relay.as_deref().unwrap_or("disabled"),
                "remembered device route updated"
            );
            if !unchanged {
                restart_remembered_receiver(runtime.clone(), updated.id.clone())?;
                record_agent_event(
                    &runtime,
                    AgentEvent::RelationshipChanged {
                        relationship_id: updated.id.clone(),
                        change: AgentRelationshipChange::RouteUpdated,
                    },
                )?;
            }
            Ok(AgentResponse::DeviceRouteUpdated { device: updated })
        }
        AgentRequest::CreateTransfer { device, paths } => {
            create_agent_transfer(runtime, device, paths).await
        }
        AgentRequest::ListTransfers => Ok(AgentResponse::Transfers {
            transfers: lock(&runtime.store)?.transfers(),
        }),
        AgentRequest::ListTransferPaths => Ok(AgentResponse::TransferPaths {
            paths: runtime.active_paths()?,
        }),
        AgentRequest::GetTransfer { transfer_id } => {
            let transfer = lock(&runtime.store)?
                .transfer(&transfer_id)?
                .ok_or_else(|| anyhow!("Transfer {transfer_id} does not exist"))?;
            Ok(AgentResponse::Transfer { transfer })
        }
        AgentRequest::ListPendingOffers => Ok(AgentResponse::PendingOffers {
            offers: runtime.pending_offers()?,
        }),
        AgentRequest::DecidePendingOffer { offer_id, decision } => {
            let offer = decide_pending_offer(&runtime, &offer_id, decision)?;
            Ok(AgentResponse::PendingOfferDecided { offer, decision })
        }
        AgentRequest::ListInbox { limit } => Ok(AgentResponse::Inbox {
            items: lock(&runtime.store)?.inbox(limit),
        }),
        AgentRequest::LatestInbox => Ok(AgentResponse::Latest {
            item: lock(&runtime.store)?.latest_inbox(),
        }),
        AgentRequest::Diagnostics => Ok(AgentResponse::Diagnostics {
            diagnostics: runtime.diagnostics()?,
        }),
        AgentRequest::Pair { label } => begin_pairing(runtime, label).await,
        AgentRequest::JoinPairing { pairing } => join_pairing(runtime, pairing).await,
    }
}

async fn create_agent_transfer(
    runtime: Arc<AgentRuntime>,
    device: String,
    paths: Vec<PathBuf>,
) -> Result<AgentResponse> {
    {
        let store = lock(&runtime.store)?;
        store.resolve_device(&device)?;
    }
    let job_store = api::TransferJobStore::new(runtime.config.state_directory.join("outbox/jobs"));
    let mut job = api::CanonicalTransferJob::new(api::CompressionPolicyV2::Smart)?;
    for path in paths {
        if !path.is_absolute() {
            bail!(
                "Agent Transfer source path must be absolute: {}",
                path.display()
            );
        }
        let path = tokio::fs::canonicalize(&path)
            .await
            .with_context(|| format!("resolve Transfer source {}", path.display()))?;
        job.add_local_path(path).await?;
        job_store.save(&job).await?;
    }
    job.prepare_all().await?;
    job_store.save(&job).await?;
    if job.lifecycle() != api::JobLifecycle::ReadyToSend {
        bail!("Transfer source preparation requires a user decision");
    }
    job.seal_for_send()?;
    job_store.save(&job).await?;

    let (content_id, transfer_id) = agent_transfer_ids(&job)?;
    let total_bytes = job
        .manifest()
        .expect("sealed Agent job has a manifest")
        .totals
        .total_plaintext_bytes;
    let transfer = lock(&runtime.store)?.create_transfer(
        &device,
        transfer_id.clone(),
        content_id,
        total_bytes,
    )?;
    record_agent_event(
        &runtime,
        AgentEvent::TransferChanged {
            transfer_id: transfer_id.to_string(),
        },
    )?;
    if !wake_active_room(&runtime, transfer.relationship_id.as_str())? {
        restart_remembered_receiver(runtime.clone(), transfer.relationship_id.to_string())?;
    }
    Ok(AgentResponse::TransferCreated { transfer })
}

fn agent_transfer_ids(job: &api::CanonicalTransferJob) -> Result<(ContentId, TransferId)> {
    let job_token = URL_SAFE_NO_PAD.encode(job.job_id().0);
    Ok((
        ContentId::parse(format!("content_{job_token}"))?,
        TransferId::parse(format!("transfer_{job_token}_{}", job.generation()))?,
    ))
}

fn agent_job_id(content_id: &ContentId) -> Result<api::JobIdV2> {
    let token = content_id
        .as_str()
        .strip_prefix("content_")
        .ok_or_else(|| anyhow!("Agent content ID has an unsupported shape"))?;
    let decoded = URL_SAFE_NO_PAD
        .decode(token)
        .context("Agent content ID is not canonical base64url")?;
    let bytes = <[u8; 16]>::try_from(decoded)
        .map_err(|_| anyhow!("Agent content ID does not contain a 16-byte job ID"))?;
    if URL_SAFE_NO_PAD.encode(bytes) != token {
        bail!("Agent content ID is not canonical base64url");
    }
    Ok(api::JobIdV2(bytes))
}

async fn begin_pairing(runtime: Arc<AgentRuntime>, label: String) -> Result<AgentResponse> {
    if runtime.shutdown.is_cancelled() {
        bail!("Agent is shutting down");
    }
    let prepared = {
        let store = lock(&runtime.store)?;
        store.prepare_device(
            &label,
            &runtime.config.broker,
            runtime.config.relay.as_deref(),
        )?
    };
    {
        let mut pairings = lock(&runtime.active_pairings)?;
        if !pairings.insert(prepared.label().to_ascii_lowercase()) {
            bail!("a pairing for this device label is already active");
        }
    }
    let invitation = match RoomControlInvite::generate(
        runtime.config.broker.clone(),
        runtime.config.relay.clone(),
    ) {
        Ok(invitation) => invitation,
        Err(error) => {
            lock(&runtime.active_pairings)?.remove(&prepared.label().to_ascii_lowercase());
            return Err(error.into());
        }
    };
    let verification_code = match generate_verification_code() {
        Ok(code) => code,
        Err(error) => {
            lock(&runtime.active_pairings)?.remove(&prepared.label().to_ascii_lowercase());
            return Err(error);
        }
    };
    let response = AgentResponse::Pairing {
        pairing: PairingInvitation {
            label: prepared.label().to_string(),
            room_code: invitation.code().to_string(),
            verification_code: verification_code.clone(),
            expires_at_unix_seconds: invitation.expires_at_unix_secs(),
        },
    };
    if let Err(error) = record_agent_event(
        &runtime,
        AgentEvent::PairingChanged {
            label: prepared.label().to_string(),
            active: true,
        },
    ) {
        lock(&runtime.active_pairings)?.remove(&prepared.label().to_ascii_lowercase());
        return Err(error);
    }
    let pairing_label = prepared.label().to_string();
    let task_runtime = runtime.clone();
    if let Err(error) = spawn_background_task(
        &runtime,
        run_initial_pairing(task_runtime, prepared, invitation, verification_code),
    ) {
        lock(&runtime.active_pairings)?.remove(&pairing_label.to_ascii_lowercase());
        record_agent_event(
            &runtime,
            AgentEvent::PairingChanged {
                label: pairing_label,
                active: false,
            },
        )?;
        return Err(error);
    }
    Ok(response)
}

#[derive(Debug)]
struct RetryableInitialPairingClose {
    reason: String,
}

impl std::fmt::Display for RetryableInitialPairingClose {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "peer closed the room before verification: {}",
            self.reason
        )
    }
}

impl std::error::Error for RetryableInitialPairingClose {}

async fn run_initial_pairing(
    runtime: Arc<AgentRuntime>,
    prepared: PreparedRememberedDevice,
    invitation: RoomControlInvite,
    verification_code: String,
) {
    let label = prepared.label().to_string();
    let device_id = prepared.id().to_string();
    // A foreground client may discover the verification request before it can
    // hand the invitation to its durable Agent owner. Keep the same one-time
    // invitation alive until its advertised expiry so that the Agent can
    // reconnect without moving credential bytes through the GUI process.
    let paired = loop {
        match establish_initial_room(
            &runtime,
            prepared.clone(),
            invitation.clone(),
            &verification_code,
        )
        .await
        {
            Ok(paired) => break Ok(paired),
            Err(error) if runtime.shutdown.is_cancelled() => break Err(error),
            Err(error) if error.is::<RetryableInitialPairingClose>() => {
                let expired = unix_seconds()
                    .map(|now| now >= invitation.expires_at_unix_secs())
                    .unwrap_or(true);
                if expired {
                    break Err(error);
                }
                tracing::debug!(device = %label, %error, "initial pairing peer disconnected; waiting for a durable owner");
                tokio::select! {
                    _ = tokio::time::sleep(INITIAL_PAIRING_RETRY_DELAY) => {}
                    _ = runtime.shutdown.cancelled() => break Err(error),
                }
            }
            Err(error) => break Err(error),
        }
    };
    lock_or_log(&runtime.active_pairings, "active pairings")
        .map(|mut pairings| pairings.remove(&label.to_ascii_lowercase()));
    if let Err(error) = record_agent_event(
        &runtime,
        AgentEvent::PairingChanged {
            label: label.clone(),
            active: false,
        },
    ) {
        tracing::error!(%error, "pairing completion event could not be recorded");
    }
    let (session, session_cancel, _device) = match paired {
        Ok(paired) => {
            if let Err(error) = record_agent_event(
                &runtime,
                AgentEvent::RelationshipChanged {
                    relationship_id: device_id.clone(),
                    change: AgentRelationshipChange::Trusted,
                },
            ) {
                tracing::error!(%error, "trusted Relationship event could not be recorded");
            }
            tracing::info!(device = %label, "device verification completed");
            paired
        }
        Err(error) if runtime.shutdown.is_cancelled() => {
            tracing::debug!(device = %label, %error, "initial pairing stopped");
            return;
        }
        Err(error) => {
            tracing::warn!(device = %label, %error, "initial pairing ended");
            return;
        }
    };
    continue_initial_room(runtime, session, session_cancel, device_id, label).await;
}

async fn join_pairing(
    runtime: Arc<AgentRuntime>,
    pairing: envoix_client::product::AgentPairingInput,
) -> Result<AgentResponse> {
    if runtime.shutdown.is_cancelled() {
        bail!("Agent is shutting down");
    }
    pairing.validate()?;
    let invitation = RoomControlInvite::parse(
        &pairing.invitation,
        runtime.config.broker.clone(),
        runtime.config.relay.clone(),
    )?;
    let prepared = {
        let store = lock(&runtime.store)?;
        store.prepare_device(&pairing.label, invitation.broker(), invitation.relay())?
    };
    let label = prepared.label().to_string();
    let device_id = prepared.id().to_string();
    {
        let mut pairings = lock(&runtime.active_pairings)?;
        if !pairings.insert(label.to_ascii_lowercase()) {
            bail!("a pairing for this device label is already active");
        }
    }
    if let Err(error) = record_agent_event(
        &runtime,
        AgentEvent::PairingChanged {
            label: label.clone(),
            active: true,
        },
    ) {
        lock(&runtime.active_pairings)?.remove(&label.to_ascii_lowercase());
        return Err(error);
    }

    let paired =
        establish_joined_room(&runtime, prepared, invitation, &pairing.verification_code).await;
    lock(&runtime.active_pairings)?.remove(&label.to_ascii_lowercase());
    record_agent_event(
        &runtime,
        AgentEvent::PairingChanged {
            label: label.clone(),
            active: false,
        },
    )?;
    let (session, session_cancel, device) = paired?;
    record_agent_event(
        &runtime,
        AgentEvent::RelationshipChanged {
            relationship_id: device_id.clone(),
            change: AgentRelationshipChange::Trusted,
        },
    )?;
    tracing::info!(device = %label, "device verification completed");

    let task_runtime = runtime.clone();
    let response_device = device.clone();
    if let Err(error) = spawn_background_task(&runtime, async move {
        continue_initial_room(task_runtime, session, session_cancel, device_id, label).await;
    }) {
        lock(&runtime.active_receivers)?.remove(&device.id);
        return Err(error);
    }
    Ok(AgentResponse::DevicePaired {
        device: response_device,
    })
}

async fn continue_initial_room(
    runtime: Arc<AgentRuntime>,
    session: Arc<RoomControlSession>,
    session_cancel: TransferCancelToken,
    device_id: String,
    label: String,
) {
    let result = run_room_session(
        &runtime,
        session.clone(),
        &device_id,
        &label,
        &session_cancel,
    )
    .await;
    session.shutdown().await;
    if let Err(error) = result
        && !runtime.shutdown.is_cancelled()
        && !session_cancel.is_cancelled()
    {
        tracing::warn!(device = %label, %error, "initial room ended");
    }
    lock_or_log(&runtime.active_receivers, "active receivers")
        .map(|mut active| active.remove(&device_id));
    if !runtime.shutdown.is_cancelled() && !session_cancel.is_cancelled() {
        spawn_remembered_receiver(runtime, device_id);
    }
}

async fn establish_initial_room(
    runtime: &AgentRuntime,
    prepared: PreparedRememberedDevice,
    invitation: RoomControlInvite,
    verification_code: &str,
) -> Result<(Arc<RoomControlSession>, TransferCancelToken, DeviceSummary)> {
    let device_id = prepared.id().to_string();
    let invitation_relay = invitation.relay().map(str::to_string);
    let session = Arc::new(
        api::connect_room_control(
            invitation,
            runtime.config.device_name.clone(),
            true,
            false,
            runtime.session_config(invitation_relay.as_deref()),
            &runtime.shutdown,
        )
        .await?,
    );
    let pairing: Result<(TransferCancelToken, DeviceSummary)> = async {
        session.request_verification(verification_code).await?;
        loop {
            match session.next_event().await? {
                RoomControlEvent::VerificationSucceeded => {
                    let credential = session.pairing_credential().ok_or_else(|| {
                        anyhow!("verified room did not expose a pairing credential")
                    })?;
                    let session_cancel = register_remembered_receiver(runtime, &device_id)?
                        .ok_or_else(|| anyhow!("remembered receiver was already active"))?;
                    let commit_result =
                        lock(&runtime.store)?.commit_device(prepared, &credential.to_opaque(), 0);
                    let device = match commit_result {
                        Ok(device) => device,
                        Err(error) => {
                            lock_or_log(&runtime.active_receivers, "active receivers")
                                .map(|mut active| active.remove(&device_id));
                            return Err(error.into());
                        }
                    };
                    return Ok((session_cancel, device));
                }
                RoomControlEvent::VerificationFailed => {
                    bail!("the one-time device verification code was rejected");
                }
                RoomControlEvent::IncomingOffer(offer) => {
                    session
                        .reject_offer(&offer.offer_id, RoomOfferRejection::Invalid)
                        .await?;
                }
                RoomControlEvent::PeerClosed(reason) => {
                    return Err(RetryableInitialPairingClose {
                        reason: format!("{reason:?}"),
                    }
                    .into());
                }
                RoomControlEvent::VerificationRequested
                | RoomControlEvent::LifetimeChanged(_)
                | RoomControlEvent::Pong { .. } => {}
                RoomControlEvent::OfferAccepted { .. } | RoomControlEvent::OfferRejected { .. } => {
                    bail!("peer sent an unexpected offer decision during verification");
                }
            }
        }
    }
    .await;
    match pairing {
        Ok((session_cancel, device)) => Ok((session, session_cancel, device)),
        Err(error) => {
            session.shutdown().await;
            Err(error)
        }
    }
}

async fn establish_joined_room(
    runtime: &AgentRuntime,
    prepared: PreparedRememberedDevice,
    invitation: RoomControlInvite,
    verification_code: &str,
) -> Result<(Arc<RoomControlSession>, TransferCancelToken, DeviceSummary)> {
    let device_id = prepared.id().to_string();
    let invitation_relay = invitation.relay().map(str::to_string);
    let session = Arc::new(
        api::connect_room_control(
            invitation,
            runtime.config.device_name.clone(),
            false,
            false,
            runtime.session_config(invitation_relay.as_deref()),
            &runtime.shutdown,
        )
        .await?,
    );
    let pairing: Result<(TransferCancelToken, DeviceSummary)> = async {
        let mut submitted = false;
        loop {
            match session.next_event().await? {
                RoomControlEvent::VerificationRequested if !submitted => {
                    session.submit_verification_code(verification_code).await?;
                    submitted = true;
                }
                RoomControlEvent::VerificationSucceeded if submitted => {
                    let credential = session.pairing_credential().ok_or_else(|| {
                        anyhow!("verified room did not expose a pairing credential")
                    })?;
                    let session_cancel = register_remembered_receiver(runtime, &device_id)?
                        .ok_or_else(|| anyhow!("remembered receiver was already active"))?;
                    match lock(&runtime.store)?.commit_device(prepared, &credential.to_opaque(), 0)
                    {
                        Ok(device) => return Ok((session_cancel, device)),
                        Err(error) => {
                            lock_or_log(&runtime.active_receivers, "active receivers")
                                .map(|mut active| active.remove(&device_id));
                            return Err(error.into());
                        }
                    }
                }
                RoomControlEvent::VerificationFailed => {
                    bail!("the one-time device verification code was rejected");
                }
                RoomControlEvent::IncomingOffer(offer) => {
                    session
                        .reject_offer(&offer.offer_id, RoomOfferRejection::Invalid)
                        .await?;
                }
                RoomControlEvent::PeerClosed(reason) => {
                    bail!("peer closed the room before verification: {reason:?}");
                }
                RoomControlEvent::VerificationRequested
                | RoomControlEvent::LifetimeChanged(_)
                | RoomControlEvent::Pong { .. } => {}
                RoomControlEvent::VerificationSucceeded => {
                    bail!("peer completed device verification before requesting a code");
                }
                RoomControlEvent::OfferAccepted { .. } | RoomControlEvent::OfferRejected { .. } => {
                    bail!("peer sent an unexpected offer decision during verification");
                }
            }
        }
    }
    .await;
    match pairing {
        Ok((session_cancel, device)) => Ok((session, session_cancel, device)),
        Err(error) => {
            session.shutdown().await;
            Err(error)
        }
    }
}

async fn receive_invitation_offer(
    runtime: &Arc<AgentRuntime>,
    transfer_id: &str,
    bootstrap: api::InvitationBootstrap,
    cancel: &TransferCancelToken,
) -> Result<PendingManifestV2Receive> {
    let relay = runtime.config.relay.as_deref();
    let broker = api::parse_broker_addr(&runtime.config.broker, relay)?;
    let events: Arc<dyn EventSink> = Arc::new(AgentIncomingEvents {
        runtime: runtime.clone(),
        transfer_id: transfer_id.to_string(),
    });
    api::receive_manifest_v2_offer_via_room(
        broker,
        bootstrap,
        envoix_client::BindAddrs::dual_stack(0),
        runtime.session_config(relay),
        events,
        cancel,
    )
    .await
    .map_err(Into::into)
}

fn spawn_remembered_receiver(runtime: Arc<AgentRuntime>, device_id: String) {
    if runtime.shutdown.is_cancelled() {
        return;
    }
    let receiver_cancel = match register_remembered_receiver(&runtime, &device_id) {
        Ok(Some(cancel)) => cancel,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(device_id, %error, "remembered receiver could not start");
            return;
        }
    };
    let task_runtime = runtime.clone();
    let task_device_id = device_id.clone();
    if let Err(error) = spawn_background_task(&runtime, async move {
        remembered_receiver_loop(task_runtime.clone(), &task_device_id, &receiver_cancel).await;
        if let Some(mut active) = lock_or_log(&task_runtime.active_receivers, "active receivers") {
            active.remove(&task_device_id);
        }
    }) {
        lock_or_log(&runtime.active_receivers, "active receivers")
            .map(|mut active| active.remove(&device_id));
        tracing::error!(device_id, %error, "remembered receiver task could not start");
    }
}

fn restart_remembered_receiver(runtime: Arc<AgentRuntime>, device_id: String) -> Result<()> {
    let cancel = lock(&runtime.active_receivers)?.get(&device_id).cloned();
    let Some(cancel) = cancel else {
        spawn_remembered_receiver(runtime, device_id);
        return Ok(());
    };
    cancel.cancel();
    let restart_runtime = runtime.clone();
    spawn_background_task(&runtime, async move {
        loop {
            if restart_runtime.shutdown.is_cancelled() {
                return;
            }
            let still_active = lock_or_log(&restart_runtime.active_receivers, "active receivers")
                .is_some_and(|active| active.contains_key(&device_id));
            if !still_active {
                spawn_remembered_receiver(restart_runtime, device_id);
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
}

fn register_remembered_receiver(
    runtime: &AgentRuntime,
    device_id: &str,
) -> Result<Option<TransferCancelToken>> {
    let mut active = lock(&runtime.active_receivers)?;
    if active.contains_key(device_id) {
        return Ok(None);
    }
    let cancel = TransferCancelToken::new();
    active.insert(device_id.to_string(), cancel.clone());
    Ok(Some(cancel))
}

/// How long to wait before the next remembered connect attempt, given how long
/// the attempt that just failed ran and how long the previous wait was.
///
/// Attempts that fail fast are refusals, so they back off geometrically and an
/// unreachable peer settles at one attempt per minute instead of one every few
/// seconds. An attempt that ran long enough to park at the broker instead
/// waited out its own Room, which is the ordinary idle state rather than a
/// failure, so it keeps the base delay and parks again promptly.
fn receiver_retry_delay(attempt: Duration, previous: Duration) -> Duration {
    if attempt >= RECEIVER_PARKED_ATTEMPT {
        return RECEIVER_RETRY_DELAY;
    }
    previous
        .saturating_mul(2)
        .min(RECEIVER_RETRY_MAX_DELAY)
        .max(RECEIVER_RETRY_DELAY)
}

/// An idle Agent parks as the responder. Queuing an outbound Transfer makes
/// this side the connector so it can authenticate against the remote Agent's
/// background listener for the same remembered generation.
fn remembered_connection_role(
    store: &ProductStore,
    device_id: &str,
) -> Result<RememberedRoomControlRole> {
    if store.dispatchable_transfers(device_id)?.is_empty() {
        Ok(RememberedRoomControlRole::Responder)
    } else {
        Ok(RememberedRoomControlRole::Connector)
    }
}

async fn remembered_receiver_loop(
    runtime: Arc<AgentRuntime>,
    device_id: &str,
    receiver_cancel: &TransferCancelToken,
) {
    let mut retry_delay = Duration::ZERO;
    while !runtime.shutdown.is_cancelled() && !receiver_cancel.is_cancelled() {
        let loaded = (|| -> Result<_> {
            let store = lock(&runtime.store)?;
            let record = store
                .device_record(device_id)
                .ok_or_else(|| anyhow!("remembered device metadata is missing"))?;
            let opaque = store.device_credential(device_id)?;
            let role = remembered_connection_role(&store, device_id)?;
            Ok((record, opaque, role))
        })();
        let (record, opaque, role) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                tracing::error!(device_id, %error, "remembered receiver cannot load device");
                return;
            }
        };
        let attempt_started = Instant::now();
        match connect_remembered_room(&runtime, &record, opaque.expose(), role, receiver_cancel)
            .await
        {
            Ok((session, next_generation)) => {
                retry_delay = Duration::ZERO;
                if let Err(error) = lock(&runtime.store).and_then(|mut store| {
                    store
                        .rotate_device(record.id(), opaque.expose(), next_generation)
                        .map_err(Into::into)
                }) {
                    session.shutdown().await;
                    tracing::error!(device = %record.label(), %error, "remembered generation could not be persisted");
                    return;
                }
                if let Err(error) = record_agent_event(
                    &runtime,
                    AgentEvent::RelationshipChanged {
                        relationship_id: record.id().to_string(),
                        change: AgentRelationshipChange::Rotated,
                    },
                ) {
                    tracing::error!(device = %record.label(), %error, "Relationship rotation event could not be recorded");
                }
                tracing::info!(
                    device = %record.label(),
                    generation = next_generation,
                    ?role,
                    "remembered room connected"
                );
                let result = run_room_session(
                    &runtime,
                    session.clone(),
                    record.id(),
                    record.label(),
                    receiver_cancel,
                )
                .await;
                session.shutdown().await;
                if let Err(error) = result
                    && !runtime.shutdown.is_cancelled()
                    && !receiver_cancel.is_cancelled()
                {
                    tracing::warn!(device = %record.label(), %error, "remembered room ended");
                }
            }
            Err(error) => {
                if runtime.shutdown.is_cancelled() || receiver_cancel.is_cancelled() {
                    return;
                }
                retry_delay = receiver_retry_delay(attempt_started.elapsed(), retry_delay);
                tracing::warn!(
                    device = %record.label(),
                    ?role,
                    %error,
                    retry_in_secs = retry_delay.as_secs(),
                    "remembered receiver retrying"
                );
                tokio::select! {
                    _ = tokio::time::sleep(retry_delay) => {}
                    _ = runtime.shutdown.cancelled() => return,
                    _ = receiver_cancel.cancelled() => return,
                }
            }
        }
    }
}

async fn connect_remembered_room(
    runtime: &Arc<AgentRuntime>,
    record: &RememberedDeviceRecord,
    opaque: &[u8],
    role: RememberedRoomControlRole,
    receiver_cancel: &TransferCancelToken,
) -> Result<(Arc<RoomControlSession>, u64)> {
    let credential = RememberedCredential::from_opaque(opaque)?;
    let relay = record.relay();
    let generation_role = match role {
        RememberedRoomControlRole::Connector => RememberedGenerationRole::Connector,
        RememberedRoomControlRole::Responder => RememberedGenerationRole::Responder,
    };
    let generations = remembered_generation_attempts(
        record.generation(),
        record.previous_generation(),
        generation_role,
    )?;
    let last_index = generations.len() - 1;
    let mut last_error = None;
    for (index, generation) in generations.into_iter().enumerate() {
        let next_generation = generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("remembered credential generation is exhausted"))?;
        let attempt_cancel = TransferCancelToken::new();
        let connect = api::connect_remembered_room_control(
            credential.derive_session(generation),
            record.broker().to_string(),
            relay.map(str::to_string),
            runtime.config.device_name.clone(),
            role,
            runtime.session_config(relay),
            &attempt_cancel,
        );
        tokio::pin!(connect);
        let result = if index < last_index {
            tokio::select! {
                result = &mut connect => result,
                _ = tokio::time::sleep(REMEMBERED_FALLBACK_TIMEOUT) => {
                    cancel_and_drain_remembered_connect(
                        &attempt_cancel,
                        connect.as_mut(),
                        REMEMBERED_CONNECT_CANCEL_GRACE_PERIOD,
                    ).await;
                    last_error = Some(anyhow!(
                        "current remembered generation did not find the peer"
                    ));
                    continue;
                }
                _ = receiver_cancel.cancelled() => {
                    cancel_and_drain_remembered_connect(
                        &attempt_cancel,
                        connect.as_mut(),
                        REMEMBERED_CONNECT_CANCEL_GRACE_PERIOD,
                    ).await;
                    return Err(anyhow!("remembered room connection cancelled"));
                }
                _ = runtime.shutdown.cancelled() => {
                    cancel_and_drain_remembered_connect(
                        &attempt_cancel,
                        connect.as_mut(),
                        REMEMBERED_CONNECT_CANCEL_GRACE_PERIOD,
                    ).await;
                    return Err(anyhow!("remembered room connection cancelled"));
                }
            }
        } else {
            tokio::select! {
                result = &mut connect => result,
                _ = receiver_cancel.cancelled() => {
                    cancel_and_drain_remembered_connect(
                        &attempt_cancel,
                        connect.as_mut(),
                        REMEMBERED_CONNECT_CANCEL_GRACE_PERIOD,
                    ).await;
                    return Err(anyhow!("remembered room connection cancelled"));
                }
                _ = runtime.shutdown.cancelled() => {
                    cancel_and_drain_remembered_connect(
                        &attempt_cancel,
                        connect.as_mut(),
                        REMEMBERED_CONNECT_CANCEL_GRACE_PERIOD,
                    ).await;
                    return Err(anyhow!("remembered room connection cancelled"));
                }
            }
        };
        match result {
            Ok(session) => return Ok((Arc::new(session), next_generation)),
            Err(error)
                if (RememberedAttemptOutcome {
                    succeeded: false,
                    authenticated: error.peer_authenticated(),
                    canceled: receiver_cancel.is_cancelled() || runtime.shutdown.is_cancelled(),
                })
                .should_stop_fallback() =>
            {
                return Err(error.into_error().into());
            }
            Err(error) => last_error = Some(error.into_error().into()),
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("remembered device has no usable generation")))
}

async fn cancel_and_drain_remembered_connect<F>(
    cancel: &TransferCancelToken,
    connect: Pin<&mut F>,
    grace_period: Duration,
) where
    F: Future,
{
    cancel.cancel();
    let _ = tokio::time::timeout(grace_period, connect).await;
}

enum RoomSessionInput {
    Control(RoomControlEvent),
    Wake,
    PendingDecision(Result<AgentOfferDecision, oneshot::error::RecvError>),
}

async fn wait_for_pending_offer_decision(
    pending: &mut Option<PendingIncomingOffer>,
) -> Result<AgentOfferDecision, oneshot::error::RecvError> {
    match pending {
        Some(pending) => (&mut pending.decision).await,
        None => std::future::pending().await,
    }
}

async fn run_room_session(
    runtime: &Arc<AgentRuntime>,
    session: Arc<RoomControlSession>,
    device_id: &str,
    device_label: &str,
    receiver_cancel: &TransferCancelToken,
) -> Result<()> {
    let wake = Arc::new(Notify::new());
    {
        let mut rooms = lock(&runtime.active_rooms)?;
        if rooms.contains_key(device_id) {
            bail!("remembered device already has an active room");
        }
        rooms.insert(device_id.to_string(), wake.clone());
    }
    wake.notify_one();

    let result = async {
        let mut pending_outgoing = None;
        let mut pending_incoming = None;
        loop {
            if pending_outgoing.is_none() && !relationship_has_active_outgoing(runtime, device_id)?
            {
                pending_outgoing =
                    offer_next_outgoing(runtime, &session, device_id, device_label).await?;
            }

            let input = tokio::select! {
                event = session.next_event() => RoomSessionInput::Control(event?),
                _ = wake.notified() => RoomSessionInput::Wake,
                decision = wait_for_pending_offer_decision(&mut pending_incoming) => {
                    RoomSessionInput::PendingDecision(decision)
                }
                _ = runtime.shutdown.cancelled() => return Ok(()),
                _ = receiver_cancel.cancelled() => return Ok(()),
            };
            match input {
                RoomSessionInput::Wake => continue,
                RoomSessionInput::PendingDecision(decision) => {
                    let pending = pending_incoming
                        .take()
                        .expect("pending decision requires a staged offer");
                    let offer = pending.offer.clone();
                    drop(pending);
                    match decision {
                        Ok(AgentOfferDecision::Approve) => {
                            match receive_room_offer(
                                runtime,
                                session.as_ref(),
                                device_id,
                                device_label,
                                offer,
                                true,
                            )
                            .await
                            {
                                Ok(item) => tracing::info!(
                                    device = device_label,
                                    item = %item.id,
                                    "approved incoming room transfer received"
                                ),
                                Err(error) => tracing::warn!(
                                    device = device_label,
                                    %error,
                                    "approved incoming room transfer failed"
                                ),
                            }
                        }
                        Ok(AgentOfferDecision::Reject) | Err(_) => {
                            session
                                .reject_offer(&offer.offer_id, RoomOfferRejection::Declined)
                                .await?;
                        }
                    }
                }
                RoomSessionInput::Control(RoomControlEvent::IncomingOffer(offer)) => {
                    if receiver_cancel.is_cancelled() {
                        return Ok(());
                    }
                    if pending_incoming.is_some() {
                        session
                            .reject_offer(&offer.offer_id, RoomOfferRejection::Busy)
                            .await?;
                        continue;
                    }
                    let allocatable_bytes = match prepare_inbox_destination(runtime) {
                        Ok((_, allocatable_bytes)) => allocatable_bytes,
                        Err(error) => {
                            session
                                .reject_offer(&offer.offer_id, RoomOfferRejection::Declined)
                                .await?;
                            tracing::warn!(
                                device = device_label,
                                offer_id = %offer.offer_id,
                                %error,
                                "incoming room transfer has no available Inbox destination"
                            );
                            continue;
                        }
                    };
                    if requires_explicit_offer_approval(offer.total_bytes, allocatable_bytes) {
                        match stage_pending_offer(
                            runtime,
                            device_id,
                            device_label,
                            offer.clone(),
                            allocatable_bytes,
                        ) {
                            Ok(pending) => {
                                tracing::info!(
                                    device = device_label,
                                    offer_id = %offer.offer_id,
                                    total_bytes = offer.total_bytes,
                                    allocatable_bytes,
                                    "incoming room transfer is waiting for approval"
                                );
                                pending_incoming = Some(pending);
                            }
                            Err(error) => {
                                session
                                    .reject_offer(&offer.offer_id, RoomOfferRejection::Busy)
                                    .await?;
                                tracing::warn!(
                                    device = device_label,
                                    offer_id = %offer.offer_id,
                                    %error,
                                    "incoming room transfer could not be staged for approval"
                                );
                            }
                        }
                        continue;
                    }
                    match receive_room_offer(
                        runtime,
                        session.as_ref(),
                        device_id,
                        device_label,
                        offer,
                        false,
                    )
                    .await
                    {
                        Ok(item) => tracing::info!(
                            device = device_label,
                            item = %item.id,
                            "incoming room transfer received"
                        ),
                        Err(error) => tracing::warn!(
                            device = device_label,
                            %error,
                            "incoming room transfer failed"
                        ),
                    }
                }
                RoomSessionInput::Control(RoomControlEvent::OfferAccepted { offer_id }) => {
                    let outgoing = take_pending_outgoing(&mut pending_outgoing, &offer_id)?;
                    start_outgoing_transfer(runtime, session.clone(), outgoing)?;
                }
                RoomSessionInput::Control(RoomControlEvent::OfferRejected { offer_id, reason }) => {
                    let outgoing = take_pending_outgoing(&mut pending_outgoing, &offer_id)?;
                    let transfer_id = outgoing.transfer.id;
                    lock(&runtime.store)?
                        .reject_outgoing_transfer(&transfer_id, room_rejection(reason))?;
                    record_agent_event(
                        runtime,
                        AgentEvent::TransferChanged {
                            transfer_id: transfer_id.to_string(),
                        },
                    )?;
                }
                RoomSessionInput::Control(RoomControlEvent::PeerClosed(_)) => return Ok(()),
                RoomSessionInput::Control(
                    RoomControlEvent::LifetimeChanged(_)
                    | RoomControlEvent::Pong { .. }
                    | RoomControlEvent::VerificationSucceeded,
                ) => {}
                RoomSessionInput::Control(
                    RoomControlEvent::VerificationRequested | RoomControlEvent::VerificationFailed,
                ) => {
                    bail!("peer attempted device verification after pairing completed");
                }
            }
        }
    }
    .await;

    let cleanup = lock(&runtime.active_rooms).map(|mut rooms| {
        if rooms
            .get(device_id)
            .is_some_and(|registered| Arc::ptr_eq(registered, &wake))
        {
            rooms.remove(device_id);
        }
    });
    if let Err(error) = cleanup {
        tracing::error!(device = device_label, %error, "active room could not be unregistered");
    }
    result
}

fn take_pending_outgoing(
    pending: &mut Option<PreparedOutgoingTransfer>,
    offer_id: &str,
) -> Result<PreparedOutgoingTransfer> {
    let outgoing = pending
        .take()
        .ok_or_else(|| anyhow!("peer decided an unknown outgoing offer"))?;
    if outgoing.offer.offer_id != offer_id {
        bail!("peer decided a different outgoing offer");
    }
    Ok(outgoing)
}

fn room_rejection(reason: RoomOfferRejection) -> TransferRejection {
    match reason {
        RoomOfferRejection::Declined => TransferRejection::UserDeclined,
        RoomOfferRejection::Busy => TransferRejection::Busy,
        RoomOfferRejection::Expired | RoomOfferRejection::Invalid => {
            TransferRejection::InvalidOffer
        }
    }
}

async fn offer_next_outgoing(
    runtime: &Arc<AgentRuntime>,
    session: &RoomControlSession,
    relationship_id: &str,
    device_label: &str,
) -> Result<Option<PreparedOutgoingTransfer>> {
    let transfers = lock(&runtime.store)?.dispatchable_transfers(relationship_id)?;
    for transfer in transfers {
        if lock(&runtime.active_outgoing)?.contains_key(transfer.id.as_str()) {
            continue;
        }
        let outgoing = match prepare_outgoing_transfer(runtime, transfer).await {
            Ok(outgoing) => outgoing,
            Err(error) => {
                let transfer_id = error.0;
                lock(&runtime.store)?.fail_outgoing_transfer(
                    &transfer_id,
                    TransferFailure {
                        code: FailureCode::InternalError,
                        phase: FailurePhase::Setup,
                        retryable: true,
                        recovery_action: RecoveryAction::Retry,
                    },
                )?;
                record_agent_event(
                    runtime,
                    AgentEvent::TransferChanged {
                        transfer_id: transfer_id.to_string(),
                    },
                )?;
                tracing::warn!(
                    device = device_label,
                    transfer_id = %transfer_id,
                    error = %error.1,
                    "queued outgoing Transfer needs attention"
                );
                continue;
            }
        };
        session.offer_transfer(outgoing.offer.clone()).await?;
        tracing::info!(
            device = device_label,
            transfer_id = %outgoing.transfer.id,
            "outgoing room offer sent"
        );
        return Ok(Some(outgoing));
    }
    Ok(None)
}

async fn prepare_outgoing_transfer(
    runtime: &AgentRuntime,
    transfer: Transfer,
) -> std::result::Result<PreparedOutgoingTransfer, (TransferId, anyhow::Error)> {
    let transfer_id = transfer.id.clone();
    let result = async {
        let record = lock(&runtime.store)?
            .device_record(transfer.relationship_id.as_str())
            .ok_or_else(|| anyhow!("outgoing Transfer Relationship is unavailable"))?;
        let job_id = agent_job_id(&transfer.content_id)?;
        let job = api::TransferJobStore::new(runtime.config.state_directory.join("outbox/jobs"))
            .load(job_id)
            .await?
            .ok_or_else(|| anyhow!("outgoing Transfer content is missing"))?;
        if job.lifecycle() != api::JobLifecycle::Sealed {
            bail!("outgoing Transfer content is not sealed");
        }
        let (expected_content_id, expected_transfer_id) = agent_transfer_ids(&job)?;
        if expected_content_id != transfer.content_id || expected_transfer_id != transfer.id {
            bail!("outgoing Transfer does not match its durable content");
        }
        let manifest = job
            .manifest()
            .ok_or_else(|| anyhow!("sealed outgoing Transfer has no manifest"))?;
        if manifest.totals.total_plaintext_bytes != transfer.total_bytes {
            bail!("outgoing Transfer total differs from its sealed manifest");
        }
        let item_count = manifest
            .totals
            .file_count
            .checked_add(manifest.totals.directory_count)
            .ok_or_else(|| anyhow!("outgoing Transfer item count overflow"))?;
        let invitation = api::create_invitation(
            record.broker().to_string(),
            record.relay().map(str::to_string).into_iter().collect(),
            TransferRole::Sender,
            unix_seconds()?,
        )?;
        let offer = RoomTransferOffer {
            offer_id: transfer.id.to_string(),
            transfer_invite: invitation.payload.clone(),
            root_names: manifest
                .roots
                .iter()
                .take(3)
                .map(|root| root.requested_name.clone())
                .collect(),
            item_count,
            directory_count: manifest.totals.directory_count,
            total_bytes: manifest.totals.total_plaintext_bytes,
        };
        Ok(PreparedOutgoingTransfer {
            transfer,
            job,
            bootstrap: invitation.into_bootstrap(),
            broker: record.broker().to_string(),
            relay: record.relay().map(str::to_string),
            offer,
        })
    }
    .await;
    result.map_err(|error| (transfer_id, error))
}

fn start_outgoing_transfer(
    runtime: &Arc<AgentRuntime>,
    session: Arc<RoomControlSession>,
    outgoing: PreparedOutgoingTransfer,
) -> Result<()> {
    let transfer_id = outgoing.transfer.id.clone();
    lock(&runtime.store)?.start_outgoing_transfer(&transfer_id)?;
    record_agent_event(
        runtime,
        AgentEvent::TransferChanged {
            transfer_id: transfer_id.to_string(),
        },
    )?;
    let cancel = TransferCancelToken::new();
    {
        let mut active = lock(&runtime.active_outgoing)?;
        if active
            .values()
            .any(|value| value.relationship_id == outgoing.transfer.relationship_id.as_str())
        {
            bail!("remembered device already has an active outgoing Transfer");
        }
        active.insert(
            transfer_id.to_string(),
            ActiveOutgoingTransfer {
                relationship_id: outgoing.transfer.relationship_id.to_string(),
                cancel: cancel.clone(),
            },
        );
    }
    let task_runtime = runtime.clone();
    if let Err(error) = spawn_background_task(runtime, async move {
        run_outgoing_transfer(task_runtime, session, outgoing, cancel).await;
    }) {
        lock(&runtime.active_outgoing)?.remove(transfer_id.as_str());
        return Err(error);
    }
    Ok(())
}

async fn run_outgoing_transfer(
    runtime: Arc<AgentRuntime>,
    session: Arc<RoomControlSession>,
    outgoing: PreparedOutgoingTransfer,
    cancel: TransferCancelToken,
) {
    let transfer_id = outgoing.transfer.id.clone();
    let relationship_id = outgoing.transfer.relationship_id.to_string();
    let _path_cleanup = ActivePathCleanup::new(
        runtime.clone(),
        transfer_id.to_string(),
        TransferDirection::Send,
    );
    let events = Arc::new(AgentOutgoingEvents::new(
        runtime.clone(),
        transfer_id.clone(),
        outgoing.transfer.transferred_bytes,
        outgoing.transfer.total_bytes,
        cancel.clone(),
    ));
    let event_sink: Arc<dyn EventSink> = events.clone();

    let result = async {
        session.set_local_transfer_active(true).await?;
        let broker = api::parse_broker_addr(&outgoing.broker, outgoing.relay.as_deref())?;
        let operation = api::send_manifest_v2_via_room(
            broker,
            outgoing.bootstrap,
            &outgoing.job,
            runtime
                .config
                .state_directory
                .join("outbox/transfer-state-v2"),
            runtime.session_config(outgoing.relay.as_deref()),
            event_sink,
            &cancel,
        );
        tokio::pin!(operation);
        tokio::select! {
            result = &mut operation => result,
            _ = runtime.shutdown.cancelled() => {
                cancel.cancel();
                operation.await
            }
        }
    }
    .await;

    if let Err(error) = session.set_local_transfer_active(false).await {
        tracing::debug!(transfer_id = %transfer_id, %error, "room activity cleanup failed");
    }

    let settlement = if runtime.shutdown.is_cancelled() {
        Ok(None)
    } else if let Some(error) = events.projection_error() {
        tracing::error!(transfer_id = %transfer_id, %error, "outgoing Transfer projection failed");
        lock(&runtime.store).and_then(|mut store| {
            store
                .fail_outgoing_transfer(
                    &transfer_id,
                    TransferFailure {
                        code: FailureCode::InternalError,
                        phase: events.failure_phase(),
                        retryable: true,
                        recovery_action: RecoveryAction::Retry,
                    },
                )
                .map(Some)
                .map_err(Into::into)
        })
    } else {
        match result {
            Ok(_) => lock(&runtime.store).and_then(|mut store| {
                store
                    .complete_outgoing_transfer(&transfer_id)
                    .map(Some)
                    .map_err(Into::into)
            }),
            Err(error) => {
                let projection = envoix_client::failure::project_session_failure(
                    &error,
                    envoix_client::model::TransferDirection::Send,
                    events.failure_phase(),
                );
                lock(&runtime.store).and_then(|mut store| {
                    if projection.failure.outcome() == FailureOutcome::Canceled {
                        store
                            .cancel_outgoing_transfer(&transfer_id)
                            .map(Some)
                            .map_err(Into::into)
                    } else {
                        store
                            .fail_outgoing_transfer(&transfer_id, projection.failure)
                            .map(Some)
                            .map_err(Into::into)
                    }
                })
            }
        }
    };

    if let Ok(mut active) = runtime.active_outgoing.lock() {
        active.remove(transfer_id.as_str());
    } else {
        tracing::error!(transfer_id = %transfer_id, "active outgoing state is poisoned");
    }
    let should_wake = settlement.is_ok();
    match settlement {
        Ok(Some(transfer)) => {
            if let Err(error) = record_agent_event(
                &runtime,
                AgentEvent::TransferChanged {
                    transfer_id: transfer_id.to_string(),
                },
            ) {
                tracing::error!(transfer_id = %transfer_id, %error, "Transfer event could not be recorded");
            }
            tracing::info!(
                transfer_id = %transfer_id,
                state = transfer.state.wire_name(),
                "outgoing Transfer settled"
            );
        }
        Ok(None) => tracing::debug!(
            transfer_id = %transfer_id,
            "outgoing Transfer preserved for restart"
        ),
        Err(error) => tracing::error!(
            transfer_id = %transfer_id,
            %error,
            "outgoing Transfer settlement failed"
        ),
    }
    if should_wake && let Err(error) = wake_active_room(&runtime, &relationship_id) {
        tracing::error!(transfer_id = %transfer_id, %error, "active room could not be notified");
    }
}

async fn receive_room_offer(
    runtime: &Arc<AgentRuntime>,
    session: &RoomControlSession,
    device_id: &str,
    device_label: &str,
    offer: RoomTransferOffer,
    exceptional_transfer_approved: bool,
) -> Result<InboxItem> {
    let bootstrap = api::parse_invitation_for_role(
        &offer.transfer_invite,
        TransferRole::Receiver,
        unix_seconds()?,
    )?
    .into_bootstrap();
    let _path_cleanup = ActivePathCleanup::new(
        runtime.clone(),
        offer.offer_id.clone(),
        TransferDirection::Receive,
    );
    let receive_cancel = TransferCancelToken::new();
    let task_cancel = receive_cancel.clone();
    let task_runtime = runtime.clone();
    let task_transfer_id = offer.offer_id.clone();
    let receiver = tokio::spawn(async move {
        tokio::select! {
            result = receive_invitation_offer(
                &task_runtime,
                &task_transfer_id,
                bootstrap,
                &task_cancel,
            ) => result,
            _ = task_runtime.shutdown.cancelled() => {
                task_cancel.cancel();
                Err(anyhow!("Agent shut down while waiting for the transfer sender"))
            }
        }
    });
    tokio::task::yield_now().await;

    if let Err(error) = session.accept_offer(&offer.offer_id).await {
        receive_cancel.cancel();
        let _ = receiver.await;
        return Err(error.into());
    }
    if let Err(error) = session.set_local_transfer_active(true).await {
        receive_cancel.cancel();
        let _ = receiver.await;
        return Err(error.into());
    }
    let pending = match receiver.await.context("transfer receiver task failed") {
        Ok(Ok(pending)) => pending,
        Err(error) => {
            let _ = session.set_local_transfer_active(false).await;
            return Err(error);
        }
        Ok(Err(error)) => {
            let _ = session.set_local_transfer_active(false).await;
            return Err(error);
        }
    };
    let manifest = &pending.offer().manifest;
    let totals = &manifest.totals;
    let item_count = u64::from(totals.file_count) + u64::from(totals.directory_count);
    let expected_root_names = manifest
        .roots
        .iter()
        .take(3)
        .map(|root| root.requested_name.as_str())
        .collect::<Vec<_>>();
    let offer_matches_manifest = u64::from(offer.item_count) == item_count
        && offer.directory_count == totals.directory_count
        && offer.total_bytes == totals.total_plaintext_bytes
        && offer.root_names.len() == expected_root_names.len()
        && offer
            .root_names
            .iter()
            .map(String::as_str)
            .eq(expected_root_names);
    if !offer_matches_manifest {
        pending.reject().await;
        let _ = session.set_local_transfer_active(false).await;
        bail!("authenticated file list did not match the accepted room offer");
    }
    let result = receive_to_inbox(
        runtime,
        device_id,
        device_label,
        pending,
        exceptional_transfer_approved,
    )
    .await;
    let inactive = session.set_local_transfer_active(false).await;
    if let Err(error) = inactive {
        return Err(error.into());
    }
    result
}

async fn receive_to_inbox(
    runtime: &AgentRuntime,
    device_id: &str,
    device_label: &str,
    pending: PendingManifestV2Receive,
    exceptional_transfer_approved: bool,
) -> Result<InboxItem> {
    let (target_directory, available) = prepare_inbox_destination(runtime)?;
    let manifest = pending.offer().manifest.clone();
    let total = manifest.totals.total_plaintext_bytes;
    let exceptional = requires_explicit_offer_approval(total, available);
    if exceptional && !exceptional_transfer_approved {
        pending.reject().await;
        bail!(
            "offer requires explicit approval ({} bytes offered, {} bytes allocatable)",
            total,
            available
        );
    }
    let summary = pending
        .receive(
            DestinationRequestV2 {
                target_directory,
                copy_staging_directory: None,
                decision: DestinationDecisionV2::UseDirectSave,
                target_allocatable_bytes: Some(available),
                staging_allocatable_bytes: None,
                stable_object_identity: true,
                exceptional_transfer_approved: exceptional && exceptional_transfer_approved,
                preplanned_root_names: None,
            },
            runtime.config.state_directory.join("transfer-state-v2"),
            &runtime.shutdown,
        )
        .await?;
    if summary.saved_root_paths.len() != manifest.roots.len() {
        bail!("receiver completion did not report every saved root");
    }
    let roots = summary
        .saved_root_paths
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .context("saved Inbox root has no UTF-8 file name")?;
            Ok(InboxRoot {
                name: name.to_string(),
                path: path.display().to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let item = InboxItem {
        id: format!(
            "{}_{}",
            URL_SAFE_NO_PAD.encode(manifest.job_id.0),
            manifest.generation
        ),
        received_at_unix_ms: unix_millis()?,
        from_device_id: device_id.to_string(),
        from_device_label: device_label.to_string(),
        roots,
        file_count: manifest.totals.file_count,
        directory_count: manifest.totals.directory_count,
        total_bytes: total,
    };
    lock(&runtime.store)?.append_inbox(item.clone())?;
    record_agent_event(
        runtime,
        AgentEvent::InboxChanged {
            item_id: item.id.clone(),
        },
    )?;
    Ok(item)
}

struct AgentOutgoingEvents {
    runtime: Arc<AgentRuntime>,
    transfer_id: TransferId,
    total_bytes: u64,
    cancel: TransferCancelToken,
    projection: Mutex<OutgoingProjection>,
}

struct OutgoingProjection {
    persisted_bytes: u64,
    phase: FailurePhase,
    error: Option<String>,
}

impl AgentOutgoingEvents {
    fn new(
        runtime: Arc<AgentRuntime>,
        transfer_id: TransferId,
        persisted_bytes: u64,
        total_bytes: u64,
        cancel: TransferCancelToken,
    ) -> Self {
        Self {
            runtime,
            transfer_id,
            total_bytes,
            cancel,
            projection: Mutex::new(OutgoingProjection {
                persisted_bytes,
                phase: FailurePhase::Pairing,
                error: None,
            }),
        }
    }

    fn failure_phase(&self) -> FailurePhase {
        self.projection
            .lock()
            .map(|projection| projection.phase)
            .unwrap_or(FailurePhase::Transferring)
    }

    fn projection_error(&self) -> Option<String> {
        match self.projection.lock() {
            Ok(projection) => projection.error.clone(),
            Err(_) => Some("outgoing Transfer projection lock is poisoned".into()),
        }
    }

    fn set_phase(&self, phase: FailurePhase) {
        match self.projection.lock() {
            Ok(mut projection) if projection.error.is_none() => projection.phase = phase,
            Ok(_) => {}
            Err(_) => self.cancel.cancel(),
        }
    }

    fn fail_projection(&self, projection: &mut OutgoingProjection, message: String) {
        if projection.error.is_none() {
            projection.error = Some(message);
            self.cancel.cancel();
        }
    }

    fn project_progress(&self, bytes_transferred: u64, total_bytes: u64) {
        let mut projection = match self.projection.lock() {
            Ok(projection) => projection,
            Err(_) => {
                self.cancel.cancel();
                return;
            }
        };
        if projection.error.is_some() {
            return;
        }
        if total_bytes != self.total_bytes || bytes_transferred > total_bytes {
            self.fail_projection(
                &mut projection,
                "data-plane progress does not match the durable Transfer".into(),
            );
            return;
        }
        if bytes_transferred < projection.persisted_bytes {
            return;
        }
        if bytes_transferred != total_bytes
            && bytes_transferred.saturating_sub(projection.persisted_bytes)
                < OUTGOING_PROGRESS_CHECKPOINT_BYTES
        {
            return;
        }
        let persisted = lock(&self.runtime.store).and_then(|mut store| {
            store
                .progress_outgoing_transfer(&self.transfer_id, bytes_transferred)
                .map_err(Into::into)
        });
        if let Err(error) = persisted {
            self.fail_projection(
                &mut projection,
                format!("persist Transfer progress: {error}"),
            );
            return;
        }
        projection.persisted_bytes = bytes_transferred;
        drop(projection);
        if let Err(error) = record_agent_event(
            &self.runtime,
            AgentEvent::TransferChanged {
                transfer_id: self.transfer_id.to_string(),
            },
        ) {
            tracing::error!(transfer_id = %self.transfer_id, %error, "progress event could not be recorded");
        }
    }
}

impl EventSink for AgentOutgoingEvents {
    fn on_event(&self, event: TransferEvent) {
        match event {
            TransferEvent::Diagnostic { message } => {
                tracing::debug!(transfer_id = %self.transfer_id, %message, "outgoing transfer diagnostic");
            }
            TransferEvent::Pairing { .. } => self.set_phase(FailurePhase::Pairing),
            TransferEvent::Connecting => self.set_phase(FailurePhase::Connecting),
            TransferEvent::Connected { path } | TransferEvent::PathChanged { path } => {
                self.set_phase(FailurePhase::Connecting);
                if let Err(error) = set_active_path(
                    &self.runtime,
                    self.transfer_id.as_str(),
                    TransferDirection::Send,
                    &path,
                ) {
                    tracing::error!(
                        transfer_id = %self.transfer_id,
                        %error,
                        "outgoing transfer path could not be recorded"
                    );
                }
            }
            TransferEvent::Progress {
                bytes_transferred,
                total_bytes,
                ..
            } => self.project_progress(bytes_transferred, total_bytes),
            TransferEvent::ManifestV2Phase { phase, .. } => {
                let phase = match phase {
                    api::ManifestV2ProgressPhase::Transferring => FailurePhase::Transferring,
                    api::ManifestV2ProgressPhase::Verifying => FailurePhase::Verifying,
                    api::ManifestV2ProgressPhase::Saving
                    | api::ManifestV2ProgressPhase::WaitingForReceiverSave
                    | api::ManifestV2ProgressPhase::FinalizingDelivery => FailurePhase::Committing,
                };
                self.set_phase(phase);
            }
            TransferEvent::StageTiming { stage, .. } => {
                let phase = match stage {
                    api::TransferStage::SessionStarted => FailurePhase::Pairing,
                    api::TransferStage::ConnectionReady => FailurePhase::Connecting,
                    api::TransferStage::AuthenticationStarted
                    | api::TransferStage::AuthenticationComplete => FailurePhase::Authenticating,
                    api::TransferStage::ManifestOffer | api::TransferStage::ManifestAccepted => {
                        FailurePhase::Negotiating
                    }
                    api::TransferStage::FirstPayload => FailurePhase::Transferring,
                    api::TransferStage::PayloadComplete => FailurePhase::Verifying,
                    api::TransferStage::DeliveryComplete => FailurePhase::Committing,
                    api::TransferStage::Canceled | api::TransferStage::Failed => return,
                };
                self.set_phase(phase);
            }
        }
    }
}

struct AgentIncomingEvents {
    runtime: Arc<AgentRuntime>,
    transfer_id: String,
}

impl EventSink for AgentIncomingEvents {
    fn on_event(&self, event: TransferEvent) {
        if let TransferEvent::Connected { path } | TransferEvent::PathChanged { path } = &event
            && let Err(error) = set_active_path(
                &self.runtime,
                &self.transfer_id,
                TransferDirection::Receive,
                path,
            )
        {
            tracing::error!(
                transfer_id = %self.transfer_id,
                %error,
                "incoming transfer path could not be recorded"
            );
        }
        tracing::debug!(?event, "incoming transfer event");
    }
}

#[cfg(all(test, unix))]
struct AgentEvents;

#[cfg(all(test, unix))]
impl EventSink for AgentEvents {
    fn on_event(&self, event: TransferEvent) {
        tracing::debug!(?event, "transfer event");
    }
}

#[cfg(unix)]
struct SocketCleanup(PathBuf);

#[cfg(unix)]
impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| anyhow!("Agent state lock is poisoned"))
}

fn lock_or_log<'a, T>(mutex: &'a Mutex<T>, label: &str) -> Option<MutexGuard<'a, T>> {
    match mutex.lock() {
        Ok(guard) => Some(guard),
        Err(_) => {
            tracing::error!(label, "Agent state lock is poisoned");
            None
        }
    }
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    Ok(())
}

fn create_directory(path: &Path) -> io::Result<()> {
    let existed = path.exists();
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    if !existed {
        fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = existed;
    Ok(())
}

fn spawn_background_task<F>(runtime: &Arc<AgentRuntime>, future: F) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let mut tasks = lock(&runtime.background_tasks)?;
    if runtime.shutdown.is_cancelled() {
        bail!("Agent is shutting down");
    }
    tasks.retain(|task| !task.is_finished());
    tasks.push(tokio::spawn(future));
    Ok(())
}

async fn shutdown_background_tasks(runtime: &AgentRuntime) {
    runtime.shutdown.cancel();
    let tasks = lock_or_log(&runtime.background_tasks, "background tasks")
        .map(|mut tasks| std::mem::take(&mut *tasks))
        .unwrap_or_default();
    let deadline = tokio::time::Instant::now() + SHUTDOWN_GRACE_PERIOD;
    for mut task in tasks {
        match tokio::time::timeout_at(deadline, &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if error.is_cancelled() => {}
            Ok(Err(error)) => tracing::warn!(%error, "Agent background task failed"),
            Err(_) => {
                task.abort();
                let _ = task.await;
            }
        }
    }
}

fn unix_seconds() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs())
}

fn generate_verification_code() -> Result<String> {
    const RANGE: u32 = 1_000_000;
    const UNBIASED_LIMIT: u32 = u32::MAX - (u32::MAX % RANGE);
    loop {
        let value = getrandom::u32()
            .map_err(|error| anyhow!("generate device verification code: {error}"))?;
        if value < UNBIASED_LIMIT {
            return Ok(format!("{:06}", value % RANGE));
        }
    }
}

fn unix_millis() -> Result<u64> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before the Unix epoch")?
            .as_millis(),
    )
    .context("system clock exceeds supported range")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use envoix_client::agent_control::AgentControlClient;
    #[cfg(unix)]
    use envoix_client::ports::{PlatformPortError, SecretBytes};
    #[cfg(unix)]
    use envoix_client::storage::VaultReference;

    #[cfg(unix)]
    struct PanicVault;

    #[cfg(unix)]
    impl SecureVaultPort for PanicVault {
        fn contains(&self, _reference: &VaultReference) -> Result<bool, PlatformPortError> {
            panic!("status polling must not access the injected vault")
        }

        fn store(
            &self,
            _reference: &VaultReference,
            _secret: &SecretBytes,
        ) -> Result<(), PlatformPortError> {
            panic!("status polling must not access the injected vault")
        }

        fn load(
            &self,
            _reference: &VaultReference,
        ) -> Result<Option<SecretBytes>, PlatformPortError> {
            panic!("status polling must not access the injected vault")
        }

        fn delete(&self, _reference: &VaultReference) -> Result<(), PlatformPortError> {
            panic!("status polling must not access the injected vault")
        }
    }

    fn opaque_credential() -> Vec<u8> {
        let mut credential = b"ENVR\x01".to_vec();
        credential.extend_from_slice(&[0x42; 32]);
        credential
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn embedded_host_does_not_poll_its_injected_vault_and_shuts_down_cleanly() {
        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let control_endpoint = state_directory.join("agent.sock");
        let host = AgentHost::new(
            AgentHostConfiguration {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                control_endpoint: control_endpoint.clone(),
                device_name: "embedded-test".into(),
                broker: DEFAULT_RENDEZVOUS_BROKER.into(),
                relay: None,
            },
            api::Client::default(),
            Arc::new(PanicVault),
            AgentCredentialProtection::OwnerOnlyFile,
        );
        let shutdown = host.shutdown_handle();
        let lifecycle = host.lifecycle_handle();
        assert_eq!(lifecycle.state(), AgentHostLifecycleState::Starting);
        let running = tokio::spawn(host.run());
        let client = AgentControlClient::new(&control_endpoint);

        let response = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match client.call(AgentRequest::Diagnostics).await {
                    Ok(response) => break response,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                        ) =>
                    {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("embedded Agent did not start: {error}"),
                }
            }
        })
        .await
        .expect("embedded Agent did not become ready");
        let AgentResponse::Diagnostics { diagnostics } = response else {
            panic!("unexpected Agent response")
        };
        assert_eq!(
            diagnostics.credential_protection,
            AgentCredentialProtection::OwnerOnlyFile
        );
        assert_eq!(lifecycle.state(), AgentHostLifecycleState::Ready);
        assert!(
            ProductStore::open_with_vault(&state_directory, Arc::new(PanicVault))
                .err()
                .is_some()
        );

        shutdown.shutdown();
        assert!(matches!(
            lifecycle.state(),
            AgentHostLifecycleState::Stopping | AgentHostLifecycleState::Stopped
        ));
        tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .expect("embedded Agent did not shut down")
            .unwrap()
            .unwrap();

        assert!(!control_endpoint.exists());
        assert_eq!(lifecycle.state(), AgentHostLifecycleState::Stopped);
        ProductStore::open_with_vault(&state_directory, Arc::new(PanicVault)).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn embedded_host_reports_a_typed_startup_failure() {
        let host = AgentHost::new(
            AgentHostConfiguration {
                state_directory: "relative-state".into(),
                inbox_directory: "relative-inbox".into(),
                control_endpoint: "relative.sock".into(),
                device_name: "embedded-test".into(),
                broker: DEFAULT_RENDEZVOUS_BROKER.into(),
                relay: None,
            },
            api::Client::default(),
            Arc::new(PanicVault),
            AgentCredentialProtection::OwnerOnlyFile,
        );
        let lifecycle = host.lifecycle_handle();

        let error = host.run().await.unwrap_err();

        assert_eq!(error.code(), AgentHostErrorCode::InvalidConfiguration);
        assert!(matches!(
            lifecycle.state(),
            AgentHostLifecycleState::Failed {
                failure: AgentHostFailure {
                    code: AgentHostErrorCode::InvalidConfiguration,
                    ..
                }
            }
        ));
    }

    #[test]
    fn agent_uses_a_distinct_identity_for_each_endpoint() {
        assert_eq!(
            agent_client(None).unwrap().identity,
            IdentityConfig::Ephemeral
        );
    }

    #[test]
    fn managed_settings_are_validated_when_loaded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.json");
        fs::write(
            &path,
            br#"{"version":1,"device_name":" WSL","inbox_directory":"/tmp/inbox"}"#,
        )
        .unwrap();

        let error = read_agent_settings(&path).unwrap_err();

        assert!(error.to_string().contains("validate Agent settings"));
    }

    #[test]
    fn request_decoder_rejects_v3_with_a_typed_version_error() {
        let error = decode_request(br#"{"command":"status"}"#).unwrap_err();
        assert_eq!(error.code, "unsupported_protocol_version");
        assert!(error.message.contains("3"));
        assert!(error.message.contains(&AGENT_PROTOCOL_VERSION.to_string()));
    }

    #[test]
    fn request_decoder_rejects_v9_with_a_typed_version_error() {
        let error = decode_request(
            br#"{"protocol_version":9,"request_id":"request_test","request":{"command":"status"}}"#,
        )
        .unwrap_err();
        assert_eq!(error.request_id, "request_test");
        assert_eq!(error.code, "unsupported_protocol_version");
        assert!(error.message.contains("9"));
        assert!(error.message.contains(&AGENT_PROTOCOL_VERSION.to_string()));
    }

    #[test]
    fn request_decoder_accepts_a_valid_v12_envelope() {
        let envelope = AgentRequestEnvelope::new("request_test", AgentRequest::Status).unwrap();
        let bytes = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(decode_request(&bytes).unwrap(), envelope);
    }

    #[test]
    fn event_log_returns_ordered_bounded_batches() {
        let mut log = AgentEventLog::new().unwrap();
        let initial = log.cursor();
        log.record(AgentEvent::PairingChanged {
            label: "MacBook".into(),
            active: true,
        })
        .unwrap();
        log.record(AgentEvent::InboxChanged {
            item_id: "job_1".into(),
        })
        .unwrap();
        log.record(AgentEvent::InboxChanged {
            item_id: "job_2".into(),
        })
        .unwrap();

        let AgentEventRead::Events { cursor, events } = log.read_after(&initial, 2) else {
            panic!("current Agent cursor unexpectedly required a snapshot");
        };
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[1].sequence, 2);
        assert_eq!(cursor.sequence, 2);

        let AgentEventRead::Events {
            cursor: final_cursor,
            events,
        } = log.read_after(&cursor, 2)
        else {
            panic!("batched Agent cursor unexpectedly required a snapshot");
        };
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 3);
        assert_eq!(final_cursor, log.cursor());
    }

    #[test]
    fn event_log_requires_a_snapshot_after_restart_or_retention_gap() {
        let mut log = AgentEventLog::new().unwrap();
        let initial = log.cursor();
        let restarted = AgentEventCursor {
            instance_id: "agent_restarted".into(),
            sequence: 0,
        };
        assert!(matches!(
            log.read_after(&restarted, 1),
            AgentEventRead::SnapshotRequired(_)
        ));

        for index in 0..=MAX_RETAINED_AGENT_EVENTS {
            log.record(AgentEvent::InboxChanged {
                item_id: format!("job_{index}"),
            })
            .unwrap();
        }
        assert!(matches!(
            log.read_after(&initial, 1),
            AgentEventRead::SnapshotRequired(_)
        ));
        let future = AgentEventCursor {
            instance_id: log.instance_id.clone(),
            sequence: log.sequence + 1,
        };
        assert!(matches!(
            log.read_after(&future, 1),
            AgentEventRead::SnapshotRequired(_)
        ));
    }

    #[tokio::test]
    async fn empty_runtime_reports_status_and_empty_inbox() {
        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let runtime = Arc::new(AgentRuntime {
            config: AgentHostConfiguration {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                control_endpoint: state_directory.join("agent.sock"),
                device_name: "test-wsl".into(),
                broker: "broker".into(),
                relay: None,
            },
            client: api::Client::default(),
            store: Mutex::new(ProductStore::open(&state_directory).unwrap()),
            credential_protection: desktop_credential_protection(),
            active_receivers: Mutex::new(HashMap::new()),
            active_rooms: Mutex::new(HashMap::new()),
            active_outgoing: Mutex::new(HashMap::new()),
            active_pairings: Mutex::new(HashSet::new()),
            active_paths: Mutex::new(Vec::new()),
            pending_offers: Mutex::new(HashMap::new()),
            events: Mutex::new(AgentEventLog::new().unwrap()),
            shutdown: TransferCancelToken::new(),
            background_tasks: Mutex::new(Vec::new()),
        });

        let response = handle_request(runtime.clone(), AgentRequest::Status).await;
        let AgentResponse::Status { status } = response else {
            panic!("unexpected response")
        };
        assert_eq!(status.device_name, "test-wsl");
        assert_eq!(status.protocol_version, AGENT_PROTOCOL_VERSION);
        assert_eq!(status.paired_devices, 0);
        assert_eq!(status.active_paths, 0);
        assert_eq!(status.pending_offers, 0);
        let AgentResponse::Snapshot { snapshot } =
            handle_request(runtime.clone(), AgentRequest::Snapshot { inbox_limit: 20 }).await
        else {
            panic!("unexpected response")
        };
        assert_eq!(snapshot.engine.contract_version, 6);
        assert!(snapshot.inbox.is_empty());
        assert!(snapshot.active_paths.is_empty());
        assert!(snapshot.pending_offers.is_empty());
        assert_eq!(snapshot.event_cursor.sequence, 0);
        snapshot.event_cursor.validate().unwrap();
        let AgentResponse::Diagnostics { diagnostics } =
            handle_request(runtime.clone(), AgentRequest::Diagnostics).await
        else {
            panic!("unexpected response")
        };
        assert_eq!(diagnostics.agent_protocol_version, AGENT_PROTOCOL_VERSION);
        assert_eq!(diagnostics.engine_sequence, 0);
        assert_eq!(diagnostics.transfers, 0);
        assert_eq!(diagnostics.active_paths, 0);
        assert_eq!(diagnostics.pending_offers, 0);

        let path_cursor = lock(&runtime.events).unwrap().cursor();
        set_active_path(
            &runtime,
            "transfer_path_fixture",
            TransferDirection::Send,
            &DataPath::Direct {
                addr: "192.168.1.20:4433".parse().unwrap(),
            },
        )
        .unwrap();
        set_active_path(
            &runtime,
            "transfer_path_fixture",
            TransferDirection::Send,
            &DataPath::Direct {
                addr: "10.0.0.20:4433".parse().unwrap(),
            },
        )
        .unwrap();
        let AgentResponse::TransferPaths { paths } =
            handle_request(runtime.clone(), AgentRequest::ListTransferPaths).await
        else {
            panic!("unexpected response")
        };
        assert_eq!(
            paths,
            vec![AgentTransferPath {
                transfer_id: "transfer_path_fixture".into(),
                direction: TransferDirection::Send,
                path: AgentPathKind::Lan,
            }]
        );
        assert_eq!(runtime.status().unwrap().active_paths, 1);

        set_active_path(
            &runtime,
            "transfer_path_fixture",
            TransferDirection::Send,
            &DataPath::Relay {
                url: "https://relay.fixture.invalid".into(),
            },
        )
        .unwrap();
        drop(ActivePathCleanup::new(
            runtime.clone(),
            "transfer_path_fixture",
            TransferDirection::Send,
        ));
        assert!(runtime.active_paths().unwrap().is_empty());
        let AgentEventRead::Events { events, .. } =
            lock(&runtime.events).unwrap().read_after(&path_cursor, 10)
        else {
            panic!("path events unexpectedly required a snapshot")
        };
        assert!(matches!(
            events.as_slice(),
            [
                AgentEventEnvelope {
                    event: AgentEvent::TransferPathChanged {
                        path: Some(AgentPathKind::Lan),
                        ..
                    },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::TransferPathChanged {
                        path: Some(AgentPathKind::Relay),
                        ..
                    },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::TransferPathChanged { path: None, .. },
                    ..
                }
            ]
        ));
        assert_eq!(
            handle_request(runtime, AgentRequest::LatestInbox).await,
            AgentResponse::Latest { item: None }
        );
    }

    #[test]
    fn exceptional_offer_policy_preserves_both_size_boundaries() {
        assert!(!requires_explicit_offer_approval(
            api::AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES,
            u64::MAX
        ));
        assert!(requires_explicit_offer_approval(
            api::AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES + 1,
            u64::MAX
        ));
        assert!(!requires_explicit_offer_approval(50, 100));
        assert!(requires_explicit_offer_approval(51, 100));
    }

    #[test]
    fn path_projection_distinguishes_lan_tailnet_direct_and_relay() {
        for address in ["192.168.1.2:4433", "10.0.0.2:4433", "[fd00::2]:4433"] {
            assert_eq!(
                project_agent_path(&DataPath::Direct {
                    addr: address.parse().unwrap(),
                }),
                AgentPathKind::Lan
            );
        }
        for address in [
            "100.64.0.2:4433",
            "100.127.255.254:4433",
            "[fd7a:115c:a1e0::2]:4433",
            "203.0.113.2:4433",
        ] {
            assert_eq!(
                project_agent_path(&DataPath::Direct {
                    addr: address.parse().unwrap(),
                }),
                AgentPathKind::Direct
            );
        }
        assert_eq!(
            project_agent_path(&DataPath::Relay {
                url: "https://relay.fixture.invalid".into(),
            }),
            AgentPathKind::Relay
        );
        assert_eq!(
            project_agent_path(&DataPath::WifiAware),
            AgentPathKind::WifiAware
        );
        assert_eq!(
            project_agent_path(&DataPath::Other {
                description: "future transport".into(),
            }),
            AgentPathKind::Other
        );
    }

    #[tokio::test]
    async fn pending_offer_is_secret_free_and_requires_one_owner_decision() {
        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let runtime = Arc::new(AgentRuntime {
            config: AgentHostConfiguration {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                control_endpoint: state_directory.join("agent.sock"),
                device_name: "test-wsl".into(),
                broker: "broker".into(),
                relay: None,
            },
            client: api::Client::default(),
            store: Mutex::new(ProductStore::open(&state_directory).unwrap()),
            credential_protection: desktop_credential_protection(),
            active_receivers: Mutex::new(HashMap::new()),
            active_rooms: Mutex::new(HashMap::new()),
            active_outgoing: Mutex::new(HashMap::new()),
            active_pairings: Mutex::new(HashSet::new()),
            active_paths: Mutex::new(Vec::new()),
            pending_offers: Mutex::new(HashMap::new()),
            events: Mutex::new(AgentEventLog::new().unwrap()),
            shutdown: TransferCancelToken::new(),
            background_tasks: Mutex::new(Vec::new()),
        });
        let initial_cursor = lock(&runtime.events).unwrap().cursor();
        let offer = RoomTransferOffer {
            offer_id: "offer_fixture".into(),
            transfer_invite: "secret-invitation-must-not-cross-control".into(),
            root_names: vec!["archive.bin".into()],
            item_count: 1,
            directory_count: 0,
            total_bytes: 51,
        };

        let mut pending =
            stage_pending_offer(&runtime, "relationship_fixture", "Fixture Mac", offer, 100)
                .unwrap();
        let AgentResponse::PendingOffers { offers } =
            handle_request(runtime.clone(), AgentRequest::ListPendingOffers).await
        else {
            panic!("unexpected response")
        };
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].offer_id, "offer_fixture");
        assert_eq!(offers[0].allocatable_bytes, 100);
        assert!(
            !serde_json::to_string(&offers)
                .unwrap()
                .contains("secret-invitation")
        );
        assert_eq!(runtime.status().unwrap().pending_offers, 1);
        assert_eq!(runtime.snapshot(20).unwrap().pending_offers, offers);

        let response = handle_request(
            runtime.clone(),
            AgentRequest::DecidePendingOffer {
                offer_id: "offer_fixture".into(),
                decision: AgentOfferDecision::Approve,
            },
        )
        .await;
        assert!(matches!(
            response,
            AgentResponse::PendingOfferDecided {
                decision: AgentOfferDecision::Approve,
                ..
            }
        ));
        assert_eq!(
            (&mut pending.decision).await.unwrap(),
            AgentOfferDecision::Approve
        );
        assert!(runtime.pending_offers().unwrap().is_empty());
        assert!(matches!(
            handle_request(
                runtime.clone(),
                AgentRequest::DecidePendingOffer {
                    offer_id: "offer_fixture".into(),
                    decision: AgentOfferDecision::Reject,
                },
            )
            .await,
            AgentResponse::Error { .. }
        ));

        drop(pending);
        assert!(lock(&runtime.pending_offers).unwrap().is_empty());
        let AgentEventRead::Events { events, .. } = lock(&runtime.events)
            .unwrap()
            .read_after(&initial_cursor, 10)
        else {
            panic!("pending-offer events unexpectedly required a snapshot")
        };
        assert!(matches!(
            events.as_slice(),
            [
                AgentEventEnvelope {
                    event: AgentEvent::PendingOfferChanged { pending: true, .. },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::PendingOfferChanged { pending: false, .. },
                    ..
                }
            ]
        ));
    }

    #[tokio::test]
    async fn creating_an_agent_transfer_seals_content_before_persisting_queued_state() {
        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let source = directory.path().join("hello.txt");
        tokio::fs::write(&source, b"hello from Agent")
            .await
            .unwrap();
        let mut store = ProductStore::open(&state_directory).unwrap();
        let pending = store
            .prepare_device("MacBook", DEFAULT_RENDEZVOUS_BROKER, None)
            .unwrap();
        let relationship_id = pending.id().to_string();
        store
            .commit_device(pending, &opaque_credential(), 0)
            .unwrap();
        let shutdown = TransferCancelToken::new();
        shutdown.cancel();
        let runtime = Arc::new(AgentRuntime {
            config: AgentHostConfiguration {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                control_endpoint: state_directory.join("agent.sock"),
                device_name: "test-agent".into(),
                broker: DEFAULT_RENDEZVOUS_BROKER.into(),
                relay: None,
            },
            client: api::Client::default(),
            store: Mutex::new(store),
            credential_protection: desktop_credential_protection(),
            active_receivers: Mutex::new(HashMap::new()),
            active_rooms: Mutex::new(HashMap::new()),
            active_outgoing: Mutex::new(HashMap::new()),
            active_pairings: Mutex::new(HashSet::new()),
            active_paths: Mutex::new(Vec::new()),
            pending_offers: Mutex::new(HashMap::new()),
            events: Mutex::new(AgentEventLog::new().unwrap()),
            shutdown,
            background_tasks: Mutex::new(Vec::new()),
        });

        assert_eq!(
            remembered_connection_role(&lock(&runtime.store).unwrap(), &relationship_id).unwrap(),
            RememberedRoomControlRole::Responder
        );

        assert!(matches!(
            handle_request(
                runtime.clone(),
                AgentRequest::CreateTransfer {
                    device: "MacBook".into(),
                    paths: vec![PathBuf::from("relative.txt")],
                },
            )
            .await,
            AgentResponse::Error { code, .. } if code == "operation_failed"
        ));

        let response = handle_request(
            runtime.clone(),
            AgentRequest::CreateTransfer {
                device: "MacBook".into(),
                paths: vec![source],
            },
        )
        .await;
        let AgentResponse::TransferCreated { transfer } = response else {
            panic!("unexpected response: {response:?}")
        };
        assert_eq!(transfer.relationship_id.as_str(), relationship_id);
        assert_eq!(transfer.state, envoix_client::model::TransferState::Queued);
        assert_eq!(transfer.total_bytes, 16);
        assert_eq!(
            remembered_connection_role(&lock(&runtime.store).unwrap(), &relationship_id).unwrap(),
            RememberedRoomControlRole::Connector
        );
        assert_eq!(
            lock(&runtime.store)
                .unwrap()
                .dispatchable_transfers(&relationship_id)
                .unwrap(),
            vec![transfer.clone()]
        );
        assert!(matches!(
            handle_request(runtime.clone(), AgentRequest::ListTransfers).await,
            AgentResponse::Transfers { transfers } if transfers == vec![transfer.clone()]
        ));
        {
            let events = lock(&runtime.events).unwrap();
            assert!(matches!(
                events.events.back().map(|event| &event.event),
                Some(AgentEvent::TransferChanged { transfer_id }) if transfer_id == transfer.id.as_str()
            ));
        }
        let outgoing = prepare_outgoing_transfer(&runtime, transfer.clone())
            .await
            .unwrap();
        assert_eq!(outgoing.offer.root_names, vec!["hello.txt"]);
        assert_eq!(outgoing.offer.item_count, 1);
        assert_eq!(outgoing.offer.total_bytes, 16);
        assert_eq!(outgoing.bootstrap.local_role(), TransferRole::Sender);
        api::parse_invitation_for_role(
            &outgoing.offer.transfer_invite,
            TransferRole::Receiver,
            unix_seconds().unwrap(),
        )
        .unwrap();
        assert_eq!(
            agent_job_id(&transfer.content_id).unwrap(),
            outgoing.job.job_id()
        );
        assert!(agent_job_id(&ContentId::parse("content_not_base64").unwrap()).is_err());
        drop(runtime);

        assert_eq!(
            api::TransferJobStore::new(state_directory.join("outbox/jobs"))
                .load_all()
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            ProductStore::open(&state_directory)
                .unwrap()
                .transfer(transfer.id.as_str())
                .unwrap()
                .unwrap()
                .state,
            envoix_client::model::TransferState::Queued
        );
    }

    #[test]
    fn outgoing_progress_is_coalesced_until_a_checkpoint_or_completion() {
        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let mut store = ProductStore::open(&state_directory).unwrap();
        let pending = store.prepare_device("MacBook", "broker", None).unwrap();
        store
            .commit_device(pending, &opaque_credential(), 0)
            .unwrap();
        let transfer_id = TransferId::parse("transfer_progress").unwrap();
        store
            .create_transfer(
                "MacBook",
                transfer_id.clone(),
                ContentId::parse("content_progress").unwrap(),
                16,
            )
            .unwrap();
        store.start_outgoing_transfer(&transfer_id).unwrap();
        let initial_sequence = store.engine_snapshot().last_sequence;
        let runtime = Arc::new(AgentRuntime {
            config: AgentHostConfiguration {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                control_endpoint: state_directory.join("agent.sock"),
                device_name: "test-agent".into(),
                broker: "broker".into(),
                relay: None,
            },
            client: api::Client::default(),
            store: Mutex::new(store),
            credential_protection: desktop_credential_protection(),
            active_receivers: Mutex::new(HashMap::new()),
            active_rooms: Mutex::new(HashMap::new()),
            active_outgoing: Mutex::new(HashMap::new()),
            active_pairings: Mutex::new(HashSet::new()),
            active_paths: Mutex::new(Vec::new()),
            pending_offers: Mutex::new(HashMap::new()),
            events: Mutex::new(AgentEventLog::new().unwrap()),
            shutdown: TransferCancelToken::new(),
            background_tasks: Mutex::new(Vec::new()),
        });
        let events = AgentOutgoingEvents::new(
            runtime.clone(),
            transfer_id.clone(),
            0,
            16,
            TransferCancelToken::new(),
        );

        events.project_progress(1, 16);
        assert_eq!(
            lock(&runtime.store)
                .unwrap()
                .engine_snapshot()
                .last_sequence,
            initial_sequence
        );
        events.project_progress(16, 16);
        let transfer = lock(&runtime.store)
            .unwrap()
            .transfer(transfer_id.as_str())
            .unwrap()
            .unwrap();
        assert_eq!(transfer.transferred_bytes, 16);
        assert_eq!(
            transfer.state,
            envoix_client::model::TransferState::Transferring
        );
        assert_eq!(
            lock(&runtime.store)
                .unwrap()
                .engine_snapshot()
                .last_sequence,
            initial_sequence + 1
        );
        events.project_progress(8, 16);
        assert!(events.projection_error().is_none());
        assert_eq!(
            lock(&runtime.store)
                .unwrap()
                .transfer(transfer_id.as_str())
                .unwrap()
                .unwrap()
                .transferred_bytes,
            16
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn remembered_room_dispatches_a_queued_transfer_and_gates_exceptional_offers() {
        use envoix_rendezvous::RoomRegistry;
        use envoix_rendezvous_iroh::{build_endpoint, endpoint_addr, serve_endpoint};
        use iroh::{RelayMode, SecretKey};

        match std::net::UdpSocket::bind(("127.0.0.1", 0)) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping Agent Transfer loopback: UDP bind denied ({error})");
                return;
            }
            Err(error) => panic!("Agent Transfer loopback pre-check failed: {error}"),
        }
        let broker = build_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            SecretKey::generate(),
            RelayMode::Disabled,
        )
        .await
        .unwrap();
        let broker_address = endpoint_addr(&broker);
        let broker_socket = *broker_address.ip_addrs().next().unwrap();
        let broker_text = format!("{}@{broker_socket}", broker_address.id);
        let registry = Arc::new(RoomRegistry::new());
        let broker_task = tokio::spawn(serve_endpoint(broker.clone(), registry.clone(), None));

        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let source = directory.path().join("hello.txt");
        tokio::fs::write(&source, b"hello from Agent")
            .await
            .unwrap();
        let credential = opaque_credential();
        let mut store = ProductStore::open(&state_directory).unwrap();
        let pending = store.prepare_device("MacBook", &broker_text, None).unwrap();
        let relationship_id = pending.id().to_string();
        store.commit_device(pending, &credential, 0).unwrap();
        let shutdown = TransferCancelToken::new();
        shutdown.cancel();
        let runtime = Arc::new(AgentRuntime {
            config: AgentHostConfiguration {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                control_endpoint: state_directory.join("agent.sock"),
                device_name: "test-agent".into(),
                broker: broker_text.clone(),
                relay: None,
            },
            client: api::Client::default(),
            store: Mutex::new(store),
            credential_protection: desktop_credential_protection(),
            active_receivers: Mutex::new(HashMap::new()),
            active_rooms: Mutex::new(HashMap::new()),
            active_outgoing: Mutex::new(HashMap::new()),
            active_pairings: Mutex::new(HashSet::new()),
            active_paths: Mutex::new(Vec::new()),
            pending_offers: Mutex::new(HashMap::new()),
            events: Mutex::new(AgentEventLog::new().unwrap()),
            shutdown,
            background_tasks: Mutex::new(Vec::new()),
        });
        let AgentResponse::TransferCreated { transfer } = handle_request(
            runtime.clone(),
            AgentRequest::CreateTransfer {
                device: "MacBook".into(),
                paths: vec![source],
            },
        )
        .await
        else {
            panic!("Agent did not create the loopback Transfer")
        };
        {
            let mut store = lock(&runtime.store).unwrap();
            store.start_outgoing_transfer(&transfer.id).unwrap();
            store.progress_outgoing_transfer(&transfer.id, 8).unwrap();
        }
        drop(runtime);
        let runtime = Arc::new(AgentRuntime {
            config: AgentHostConfiguration {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                control_endpoint: state_directory.join("agent.sock"),
                device_name: "test-agent".into(),
                broker: broker_text.clone(),
                relay: None,
            },
            client: api::Client::default(),
            store: Mutex::new(ProductStore::open(&state_directory).unwrap()),
            credential_protection: desktop_credential_protection(),
            active_receivers: Mutex::new(HashMap::new()),
            active_rooms: Mutex::new(HashMap::new()),
            active_outgoing: Mutex::new(HashMap::new()),
            active_pairings: Mutex::new(HashSet::new()),
            active_paths: Mutex::new(Vec::new()),
            pending_offers: Mutex::new(HashMap::new()),
            events: Mutex::new(AgentEventLog::new().unwrap()),
            shutdown: TransferCancelToken::new(),
            background_tasks: Mutex::new(Vec::new()),
        });
        assert_eq!(
            lock(&runtime.store)
                .unwrap()
                .transfer(transfer.id.as_str())
                .unwrap()
                .unwrap()
                .state,
            envoix_client::model::TransferState::Transferring
        );

        let responder_broker = broker_text.clone();
        let responder_config = runtime.session_config(None);
        let responder_credential = RememberedCredential::from_opaque(&credential)
            .unwrap()
            .derive_session(0);
        let responder_connect = tokio::spawn(async move {
            let cancel = TransferCancelToken::new();
            api::connect_remembered_room_control(
                responder_credential,
                responder_broker,
                None,
                "Agent".into(),
                RememberedRoomControlRole::Responder,
                responder_config,
                &cancel,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while registry.metrics_snapshot().waiting_creators == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let connector_credential = RememberedCredential::from_opaque(&credential)
            .unwrap()
            .derive_session(0);
        let connector_cancel = TransferCancelToken::new();
        let connector = api::connect_remembered_room_control(
            connector_credential,
            broker_text.clone(),
            None,
            "MacBook".into(),
            RememberedRoomControlRole::Connector,
            runtime.session_config(None),
            &connector_cancel,
        )
        .await
        .unwrap();
        let responder = Arc::new(responder_connect.await.unwrap().unwrap());

        let room_cancel = TransferCancelToken::new();
        let room_runtime = runtime.clone();
        let room_session = responder.clone();
        let room_relationship_id = relationship_id.clone();
        let room_task = tokio::spawn(async move {
            run_room_session(
                &room_runtime,
                room_session,
                &room_relationship_id,
                "MacBook",
                &room_cancel,
            )
            .await
        });
        let offer = tokio::time::timeout(Duration::from_secs(10), connector.next_event())
            .await
            .unwrap()
            .unwrap();
        let RoomControlEvent::IncomingOffer(offer) = offer else {
            panic!("MacBook did not receive the Agent Transfer offer")
        };
        assert_eq!(offer.offer_id, transfer.id.as_str());
        let bootstrap = api::parse_invitation_for_role(
            &offer.transfer_invite,
            TransferRole::Receiver,
            unix_seconds().unwrap(),
        )
        .unwrap()
        .into_bootstrap();
        let receiver_cancel = TransferCancelToken::new();
        let receiver_task_cancel = receiver_cancel.clone();
        let receiver_broker = api::parse_broker_addr(&broker_text, None).unwrap();
        let receiver_config = runtime.session_config(None);
        let receiver_task = tokio::spawn(async move {
            api::receive_manifest_v2_offer_via_room(
                receiver_broker,
                bootstrap,
                envoix_client::BindAddrs::dual_stack(0),
                receiver_config,
                Arc::new(AgentEvents),
                &receiver_task_cancel,
            )
            .await
        });
        tokio::task::yield_now().await;
        connector.accept_offer(&offer.offer_id).await.unwrap();
        let pending = tokio::time::timeout(Duration::from_secs(15), receiver_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let target_directory = directory.path().join("received");
        tokio::fs::create_dir_all(&target_directory).await.unwrap();
        let target_directory = tokio::fs::canonicalize(target_directory).await.unwrap();
        let available = api::local_allocatable_bytes(&target_directory).unwrap();
        let summary = tokio::time::timeout(
            Duration::from_secs(15),
            pending.receive(
                DestinationRequestV2 {
                    target_directory,
                    copy_staging_directory: None,
                    decision: DestinationDecisionV2::UseDirectSave,
                    target_allocatable_bytes: Some(available),
                    staging_allocatable_bytes: None,
                    stable_object_identity: true,
                    exceptional_transfer_approved: false,
                    preplanned_root_names: None,
                },
                directory.path().join("receiver-state"),
                &receiver_cancel,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(summary.saved_root_paths.len(), 1);
        assert_eq!(
            tokio::fs::read(&summary.saved_root_paths[0]).await.unwrap(),
            b"hello from Agent"
        );

        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let state = lock(&runtime.store)
                    .unwrap()
                    .transfer(transfer.id.as_str())
                    .unwrap()
                    .unwrap()
                    .state;
                if state == envoix_client::model::TransferState::Delivered {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(lock(&runtime.active_outgoing).unwrap().is_empty());

        let invitation = api::create_invitation(
            broker_text.clone(),
            Vec::new(),
            TransferRole::Sender,
            unix_seconds().unwrap(),
        )
        .unwrap();
        connector
            .offer_transfer(RoomTransferOffer {
                offer_id: "offer_requires_approval".into(),
                transfer_invite: invitation.payload,
                root_names: vec!["large.bin".into()],
                item_count: 1,
                directory_count: 0,
                total_bytes: api::AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES + 1,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while runtime.pending_offers().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let premature_decision = tokio::time::timeout(Duration::from_millis(100), async {
            loop {
                let event = connector.next_event().await.unwrap();
                if matches!(
                    event,
                    RoomControlEvent::OfferAccepted { .. } | RoomControlEvent::OfferRejected { .. }
                ) {
                    break event;
                }
            }
        })
        .await;
        assert!(premature_decision.is_err());

        assert!(matches!(
            handle_request(
                runtime.clone(),
                AgentRequest::DecidePendingOffer {
                    offer_id: "offer_requires_approval".into(),
                    decision: AgentOfferDecision::Reject,
                },
            )
            .await,
            AgentResponse::PendingOfferDecided {
                decision: AgentOfferDecision::Reject,
                ..
            }
        ));
        let rejection = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = connector.next_event().await.unwrap();
                if matches!(event, RoomControlEvent::OfferRejected { .. }) {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            rejection,
            RoomControlEvent::OfferRejected {
                offer_id,
                reason: RoomOfferRejection::Declined,
            } if offer_id == "offer_requires_approval"
        ));
        assert!(lock(&runtime.store).unwrap().inbox(usize::MAX).is_empty());

        runtime.shutdown.cancel();
        connector.shutdown().await;
        responder.shutdown().await;
        tokio::time::timeout(Duration::from_secs(5), room_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        shutdown_background_tasks(&runtime).await;
        broker.close().await;
        broker_task.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_socket_round_trips_a_v12_envelope() {
        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let runtime = Arc::new(AgentRuntime {
            config: AgentHostConfiguration {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                control_endpoint: state_directory.join("agent.sock"),
                device_name: "test-wsl".into(),
                broker: "broker".into(),
                relay: None,
            },
            client: api::Client::default(),
            store: Mutex::new(ProductStore::open(&state_directory).unwrap()),
            credential_protection: desktop_credential_protection(),
            active_receivers: Mutex::new(HashMap::new()),
            active_rooms: Mutex::new(HashMap::new()),
            active_outgoing: Mutex::new(HashMap::new()),
            active_pairings: Mutex::new(HashSet::new()),
            active_paths: Mutex::new(Vec::new()),
            pending_offers: Mutex::new(HashMap::new()),
            events: Mutex::new(AgentEventLog::new().unwrap()),
            shutdown: TransferCancelToken::new(),
            background_tasks: Mutex::new(Vec::new()),
        });
        let (client, server) = UnixStream::pair().unwrap();
        let server_task = tokio::spawn(serve_connection(runtime, server));
        let (read, mut write) = client.into_split();
        let request = AgentRequestEnvelope::new("request_test", AgentRequest::Status).unwrap();
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        write.write_all(&bytes).await.unwrap();
        write.shutdown().await.unwrap();
        let mut line = String::new();
        BufReader::new(read).read_line(&mut line).await.unwrap();
        server_task.await.unwrap().unwrap();

        let response: AgentResponseEnvelope = serde_json::from_str(&line).unwrap();
        response.validate_for("request_test").unwrap();
        assert!(matches!(response.response, AgentResponse::Status { .. }));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_named_pipe_round_trips_a_v12_envelope_for_its_owner() {
        use tokio::net::windows::named_pipe::ClientOptions;

        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let endpoint = format!(
            r"\\.\pipe\envoix-agent-test-{}-{}",
            std::process::id(),
            getrandom::u32().unwrap()
        );
        let runtime = Arc::new(AgentRuntime {
            config: AgentHostConfiguration {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                control_endpoint: PathBuf::from(&endpoint),
                device_name: "test-windows".into(),
                broker: "broker".into(),
                relay: None,
            },
            client: api::Client::default(),
            store: Mutex::new(ProductStore::open(&state_directory).unwrap()),
            credential_protection: desktop_credential_protection(),
            active_receivers: Mutex::new(HashMap::new()),
            active_rooms: Mutex::new(HashMap::new()),
            active_outgoing: Mutex::new(HashMap::new()),
            active_pairings: Mutex::new(HashSet::new()),
            active_paths: Mutex::new(Vec::new()),
            pending_offers: Mutex::new(HashMap::new()),
            events: Mutex::new(AgentEventLog::new().unwrap()),
            shutdown: TransferCancelToken::new(),
            background_tasks: Mutex::new(Vec::new()),
        });
        let owner_sid = current_windows_user_sid().unwrap();
        let server = create_windows_pipe(&endpoint, &owner_sid, true).unwrap();
        assert!(create_windows_pipe(&endpoint, &owner_sid, true).is_err());
        let client = ClientOptions::new().open(&endpoint).unwrap();
        server.connect().await.unwrap();
        validate_windows_peer(&server, &owner_sid).unwrap();

        let server_task = tokio::spawn(serve_connection(runtime, server));
        let (read, mut write) = tokio::io::split(client);
        let request = AgentRequestEnvelope::new("request_test", AgentRequest::Status).unwrap();
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        write.write_all(&bytes).await.unwrap();
        write.shutdown().await.unwrap();
        let mut line = String::new();
        BufReader::new(read).read_line(&mut line).await.unwrap();
        server_task.await.unwrap().unwrap();

        let response: AgentResponseEnvelope = serde_json::from_str(&line).unwrap();
        response.validate_for("request_test").unwrap();
        assert!(matches!(response.response, AgentResponse::Status { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_peer_identity_must_match_the_socket_owner() {
        let (client, _server) = UnixStream::pair().unwrap();
        let peer_uid = client.peer_cred().unwrap().uid();
        validate_unix_peer(&client, peer_uid).unwrap();

        let different_uid = if peer_uid == u32::MAX {
            peer_uid - 1
        } else {
            peer_uid + 1
        };
        let error = validate_unix_peer(&client, different_uid).unwrap_err();
        assert!(error.to_string().contains("does not match socket owner"));
    }

    #[tokio::test]
    async fn forgetting_device_cancels_its_remembered_receiver() {
        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let mut store = ProductStore::open(&state_directory).unwrap();
        let pending = store.prepare_device("MacBook", "broker", None).unwrap();
        let device_id = pending.id().to_string();
        store
            .commit_device(pending, &opaque_credential(), 0)
            .unwrap();
        let queued_transfer_id = TransferId::parse("transfer_queued_before_revoke").unwrap();
        store
            .create_transfer(
                &device_id,
                queued_transfer_id.clone(),
                ContentId::parse("content_queued_before_revoke").unwrap(),
                42,
            )
            .unwrap();
        let receiver_cancel = TransferCancelToken::new();
        let outgoing_cancel = TransferCancelToken::new();
        let runtime = Arc::new(AgentRuntime {
            config: AgentHostConfiguration {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                control_endpoint: state_directory.join("agent.sock"),
                device_name: "test-wsl".into(),
                broker: "broker".into(),
                relay: None,
            },
            client: api::Client::default(),
            store: Mutex::new(store),
            credential_protection: desktop_credential_protection(),
            active_receivers: Mutex::new(HashMap::from([(
                device_id.clone(),
                receiver_cancel.clone(),
            )])),
            active_rooms: Mutex::new(HashMap::new()),
            active_outgoing: Mutex::new(HashMap::from([(
                "transfer_test".into(),
                ActiveOutgoingTransfer {
                    relationship_id: device_id.clone(),
                    cancel: outgoing_cancel.clone(),
                },
            )])),
            active_pairings: Mutex::new(HashSet::new()),
            active_paths: Mutex::new(Vec::new()),
            pending_offers: Mutex::new(HashMap::new()),
            events: Mutex::new(AgentEventLog::new().unwrap()),
            shutdown: TransferCancelToken::new(),
            background_tasks: Mutex::new(Vec::new()),
        });

        let response = handle_request(
            runtime.clone(),
            AgentRequest::RevokeDevice {
                device: "macbook".into(),
            },
        )
        .await;

        let AgentResponse::DeviceRevoked { device } = response else {
            panic!("unexpected response")
        };
        assert_eq!(device.id, device_id);
        assert!(receiver_cancel.is_cancelled());
        assert!(outgoing_cancel.is_cancelled());
        assert!(lock(&runtime.store).unwrap().devices().is_empty());
        assert_eq!(
            lock(&runtime.store)
                .unwrap()
                .transfer(queued_transfer_id.as_str())
                .unwrap()
                .unwrap()
                .state,
            envoix_client::model::TransferState::Canceled
        );
        let events = lock(&runtime.events).unwrap();
        assert!(matches!(
            events.events.back(),
            Some(AgentEventEnvelope {
                event: AgentEvent::RelationshipChanged {
                    relationship_id,
                    change: AgentRelationshipChange::Revoked,
                },
                ..
            }) if relationship_id == &device_id
        ));
    }

    #[tokio::test]
    async fn route_update_is_persisted_and_emits_a_typed_event() {
        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let mut store = ProductStore::open(&state_directory).unwrap();
        let old_broker = format!(
            "{}@127.0.0.1:8555",
            DEFAULT_RENDEZVOUS_BROKER.split('@').next().unwrap()
        );
        let pending = store
            .prepare_device(
                "MacBook",
                &old_broker,
                Some("https://old-relay.example.test"),
            )
            .unwrap();
        let device_id = pending.id().to_string();
        store
            .commit_device(pending, &opaque_credential(), 0)
            .unwrap();
        let shutdown = TransferCancelToken::new();
        shutdown.cancel();
        let runtime = Arc::new(AgentRuntime {
            config: AgentHostConfiguration {
                state_directory: state_directory.clone(),
                inbox_directory: state_directory.join("inbox"),
                control_endpoint: state_directory.join("agent.sock"),
                device_name: "test-wsl".into(),
                broker: DEFAULT_RENDEZVOUS_BROKER.into(),
                relay: Some(DEFAULT_RELAY_URL.into()),
            },
            client: api::Client::default(),
            store: Mutex::new(store),
            credential_protection: desktop_credential_protection(),
            active_receivers: Mutex::new(HashMap::new()),
            active_rooms: Mutex::new(HashMap::new()),
            active_outgoing: Mutex::new(HashMap::new()),
            active_pairings: Mutex::new(HashSet::new()),
            active_paths: Mutex::new(Vec::new()),
            pending_offers: Mutex::new(HashMap::new()),
            events: Mutex::new(AgentEventLog::new().unwrap()),
            shutdown,
            background_tasks: Mutex::new(Vec::new()),
        });

        let response = handle_request(
            runtime.clone(),
            AgentRequest::UpdateDeviceRoute {
                device: "macbook".into(),
                broker: DEFAULT_RENDEZVOUS_BROKER.into(),
                relay: Some(DEFAULT_RELAY_URL.into()),
            },
        )
        .await;

        let AgentResponse::DeviceRouteUpdated { device } = response else {
            panic!("unexpected response")
        };
        assert_eq!(device.id, device_id);
        assert_eq!(device.broker, DEFAULT_RENDEZVOUS_BROKER);
        assert_eq!(device.relay.as_deref(), Some(DEFAULT_RELAY_URL));
        assert!(matches!(
            lock(&runtime.events).unwrap().events.back(),
            Some(AgentEventEnvelope {
                event: AgentEvent::RelationshipChanged {
                    relationship_id,
                    change: AgentRelationshipChange::RouteUpdated,
                },
                ..
            }) if relationship_id == &device_id
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn socket_preparation_never_removes_a_regular_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.sock");
        fs::write(&path, b"keep me").unwrap();

        let error = prepare_socket(&path).await.unwrap_err();

        assert!(error.to_string().contains("refusing to remove"));
        assert_eq!(fs::read(path).unwrap(), b"keep me");
    }

    #[cfg(unix)]
    #[test]
    fn existing_custom_directory_permissions_are_preserved() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();

        create_directory(directory.path()).unwrap();

        assert_eq!(
            fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn verification_code_is_always_six_ascii_digits() {
        for _ in 0..32 {
            let code = generate_verification_code().unwrap();
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
        }
    }

    #[test]
    fn windows_pipe_contract_is_local_flat_and_bounded() {
        assert_eq!(
            validate_windows_pipe_endpoint(Path::new(
                r"\\.\pipe\envoix-agent-S-1-5-21-100-200-300-1001"
            ))
            .unwrap(),
            r"\\.\pipe\envoix-agent-S-1-5-21-100-200-300-1001"
        );
        for invalid in [
            r"\\server\pipe\envoix-agent-test",
            r"\\.\pipe\",
            r"\\.\pipe\envoix\nested",
            r"\\.\pipe\envoix-agent:test",
        ] {
            assert!(
                validate_windows_pipe_endpoint(Path::new(invalid)).is_err(),
                "{invalid}"
            );
        }
        let oversized = format!(r"\\.\pipe\{}", "a".repeat(250));
        assert!(validate_windows_pipe_endpoint(Path::new(&oversized)).is_err());
    }

    #[test]
    fn refused_receiver_attempts_back_off_to_a_bounded_delay() {
        let refused = Duration::from_millis(200);
        assert_eq!(
            receiver_retry_delay(refused, Duration::ZERO),
            RECEIVER_RETRY_DELAY
        );
        assert_eq!(
            receiver_retry_delay(refused, RECEIVER_RETRY_DELAY),
            RECEIVER_RETRY_DELAY * 2
        );
        let mut delay = Duration::ZERO;
        for _ in 0..16 {
            delay = receiver_retry_delay(refused, delay);
        }
        assert_eq!(delay, RECEIVER_RETRY_MAX_DELAY);
    }

    #[test]
    fn a_parked_receiver_attempt_keeps_the_base_delay() {
        // Waiting out a Room is the ordinary idle state, so the responder must
        // park again promptly instead of inheriting a refusal backoff.
        assert_eq!(
            receiver_retry_delay(RECEIVER_PARKED_ATTEMPT, RECEIVER_RETRY_MAX_DELAY),
            RECEIVER_RETRY_DELAY
        );
        assert_eq!(
            receiver_retry_delay(Duration::from_secs(300), RECEIVER_RETRY_MAX_DELAY),
            RECEIVER_RETRY_DELAY
        );
    }

    #[tokio::test]
    async fn cancelled_remembered_connect_drain_is_bounded() {
        let cancel = TransferCancelToken::new();
        let connect = std::future::pending::<()>();
        tokio::pin!(connect);

        tokio::time::timeout(
            Duration::from_secs(1),
            cancel_and_drain_remembered_connect(
                &cancel,
                connect.as_mut(),
                Duration::from_millis(10),
            ),
        )
        .await
        .expect("cancelled remembered connect drain exceeded its grace period");

        assert!(cancel.is_cancelled());
    }
}
