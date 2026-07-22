#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiInviteRole {
    Send,
    Receive,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiPairingInvite {
    /// Short pairing code typed by users and reused as the mDNS token.
    pub code: String,
    /// `envoix://pair/...` payload rendered into the QR code.
    pub payload: String,
    /// Broker advertised by the QR payload, empty when the input was a bare code.
    pub broker: String,
    /// Relay advertised by the QR payload, empty when not supplied.
    pub relay: String,
    /// Role advertised by the payload creator; scanners should choose the opposite.
    pub role: FfiInviteRole,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
pub enum FfiTransferDirection {
    Send,
    Receive,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferMode {
    Manual,
    Invite,
    ShowManual,
    ShowInvite,
    Mdns,
    Room,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiPathPolicy {
    Auto,
    RelayOnly,
    DirectOnly,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferLimits {
    /// Maximum independent transfer tasks a native queue may run at once.
    pub max_parallel_transfers: u32,
    /// Reserved for directory/multi-file sends. Current engine supports one file.
    pub max_parallel_files: u32,
    /// Reserved for future chunk-level parallelism. Current engine supports one chunk stream.
    pub max_parallel_chunks_per_file: u32,
    /// Advisory speed cap in bytes/s. Zero means unlimited; current engine does not enforce it.
    pub speed_limit_bps: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiRendezvousPlan {
    /// Try the hosted rendezvous room before any local-network fallback.
    pub use_room: bool,
    /// Reuse the room code as the mDNS token when room pairing is unavailable.
    pub use_mdns: bool,
    /// Whether the native shell currently considers broker access viable.
    pub internet_available: bool,
}

impl Default for FfiTransferLimits {
    fn default() -> Self {
        Self {
            max_parallel_transfers: 1,
            max_parallel_files: 1,
            max_parallel_chunks_per_file: 1,
            speed_limit_bps: 0,
        }
    }
}

impl Default for FfiRendezvousPlan {
    fn default() -> Self {
        Self {
            use_room: true,
            use_mdns: true,
            internet_available: true,
        }
    }
}

impl FfiRendezvousPlan {
    fn for_mode(mode: FfiTransferMode) -> Self {
        match mode {
            FfiTransferMode::Room => Self::default(),
            FfiTransferMode::Mdns => Self {
                use_room: false,
                use_mdns: true,
                internet_available: true,
            },
            _ => Self {
                use_room: false,
                use_mdns: false,
                internet_available: true,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferRequest {
    /// Native-side activity id used to correlate pre-start events in a queue.
    pub activity_id: String,
    pub direction: FfiTransferDirection,
    pub mode: FfiTransferMode,
    pub file_path: String,
    pub output_dir: String,
    pub peer_descriptor: String,
    pub invite: String,
    pub code: String,
    pub token: String,
    pub broker: String,
    pub relay: String,
    pub config_path: String,
    pub path_policy: FfiPathPolicy,
    pub resume: bool,
    /// Receive into staging, then wait for the native shell to publish to the
    /// user-selected Files/MediaStore destination.
    pub publication_required: bool,
    pub limits: FfiTransferLimits,
    pub rendezvous: FfiRendezvousPlan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferEventKind {
    Binding,
    Advertised,
    Pairing,
    Connecting,
    Connected,
    PathChanged,
    Started,
    Progress,
    Verifying,
    Verified,
    Completed,
    Failed,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiPairingStep {
    None,
    Joining,
    Matched,
    Exchanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiDataPathKind {
    None,
    Direct,
    Relay,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferEvent {
    pub activity_id: String,
    pub kind: FfiTransferEventKind,
    pub ts_ms: u64,
    pub direction: FfiTransferDirection,
    pub mode: FfiTransferMode,
    pub transfer_id: String,
    pub file_name: String,
    pub total_bytes: u64,
    pub bytes_transferred: u64,
    pub bytes_resumed: u64,
    pub pairing_step: FfiPairingStep,
    pub data_path_kind: FfiDataPathKind,
    pub data_path_detail: String,
    pub invite: String,
    pub token: String,
    pub peer_descriptor: String,
    pub diagnostic_message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferActivityState {
    Queued,
    Binding,
    WaitingForPeer,
    Pairing,
    Connecting,
    Transferring,
    Verifying,
    Unconfirmed,
    Publishing,
    Completed,
    Failed,
    Paused,
    Canceled,
    Unknown,
}

/// Runtime identity used to detect a stale but otherwise loadable native core.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiCoreInfo {
    pub ffi_api_version: u32,
    pub core_version: String,
    pub capabilities: Vec<String>,
}

/// Canonical action policy for an Activity card.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferActivityActions {
    pub can_pause: bool,
    pub can_resume: bool,
    pub can_cancel: bool,
    pub can_delete: bool,
    pub is_finalizing: bool,
}

impl FfiTransferActivityActions {
    fn for_record(activity: &FfiTransferActivityRecord) -> Self {
        let is_finalizing = is_finalizing_activity(activity);
        let can_pause = can_pause_durable_activity(activity);
        let can_resume = matches!(
            activity.state,
            FfiTransferActivityState::Paused | FfiTransferActivityState::Unconfirmed
        ) || matches!(activity.state, FfiTransferActivityState::Failed)
            && activity.retryable
            || matches!(activity.state, FfiTransferActivityState::Publishing) && activity.retryable;
        let can_cancel = matches!(
            activity.state,
            FfiTransferActivityState::Queued
                | FfiTransferActivityState::Binding
                | FfiTransferActivityState::WaitingForPeer
                | FfiTransferActivityState::Pairing
                | FfiTransferActivityState::Connecting
                | FfiTransferActivityState::Transferring
                | FfiTransferActivityState::Verifying
                | FfiTransferActivityState::Unconfirmed
                | FfiTransferActivityState::Paused
        ) && !is_finalizing
            || matches!(activity.state, FfiTransferActivityState::Publishing) && activity.retryable;
        let can_delete = matches!(
            activity.state,
            FfiTransferActivityState::Completed
                | FfiTransferActivityState::Failed
                | FfiTransferActivityState::Canceled
        );

        Self {
            can_pause,
            can_resume,
            can_cancel,
            can_delete,
            is_finalizing,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferActivityRecord {
    pub activity_id: String,
    /// Monotonic canonical snapshot sequence; native clients discard older
    /// deliveries when platform callback scheduling reorders them.
    pub sequence: u64,
    pub attempt_id: String,
    pub state: FfiTransferActivityState,
    pub direction: FfiTransferDirection,
    pub mode: FfiTransferMode,
    pub transfer_id: String,
    pub file_name: String,
    pub total_bytes: u64,
    pub bytes_transferred: u64,
    pub bytes_resumed: u64,
    pub speed_bps: u64,
    pub average_speed_bps: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
    pub completed_file_path: String,
    pub data_path_kind: FfiDataPathKind,
    pub data_path_detail: String,
    pub invite: String,
    pub token: String,
    pub peer_descriptor: String,
    pub diagnostic_message: String,
    pub failure_code: FfiFailureCode,
    pub failure_category: FfiFailureCategory,
    pub failure_phase: FfiFailurePhase,
    pub failure_origin: FfiFailureOrigin,
    pub user_message_key: String,
    pub retryable: bool,
    pub recovery_action: FfiRecoveryAction,
    pub limits: FfiTransferLimits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
pub enum FfiFailureCode {
    UserCanceled,
    PeerCanceled,
    NetworkLost,
    PeerUnreachable,
    AuthenticationFailed,
    PermissionDenied,
    DiskFull,
    HashMismatch,
    ProtocolError,
    DestinationConflict,
    UnsupportedFeature,
    Timeout,
    InternalError,
    SenderSourceUnavailable,
    SenderPermissionLost,
    SenderSourceChanged,
    SenderItemRemoved,
    SenderCanceled,
    ProtocolOrIntegrityFailure,
    ReceiverSpaceInsufficient,
    ReceiverDestinationDecisionRequired,
    ReceiverDestinationUnavailable,
    ReceiverSaveFailed,
    ReceiverReusedObjectLost,
    ReceiverFinalizationOutcomeUnknown,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
pub enum FfiFailureCategory {
    User,
    Network,
    Authentication,
    Permission,
    Storage,
    Integrity,
    Protocol,
    Unsupported,
    Internal,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
pub enum FfiFailureOrigin {
    Local,
    Peer,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
pub enum FfiFailurePhase {
    Setup,
    Binding,
    Advertising,
    Pairing,
    Connecting,
    Authenticating,
    Negotiating,
    Transferring,
    Verifying,
    Committing,
    Acknowledging,
    CleaningUp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Enum)]
pub enum FfiRecoveryAction {
    Retry,
    Resume,
    ChooseFolder,
    OpenSettings,
    RePair,
    UpdateApp,
    SwitchPairingMethod,
    DiscardPartial,
    None,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Record)]
pub struct FfiTransferFailure {
    pub code: FfiFailureCode,
    pub category: FfiFailureCategory,
    pub phase: FfiFailurePhase,
    pub origin: FfiFailureOrigin,
    pub direction: FfiTransferDirection,
    pub transfer_id: String,
    pub attempt_id: String,
    pub retryable: bool,
    pub recovery_action: FfiRecoveryAction,
    pub user_message_key: String,
    pub diagnostic_message: String,
}

/// Frontend-owned destination for publishing a staged receive.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, uniffi::Record)]
pub struct FfiNativePublicationTarget {
    pub destination_path: String,
    pub bookmark: Vec<u8>,
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
struct PersistedNativePublication {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<FfiNativePublicationTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<FfiTransferFailure>,
}

/// Observer implemented by the native UI to receive transfer updates.
///
/// Callbacks arrive on a Rust runtime thread; the UI must marshal to its main
/// thread before mutating UI state. Exactly one of [`on_completed`] /
/// [`on_failed`] fires per operation.
///
/// [`on_completed`]: TransferObserver::on_completed
/// [`on_failed`]: TransferObserver::on_failed
#[uniffi::export(with_foreign)]
pub trait TransferObserver: Send + Sync {
    /// Receiver only: the `envoix:…` invite string to render as a QR / share.
    fn on_invite_ready(&self, invite: String);
    /// A transfer started; `total_bytes` is the full file size.
    fn on_started(&self, file_name: String, total_bytes: u64);
    /// Progress update: `transferred` of `total` plaintext bytes.
    fn on_progress(&self, transferred: u64, total: u64);
    /// Terminal success: the transfer finished and was verified.
    fn on_completed(&self, bytes: u64);
    /// Terminal failure with machine-readable classification.
    fn on_transfer_failed(&self, failure: FfiTransferFailure);
    /// Terminal failure with a human-readable reason.
    fn on_failed(&self, reason: String);
    /// Structured lifecycle event for Activity, queues, and diagnostics.
    fn on_transfer_event(&self, event: FfiTransferEvent);
    /// Folded Activity/queue snapshot after each lifecycle event.
    fn on_transfer_activity(&self, record: FfiTransferActivityRecord);
    /// Free-form lifecycle/status text for display or logging.
    fn on_status(&self, message: String);
}

/// Platform courier for the opaque completion-receipt mailbox. The Rust
/// driver owns keys, sealing, verification, polling, and state transitions;
/// native code only performs HTTPS GET/POST and reports the result back.
#[uniffi::export(with_foreign)]
pub trait MailboxObserver: Send + Sync {
    fn on_fetch_receipt(&self, activity_id: String, key: String);
    fn on_post_receipt(&self, activity_id: String, key: String, blob: Vec<u8>);
}

/// Versioned native receipt courier that receives the endpoint frozen in the
/// durable session. `None` is reserved for records created before that field
/// existed, allowing the frontend to use its current configured endpoint.
#[uniffi::export(with_foreign)]
pub trait MailboxObserverV2: Send + Sync {
    fn on_fetch_receipt(&self, activity_id: String, key: String, server: Option<String>);
    fn on_post_receipt(
        &self,
        activity_id: String,
        key: String,
        blob: Vec<u8>,
        server: Option<String>,
    );
}
