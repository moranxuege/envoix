//! UniFFI bridge for the canonical Manifest v2 job and session APIs.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use envoix_client::api::{
    Capabilities, Client, CreatedInvitation, InvitationError, InvitationErrorCode, InviteV2,
    PathPolicy, PeerSource, RememberedCredentialRef, RoomCode, TransferOptions, TransferRole,
    ValidatedInvitation, register_remembered_credential,
};
use envoix_client::{DEFAULT_RELAY_URL, DEFAULT_RENDEZVOUS_BROKER, PeerDescriptor};

uniffi::setup_scaffolding!();

mod datagram_transport;
pub use datagram_transport::*;
mod application_contract;
pub use application_contract::*;
mod manifest_v2_job;
pub use manifest_v2_job::*;
mod manifest_v2_session;
pub use manifest_v2_session::*;
mod logging;
pub use logging::*;
mod native_transport;
pub use native_transport::*;
mod platform_destination;
pub use platform_destination::*;
mod nearby_invite;
pub use nearby_invite::*;
mod room_control;
pub use room_control::*;
#[cfg(feature = "android-jni")]
mod android_jni;

const ENVOIX_FFI_API_VERSION: u32 = 20;

static FFI_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static CREATED_INVITATIONS: OnceLock<Mutex<HashMap<(String, TransferRole), PeerSource>>> =
    OnceLock::new();

fn ffi_runtime() -> &'static tokio::runtime::Runtime {
    FFI_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("envoix-ffi")
            .build()
            .expect("create Envoix FFI Tokio runtime")
    })
}

/// UniFFI polls exported async functions on its own executor. Enter the owned
/// Tokio runtime for every poll so Tokio filesystem, timer, and network
/// resources are always created with a live reactor on Apple platforms.
pub(crate) async fn on_ffi_runtime<F: Future>(future: F) -> F::Output {
    RuntimeContextFuture {
        inner: Box::pin(future),
    }
    .await
}

/// Runs an owned async operation as a Tokio task, rather than merely entering
/// the runtime while an external executor polls it. Long-lived network futures
/// need Tokio's task context as well as its reactor on Apple platforms.
pub(crate) async fn spawn_on_ffi_runtime<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match ffi_runtime().spawn(future).await {
        Ok(output) => output,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => panic!("Envoix FFI runtime task was cancelled: {error}"),
    }
}

struct RuntimeContextFuture<F: Future> {
    inner: Pin<Box<F>>,
}

impl<F: Future> Unpin for RuntimeContextFuture<F> {}

impl<F: Future> Future for RuntimeContextFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let _guard = ffi_runtime().enter();
        self.get_mut().inner.as_mut().poll(context)
    }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EnvoixError {
    #[error("{reason}")]
    Operation { reason: String },
    #[error("{reason}")]
    Invitation {
        code: FfiInvitationErrorCode,
        reason: String,
    },
}

pub(crate) fn op_err(error: impl std::fmt::Display) -> EnvoixError {
    EnvoixError::Operation {
        reason: error.to_string(),
    }
}

fn invitation_err(error: InvitationError) -> EnvoixError {
    let code = match error.code() {
        InvitationErrorCode::Malformed => FfiInvitationErrorCode::Malformed,
        InvitationErrorCode::Oversized => FfiInvitationErrorCode::Oversized,
        InvitationErrorCode::Expired => FfiInvitationErrorCode::Expired,
        InvitationErrorCode::UnsupportedVersion => FfiInvitationErrorCode::UnsupportedVersion,
        InvitationErrorCode::UnsupportedCapability => FfiInvitationErrorCode::UnsupportedCapability,
        InvitationErrorCode::RoleConflict => FfiInvitationErrorCode::RoleConflict,
        InvitationErrorCode::AuthenticationFailed => FfiInvitationErrorCode::AuthenticationFailure,
        InvitationErrorCode::Replay => FfiInvitationErrorCode::Replay,
        _ => FfiInvitationErrorCode::Malformed,
    };
    EnvoixError::Invitation {
        code,
        reason: error.to_string(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct EnvoixRuntimeSettings {
    pub concurrent_transfers: bool,
    pub language: String,
    pub server_url: String,
    pub relay_url: String,
    pub config_path: String,
    pub speed_limit_mbps: u64,
}

impl Default for EnvoixRuntimeSettings {
    fn default() -> Self {
        Self {
            concurrent_transfers: true,
            language: "en".into(),
            server_url: String::new(),
            relay_url: String::new(),
            config_path: String::new(),
            speed_limit_mbps: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiInviteRole {
    Send,
    Receive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiInvitationErrorCode {
    Malformed,
    Oversized,
    Expired,
    UnsupportedVersion,
    UnsupportedCapability,
    RoleConflict,
    AuthenticationFailure,
    Replay,
}

#[derive(Clone, Eq, PartialEq, uniffi::Record)]
pub struct FfiPairingInvite {
    pub room_code: String,
    pub payload: String,
    pub broker: String,
    pub relay_urls: Vec<String>,
    pub creator_role: FfiInviteRole,
    pub joiner_role: FfiInviteRole,
    pub expires_at: u64,
}

impl std::fmt::Debug for FfiPairingInvite {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FfiPairingInvite")
            .field("room_code", &"<redacted>")
            .field("payload", &"<redacted>")
            .field("broker", &self.broker)
            .field("relay_urls", &self.relay_urls)
            .field("creator_role", &self.creator_role)
            .field("joiner_role", &self.joiner_role)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferDirection {
    Send,
    Receive,
}

/// Stable, secret-free milestones for one native transfer attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferStage {
    SessionStarted,
    ConnectionReady,
    AuthenticationStarted,
    AuthenticationComplete,
    ManifestOffer,
    ManifestAccepted,
    FirstPayload,
    PayloadComplete,
    DeliveryComplete,
    Canceled,
    Failed,
}

/// Monotonic elapsed time projected from the native transfer timeline.
///
/// This record intentionally excludes endpoint, invitation, and credential
/// material. `transfer_id` may be absent until the manifest identifies the
/// transfer.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferStageTiming {
    pub stage: FfiTransferStage,
    pub direction: FfiTransferDirection,
    pub attempt_id: u64,
    pub transfer_id: Option<String>,
    pub elapsed_us: u64,
    pub delta_us: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferMode {
    Manual,
    Invite,
    Remembered,
    ShowManual,
    ShowInvite,
    Mdns,
    Room,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiPathPolicy {
    Auto,
    RelayOnly,
    DirectOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiRendezvousPlan {
    pub use_room: bool,
    pub use_mdns: bool,
    pub internet_available: bool,
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

/// Session-only inputs. Source paths and destination paths live in the job or
/// destination request, never in this rendezvous record.
#[derive(Clone, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferRequest {
    pub direction: FfiTransferDirection,
    pub mode: FfiTransferMode,
    pub peer_descriptor: String,
    pub invite: String,
    pub code: String,
    pub token: String,
    pub remember_consent: bool,
    pub remembered_credential_ref: String,
    pub remembered_generation: u64,
    pub remembered_previous_generation: Option<u64>,
    pub broker: String,
    pub relay: String,
    pub config_path: String,
    pub path_policy: FfiPathPolicy,
    pub rendezvous: FfiRendezvousPlan,
}

impl std::fmt::Debug for FfiTransferRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FfiTransferRequest")
            .field("direction", &self.direction)
            .field("mode", &self.mode)
            .field("peer_descriptor", &self.peer_descriptor)
            .field("invite", &"<redacted>")
            .field("code", &"<redacted>")
            .field("token", &"<redacted>")
            .field("remember_consent", &self.remember_consent)
            .field("remembered_credential_ref", &"<redacted>")
            .field("remembered_generation", &self.remembered_generation)
            .field(
                "remembered_previous_generation",
                &self.remembered_previous_generation,
            )
            .field("broker", &self.broker)
            .field("relay", &self.relay)
            .field("config_path", &self.config_path)
            .field("path_policy", &self.path_policy)
            .field("rendezvous", &self.rendezvous)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiManifestV2Phase {
    Pairing,
    Connecting,
    Transferring,
    Verifying,
    Saving,
    WaitingForReceiverSave,
    FinalizingDelivery,
    Delivered,
    WaitingForPeer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailureCode {
    UserCanceled,
    NetworkLost,
    AuthenticationFailed,
    RoomNotFound,
    RoomExpired,
    RoomFull,
    RoomRateLimited,
    RoomUnderAttack,
    EndpointRateLimited,
    IpRateLimited,
    ServerBusy,
    MalformedJoin,
    UnsupportedRendezvousVersion,
    UnsupportedFeature,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailureCategory {
    User,
    Network,
    Authentication,
    Permission,
    Storage,
    Integrity,
    Unsupported,
    Internal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailureOrigin {
    Local,
    Peer,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailurePhase {
    Setup,
    Pairing,
    Connecting,
    Authenticating,
    Negotiating,
    Transferring,
    Verifying,
    Committing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiRecoveryAction {
    Retry,
    Resume,
    ChooseFolder,
    OpenSettings,
    RePair,
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailureOutcome {
    Canceled,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiFailureSessionDisposition {
    RetainForRecovery,
    Release,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferFailure {
    pub code: FfiFailureCode,
    pub category: FfiFailureCategory,
    pub phase: FfiFailurePhase,
    pub origin: FfiFailureOrigin,
    pub direction: FfiTransferDirection,
    pub retryable: bool,
    pub recovery_action: FfiRecoveryAction,
    pub outcome: FfiFailureOutcome,
    pub session_disposition: FfiFailureSessionDisposition,
    pub user_message_key: String,
    pub diagnostic_message: String,
}

/// Privacy-safe classification of the data path selected by the transport.
///
/// Direct paths expose only their IP family. Endpoint addresses and relay URLs
/// remain internal diagnostics and are deliberately excluded from this
/// product-facing contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiDataPathKind {
    /// Compatibility value retained for activity records from older builds.
    Direct,
    DirectIpv4,
    DirectIpv6,
    Relay,
    WifiAware,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiConnectionPathEventKind {
    /// The first data path selected for an established transfer connection.
    Selected,
    /// A later transport migration, such as a relay-to-direct upgrade.
    Changed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiConnectionPathEvent {
    pub path_kind: FfiDataPathKind,
    pub event_kind: FfiConnectionPathEventKind,
}

#[uniffi::export(with_foreign)]
pub trait TransferObserver: Send + Sync {
    fn on_invite_ready(&self, invite: String);
    fn on_started(&self, item_count: u32, total_bytes: u64);
    fn on_phase(&self, phase: FfiManifestV2Phase);
    fn on_progress(&self, transferred: u64, total: u64);
    fn on_completed(&self, bytes: u64);
    fn on_transfer_failed(&self, failure: FfiTransferFailure);
    fn on_connection_path(&self, event: FfiConnectionPathEvent);
    fn on_stage_timing(&self, event: FfiTransferStageTiming);
    fn on_diagnostic(&self, message: String);
    /// Called only on the native worker at the authenticated persistence
    /// boundary. Implementations store these bytes immediately and must never
    /// project or log them.
    fn on_remembered_credential(&self, opaque_credential: Vec<u8>, generation: u64) -> bool;
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiCoreInfo {
    pub ffi_api_version: u32,
    pub core_version: String,
    pub capabilities: Vec<String>,
}

#[uniffi::export]
pub fn envoix_core_info() -> FfiCoreInfo {
    FfiCoreInfo {
        ffi_api_version: ENVOIX_FFI_API_VERSION,
        core_version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: vec![
            "canonical_transfer_job_v2".into(),
            "typed_staged_provider_job_v1".into(),
            "manifest_v2_session".into(),
            "paged_transfer_inventory_v2".into(),
            "delivery_proof_v2".into(),
            "native_duplex_transport_v1".into(),
            "wifi_aware_manifest_v2".into(),
            "wifi_aware_nearby_hybrid_v1".into(),
            "structured_connection_path".into(),
            "structured_stage_timing_v1".into(),
            "canonical_failure_projection_v1".into(),
            "platform_manifest_v2_destination_v1".into(),
            "foreground_room_control_v5".into(),
            "remembered_room_control_v1".into(),
            "typed_room_control_errors_v1".into(),
            "nearby_invite_inbox_v1".into(),
            "typed_log_sink_v1".into(),
            "remembered_devices_v1".into(),
            "typed_application_contract_v6".into(),
        ],
    }
}

/// Compatibility helper for bindings that still carry the retained RoomCode
/// field. Its output cannot be used to join an external InviteV2 invitation.
#[uniffi::export]
pub fn generate_room_code() -> Result<String, EnvoixError> {
    RoomCode::generate()
        .map(|code| code.to_string())
        .map_err(invitation_err)
}

#[uniffi::export]
pub fn make_pairing_invite(
    role: FfiInviteRole,
    broker: String,
    relay: String,
) -> Result<FfiPairingInvite, EnvoixError> {
    let broker_input = broker.trim();
    let relay = relay_for_pairing_invite(broker_input, &relay);
    let broker = broker_for_pairing_invite(broker_input);
    let invitation = InviteV2::create(
        broker,
        relay.into_iter().collect(),
        core_invite_role(role),
        Capabilities::current(),
        now_unix_seconds(),
    )
    .map_err(invitation_err)?;
    register_created_invitation(&invitation)?;
    Ok(FfiPairingInvite::from_created(&invitation))
}

#[uniffi::export]
pub fn parse_pairing_invite(input: String) -> Result<FfiPairingInvite, EnvoixError> {
    let invitation = InviteV2::parse(&input, now_unix_seconds()).map_err(invitation_err)?;
    Ok(FfiPairingInvite::from_validated(&invitation))
}

#[uniffi::export]
pub fn parse_pairing_invite_for_role(
    input: String,
    local_role: FfiInviteRole,
) -> Result<FfiPairingInvite, EnvoixError> {
    let invitation =
        InviteV2::parse_for_role(&input, core_invite_role(local_role), now_unix_seconds())
            .map_err(invitation_err)?;
    Ok(FfiPairingInvite::from_validated(&invitation))
}

/// Return the public Room locator shared by both sides of an InviteV2 flow.
/// The complete Room Code remains creator-only bootstrap state.
#[uniffi::export]
pub fn transfer_invitation_room_id(request: FfiTransferRequest) -> Result<String, EnvoixError> {
    let role = transfer_role(request.direction);
    match request.mode {
        FfiTransferMode::Invite => {
            let invitation = InviteV2::parse_for_role(
                required(&request.invite, "invite")?,
                role,
                now_unix_seconds(),
            )
            .map_err(invitation_err)?;
            Ok(invitation.into_bootstrap().room_id().to_string())
        }
        FfiTransferMode::Room => {
            let room_code =
                RoomCode::parse(required(&request.code, "code")?).map_err(invitation_err)?;
            match created_invitation_source(&room_code, role) {
                Some(PeerSource::Invitation { room_id, .. }) => Ok(room_id),
                _ => Err(EnvoixError::Operation {
                    reason: "Naked InviteV2 Room Codes are no longer supported".into(),
                }),
            }
        }
        _ => Err(EnvoixError::Operation {
            reason: "invitation Room ID requires Room or Invite mode".into(),
        }),
    }
}

/// Compatibility normalizer for the retained RoomCode field. Normalization
/// does not authorize naked-code InviteV2 joining.
#[uniffi::export]
pub fn normalize_room_code(input: String) -> Result<String, EnvoixError> {
    RoomCode::parse(&input)
        .map(|code| code.to_string())
        .map_err(invitation_err)
}

/// Validate protected bytes loaded by the platform and retain them behind a
/// process-only handle for subsequent transfer requests.
#[uniffi::export]
pub fn register_protected_remembered_credential(
    opaque_credential: Vec<u8>,
) -> Result<String, EnvoixError> {
    register_remembered_credential(&opaque_credential)
        .map(|reference| reference.as_str().to_string())
        .map_err(op_err)
}

impl FfiPairingInvite {
    fn from_created(invite: &CreatedInvitation) -> Self {
        let public = &invite.invitation().public_context;
        Self {
            room_code: invite.room_code.to_string(),
            payload: invite.payload.clone(),
            broker: public.broker.clone(),
            relay_urls: public.relay_urls.clone(),
            creator_role: ffi_invite_role(invite.creator_role),
            joiner_role: ffi_invite_role(invite.joiner_role),
            expires_at: invite.expires_at,
        }
    }

    fn from_validated(invite: &ValidatedInvitation) -> Self {
        let public = &invite.invitation().public_context;
        Self {
            room_code: String::new(),
            payload: String::new(),
            broker: public.broker.clone(),
            relay_urls: public.relay_urls.clone(),
            creator_role: ffi_invite_role(public.creator_transfer_role),
            joiner_role: ffi_invite_role(public.joiner_transfer_role),
            expires_at: public.expires_at,
        }
    }
}

pub(crate) struct RouteAttempt {
    pub source: PeerSource,
    pub path_policy_override: Option<PathPolicy>,
}

pub(crate) fn build_client_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<Client, EnvoixError> {
    let path = non_empty(&request.config_path)
        .or_else(|| non_empty(&settings.config_path))
        .map(Path::new);
    Client::from_runtime_sources(path).map_err(op_err)
}

pub(crate) fn transfer_options_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
    path_override: Option<PathPolicy>,
) -> Result<TransferOptions, EnvoixError> {
    let path = path_override.unwrap_or(match request.path_policy {
        FfiPathPolicy::Auto => PathPolicy::Auto,
        FfiPathPolicy::RelayOnly => PathPolicy::RelayOnly,
        FfiPathPolicy::DirectOnly => PathPolicy::DirectOnly,
    });
    let relay = non_empty(&request.relay)
        .or_else(|| non_empty(&settings.relay_url))
        .map(str::to_string);
    if path == PathPolicy::RelayOnly && relay.is_none() {
        return Err(EnvoixError::Operation {
            reason: "relay-only routing requires a relay URL".into(),
        });
    }
    let mut options = TransferOptions::default();
    options.relay = relay;
    options.path = path;
    Ok(options)
}

pub(crate) fn peer_sources_for_request(
    settings: &EnvoixRuntimeSettings,
    request: &FfiTransferRequest,
) -> Result<Vec<RouteAttempt>, EnvoixError> {
    let single = |source| {
        Ok(vec![RouteAttempt {
            source,
            path_policy_override: None,
        }])
    };
    match request.mode {
        FfiTransferMode::Manual => single(
            PeerSource::manual(
                required(&request.peer_descriptor, "peer_descriptor")?
                    .parse::<PeerDescriptor>()
                    .map_err(op_err)?,
                required(&request.token, "token")?.to_string(),
            )
            .map_err(op_err)?,
        ),
        FfiTransferMode::Invite => {
            let invite = InviteV2::parse_for_role(
                required(&request.invite, "invite")?,
                transfer_role(request.direction),
                now_unix_seconds(),
            )
            .map_err(invitation_err)?;
            let broker = invite.invitation().public_context.broker.clone();
            single(PeerSource::invitation(invite.into_bootstrap(), broker).map_err(op_err)?)
        }
        FfiTransferMode::Remembered => {
            if !request.rendezvous.use_room || !request.rendezvous.internet_available {
                return Err(EnvoixError::Operation {
                    reason: "remembered devices require an available rendezvous broker".into(),
                });
            }
            let credential_ref = RememberedCredentialRef::from_process_handle(
                required(
                    &request.remembered_credential_ref,
                    "remembered_credential_ref",
                )?
                .to_string(),
            );
            let broker = non_empty(&request.broker)
                .or_else(|| non_empty(&settings.server_url))
                .unwrap_or(DEFAULT_RENDEZVOUS_BROKER)
                .to_string();
            single(PeerSource::remembered_registered(
                credential_ref,
                request.remembered_generation,
                request.remembered_previous_generation,
                broker,
            ))
        }
        FfiTransferMode::ShowManual => single(
            PeerSource::show_manual(non_empty(&request.token).map(str::to_string))
                .map_err(op_err)?,
        ),
        FfiTransferMode::ShowInvite => {
            let broker = non_empty(&request.broker)
                .or_else(|| non_empty(&settings.server_url))
                .unwrap_or(DEFAULT_RENDEZVOUS_BROKER)
                .to_string();
            let relay_urls = non_empty(&request.relay)
                .or_else(|| non_empty(&settings.relay_url))
                .map(str::to_string)
                .into_iter()
                .collect();
            let created = InviteV2::create(
                broker.clone(),
                relay_urls,
                transfer_role(request.direction),
                Capabilities::current(),
                now_unix_seconds(),
            )
            .map_err(invitation_err)?;
            single(PeerSource::invitation(created.into_bootstrap(), broker).map_err(op_err)?)
        }
        FfiTransferMode::Mdns => {
            single(PeerSource::mdns(non_empty(&request.token).map(str::to_string)).map_err(op_err)?)
        }
        FfiTransferMode::Room => {
            let room_code =
                RoomCode::parse(required(&request.code, "code")?).map_err(invitation_err)?;
            let role = transfer_role(request.direction);
            let source = created_invitation_source(&room_code, role).ok_or_else(|| {
                EnvoixError::Operation {
                    reason: "Naked InviteV2 Room Codes are no longer supported".into(),
                }
            })?;
            if !request.rendezvous.use_room || !request.rendezvous.internet_available {
                return Err(EnvoixError::Operation {
                    reason: "Creator invitation bootstrap requires an available rendezvous broker"
                        .into(),
                });
            }
            single(source)
        }
    }
}

fn required<'a>(value: &'a str, field: &str) -> Result<&'a str, EnvoixError> {
    non_empty(value).ok_or_else(|| EnvoixError::Operation {
        reason: format!("{field} must not be empty"),
    })
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn register_created_invitation(invitation: &CreatedInvitation) -> Result<(), EnvoixError> {
    let source = PeerSource::invitation(
        invitation.clone().into_bootstrap(),
        invitation.invitation().public_context.broker.clone(),
    )
    .map_err(op_err)?;
    CREATED_INVITATIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("created invitation store poisoned")
        .insert(
            (invitation.room_code.to_string(), invitation.creator_role),
            source,
        );
    Ok(())
}

fn created_invitation_source(room_code: &RoomCode, local_role: TransferRole) -> Option<PeerSource> {
    CREATED_INVITATIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("created invitation store poisoned")
        .get(&(room_code.to_string(), local_role))
        .cloned()
}

fn broker_for_pairing_invite(broker: &str) -> String {
    non_empty(broker)
        .unwrap_or(DEFAULT_RENDEZVOUS_BROKER)
        .to_string()
}

fn relay_for_pairing_invite(broker: &str, relay: &str) -> Option<String> {
    non_empty(relay).map(str::to_string).or_else(|| {
        broker
            .trim()
            .is_empty()
            .then(|| DEFAULT_RELAY_URL.to_string())
    })
}

fn core_invite_role(role: FfiInviteRole) -> TransferRole {
    match role {
        FfiInviteRole::Send => TransferRole::Sender,
        FfiInviteRole::Receive => TransferRole::Receiver,
    }
}

fn ffi_invite_role(role: TransferRole) -> FfiInviteRole {
    match role {
        TransferRole::Sender => FfiInviteRole::Send,
        TransferRole::Receiver => FfiInviteRole::Receive,
    }
}

fn transfer_role(direction: FfiTransferDirection) -> TransferRole {
    match direction {
        FfiTransferDirection::Send => TransferRole::Sender,
        FfiTransferDirection::Receive => TransferRole::Receiver,
    }
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod created_invitation_tests {
    use super::*;
    use envoix_client::api::acquire_invitation;

    const TEST_BROKER: &str =
        "e946a31a2207efcd68b9dbf409c4bf241aa02a0cbc0028af2e1ed11472064eff@67.230.187.238:8445";

    fn room_request(code: String) -> FfiTransferRequest {
        FfiTransferRequest {
            direction: FfiTransferDirection::Send,
            mode: FfiTransferMode::Room,
            peer_descriptor: String::new(),
            invite: String::new(),
            code,
            token: String::new(),
            remember_consent: false,
            remembered_credential_ref: String::new(),
            remembered_generation: 0,
            remembered_previous_generation: None,
            broker: TEST_BROKER.into(),
            relay: String::new(),
            config_path: String::new(),
            path_policy: FfiPathPolicy::Auto,
            rendezvous: FfiRendezvousPlan::default(),
        }
    }

    fn invite_request(invite: String) -> FfiTransferRequest {
        FfiTransferRequest {
            direction: FfiTransferDirection::Receive,
            mode: FfiTransferMode::Invite,
            peer_descriptor: String::new(),
            invite,
            code: String::new(),
            token: String::new(),
            remember_consent: false,
            remembered_credential_ref: String::new(),
            remembered_generation: 0,
            remembered_previous_generation: None,
            broker: TEST_BROKER.into(),
            relay: String::new(),
            config_path: String::new(),
            path_policy: FfiPathPolicy::Auto,
            rendezvous: FfiRendezvousPlan::default(),
        }
    }

    #[test]
    fn creator_and_joiner_share_public_invitation_room_id() {
        let invitation =
            make_pairing_invite(FfiInviteRole::Send, TEST_BROKER.into(), String::new())
                .expect("create invitation");

        let creator_room_id =
            transfer_invitation_room_id(room_request(invitation.room_code.clone()))
                .expect("creator Room ID");
        let joiner_room_id =
            transfer_invitation_room_id(invite_request(invitation.payload.clone()))
                .expect("joiner Room ID");

        assert_eq!(creator_room_id, joiner_room_id);
        assert_eq!(creator_room_id.len(), 6);
        assert!(creator_room_id.bytes().all(|byte| byte.is_ascii_digit()));
        assert_ne!(creator_room_id, invitation.room_code);
    }

    #[test]
    fn invitation_room_id_rejects_non_invitation_modes() {
        let mut request = room_request("123456-k7m4-9v2d".into());
        request.mode = FfiTransferMode::Mdns;

        assert!(matches!(
            transfer_invitation_room_id(request),
            Err(EnvoixError::Operation { reason })
                if reason == "invitation Room ID requires Room or Invite mode"
        ));
    }

    #[test]
    fn external_naked_invite_v2_room_code_is_rejected() {
        let settings = EnvoixRuntimeSettings::default();
        let result = peer_sources_for_request(&settings, &room_request("123456-k7m4-9v2d".into()));

        assert!(matches!(
            result,
            Err(EnvoixError::Operation { reason })
                if reason == "Naked InviteV2 Room Codes are no longer supported"
        ));

        let mut offline = room_request("123456-k7m4-9v2d".into());
        offline.rendezvous.internet_available = false;
        assert!(matches!(
            peer_sources_for_request(&settings, &offline),
            Err(EnvoixError::Operation { reason })
                if reason == "Naked InviteV2 Room Codes are no longer supported"
        ));
    }

    #[test]
    fn creator_room_source_survives_pre_authentication_retry() {
        let invitation = InviteV2::create(
            TEST_BROKER.into(),
            Vec::new(),
            TransferRole::Sender,
            Capabilities::current(),
            now_unix_seconds(),
        )
        .expect("create invitation");
        let room_code = invitation.room_code.clone();
        register_created_invitation(&invitation).expect("register invitation");

        let first = created_invitation_source(&room_code, TransferRole::Sender)
            .expect("first creator source");
        let second = created_invitation_source(&room_code, TransferRole::Sender)
            .expect("creator source remains registered");
        assert_eq!(first, second);
        let routed = peer_sources_for_request(
            &EnvoixRuntimeSettings::default(),
            &room_request(room_code.to_string()),
        )
        .expect("registered creator code remains routable");
        assert_eq!(routed.len(), 1);
        assert_eq!(routed[0].source, first);

        let PeerSource::Invitation { secret_ref, .. } = first else {
            panic!("expected invitation source");
        };
        drop(acquire_invitation(&secret_ref).expect("first lease"));
        drop(acquire_invitation(&secret_ref).expect("pre-authentication retry"));

        CREATED_INVITATIONS
            .get()
            .expect("created invitation store")
            .lock()
            .expect("created invitation store poisoned")
            .remove(&(room_code.to_string(), TransferRole::Sender));
    }
}
