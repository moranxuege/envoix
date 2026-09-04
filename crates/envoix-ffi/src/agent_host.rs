//! Typed UniFFI boundary for the durable desktop Agent host and its local
//! owner-only control client.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use envoix_agent::{
    AgentHost, AgentHostConfiguration, AgentHostError, AgentHostErrorCode,
    AgentHostLifecycleHandle, AgentHostLifecycleState, AgentShutdownHandle,
};
use envoix_client::agent_control::AgentControlClient;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use envoix_client::product::AGENT_PROTOCOL_VERSION;
use envoix_client::product::{
    AgentControlTransport, AgentCredentialProtection, AgentDiagnostics, AgentEvent,
    AgentEventCursor, AgentEventEnvelope, AgentOfferDecision, AgentPathKind, AgentPendingOffer,
    AgentRelationshipChange, AgentRequest, AgentRequestEnvelope, AgentResponse, AgentSnapshot,
    AgentStatus, AgentTransferPath, DeviceSummary, InboxItem, InboxRoot, PairingInvitation,
};

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use crate::ForeignApplicationVault;
use crate::{
    FfiApplicationSnapshot, FfiApplicationTransfer, FfiApplicationVault, FfiTransferDirection,
    ffi_application_transfer, ffi_direction, ffi_snapshot, spawn_on_ffi_runtime,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiAgentCredentialProtection {
    OwnerOnlyFile,
    WindowsDpapi,
    AppleKeychain,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentHostConfiguration {
    pub state_directory: String,
    pub inbox_directory: String,
    pub control_endpoint: String,
    pub device_name: String,
    pub broker: String,
    pub relay: Option<String>,
    /// Non-secret diagnostic classification; it must truthfully describe the
    /// injected vault implementation.
    pub credential_protection: FfiAgentCredentialProtection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiAgentHostErrorCode {
    UnsupportedPlatform,
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
    ShutdownBeforeReady,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentHostFailure {
    pub code: FfiAgentHostErrorCode,
    pub reason: String,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiAgentHostError {
    #[error("{reason}")]
    Failed {
        code: FfiAgentHostErrorCode,
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiAgentHostLifecycleState {
    Starting,
    Ready,
    Stopping,
    Stopped,
    Failed { failure: FfiAgentHostFailure },
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentHostReady {
    pub control_endpoint: String,
    pub agent_protocol_version: u16,
    pub application_contract_version: u16,
}

#[derive(uniffi::Object)]
pub struct FfiAgentHost {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    control_endpoint: String,
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    _unsupported: (),
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    shutdown: AgentShutdownHandle,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    lifecycle: AgentHostLifecycleHandle,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    completion: tokio::sync::Mutex<Option<tokio::task::JoinHandle<Result<(), AgentHostError>>>>,
}

#[uniffi::export]
impl FfiAgentHost {
    /// Starts one durable desktop Agent owner in the background. Call
    /// `wait_until_ready` before constructing a control client.
    #[uniffi::constructor]
    pub fn start(
        configuration: FfiAgentHostConfiguration,
        vault: Arc<dyn FfiApplicationVault>,
    ) -> Result<Arc<Self>, FfiAgentHostError> {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            let control_endpoint = configuration.control_endpoint.clone();
            let credential_protection =
                core_credential_protection(configuration.credential_protection);
            let host = AgentHost::new(
                AgentHostConfiguration {
                    state_directory: PathBuf::from(configuration.state_directory),
                    inbox_directory: PathBuf::from(configuration.inbox_directory),
                    control_endpoint: PathBuf::from(&control_endpoint),
                    device_name: configuration.device_name,
                    broker: configuration.broker,
                    relay: configuration.relay,
                },
                envoix_client::api::Client::default(),
                Arc::new(ForeignApplicationVault::new(vault)),
                credential_protection,
            );
            let shutdown = host.shutdown_handle();
            let lifecycle = host.lifecycle_handle();
            let completion = crate::ffi_runtime().spawn(host.run());
            Ok(Arc::new(Self {
                control_endpoint,
                shutdown,
                lifecycle,
                completion: tokio::sync::Mutex::new(Some(completion)),
            }))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = (configuration, vault);
            Err(host_failure(
                FfiAgentHostErrorCode::UnsupportedPlatform,
                "the durable Agent host is supported only on desktop platforms",
            ))
        }
    }

    pub fn lifecycle(&self) -> FfiAgentHostLifecycleState {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            ffi_host_lifecycle(self.lifecycle.state())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            FfiAgentHostLifecycleState::Failed {
                failure: FfiAgentHostFailure {
                    code: FfiAgentHostErrorCode::UnsupportedPlatform,
                    reason: "the durable Agent host is unsupported on this platform".into(),
                },
            }
        }
    }

    /// Waits until the owner-only control endpoint is bound and ready, or
    /// returns the typed startup failure.
    pub async fn wait_until_ready(&self) -> Result<FfiAgentHostReady, FfiAgentHostError> {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            let mut lifecycle = self.lifecycle.clone();
            loop {
                match lifecycle.state() {
                    AgentHostLifecycleState::Ready => {
                        return Ok(FfiAgentHostReady {
                            control_endpoint: self.control_endpoint.clone(),
                            agent_protocol_version: AGENT_PROTOCOL_VERSION,
                            application_contract_version:
                                envoix_client::APPLICATION_CONTRACT_VERSION,
                        });
                    }
                    AgentHostLifecycleState::Failed { failure } => {
                        return Err(host_failure(
                            ffi_host_error_code(failure.code),
                            failure.reason,
                        ));
                    }
                    AgentHostLifecycleState::Stopped => {
                        return Err(host_failure(
                            FfiAgentHostErrorCode::ShutdownBeforeReady,
                            "Agent host stopped before its control endpoint became ready",
                        ));
                    }
                    AgentHostLifecycleState::Starting | AgentHostLifecycleState::Stopping => {}
                }
                if lifecycle.changed().await.is_none() {
                    return Err(host_failure(
                        FfiAgentHostErrorCode::Internal,
                        "Agent host lifecycle ended without a terminal state",
                    ));
                }
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Err(host_failure(
                FfiAgentHostErrorCode::UnsupportedPlatform,
                "the durable Agent host is unsupported on this platform",
            ))
        }
    }

    /// Requests shutdown and waits for the durable Engine owner and control
    /// endpoint to be released. The operation is idempotent.
    pub async fn shutdown(&self) -> Result<FfiAgentHostLifecycleState, FfiAgentHostError> {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            self.shutdown.shutdown();
            let mut completion = self.completion.lock().await;
            if let Some(task) = completion.take() {
                match task.await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => return Err(ffi_host_error(error)),
                    Err(error) => {
                        return Err(host_failure(
                            FfiAgentHostErrorCode::Internal,
                            if error.is_cancelled() {
                                "Agent host task was canceled"
                            } else {
                                "Agent host task failed"
                            },
                        ));
                    }
                }
            }
            match self.lifecycle.state() {
                AgentHostLifecycleState::Failed { failure } => Err(host_failure(
                    ffi_host_error_code(failure.code),
                    failure.reason,
                )),
                state => Ok(ffi_host_lifecycle(state)),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Err(host_failure(
                FfiAgentHostErrorCode::UnsupportedPlatform,
                "the durable Agent host is unsupported on this platform",
            ))
        }
    }
}

impl Drop for FfiAgentHost {
    fn drop(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        self.shutdown.shutdown();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiAgentControlErrorCode {
    UnsupportedPlatform,
    InvalidInput,
    Unavailable,
    PermissionDenied,
    IncompatibleProtocol,
    InvalidResponse,
    IoFailure,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiAgentControlError {
    #[error("{reason}")]
    Failed {
        code: FfiAgentControlErrorCode,
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentEventCursor {
    pub instance_id: String,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiAgentOfferDecision {
    Approve,
    Reject,
}

#[derive(Clone, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentPairingInput {
    pub label: String,
    pub invitation: String,
    pub verification_code: String,
}

impl std::fmt::Debug for FfiAgentPairingInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FfiAgentPairingInput")
            .field("label", &self.label)
            .field("invitation", &"<redacted>")
            .field("verification_code", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiAgentRequest {
    Status,
    Snapshot {
        inbox_limit: u64,
    },
    Events {
        after: FfiAgentEventCursor,
        limit: u64,
    },
    Pair {
        label: String,
    },
    JoinPairing {
        pairing: FfiAgentPairingInput,
    },
    ListDevices,
    RevokeDevice {
        device: String,
    },
    UpdateDeviceRoute {
        device: String,
        broker: String,
        relay: Option<String>,
    },
    CreateTransfer {
        device: String,
        paths: Vec<String>,
    },
    ListTransfers,
    ListTransferPaths,
    GetTransfer {
        transfer_id: String,
    },
    ListPendingOffers,
    DecidePendingOffer {
        offer_id: String,
        decision: FfiAgentOfferDecision,
    },
    ListInbox {
        limit: u64,
    },
    LatestInbox,
    Diagnostics,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentStatus {
    pub protocol_version: u16,
    pub pid: u32,
    pub device_name: String,
    pub state_directory: String,
    pub inbox_directory: String,
    pub broker: String,
    pub relay: Option<String>,
    pub paired_devices: u64,
    pub active_receivers: u64,
    pub active_pairings: u64,
    pub active_paths: u64,
    pub pending_offers: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiAgentControlTransport {
    UnixSocket,
    WindowsNamedPipe,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentDiagnostics {
    pub agent_protocol_version: u16,
    pub application_contract_version: u16,
    pub engine_schema_version: u16,
    pub platform: String,
    pub control_transport: FfiAgentControlTransport,
    pub credential_protection: FfiAgentCredentialProtection,
    pub engine_sequence: u64,
    pub relationships: u64,
    pub transfers: u64,
    pub inbox_items: u64,
    pub active_paths: u64,
    pub pending_offers: u64,
}

#[derive(Clone, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentPairingInvitation {
    pub label: String,
    pub room_code: String,
    pub verification_code: String,
    pub expires_at_unix_seconds: u64,
}

impl std::fmt::Debug for FfiAgentPairingInvitation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FfiAgentPairingInvitation")
            .field("label", &self.label)
            .field("room_code", &"<redacted>")
            .field("verification_code", &"<redacted>")
            .field("expires_at_unix_seconds", &self.expires_at_unix_seconds)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentDeviceSummary {
    pub id: String,
    pub label: String,
    pub generation: u64,
    pub previous_generation: Option<u64>,
    pub broker: String,
    pub relay: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiAgentPathKind {
    Lan,
    Direct,
    Relay,
    WifiAware,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentTransferPath {
    pub transfer_id: String,
    pub direction: FfiTransferDirection,
    pub path: FfiAgentPathKind,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentPendingOffer {
    pub offer_id: String,
    pub from_device_id: String,
    pub from_device_label: String,
    pub root_names: Vec<String>,
    pub item_count: u32,
    pub directory_count: u32,
    pub total_bytes: u64,
    pub allocatable_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentInboxRoot {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentInboxItem {
    pub id: String,
    pub received_at_unix_ms: u64,
    pub from_device_id: String,
    pub from_device_label: String,
    pub roots: Vec<FfiAgentInboxRoot>,
    pub file_count: u32,
    pub directory_count: u32,
    pub total_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiAgentRelationshipChange {
    Trusted,
    Rotated,
    RouteUpdated,
    Revoked,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiAgentEvent {
    PairingChanged {
        label: String,
        active: bool,
    },
    RelationshipChanged {
        relationship_id: String,
        change: FfiAgentRelationshipChange,
    },
    InboxChanged {
        item_id: String,
    },
    TransferChanged {
        transfer_id: String,
    },
    TransferPathChanged {
        transfer_id: String,
        direction: FfiTransferDirection,
        path: Option<FfiAgentPathKind>,
    },
    PendingOfferChanged {
        offer_id: String,
        pending: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentEventEnvelope {
    pub protocol_version: u16,
    pub instance_id: String,
    pub sequence: u64,
    pub event: FfiAgentEvent,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiAgentSnapshot {
    pub status: FfiAgentStatus,
    pub engine: FfiApplicationSnapshot,
    pub inbox: Vec<FfiAgentInboxItem>,
    pub active_paths: Vec<FfiAgentTransferPath>,
    pub pending_offers: Vec<FfiAgentPendingOffer>,
    pub event_cursor: FfiAgentEventCursor,
}

/// UniFFI does not lower `Box<Record>`; keep snapshot responses as value types
/// so Swift and Kotlin receive the same immutable result shape.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiAgentResponse {
    Status {
        status: FfiAgentStatus,
    },
    Pairing {
        pairing: FfiAgentPairingInvitation,
    },
    DevicePaired {
        device: FfiAgentDeviceSummary,
    },
    Devices {
        devices: Vec<FfiAgentDeviceSummary>,
    },
    DeviceRevoked {
        device: FfiAgentDeviceSummary,
    },
    DeviceRouteUpdated {
        device: FfiAgentDeviceSummary,
    },
    TransferCreated {
        transfer: FfiApplicationTransfer,
    },
    Transfers {
        transfers: Vec<FfiApplicationTransfer>,
    },
    TransferPaths {
        paths: Vec<FfiAgentTransferPath>,
    },
    Transfer {
        transfer: FfiApplicationTransfer,
    },
    PendingOffers {
        offers: Vec<FfiAgentPendingOffer>,
    },
    PendingOfferDecided {
        offer: FfiAgentPendingOffer,
        decision: FfiAgentOfferDecision,
    },
    Inbox {
        items: Vec<FfiAgentInboxItem>,
    },
    Latest {
        item: Option<FfiAgentInboxItem>,
    },
    Snapshot {
        snapshot: FfiAgentSnapshot,
    },
    Events {
        cursor: FfiAgentEventCursor,
        events: Vec<FfiAgentEventEnvelope>,
    },
    SnapshotRequired {
        cursor: FfiAgentEventCursor,
    },
    Diagnostics {
        diagnostics: FfiAgentDiagnostics,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(uniffi::Object)]
pub struct FfiAgentControlClient {
    client: AgentControlClient,
}

#[uniffi::export]
impl FfiAgentControlClient {
    #[uniffi::constructor]
    pub fn new(control_endpoint: String) -> Result<Arc<Self>, FfiAgentControlError> {
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = control_endpoint;
            return Err(control_failure(
                FfiAgentControlErrorCode::UnsupportedPlatform,
                "the local Agent control transport is supported only on desktop platforms",
            ));
        }
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            validate_control_endpoint(&control_endpoint)?;
            Ok(Arc::new(Self {
                client: AgentControlClient::new(control_endpoint),
            }))
        }
    }

    pub async fn call(
        &self,
        request: FfiAgentRequest,
    ) -> Result<FfiAgentResponse, FfiAgentControlError> {
        let request = core_agent_request(request)?;
        let client = self.client.clone();
        spawn_on_ffi_runtime(async move { client.call(request).await })
            .await
            .map(ffi_agent_response)
            .map_err(control_io_error)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn validate_control_endpoint(endpoint: &str) -> Result<(), FfiAgentControlError> {
    if endpoint.is_empty() || endpoint.len() > 4_096 || endpoint.chars().any(char::is_control) {
        return Err(control_failure(
            FfiAgentControlErrorCode::InvalidInput,
            "Agent control endpoint must be a bounded visible path",
        ));
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if !PathBuf::from(endpoint).is_absolute() {
        return Err(control_failure(
            FfiAgentControlErrorCode::InvalidInput,
            "Agent control endpoint must be absolute",
        ));
    }
    Ok(())
}

fn core_agent_request(request: FfiAgentRequest) -> Result<AgentRequest, FfiAgentControlError> {
    let request = match request {
        FfiAgentRequest::Status => AgentRequest::Status,
        FfiAgentRequest::Snapshot { inbox_limit } => AgentRequest::Snapshot {
            inbox_limit: bounded_usize(inbox_limit, "Inbox limit")?,
        },
        FfiAgentRequest::Events { after, limit } => AgentRequest::Events {
            after: AgentEventCursor {
                instance_id: after.instance_id,
                sequence: after.sequence,
            },
            limit: bounded_usize(limit, "event limit")?,
        },
        FfiAgentRequest::Pair { label } => AgentRequest::Pair { label },
        FfiAgentRequest::JoinPairing { pairing } => AgentRequest::JoinPairing {
            pairing: envoix_client::product::AgentPairingInput {
                label: pairing.label,
                invitation: pairing.invitation,
                verification_code: pairing.verification_code,
            },
        },
        FfiAgentRequest::ListDevices => AgentRequest::ListDevices,
        FfiAgentRequest::RevokeDevice { device } => AgentRequest::RevokeDevice { device },
        FfiAgentRequest::UpdateDeviceRoute {
            device,
            broker,
            relay,
        } => AgentRequest::UpdateDeviceRoute {
            device,
            broker,
            relay,
        },
        FfiAgentRequest::CreateTransfer { device, paths } => AgentRequest::CreateTransfer {
            device,
            paths: paths.into_iter().map(PathBuf::from).collect(),
        },
        FfiAgentRequest::ListTransfers => AgentRequest::ListTransfers,
        FfiAgentRequest::ListTransferPaths => AgentRequest::ListTransferPaths,
        FfiAgentRequest::GetTransfer { transfer_id } => AgentRequest::GetTransfer { transfer_id },
        FfiAgentRequest::ListPendingOffers => AgentRequest::ListPendingOffers,
        FfiAgentRequest::DecidePendingOffer { offer_id, decision } => {
            AgentRequest::DecidePendingOffer {
                offer_id,
                decision: core_offer_decision(decision),
            }
        }
        FfiAgentRequest::ListInbox { limit } => AgentRequest::ListInbox {
            limit: bounded_usize(limit, "Inbox limit")?,
        },
        FfiAgentRequest::LatestInbox => AgentRequest::LatestInbox,
        FfiAgentRequest::Diagnostics => AgentRequest::Diagnostics,
    };
    AgentRequestEnvelope::new("ffi_validation", request.clone())
        .and_then(|envelope| envelope.validate())
        .map_err(control_io_error)?;
    Ok(request)
}

fn bounded_usize(value: u64, name: &str) -> Result<usize, FfiAgentControlError> {
    usize::try_from(value).map_err(|_| {
        control_failure(
            FfiAgentControlErrorCode::InvalidInput,
            format!("Agent {name} does not fit this platform"),
        )
    })
}

fn core_offer_decision(decision: FfiAgentOfferDecision) -> AgentOfferDecision {
    match decision {
        FfiAgentOfferDecision::Approve => AgentOfferDecision::Approve,
        FfiAgentOfferDecision::Reject => AgentOfferDecision::Reject,
    }
}

fn ffi_offer_decision(decision: AgentOfferDecision) -> FfiAgentOfferDecision {
    match decision {
        AgentOfferDecision::Approve => FfiAgentOfferDecision::Approve,
        AgentOfferDecision::Reject => FfiAgentOfferDecision::Reject,
    }
}

fn ffi_agent_response(response: AgentResponse) -> FfiAgentResponse {
    match response {
        AgentResponse::Status { status } => FfiAgentResponse::Status {
            status: ffi_agent_status(status),
        },
        AgentResponse::Pairing { pairing } => FfiAgentResponse::Pairing {
            pairing: ffi_pairing_invitation(pairing),
        },
        AgentResponse::DevicePaired { device } => FfiAgentResponse::DevicePaired {
            device: ffi_device_summary(device),
        },
        AgentResponse::Devices { devices } => FfiAgentResponse::Devices {
            devices: devices.into_iter().map(ffi_device_summary).collect(),
        },
        AgentResponse::DeviceRevoked { device } => FfiAgentResponse::DeviceRevoked {
            device: ffi_device_summary(device),
        },
        AgentResponse::DeviceRouteUpdated { device } => FfiAgentResponse::DeviceRouteUpdated {
            device: ffi_device_summary(device),
        },
        AgentResponse::TransferCreated { transfer } => FfiAgentResponse::TransferCreated {
            transfer: ffi_application_transfer(&transfer),
        },
        AgentResponse::Transfers { transfers } => FfiAgentResponse::Transfers {
            transfers: transfers.iter().map(ffi_application_transfer).collect(),
        },
        AgentResponse::TransferPaths { paths } => FfiAgentResponse::TransferPaths {
            paths: paths.into_iter().map(ffi_transfer_path).collect(),
        },
        AgentResponse::Transfer { transfer } => FfiAgentResponse::Transfer {
            transfer: ffi_application_transfer(&transfer),
        },
        AgentResponse::PendingOffers { offers } => FfiAgentResponse::PendingOffers {
            offers: offers.into_iter().map(ffi_pending_offer).collect(),
        },
        AgentResponse::PendingOfferDecided { offer, decision } => {
            FfiAgentResponse::PendingOfferDecided {
                offer: ffi_pending_offer(offer),
                decision: ffi_offer_decision(decision),
            }
        }
        AgentResponse::Inbox { items } => FfiAgentResponse::Inbox {
            items: items.into_iter().map(ffi_inbox_item).collect(),
        },
        AgentResponse::Latest { item } => FfiAgentResponse::Latest {
            item: item.map(ffi_inbox_item),
        },
        AgentResponse::Snapshot { snapshot } => FfiAgentResponse::Snapshot {
            snapshot: ffi_agent_snapshot(*snapshot),
        },
        AgentResponse::Events { cursor, events } => FfiAgentResponse::Events {
            cursor: ffi_event_cursor(cursor),
            events: events.into_iter().map(ffi_event_envelope).collect(),
        },
        AgentResponse::SnapshotRequired { cursor } => FfiAgentResponse::SnapshotRequired {
            cursor: ffi_event_cursor(cursor),
        },
        AgentResponse::Diagnostics { diagnostics } => FfiAgentResponse::Diagnostics {
            diagnostics: ffi_diagnostics(diagnostics),
        },
        AgentResponse::Error { code, message } => FfiAgentResponse::Error { code, message },
    }
}

fn ffi_agent_status(status: AgentStatus) -> FfiAgentStatus {
    FfiAgentStatus {
        protocol_version: status.protocol_version,
        pid: status.pid,
        device_name: status.device_name,
        state_directory: status.state_directory,
        inbox_directory: status.inbox_directory,
        broker: status.broker,
        relay: status.relay,
        paired_devices: status.paired_devices as u64,
        active_receivers: status.active_receivers as u64,
        active_pairings: status.active_pairings as u64,
        active_paths: status.active_paths as u64,
        pending_offers: status.pending_offers as u64,
    }
}

fn ffi_pairing_invitation(pairing: PairingInvitation) -> FfiAgentPairingInvitation {
    FfiAgentPairingInvitation {
        label: pairing.label,
        room_code: pairing.room_code,
        verification_code: pairing.verification_code,
        expires_at_unix_seconds: pairing.expires_at_unix_seconds,
    }
}

fn ffi_device_summary(device: DeviceSummary) -> FfiAgentDeviceSummary {
    FfiAgentDeviceSummary {
        id: device.id,
        label: device.label,
        generation: device.generation,
        previous_generation: device.previous_generation,
        broker: device.broker,
        relay: device.relay,
    }
}

fn ffi_transfer_path(path: AgentTransferPath) -> FfiAgentTransferPath {
    FfiAgentTransferPath {
        transfer_id: path.transfer_id,
        direction: ffi_direction(path.direction),
        path: ffi_path_kind(path.path),
    }
}

fn ffi_path_kind(path: AgentPathKind) -> FfiAgentPathKind {
    match path {
        AgentPathKind::Lan => FfiAgentPathKind::Lan,
        AgentPathKind::Direct => FfiAgentPathKind::Direct,
        AgentPathKind::Relay => FfiAgentPathKind::Relay,
        AgentPathKind::WifiAware => FfiAgentPathKind::WifiAware,
        AgentPathKind::Other => FfiAgentPathKind::Other,
    }
}

fn ffi_pending_offer(offer: AgentPendingOffer) -> FfiAgentPendingOffer {
    FfiAgentPendingOffer {
        offer_id: offer.offer_id,
        from_device_id: offer.from_device_id,
        from_device_label: offer.from_device_label,
        root_names: offer.root_names,
        item_count: offer.item_count,
        directory_count: offer.directory_count,
        total_bytes: offer.total_bytes,
        allocatable_bytes: offer.allocatable_bytes,
    }
}

fn ffi_inbox_item(item: InboxItem) -> FfiAgentInboxItem {
    FfiAgentInboxItem {
        id: item.id,
        received_at_unix_ms: item.received_at_unix_ms,
        from_device_id: item.from_device_id,
        from_device_label: item.from_device_label,
        roots: item.roots.into_iter().map(ffi_inbox_root).collect(),
        file_count: item.file_count,
        directory_count: item.directory_count,
        total_bytes: item.total_bytes,
    }
}

fn ffi_inbox_root(root: InboxRoot) -> FfiAgentInboxRoot {
    FfiAgentInboxRoot {
        name: root.name,
        path: root.path,
    }
}

fn ffi_event_cursor(cursor: AgentEventCursor) -> FfiAgentEventCursor {
    FfiAgentEventCursor {
        instance_id: cursor.instance_id,
        sequence: cursor.sequence,
    }
}

fn ffi_event_envelope(envelope: AgentEventEnvelope) -> FfiAgentEventEnvelope {
    FfiAgentEventEnvelope {
        protocol_version: envelope.protocol_version,
        instance_id: envelope.instance_id,
        sequence: envelope.sequence,
        event: ffi_event(envelope.event),
    }
}

fn ffi_event(event: AgentEvent) -> FfiAgentEvent {
    match event {
        AgentEvent::PairingChanged { label, active } => {
            FfiAgentEvent::PairingChanged { label, active }
        }
        AgentEvent::RelationshipChanged {
            relationship_id,
            change,
        } => FfiAgentEvent::RelationshipChanged {
            relationship_id,
            change: ffi_relationship_change(change),
        },
        AgentEvent::InboxChanged { item_id } => FfiAgentEvent::InboxChanged { item_id },
        AgentEvent::TransferChanged { transfer_id } => {
            FfiAgentEvent::TransferChanged { transfer_id }
        }
        AgentEvent::TransferPathChanged {
            transfer_id,
            direction,
            path,
        } => FfiAgentEvent::TransferPathChanged {
            transfer_id,
            direction: ffi_direction(direction),
            path: path.map(ffi_path_kind),
        },
        AgentEvent::PendingOfferChanged { offer_id, pending } => {
            FfiAgentEvent::PendingOfferChanged { offer_id, pending }
        }
    }
}

fn ffi_relationship_change(change: AgentRelationshipChange) -> FfiAgentRelationshipChange {
    match change {
        AgentRelationshipChange::Trusted => FfiAgentRelationshipChange::Trusted,
        AgentRelationshipChange::Rotated => FfiAgentRelationshipChange::Rotated,
        AgentRelationshipChange::RouteUpdated => FfiAgentRelationshipChange::RouteUpdated,
        AgentRelationshipChange::Revoked => FfiAgentRelationshipChange::Revoked,
    }
}

fn ffi_agent_snapshot(snapshot: AgentSnapshot) -> FfiAgentSnapshot {
    FfiAgentSnapshot {
        status: ffi_agent_status(snapshot.status),
        engine: ffi_snapshot(&snapshot.engine),
        inbox: snapshot.inbox.into_iter().map(ffi_inbox_item).collect(),
        active_paths: snapshot
            .active_paths
            .into_iter()
            .map(ffi_transfer_path)
            .collect(),
        pending_offers: snapshot
            .pending_offers
            .into_iter()
            .map(ffi_pending_offer)
            .collect(),
        event_cursor: ffi_event_cursor(snapshot.event_cursor),
    }
}

fn ffi_diagnostics(diagnostics: AgentDiagnostics) -> FfiAgentDiagnostics {
    FfiAgentDiagnostics {
        agent_protocol_version: diagnostics.agent_protocol_version,
        application_contract_version: diagnostics.application_contract_version,
        engine_schema_version: diagnostics.engine_schema_version,
        platform: diagnostics.platform,
        control_transport: match diagnostics.control_transport {
            AgentControlTransport::UnixSocket => FfiAgentControlTransport::UnixSocket,
            AgentControlTransport::WindowsNamedPipe => FfiAgentControlTransport::WindowsNamedPipe,
        },
        credential_protection: ffi_credential_protection(diagnostics.credential_protection),
        engine_sequence: diagnostics.engine_sequence,
        relationships: diagnostics.relationships as u64,
        transfers: diagnostics.transfers as u64,
        inbox_items: diagnostics.inbox_items as u64,
        active_paths: diagnostics.active_paths as u64,
        pending_offers: diagnostics.pending_offers as u64,
    }
}

fn ffi_credential_protection(
    protection: AgentCredentialProtection,
) -> FfiAgentCredentialProtection {
    match protection {
        AgentCredentialProtection::OwnerOnlyFile => FfiAgentCredentialProtection::OwnerOnlyFile,
        AgentCredentialProtection::WindowsDpapi => FfiAgentCredentialProtection::WindowsDpapi,
        AgentCredentialProtection::AppleKeychain => FfiAgentCredentialProtection::AppleKeychain,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn core_credential_protection(
    protection: FfiAgentCredentialProtection,
) -> AgentCredentialProtection {
    match protection {
        FfiAgentCredentialProtection::OwnerOnlyFile => AgentCredentialProtection::OwnerOnlyFile,
        FfiAgentCredentialProtection::WindowsDpapi => AgentCredentialProtection::WindowsDpapi,
        FfiAgentCredentialProtection::AppleKeychain => AgentCredentialProtection::AppleKeychain,
    }
}

fn control_io_error(error: io::Error) -> FfiAgentControlError {
    let reason = error.to_string();
    let code = match error.kind() {
        io::ErrorKind::InvalidInput => FfiAgentControlErrorCode::InvalidInput,
        io::ErrorKind::PermissionDenied => FfiAgentControlErrorCode::PermissionDenied,
        io::ErrorKind::NotFound
        | io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::NotConnected
        | io::ErrorKind::BrokenPipe
        | io::ErrorKind::TimedOut => FfiAgentControlErrorCode::Unavailable,
        io::ErrorKind::Unsupported if reason.contains("unsupported Agent protocol") => {
            FfiAgentControlErrorCode::IncompatibleProtocol
        }
        io::ErrorKind::Unsupported => FfiAgentControlErrorCode::UnsupportedPlatform,
        io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => {
            FfiAgentControlErrorCode::InvalidResponse
        }
        _ => FfiAgentControlErrorCode::IoFailure,
    };
    control_failure(code, reason)
}

fn control_failure(
    code: FfiAgentControlErrorCode,
    reason: impl Into<String>,
) -> FfiAgentControlError {
    FfiAgentControlError::Failed {
        code,
        reason: reason.into(),
    }
}

fn host_failure(code: FfiAgentHostErrorCode, reason: impl Into<String>) -> FfiAgentHostError {
    FfiAgentHostError::Failed {
        code,
        reason: reason.into(),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn ffi_host_error(error: AgentHostError) -> FfiAgentHostError {
    host_failure(ffi_host_error_code(error.code()), error.reason())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn ffi_host_error_code(code: AgentHostErrorCode) -> FfiAgentHostErrorCode {
    match code {
        AgentHostErrorCode::InvalidConfiguration => FfiAgentHostErrorCode::InvalidConfiguration,
        AgentHostErrorCode::StateAlreadyOwned => FfiAgentHostErrorCode::StateAlreadyOwned,
        AgentHostErrorCode::UnsupportedPersistentState => {
            FfiAgentHostErrorCode::UnsupportedPersistentState
        }
        AgentHostErrorCode::StateCorrupt => FfiAgentHostErrorCode::StateCorrupt,
        AgentHostErrorCode::VaultUnavailable => FfiAgentHostErrorCode::VaultUnavailable,
        AgentHostErrorCode::VaultInteractionRequired => {
            FfiAgentHostErrorCode::VaultInteractionRequired
        }
        AgentHostErrorCode::VaultPermissionDenied => FfiAgentHostErrorCode::VaultPermissionDenied,
        AgentHostErrorCode::VaultCorrupt => FfiAgentHostErrorCode::VaultCorrupt,
        AgentHostErrorCode::VaultCanceled => FfiAgentHostErrorCode::VaultCanceled,
        AgentHostErrorCode::IoFailure => FfiAgentHostErrorCode::IoFailure,
        AgentHostErrorCode::Internal => FfiAgentHostErrorCode::Internal,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn ffi_host_lifecycle(state: AgentHostLifecycleState) -> FfiAgentHostLifecycleState {
    match state {
        AgentHostLifecycleState::Starting => FfiAgentHostLifecycleState::Starting,
        AgentHostLifecycleState::Ready => FfiAgentHostLifecycleState::Ready,
        AgentHostLifecycleState::Stopping => FfiAgentHostLifecycleState::Stopping,
        AgentHostLifecycleState::Stopped => FfiAgentHostLifecycleState::Stopped,
        AgentHostLifecycleState::Failed { failure } => FfiAgentHostLifecycleState::Failed {
            failure: FfiAgentHostFailure {
                code: ffi_host_error_code(failure.code),
                reason: failure.reason,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;

    use envoix_client::product::AgentResponseEnvelope;

    use super::*;
    use crate::FfiApplicationVaultError;

    #[derive(Default)]
    struct MemoryVault {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl FfiApplicationVault for MemoryVault {
        fn contains(&self, reference: String) -> Result<bool, FfiApplicationVaultError> {
            Ok(self.values.lock().unwrap().contains_key(&reference))
        }

        fn store(
            &self,
            reference: String,
            opaque_credential: Vec<u8>,
        ) -> Result<(), FfiApplicationVaultError> {
            self.values
                .lock()
                .unwrap()
                .insert(reference, opaque_credential);
            Ok(())
        }

        fn load(&self, reference: String) -> Result<Option<Vec<u8>>, FfiApplicationVaultError> {
            Ok(self.values.lock().unwrap().get(&reference).cloned())
        }

        fn delete(&self, reference: String) -> Result<(), FfiApplicationVaultError> {
            self.values.lock().unwrap().remove(&reference);
            Ok(())
        }
    }

    #[test]
    fn api_v25_advertises_the_agent_host_control_capability() {
        let info = crate::envoix_core_info();
        assert_eq!(info.ffi_api_version, 25);
        assert!(
            info.capabilities
                .iter()
                .any(|capability| capability == "agent_host_control_v2")
        );
    }

    #[test]
    fn typed_requests_cover_every_agent_command() {
        let sensitive = FfiAgentRequest::JoinPairing {
            pairing: FfiAgentPairingInput {
                label: "Fixture WSL".into(),
                invitation: "123456-debug-redaction".into(),
                verification_code: "654321".into(),
            },
        };
        let debug = format!("{sensitive:?}");
        assert!(!debug.contains("debug-redaction"));
        assert!(!debug.contains("654321"));

        let requests = vec![
            FfiAgentRequest::Status,
            FfiAgentRequest::Snapshot { inbox_limit: 20 },
            FfiAgentRequest::Events {
                after: FfiAgentEventCursor {
                    instance_id: "agent_fixture".into(),
                    sequence: 0,
                },
                limit: 64,
            },
            FfiAgentRequest::Pair {
                label: "Fixture Mac".into(),
            },
            FfiAgentRequest::JoinPairing {
                pairing: FfiAgentPairingInput {
                    label: "Fixture WSL".into(),
                    invitation: "123456-fixture-room".into(),
                    verification_code: "654321".into(),
                },
            },
            FfiAgentRequest::ListDevices,
            FfiAgentRequest::RevokeDevice {
                device: "relationship_fixture".into(),
            },
            FfiAgentRequest::UpdateDeviceRoute {
                device: "relationship_fixture".into(),
                broker: envoix_client::DEFAULT_RENDEZVOUS_BROKER.into(),
                relay: Some(envoix_client::DEFAULT_RELAY_URL.into()),
            },
            FfiAgentRequest::CreateTransfer {
                device: "relationship_fixture".into(),
                paths: vec!["/tmp/fixture.txt".into()],
            },
            FfiAgentRequest::ListTransfers,
            FfiAgentRequest::ListTransferPaths,
            FfiAgentRequest::GetTransfer {
                transfer_id: "transfer_fixture".into(),
            },
            FfiAgentRequest::ListPendingOffers,
            FfiAgentRequest::DecidePendingOffer {
                offer_id: "offer_fixture".into(),
                decision: FfiAgentOfferDecision::Approve,
            },
            FfiAgentRequest::ListInbox { limit: 20 },
            FfiAgentRequest::LatestInbox,
            FfiAgentRequest::Diagnostics,
        ];

        for request in requests {
            core_agent_request(request).unwrap();
        }
    }

    #[test]
    fn invalid_typed_request_fails_before_control_transport() {
        let error = core_agent_request(FfiAgentRequest::CreateTransfer {
            device: "relationship_fixture".into(),
            paths: Vec::new(),
        })
        .unwrap_err();

        assert!(matches!(
            error,
            FfiAgentControlError::Failed {
                code: FfiAgentControlErrorCode::InvalidInput,
                ..
            }
        ));
    }

    #[test]
    fn incompatible_protocol_has_a_stable_control_error() {
        let error = control_io_error(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported Agent protocol 9; expected 10",
        ));

        assert!(matches!(
            error,
            FfiAgentControlError::Failed {
                code: FfiAgentControlErrorCode::IncompatibleProtocol,
                ..
            }
        ));
    }

    #[test]
    fn typed_responses_cover_every_agent_response() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v9.json"
        ))
        .unwrap();
        let responses = fixture["responses"].as_array().unwrap();

        for response in responses {
            let envelope: AgentResponseEnvelope = serde_json::from_value(response.clone()).unwrap();
            let _ = ffi_agent_response(envelope.response);
        }
    }

    #[test]
    fn pairing_debug_output_redacts_ephemeral_codes() {
        let pairing = FfiAgentPairingInvitation {
            label: "Mac".into(),
            room_code: "room-secret".into(),
            verification_code: "verify-secret".into(),
            expires_at_unix_seconds: 1,
        };

        let output = format!("{pairing:?}");
        assert!(!output.contains("room-secret"));
        assert!(!output.contains("verify-secret"));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn ffi_host_reaches_readiness_serves_typed_control_and_releases_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let state_directory = directory.path().join("state");
        let inbox_directory = directory.path().join("inbox");
        let control_endpoint = directory.path().join("agent.sock");
        let configuration = FfiAgentHostConfiguration {
            state_directory: state_directory.display().to_string(),
            inbox_directory: inbox_directory.display().to_string(),
            control_endpoint: control_endpoint.display().to_string(),
            device_name: "ffi-host-test".into(),
            broker: envoix_client::DEFAULT_RENDEZVOUS_BROKER.into(),
            relay: None,
            credential_protection: FfiAgentCredentialProtection::AppleKeychain,
        };
        let vault: Arc<dyn FfiApplicationVault> = Arc::new(MemoryVault::default());
        let host = FfiAgentHost::start(configuration.clone(), vault.clone()).unwrap();

        let ready = tokio::time::timeout(Duration::from_secs(5), host.wait_until_ready())
            .await
            .expect("FFI Agent host did not become ready")
            .unwrap();
        assert_eq!(ready.agent_protocol_version, AGENT_PROTOCOL_VERSION);
        assert_eq!(host.lifecycle(), FfiAgentHostLifecycleState::Ready);

        let contender = FfiAgentHost::start(configuration, vault).unwrap();
        let contention = tokio::time::timeout(Duration::from_secs(5), contender.wait_until_ready())
            .await
            .expect("competing FFI Agent host did not report its failure")
            .unwrap_err();
        assert!(matches!(
            contention,
            FfiAgentHostError::Failed {
                code: FfiAgentHostErrorCode::StateAlreadyOwned,
                ..
            }
        ));
        assert!(matches!(
            contender.lifecycle(),
            FfiAgentHostLifecycleState::Failed {
                failure: FfiAgentHostFailure {
                    code: FfiAgentHostErrorCode::StateAlreadyOwned,
                    ..
                }
            }
        ));

        let client = FfiAgentControlClient::new(ready.control_endpoint).unwrap();
        let response = client.call(FfiAgentRequest::Diagnostics).await.unwrap();
        let FfiAgentResponse::Diagnostics { diagnostics } = response else {
            panic!("expected typed Agent diagnostics")
        };
        assert_eq!(
            diagnostics.credential_protection,
            FfiAgentCredentialProtection::AppleKeychain
        );
        assert_eq!(diagnostics.agent_protocol_version, AGENT_PROTOCOL_VERSION);

        let stopped = tokio::time::timeout(Duration::from_secs(5), host.shutdown())
            .await
            .expect("FFI Agent host did not shut down")
            .unwrap();
        assert_eq!(stopped, FfiAgentHostLifecycleState::Stopped);
        assert!(!control_endpoint.exists());
        assert_eq!(
            host.shutdown().await.unwrap(),
            FfiAgentHostLifecycleState::Stopped
        );
    }
}
