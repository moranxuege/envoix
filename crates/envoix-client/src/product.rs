//! Shared product-level state and the local Agent wire contract.
//!
//! Transfer bytes still flow through the canonical Manifest v2 session APIs.
//! This module names the user-facing concepts above that protocol: remembered
//! devices, a durable Inbox, and commands exchanged with a local Agent.

use std::collections::BTreeSet;
use std::env;
#[cfg(test)]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::api::DesktopCredentialStore;
use crate::api::RememberedCredential;
use crate::command::{CommandEnvelope, EngineCommand};
use crate::decision::decide;
use crate::effect::EngineEffect;
use crate::event::{EngineEvent, EventEnvelope};
use crate::model::{
    CommandId, ContentId, DeviceId, FailureCode, FailurePhase, RecoveryAction, RelationshipId,
    RelationshipState, Transfer, TransferDirection, TransferFailure, TransferId, TransferRejection,
    TransferState,
};
use crate::ports::{PlatformPortError, SecretBytes, SecureVaultPort};
use crate::snapshot::{ApplyOutcome, EngineSnapshot};
use crate::storage::{
    DurableRelationship, EngineState, EngineStore, EngineStoreError, MAX_DURABLE_ENTITIES,
    VaultReference,
};

pub const AGENT_PROTOCOL_VERSION: u16 = 14;
pub const AGENT_SETTINGS_VERSION: u16 = 2;
pub const AGENT_PREFERENCES_VERSION: u16 = 1;
pub const MAX_AGENT_REQUEST_BYTES: u64 = 64 * 1024;
pub const MAX_AGENT_RESPONSE_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_AGENT_EVENT_BATCH: usize = 256;
pub const MAX_AGENT_ACTIVE_PATHS: usize = 256;
pub const MAX_AGENT_TRANSFER_TELEMETRY: usize = 256;
pub const MAX_AGENT_PENDING_OFFERS: usize = 64;
pub const MAX_AGENT_TRANSFER_PATHS: usize = 64;
const MAX_AGENT_REQUEST_ID_BYTES: usize = 64;
const MAX_AGENT_OFFER_ID_BYTES: usize = 128;
const MAX_AGENT_OFFER_ROOTS: usize = 3;
const MAX_AGENT_OFFER_ROOT_NAME_BYTES: usize = 255;
const MAX_AGENT_ENDPOINT_BYTES: usize = 2_048;
const MAX_AGENT_PATH_BYTES: usize = 4_096;
const MAX_AGENT_PAIRING_INVITATION_BYTES: usize = 8 * 1_024;
const MAX_INBOX_ITEMS: usize = 1_000;
#[cfg(any(target_os = "macos", test))]
const MACOS_AGENT_STATE_RELATIVE_PATH: &str = "Library/Application Support/com.envoix.app/agent-v1";
#[cfg(any(target_os = "macos", test))]
const AGENT_CONTROL_SOCKET_NAME: &str = "agent.sock";

/// User-owned settings loaded by a managed Agent process.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSettings {
    pub version: u16,
    pub device_name: String,
    pub inbox_directory: PathBuf,
    #[serde(default = "default_agent_broker")]
    pub broker: String,
    #[serde(default = "default_agent_relay")]
    pub relay: Option<String>,
}

pub fn default_agent_broker() -> String {
    crate::DEFAULT_RENDEZVOUS_BROKER.to_string()
}

pub fn default_agent_relay() -> Option<String> {
    Some(crate::DEFAULT_RELAY_URL.to_string())
}

impl AgentSettings {
    pub fn validate(&self) -> io::Result<()> {
        if self.version == 0 || self.version > AGENT_SETTINGS_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported Agent settings version {}", self.version),
            ));
        }
        if validate_label(&self.device_name)? != self.device_name {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Agent device name cannot have leading or trailing whitespace",
            ));
        }
        if !self.inbox_directory.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Agent Inbox directory must be an absolute path",
            ));
        }
        crate::api::parse_broker_addr(&self.broker, self.relay.as_deref())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        Ok(())
    }
}

/// Small user preference file owned by the running Agent.
///
/// Managed-service settings still choose identity and deployment defaults;
/// this separate record lets a local authenticated UI change the receive
/// destination without rewriting service definitions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPreferences {
    pub version: u16,
    pub inbox_directory: PathBuf,
}

impl AgentPreferences {
    pub fn validate(&self) -> io::Result<()> {
        if self.version != AGENT_PREFERENCES_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported Agent preferences version {}", self.version),
            ));
        }
        validate_agent_directory_path(&self.inbox_directory)
    }
}

/// One request is sent as one JSON line over the local Agent socket.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentRequest {
    Status,
    Snapshot {
        inbox_limit: usize,
    },
    Events {
        after: AgentEventCursor,
        limit: usize,
    },
    Pair {
        label: String,
    },
    JoinPairing {
        pairing: AgentPairingInput,
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
        paths: Vec<PathBuf>,
    },
    ListTransfers,
    ListTransferPaths,
    GetTransfer {
        transfer_id: String,
    },
    PauseTransfer {
        transfer_id: String,
    },
    ResumeTransfer {
        transfer_id: String,
    },
    RecoverTransfer {
        transfer_id: String,
    },
    CancelTransfer {
        transfer_id: String,
    },
    RemoveTransfer {
        transfer_id: String,
    },
    SetInboxDirectory {
        path: PathBuf,
    },
    ListPendingOffers,
    DecidePendingOffer {
        offer_id: String,
        decision: AgentOfferDecision,
    },
    ListInbox {
        limit: usize,
    },
    LatestInbox,
    Diagnostics,
}

/// One response is returned as one JSON line over the local Agent socket.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentResponse {
    Status {
        status: AgentStatus,
    },
    Pairing {
        pairing: PairingInvitation,
    },
    DevicePaired {
        device: DeviceSummary,
    },
    Devices {
        devices: Vec<DeviceSummary>,
    },
    DeviceRevoked {
        device: DeviceSummary,
    },
    DeviceRouteUpdated {
        device: DeviceSummary,
    },
    TransferCreated {
        transfer: Transfer,
    },
    Transfers {
        transfers: Vec<Transfer>,
    },
    TransferPaths {
        paths: Vec<AgentTransferPath>,
    },
    Transfer {
        transfer: Transfer,
    },
    TransferRemoved {
        transfer_id: String,
    },
    PreferencesUpdated {
        preferences: AgentPreferences,
    },
    PendingOffers {
        offers: Vec<AgentPendingOffer>,
    },
    PendingOfferDecided {
        offer: AgentPendingOffer,
        decision: AgentOfferDecision,
    },
    Inbox {
        items: Vec<InboxItem>,
    },
    Latest {
        item: Option<InboxItem>,
    },
    Snapshot {
        snapshot: Box<AgentSnapshot>,
    },
    Events {
        cursor: AgentEventCursor,
        events: Vec<AgentEventEnvelope>,
    },
    SnapshotRequired {
        cursor: AgentEventCursor,
    },
    Diagnostics {
        diagnostics: AgentDiagnostics,
    },
    Error {
        code: String,
        message: String,
    },
}

impl AgentResponse {
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRequestEnvelope {
    pub protocol_version: u16,
    pub request_id: String,
    pub request: AgentRequest,
}

impl AgentRequestEnvelope {
    pub fn new(request_id: impl Into<String>, request: AgentRequest) -> Result<Self, io::Error> {
        let request_id = validate_request_id(request_id.into())?;
        Ok(Self {
            protocol_version: AGENT_PROTOCOL_VERSION,
            request_id,
            request,
        })
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.protocol_version != AGENT_PROTOCOL_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "unsupported Agent protocol {}; expected {}",
                    self.protocol_version, AGENT_PROTOCOL_VERSION
                ),
            ));
        }
        validate_request_id(self.request_id.clone())?;
        match &self.request {
            AgentRequest::Events { after, .. } => after.validate()?,
            AgentRequest::JoinPairing { pairing } => pairing.validate()?,
            AgentRequest::UpdateDeviceRoute {
                device,
                broker,
                relay,
            } => {
                validate_device_selector(device)?;
                crate::api::parse_broker_addr(broker, relay.as_deref()).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
                })?;
            }
            AgentRequest::CreateTransfer { device, paths } => {
                validate_device_selector(device)?;
                if paths.is_empty() || paths.len() > MAX_AGENT_TRANSFER_PATHS {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "Agent Transfer requires 1 to {MAX_AGENT_TRANSFER_PATHS} source paths"
                        ),
                    ));
                }
                for path in paths {
                    validate_agent_source_path(path)?;
                }
            }
            AgentRequest::GetTransfer { transfer_id }
            | AgentRequest::PauseTransfer { transfer_id }
            | AgentRequest::ResumeTransfer { transfer_id }
            | AgentRequest::RecoverTransfer { transfer_id }
            | AgentRequest::CancelTransfer { transfer_id }
            | AgentRequest::RemoveTransfer { transfer_id } => {
                TransferId::parse(transfer_id.clone()).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
                })?;
            }
            AgentRequest::SetInboxDirectory { path } => {
                validate_agent_directory_path(path)?;
            }
            AgentRequest::DecidePendingOffer { offer_id, .. } => {
                validate_agent_offer_id(offer_id)?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentResponseEnvelope {
    pub protocol_version: u16,
    pub request_id: String,
    pub response: AgentResponse,
}

impl AgentResponseEnvelope {
    pub fn new(request_id: impl Into<String>, response: AgentResponse) -> io::Result<Self> {
        let request_id = validate_request_id(request_id.into())?;
        Ok(Self {
            protocol_version: AGENT_PROTOCOL_VERSION,
            request_id,
            response,
        })
    }

    pub fn validate_for(&self, request_id: &str) -> io::Result<()> {
        if self.protocol_version != AGENT_PROTOCOL_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "unsupported Agent protocol {}; expected {}",
                    self.protocol_version, AGENT_PROTOCOL_VERSION
                ),
            ));
        }
        validate_request_id(self.request_id.clone())?;
        if self.request_id != request_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Agent response request ID does not match the command",
            ));
        }
        match &self.response {
            AgentResponse::Status { status } => validate_agent_status(status)?,
            AgentResponse::Snapshot { snapshot } => {
                validate_agent_status(&snapshot.status)?;
                snapshot.engine.validate_contract().map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
                })?;
                if snapshot.inbox.len() > MAX_INBOX_ITEMS {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Agent snapshot contains too many Inbox items",
                    ));
                }
                validate_agent_transfer_paths(&snapshot.active_paths)?;
                validate_agent_transfer_telemetry(&snapshot.telemetry)?;
                validate_agent_pending_offers(&snapshot.pending_offers)?;
                snapshot.event_cursor.validate()?;
            }
            AgentResponse::Events { cursor, events } => {
                cursor.validate()?;
                if events.len() > MAX_AGENT_EVENT_BATCH {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Agent event response exceeds the batch limit",
                    ));
                }
                let mut previous_sequence = None;
                for event in events {
                    event.validate()?;
                    if event.instance_id != cursor.instance_id
                        || event.sequence > cursor.sequence
                        || previous_sequence.is_some_and(|previous| event.sequence <= previous)
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Agent events do not match their cursor",
                        ));
                    }
                    previous_sequence = Some(event.sequence);
                }
                if previous_sequence.is_some_and(|sequence| sequence != cursor.sequence) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Agent event cursor does not identify the final event",
                    ));
                }
            }
            AgentResponse::SnapshotRequired { cursor } => cursor.validate()?,
            AgentResponse::TransferCreated { transfer } | AgentResponse::Transfer { transfer } => {
                validate_agent_transfer(transfer)?
            }
            AgentResponse::TransferRemoved { transfer_id } => {
                TransferId::parse(transfer_id.clone()).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
                })?;
            }
            AgentResponse::PreferencesUpdated { preferences } => {
                preferences.validate()?;
            }
            AgentResponse::Transfers { transfers } => {
                if transfers.len() > MAX_DURABLE_ENTITIES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Agent response contains too many Transfers",
                    ));
                }
                let mut ids = BTreeSet::new();
                for transfer in transfers {
                    validate_agent_transfer(transfer)?;
                    if !ids.insert(&transfer.id) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Agent response contains duplicate Transfers",
                        ));
                    }
                }
            }
            AgentResponse::TransferPaths { paths } => {
                validate_agent_transfer_paths(paths)?;
            }
            AgentResponse::PendingOffers { offers } => {
                validate_agent_pending_offers(offers)?;
            }
            AgentResponse::PendingOfferDecided { offer, .. } => {
                validate_agent_pending_offer(offer)?;
            }
            AgentResponse::DevicePaired { device }
            | AgentResponse::DeviceRevoked { device }
            | AgentResponse::DeviceRouteUpdated { device } => {
                validate_agent_device(device)?;
            }
            AgentResponse::Devices { devices } => {
                if devices.len() > MAX_DURABLE_ENTITIES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Agent response contains too many Devices",
                    ));
                }
                let mut ids = BTreeSet::new();
                let mut labels = BTreeSet::new();
                for device in devices {
                    validate_agent_device(device)?;
                    if !ids.insert(&device.id) || !labels.insert(device.label.to_ascii_lowercase())
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Agent response contains duplicate Devices",
                        ));
                    }
                }
            }
            AgentResponse::Diagnostics { diagnostics } => {
                diagnostics.validate()?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStatus {
    pub protocol_version: u16,
    pub pid: u32,
    pub device_name: String,
    pub state_directory: String,
    pub inbox_directory: String,
    pub broker: String,
    pub relay: Option<String>,
    pub paired_devices: usize,
    pub active_receivers: usize,
    pub active_pairings: usize,
    pub active_paths: usize,
    pub pending_offers: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentControlTransport {
    UnixSocket,
    WindowsNamedPipe,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCredentialProtection {
    OwnerOnlyFile,
    WindowsDpapi,
    AppleKeychain,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentDiagnostics {
    pub agent_protocol_version: u16,
    pub application_contract_version: u16,
    pub engine_schema_version: u16,
    pub platform: String,
    pub control_transport: AgentControlTransport,
    pub credential_protection: AgentCredentialProtection,
    pub engine_sequence: u64,
    pub relationships: usize,
    pub transfers: usize,
    pub inbox_items: usize,
    pub active_paths: usize,
    pub pending_offers: usize,
}

impl AgentDiagnostics {
    fn validate(&self) -> io::Result<()> {
        if self.agent_protocol_version != AGENT_PROTOCOL_VERSION
            || self.application_contract_version != crate::APPLICATION_CONTRACT_VERSION
            || self.engine_schema_version != crate::storage::ENGINE_STATE_SCHEMA_VERSION
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Agent diagnostics report incompatible contract versions",
            ));
        }
        if self.platform.is_empty()
            || self.platform.len() > 64
            || self.platform.chars().any(char::is_control)
            || self.relationships > MAX_DURABLE_ENTITIES
            || self.transfers > MAX_DURABLE_ENTITIES
            || self.inbox_items > MAX_INBOX_ITEMS
            || self.active_paths > MAX_AGENT_ACTIVE_PATHS
            || self.pending_offers > MAX_AGENT_PENDING_OFFERS
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Agent diagnostics report invalid bounded values",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSnapshot {
    pub status: AgentStatus,
    pub engine: EngineSnapshot,
    pub inbox: Vec<InboxItem>,
    pub active_paths: Vec<AgentTransferPath>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub telemetry: Vec<AgentTransferTelemetry>,
    pub pending_offers: Vec<AgentPendingOffer>,
    pub event_cursor: AgentEventCursor,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEventCursor {
    pub instance_id: String,
    pub sequence: u64,
}

impl AgentEventCursor {
    pub fn validate(&self) -> io::Result<()> {
        validate_request_id(self.instance_id.clone()).map(|_| ())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRelationshipChange {
    Trusted,
    Rotated,
    RouteUpdated,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentEvent {
    PairingChanged {
        label: String,
        active: bool,
    },
    RelationshipChanged {
        relationship_id: String,
        change: AgentRelationshipChange,
    },
    InboxChanged {
        item_id: String,
    },
    TransferChanged {
        transfer_id: String,
    },
    TransferPathChanged {
        transfer_id: String,
        direction: TransferDirection,
        path: Option<AgentPathKind>,
    },
    TransferTelemetryChanged {
        transfer_id: String,
    },
    InboxDirectoryChanged,
    PendingOfferChanged {
        offer_id: String,
        pending: bool,
    },
}

impl AgentEvent {
    fn validate(&self) -> io::Result<()> {
        match self {
            Self::PairingChanged { label, .. } => {
                if validate_label(label)? != *label {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "pairing label cannot have leading or trailing whitespace",
                    ));
                }
            }
            Self::RelationshipChanged {
                relationship_id, ..
            } => {
                RelationshipId::parse(relationship_id.clone()).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
                })?;
            }
            Self::InboxChanged { item_id } => {
                TransferId::parse(item_id.clone()).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
                })?;
            }
            Self::TransferChanged { transfer_id }
            | Self::TransferPathChanged { transfer_id, .. }
            | Self::TransferTelemetryChanged { transfer_id } => {
                TransferId::parse(transfer_id.clone()).map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
                })?;
            }
            Self::PendingOfferChanged { offer_id, .. } => validate_agent_offer_id(offer_id)?,
            Self::InboxDirectoryChanged => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEventEnvelope {
    pub protocol_version: u16,
    pub instance_id: String,
    pub sequence: u64,
    pub event: AgentEvent,
}

impl AgentEventEnvelope {
    pub fn new(
        instance_id: impl Into<String>,
        sequence: u64,
        event: AgentEvent,
    ) -> io::Result<Self> {
        let instance_id = validate_request_id(instance_id.into())?;
        if sequence == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Agent event sequence must be non-zero",
            ));
        }
        event.validate()?;
        Ok(Self {
            protocol_version: AGENT_PROTOCOL_VERSION,
            instance_id,
            sequence,
            event,
        })
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.protocol_version != AGENT_PROTOCOL_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "unsupported Agent protocol {}; expected {}",
                    self.protocol_version, AGENT_PROTOCOL_VERSION
                ),
            ));
        }
        validate_request_id(self.instance_id.clone())?;
        if self.sequence == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Agent event sequence must be non-zero",
            ));
        }
        self.event.validate()?;
        Ok(())
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairingInvitation {
    pub label: String,
    pub room_code: String,
    pub verification_code: String,
    pub expires_at_unix_seconds: u64,
}

impl std::fmt::Debug for PairingInvitation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PairingInvitation")
            .field("label", &self.label)
            .field("room_code", &"<redacted>")
            .field("verification_code", &"<redacted>")
            .field("expires_at_unix_seconds", &self.expires_at_unix_seconds)
            .finish()
    }
}

/// One-time inputs used by the Agent to own first-contact verification.
///
/// These values cross only the owner-scoped local control endpoint. The
/// resulting long-lived credential never leaves the Agent's vault boundary.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPairingInput {
    pub label: String,
    pub invitation: String,
    pub verification_code: String,
}

impl std::fmt::Debug for AgentPairingInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentPairingInput")
            .field("label", &self.label)
            .field("invitation", &"<redacted>")
            .field("verification_code", &"<redacted>")
            .finish()
    }
}

impl AgentPairingInput {
    pub fn validate(&self) -> io::Result<()> {
        if validate_label(&self.label)? != self.label {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "device label cannot have leading or trailing whitespace",
            ));
        }
        if self.invitation.is_empty()
            || self.invitation.len() > MAX_AGENT_PAIRING_INVITATION_BYTES
            || self.invitation.trim() != self.invitation
            || self.invitation.chars().any(char::is_control)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Room invitation must be a bounded, non-empty single-line value",
            ));
        }
        if self.verification_code.len() != 6
            || !self
                .verification_code
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "device verification code must contain exactly six digits",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSummary {
    pub id: String,
    pub label: String,
    pub generation: u64,
    pub previous_generation: Option<u64>,
    pub broker: String,
    pub relay: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOfferDecision {
    Approve,
    Reject,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPathKind {
    Lan,
    Direct,
    Relay,
    WifiAware,
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTransferPath {
    pub transfer_id: String,
    pub direction: TransferDirection,
    pub path: AgentPathKind,
}

/// Ephemeral measurements projected by the Agent for a live Transfer.
///
/// The Engine's byte checkpoint remains authoritative after restart; rate and
/// ETA are intentionally kept out of durable state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTransferPhase {
    Pairing,
    Connecting,
    Authenticating,
    Negotiating,
    Transferring,
    Verifying,
    Saving,
    WaitingForReceiver,
    Finalizing,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTransferTelemetry {
    pub transfer_id: String,
    pub relationship_id: String,
    pub direction: TransferDirection,
    pub root_names: Vec<String>,
    pub item_count: u32,
    pub directory_count: u32,
    pub phase: AgentTransferPhase,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub current_bytes_per_second: u64,
    pub average_bytes_per_second: u64,
    pub eta_seconds: Option<u64>,
    pub sampled_at_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPendingOffer {
    pub offer_id: String,
    pub from_device_id: String,
    pub from_device_label: String,
    pub root_names: Vec<String>,
    pub item_count: u32,
    pub directory_count: u32,
    pub total_bytes: u64,
    pub allocatable_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InboxRoot {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InboxItem {
    pub id: String,
    pub received_at_unix_ms: u64,
    pub from_device_id: String,
    pub from_device_label: String,
    pub roots: Vec<InboxRoot>,
    pub file_count: u32,
    pub directory_count: u32,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedRememberedDevice {
    id: String,
    label: String,
    credential_reference: String,
    broker: String,
    relay: Option<String>,
}

impl PreparedRememberedDevice {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RememberedDeviceRecord {
    id: String,
    label: String,
    credential_reference: String,
    generation: u64,
    previous_generation: Option<u64>,
    broker: String,
    relay: Option<String>,
}

impl std::fmt::Debug for RememberedDeviceRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RememberedDeviceRecord")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("credential_reference", &"<redacted>")
            .field("generation", &self.generation)
            .field("previous_generation", &self.previous_generation)
            .field("broker", &self.broker)
            .field("relay", &self.relay)
            .finish()
    }
}

impl RememberedDeviceRecord {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn previous_generation(&self) -> Option<u64> {
        self.previous_generation
    }

    pub fn broker(&self) -> &str {
        &self.broker
    }

    pub fn relay(&self) -> Option<&str> {
        self.relay.as_deref()
    }

    pub fn summary(&self) -> DeviceSummary {
        DeviceSummary {
            id: self.id.clone(),
            label: self.label.clone(),
            generation: self.generation,
            previous_generation: self.previous_generation,
            broker: self.broker.clone(),
            relay: self.relay.clone(),
        }
    }
}

/// Agent-facing projection of the unified Engine store and desktop vault.
pub struct ProductStore {
    engine: EngineStore,
    vault: Arc<dyn SecureVaultPort>,
}

impl ProductStore {
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self, EngineStoreError> {
        let directory = directory.into();
        let vault = Arc::new(DesktopCredentialStore::new(directory.join("vault")));
        Self::open_with_vault(directory, vault)
    }

    pub fn open_with_vault(
        directory: impl Into<PathBuf>,
        vault: Arc<dyn SecureVaultPort>,
    ) -> Result<Self, EngineStoreError> {
        let directory = directory.into();
        let engine = EngineStore::open(&directory)?;
        Ok(Self { engine, vault })
    }

    pub fn prepare_device(
        &self,
        label: &str,
        broker: &str,
        relay: Option<&str>,
    ) -> Result<PreparedRememberedDevice, EngineStoreError> {
        let label = validate_label(label)?;
        if self
            .device_records()
            .iter()
            .any(|device| device.label.eq_ignore_ascii_case(&label))
        {
            return Err(EngineStoreError::InvalidState(format!(
                "device label {label:?} is already paired"
            )));
        }
        let prepared = PreparedRememberedDevice {
            id: random_identifier("dev")?,
            label,
            credential_reference: random_identifier("cred")?,
            broker: broker.to_string(),
            relay: relay.map(str::to_string),
        };
        let durable = DurableRelationship {
            vault_reference: Some(VaultReference::parse(
                prepared.credential_reference.clone(),
            )?),
            broker: prepared.broker.clone(),
            relay: prepared.relay.clone(),
        };
        durable.validate(RelationshipState::Trusted)?;
        Ok(prepared)
    }

    pub fn commit_device(
        &mut self,
        prepared: PreparedRememberedDevice,
        opaque_credential: &[u8],
        generation: u64,
    ) -> Result<DeviceSummary, EngineStoreError> {
        RememberedCredential::from_opaque(opaque_credential).map_err(|_| {
            EngineStoreError::InvalidState("remembered credential is corrupt or unsupported".into())
        })?;
        if self.engine.state().snapshot.devices.contains_key(
            &DeviceId::parse(prepared.id.clone())
                .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?,
        ) || self
            .device_records()
            .iter()
            .any(|device| device.label.eq_ignore_ascii_case(&prepared.label))
        {
            return Err(EngineStoreError::InvalidState(
                "remembered device already exists".into(),
            ));
        }
        let vault_reference = VaultReference::parse(prepared.credential_reference.clone())?;
        if self.vault.contains(&vault_reference)? {
            return Err(EngineStoreError::InvalidState(
                "prepared vault reference already exists".into(),
            ));
        }

        let device_id = DeviceId::parse(prepared.id.clone())
            .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?;
        let relationship_id = RelationshipId::parse(prepared.id.clone())
            .expect("Device and Relationship identifiers share validation");
        let mut state = self.engine.state().clone();
        apply_product_event(
            &mut state,
            EngineEvent::DeviceObserved {
                device_id: device_id.clone(),
                display_name: prepared.label.clone(),
            },
        )?;
        apply_product_event(
            &mut state,
            EngineEvent::RelationshipTrusted {
                relationship_id: relationship_id.clone(),
                device_id,
                generation,
            },
        )?;
        state.durable_relationships.insert(
            relationship_id,
            DurableRelationship {
                vault_reference: Some(vault_reference.clone()),
                broker: prepared.broker,
                relay: prepared.relay,
            },
        );

        let secret = SecretBytes::new(opaque_credential.to_vec())?;
        self.vault.store(&vault_reference, &secret)?;
        if let Err(error) = self.engine.replace(state) {
            self.vault.delete(&vault_reference).map_err(|rollback| {
                EngineStoreError::InvalidState(format!(
                    "{error}; vault rollback also failed: {rollback}"
                ))
            })?;
            return Err(error);
        }
        self.device_record(&prepared.id)
            .ok_or_else(|| EngineStoreError::InvalidState("committed device is missing".into()))
            .map(|record| record.summary())
    }

    pub fn rotate_device(
        &mut self,
        id: &str,
        opaque_credential: &[u8],
        generation: u64,
    ) -> Result<(), EngineStoreError> {
        RememberedCredential::from_opaque(opaque_credential).map_err(|_| {
            EngineStoreError::InvalidState("remembered credential is corrupt or unsupported".into())
        })?;
        let current = self
            .device_record(id)
            .ok_or_else(|| EngineStoreError::InvalidState("remembered device is missing".into()))?;
        if generation < current.generation {
            return Err(EngineStoreError::InvalidState(
                "remembered generation moved backwards".into(),
            ));
        }
        if generation == current.generation {
            return Ok(());
        }
        let vault_reference = VaultReference::parse(current.credential_reference.clone())?;
        let old_credential = self
            .vault
            .load(&vault_reference)?
            .ok_or(EngineStoreError::MissingVaultCredential)?;
        RememberedCredential::from_opaque(old_credential.expose())
            .map_err(|_| crate::ports::PlatformPortError::CorruptData)?;
        let changed = old_credential.expose() != opaque_credential;
        if changed {
            let secret = SecretBytes::new(opaque_credential.to_vec())?;
            self.vault.store(&vault_reference, &secret)?;
        }

        let relationship_id =
            RelationshipId::parse(current.id.clone()).expect("stored device ID was validated");
        let mut state = self.engine.state().clone();
        apply_product_event(
            &mut state,
            EngineEvent::RelationshipRotated {
                relationship_id,
                generation,
            },
        )?;
        if let Err(error) = self.engine.replace(state) {
            if changed {
                self.vault
                    .store(&vault_reference, &old_credential)
                    .map_err(|rollback| {
                        EngineStoreError::InvalidState(format!(
                            "{error}; vault rollback also failed: {rollback}"
                        ))
                    })?;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn devices(&self) -> Vec<DeviceSummary> {
        let mut devices = self
            .device_records()
            .into_iter()
            .map(|record| record.summary())
            .collect::<Vec<_>>();
        devices.sort_by(|left, right| {
            left.label
                .to_ascii_lowercase()
                .cmp(&right.label.to_ascii_lowercase())
        });
        devices
    }

    pub fn rename_device(
        &mut self,
        id: &str,
        label: &str,
    ) -> Result<DeviceSummary, EngineStoreError> {
        let label = validate_label(label)?;
        let record = self
            .device_record(id)
            .ok_or_else(|| EngineStoreError::InvalidState("remembered device is missing".into()))?;
        if record.label == label {
            return Ok(record.summary());
        }
        if self
            .device_records()
            .iter()
            .any(|device| device.id != id && device.label.eq_ignore_ascii_case(&label))
        {
            return Err(EngineStoreError::InvalidState(format!(
                "device label {label:?} is already paired"
            )));
        }

        let device_id = DeviceId::parse(record.id)
            .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?;
        let mut state = self.engine.state().clone();
        apply_product_event(
            &mut state,
            EngineEvent::DeviceObserved {
                device_id,
                display_name: label,
            },
        )?;
        self.engine.replace(state)?;
        self.device_record(id)
            .ok_or_else(|| EngineStoreError::InvalidState("renamed device is missing".into()))
            .map(|device| device.summary())
    }

    pub fn device_records(&self) -> Vec<RememberedDeviceRecord> {
        self.engine
            .state()
            .snapshot
            .relationships
            .iter()
            .filter_map(|(relationship_id, relationship)| {
                if relationship.state != RelationshipState::Trusted {
                    return None;
                }
                let device = self
                    .engine
                    .state()
                    .snapshot
                    .devices
                    .get(&relationship.device_id)?;
                let durable = self
                    .engine
                    .state()
                    .durable_relationships
                    .get(relationship_id)?;
                Some(RememberedDeviceRecord {
                    id: relationship_id.as_str().to_string(),
                    label: device.display_name.clone(),
                    credential_reference: durable.vault_reference.as_ref()?.as_str().to_string(),
                    generation: relationship.generation,
                    previous_generation: relationship.previous_generation,
                    broker: durable.broker.clone(),
                    relay: durable.relay.clone(),
                })
            })
            .collect()
    }

    pub fn device_record(&self, id: &str) -> Option<RememberedDeviceRecord> {
        self.device_records()
            .into_iter()
            .find(|device| device.id == id)
    }

    pub fn device_credential(&self, id: &str) -> Result<SecretBytes, EngineStoreError> {
        let device = self
            .device_record(id)
            .ok_or_else(|| EngineStoreError::InvalidState("remembered device is missing".into()))?;
        let reference = VaultReference::parse(device.credential_reference)?;
        let credential = self
            .vault
            .load(&reference)?
            .ok_or(EngineStoreError::MissingVaultCredential)?;
        RememberedCredential::from_opaque(credential.expose())
            .map_err(|_| crate::ports::PlatformPortError::CorruptData)?;
        Ok(credential)
    }

    pub fn forget_device(&mut self, selector: &str) -> Result<DeviceSummary, EngineStoreError> {
        let record = self.resolve_device(selector)?;
        let vault_reference = VaultReference::parse(record.credential_reference.clone())?;
        let credential = match self.vault.load(&vault_reference) {
            Ok(credential) => credential,
            Err(PlatformPortError::CorruptData) => None,
            Err(error) => return Err(error.into()),
        };
        self.vault.delete(&vault_reference)?;

        let relationship_id =
            RelationshipId::parse(record.id.clone()).expect("stored device ID was validated");
        let mut state = self.engine.state().clone();
        settle_transfers_for_revoked_relationship(&mut state, &relationship_id)?;
        apply_product_event(
            &mut state,
            EngineEvent::RelationshipRevoked {
                relationship_id: relationship_id.clone(),
            },
        )?;
        state
            .durable_relationships
            .get_mut(&relationship_id)
            .expect("trusted relationship has durable metadata")
            .vault_reference = None;
        if let Err(error) = self.engine.replace(state) {
            if let Some(credential) = credential {
                self.vault
                    .store(&vault_reference, &credential)
                    .map_err(|rollback| {
                        EngineStoreError::InvalidState(format!(
                            "{error}; vault rollback also failed: {rollback}"
                        ))
                    })?;
            }
            return Err(error);
        }
        Ok(record.summary())
    }

    pub fn update_device_route(
        &mut self,
        selector: &str,
        broker: &str,
        relay: Option<&str>,
    ) -> Result<DeviceSummary, EngineStoreError> {
        crate::api::parse_broker_addr(broker, relay)
            .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?;
        let record = self.resolve_device(selector)?;
        if record.broker == broker && record.relay() == relay {
            return Ok(record.summary());
        }
        let relationship_id =
            RelationshipId::parse(record.id.clone()).expect("stored device ID was validated");
        let mut state = self.engine.state().clone();
        let durable = state
            .durable_relationships
            .get_mut(&relationship_id)
            .ok_or_else(|| EngineStoreError::InvalidState("device route is missing".into()))?;
        durable.broker = broker.to_string();
        durable.relay = relay.map(str::to_string);
        self.engine.replace(state)?;
        self.device_record(&record.id)
            .ok_or_else(|| EngineStoreError::InvalidState("updated device is missing".into()))
            .map(|device| device.summary())
    }

    pub fn resolve_device(
        &self,
        selector: &str,
    ) -> Result<RememberedDeviceRecord, EngineStoreError> {
        let selector = selector.trim();
        validate_device_selector(selector).map_err(EngineStoreError::Io)?;
        self.device_records()
            .into_iter()
            .find(|device| device.id == selector || device.label.eq_ignore_ascii_case(selector))
            .ok_or_else(|| EngineStoreError::InvalidState("remembered device is missing".into()))
    }

    pub fn create_transfer(
        &mut self,
        device: &str,
        transfer_id: TransferId,
        content_id: ContentId,
        total_bytes: u64,
    ) -> Result<Transfer, EngineStoreError> {
        let record = self.resolve_device(device)?;
        let relationship_id = RelationshipId::parse(record.id)
            .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?;
        let command_id = CommandId::parse(random_identifier("command")?)
            .expect("generated command identifier is valid");
        let effect = decide(
            &self.engine.state().snapshot,
            CommandEnvelope {
                contract_version: crate::APPLICATION_CONTRACT_VERSION,
                command_id,
                command: EngineCommand::CreateTransfer {
                    relationship_id: relationship_id.clone(),
                    content_id: content_id.clone(),
                    direction: TransferDirection::Send,
                },
            },
        )
        .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?;
        match effect.effect {
            EngineEffect::CreateTransfer {
                relationship_id: decided_relationship,
                content_id: decided_content,
                direction: TransferDirection::Send,
            } if decided_relationship == relationship_id && decided_content == content_id => {}
            _ => {
                return Err(EngineStoreError::InvalidState(
                    "CreateTransfer decision returned an unexpected effect".into(),
                ));
            }
        }

        let mut state = self.engine.state().clone();
        apply_product_event(
            &mut state,
            EngineEvent::TransferCreated {
                transfer_id: transfer_id.clone(),
                relationship_id,
                room_id: None,
                content_id,
                direction: TransferDirection::Send,
                total_bytes,
            },
        )?;
        let transfer = state
            .snapshot
            .transfers
            .get(&transfer_id)
            .cloned()
            .ok_or_else(|| EngineStoreError::InvalidState("created Transfer is missing".into()))?;
        self.engine.replace(state)?;
        Ok(transfer)
    }

    pub fn transfers(&self) -> Vec<Transfer> {
        self.engine
            .state()
            .snapshot
            .transfers
            .values()
            .cloned()
            .collect()
    }

    pub fn transfer(&self, transfer_id: &str) -> Result<Option<Transfer>, EngineStoreError> {
        let transfer_id = TransferId::parse(transfer_id.to_string())
            .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?;
        Ok(self
            .engine
            .state()
            .snapshot
            .transfers
            .get(&transfer_id)
            .cloned())
    }

    pub fn dispatchable_transfers(
        &self,
        relationship_id: &str,
    ) -> Result<Vec<Transfer>, EngineStoreError> {
        let relationship_id = RelationshipId::parse(relationship_id.to_string())
            .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?;
        Ok(self
            .engine
            .state()
            .snapshot
            .transfers
            .values()
            .filter(|transfer| {
                transfer.relationship_id == relationship_id
                    && transfer.direction == TransferDirection::Send
                    && matches!(
                        transfer.state,
                        TransferState::Queued
                            | TransferState::Connecting
                            | TransferState::Transferring
                            | TransferState::AwaitingDeliveryProof
                    )
            })
            .cloned()
            .collect())
    }

    pub fn start_outgoing_transfer(
        &mut self,
        transfer_id: &TransferId,
    ) -> Result<Transfer, EngineStoreError> {
        let transfer = self.required_outgoing_transfer(transfer_id)?;
        match transfer.state {
            TransferState::Queued => self.apply_transfer_events(
                transfer_id,
                [EngineEvent::TransferStarted {
                    transfer_id: transfer_id.clone(),
                }],
            ),
            TransferState::Connecting
            | TransferState::Transferring
            | TransferState::AwaitingDeliveryProof => Ok(transfer),
            state => Err(EngineStoreError::InvalidState(format!(
                "cannot start outgoing Transfer {transfer_id} from {}",
                state.wire_name()
            ))),
        }
    }

    pub fn progress_outgoing_transfer(
        &mut self,
        transfer_id: &TransferId,
        transferred_bytes: u64,
    ) -> Result<Transfer, EngineStoreError> {
        let transfer = self.required_outgoing_transfer(transfer_id)?;
        if transferred_bytes == transfer.transferred_bytes
            || matches!(
                transfer.state,
                TransferState::AwaitingDeliveryProof | TransferState::Delivered
            ) && transferred_bytes == transfer.total_bytes
        {
            return Ok(transfer);
        }
        self.apply_transfer_events(
            transfer_id,
            [EngineEvent::TransferProgressed {
                transfer_id: transfer_id.clone(),
                transferred_bytes,
            }],
        )
    }

    pub fn complete_outgoing_transfer(
        &mut self,
        transfer_id: &TransferId,
    ) -> Result<Transfer, EngineStoreError> {
        let transfer = self.required_outgoing_transfer(transfer_id)?;
        let mut events = Vec::with_capacity(3);
        match transfer.state {
            TransferState::Connecting | TransferState::Transferring => {
                if transfer.transferred_bytes != transfer.total_bytes {
                    events.push(EngineEvent::TransferProgressed {
                        transfer_id: transfer_id.clone(),
                        transferred_bytes: transfer.total_bytes,
                    });
                }
                events.push(EngineEvent::TransferPayloadCompleted {
                    transfer_id: transfer_id.clone(),
                });
                events.push(EngineEvent::TransferDeliveryProofVerified {
                    transfer_id: transfer_id.clone(),
                });
            }
            TransferState::AwaitingDeliveryProof => {
                events.push(EngineEvent::TransferDeliveryProofVerified {
                    transfer_id: transfer_id.clone(),
                });
            }
            TransferState::Delivered => return Ok(transfer),
            state => {
                return Err(EngineStoreError::InvalidState(format!(
                    "cannot complete outgoing Transfer {transfer_id} from {}",
                    state.wire_name()
                )));
            }
        }
        self.apply_transfer_events(transfer_id, events)
    }

    pub fn reject_outgoing_transfer(
        &mut self,
        transfer_id: &TransferId,
        reason: TransferRejection,
    ) -> Result<Transfer, EngineStoreError> {
        self.required_outgoing_transfer(transfer_id)?;
        self.apply_transfer_events(
            transfer_id,
            [EngineEvent::TransferRejected {
                transfer_id: transfer_id.clone(),
                reason,
            }],
        )
    }

    pub fn fail_outgoing_transfer(
        &mut self,
        transfer_id: &TransferId,
        failure: TransferFailure,
    ) -> Result<Transfer, EngineStoreError> {
        let transfer = self.required_outgoing_transfer(transfer_id)?;
        if transfer.state == TransferState::Failed && transfer.failure.as_ref() == Some(&failure) {
            return Ok(transfer);
        }
        self.apply_transfer_events(
            transfer_id,
            [EngineEvent::TransferFailed {
                transfer_id: transfer_id.clone(),
                failure,
            }],
        )
    }

    pub fn cancel_outgoing_transfer(
        &mut self,
        transfer_id: &TransferId,
    ) -> Result<Transfer, EngineStoreError> {
        let transfer = self.required_outgoing_transfer(transfer_id)?;
        if transfer.state == TransferState::Canceled {
            return Ok(transfer);
        }
        self.apply_transfer_events(
            transfer_id,
            [EngineEvent::TransferCanceled {
                transfer_id: transfer_id.clone(),
            }],
        )
    }

    pub fn pause_transfer(
        &mut self,
        transfer_id: &TransferId,
    ) -> Result<Transfer, EngineStoreError> {
        self.control_transfer(EngineCommand::PauseTransfer {
            transfer_id: transfer_id.clone(),
        })?
        .ok_or_else(|| EngineStoreError::InvalidState("paused Transfer is missing".into()))
    }

    pub fn resume_transfer(
        &mut self,
        transfer_id: &TransferId,
    ) -> Result<Transfer, EngineStoreError> {
        self.control_transfer(EngineCommand::ResumeTransfer {
            transfer_id: transfer_id.clone(),
        })?
        .ok_or_else(|| EngineStoreError::InvalidState("resumed Transfer is missing".into()))
    }

    pub fn recover_transfer(
        &mut self,
        transfer_id: &TransferId,
    ) -> Result<Transfer, EngineStoreError> {
        self.control_transfer(EngineCommand::RecoverTransfer {
            transfer_id: transfer_id.clone(),
        })?
        .ok_or_else(|| EngineStoreError::InvalidState("recovering Transfer is missing".into()))
    }

    pub fn cancel_transfer(
        &mut self,
        transfer_id: &TransferId,
    ) -> Result<Transfer, EngineStoreError> {
        self.control_transfer(EngineCommand::CancelTransfer {
            transfer_id: transfer_id.clone(),
        })?
        .ok_or_else(|| EngineStoreError::InvalidState("canceled Transfer is missing".into()))
    }

    pub fn remove_transfer(&mut self, transfer_id: &TransferId) -> Result<(), EngineStoreError> {
        if self
            .control_transfer(EngineCommand::RemoveTransfer {
                transfer_id: transfer_id.clone(),
            })?
            .is_some()
        {
            return Err(EngineStoreError::InvalidState(
                "removed Transfer remains in the Engine snapshot".into(),
            ));
        }
        Ok(())
    }

    fn control_transfer(
        &mut self,
        command: EngineCommand,
    ) -> Result<Option<Transfer>, EngineStoreError> {
        let command_id = CommandId::parse(random_identifier("command")?)
            .expect("generated command identifier is valid");
        let effect = decide(
            &self.engine.state().snapshot,
            CommandEnvelope {
                contract_version: crate::APPLICATION_CONTRACT_VERSION,
                command_id,
                command,
            },
        )
        .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?;
        let (transfer_id, event, removed) = match effect.effect {
            EngineEffect::PauseTransfer { transfer_id } => {
                let event = EngineEvent::TransferPaused {
                    transfer_id: transfer_id.clone(),
                };
                (transfer_id, event, false)
            }
            EngineEffect::ResumeTransfer { transfer_id } => {
                let event = EngineEvent::TransferResumed {
                    transfer_id: transfer_id.clone(),
                };
                (transfer_id, event, false)
            }
            EngineEffect::RecoverTransfer { transfer_id, .. } => {
                let event = EngineEvent::TransferRecoveryStarted {
                    transfer_id: transfer_id.clone(),
                };
                (transfer_id, event, false)
            }
            EngineEffect::CancelTransfer { transfer_id } => {
                let event = EngineEvent::TransferCanceled {
                    transfer_id: transfer_id.clone(),
                };
                (transfer_id, event, false)
            }
            EngineEffect::RemoveTransfer { transfer_id } => {
                let event = EngineEvent::TransferRemoved {
                    transfer_id: transfer_id.clone(),
                };
                (transfer_id, event, true)
            }
            _ => {
                return Err(EngineStoreError::InvalidState(
                    "Transfer control decision returned an unexpected effect".into(),
                ));
            }
        };

        let mut state = self.engine.state().clone();
        apply_product_event(&mut state, event)?;
        let transfer = state.snapshot.transfers.get(&transfer_id).cloned();
        if removed == transfer.is_some() {
            return Err(EngineStoreError::InvalidState(
                "Transfer control produced an inconsistent snapshot".into(),
            ));
        }
        self.engine.replace(state)?;
        Ok(transfer)
    }

    fn required_outgoing_transfer(
        &self,
        transfer_id: &TransferId,
    ) -> Result<Transfer, EngineStoreError> {
        let transfer = self
            .engine
            .state()
            .snapshot
            .transfers
            .get(transfer_id)
            .cloned()
            .ok_or_else(|| {
                EngineStoreError::InvalidState(format!("Transfer {transfer_id} does not exist"))
            })?;
        if transfer.direction != TransferDirection::Send {
            return Err(EngineStoreError::InvalidState(format!(
                "Transfer {transfer_id} is not outgoing"
            )));
        }
        Ok(transfer)
    }

    fn apply_transfer_events(
        &mut self,
        transfer_id: &TransferId,
        events: impl IntoIterator<Item = EngineEvent>,
    ) -> Result<Transfer, EngineStoreError> {
        let mut state = self.engine.state().clone();
        for event in events {
            apply_product_event(&mut state, event)?;
        }
        let transfer = state
            .snapshot
            .transfers
            .get(transfer_id)
            .cloned()
            .ok_or_else(|| {
                EngineStoreError::InvalidState(format!("Transfer {transfer_id} does not exist"))
            })?;
        self.engine.replace(state)?;
        Ok(transfer)
    }

    pub fn append_inbox(&mut self, item: InboxItem) -> Result<(), EngineStoreError> {
        if self
            .engine
            .state()
            .inbox
            .iter()
            .any(|existing| existing.id == item.id)
        {
            return Ok(());
        }
        let mut state = self.engine.state().clone();
        state.inbox.push(item);
        state.inbox.sort_by_key(|item| item.received_at_unix_ms);
        let overflow = state.inbox.len().saturating_sub(MAX_INBOX_ITEMS);
        if overflow > 0 {
            state.inbox.drain(..overflow);
        }
        self.engine.replace(state)
    }

    pub fn inbox(&self, limit: usize) -> Vec<InboxItem> {
        self.engine
            .state()
            .inbox
            .iter()
            .rev()
            .take(limit.min(MAX_INBOX_ITEMS))
            .cloned()
            .collect()
    }

    pub fn latest_inbox(&self) -> Option<InboxItem> {
        self.engine.state().inbox.last().cloned()
    }

    pub fn apply_event(
        &mut self,
        envelope: EventEnvelope,
    ) -> Result<ApplyOutcome, EngineStoreError> {
        let mut state = self.engine.state().clone();
        let outcome = state
            .snapshot
            .apply(envelope)
            .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?;
        if outcome == ApplyOutcome::Applied {
            self.engine.replace(state)?;
        }
        Ok(outcome)
    }

    pub fn engine_snapshot(&self) -> EngineSnapshot {
        self.engine.state().snapshot.clone()
    }

    /// Borrows the current snapshot while the caller retains this store owner.
    pub fn engine_snapshot_ref(&self) -> &EngineSnapshot {
        &self.engine.state().snapshot
    }
}

fn apply_product_event(
    state: &mut EngineState,
    event: EngineEvent,
) -> Result<(), EngineStoreError> {
    let sequence = state
        .snapshot
        .last_sequence
        .checked_add(1)
        .ok_or_else(|| EngineStoreError::InvalidState("event sequence is exhausted".into()))?;
    state
        .snapshot
        .apply(EventEnvelope {
            contract_version: crate::APPLICATION_CONTRACT_VERSION,
            sequence,
            event,
        })
        .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?;
    Ok(())
}

fn settle_transfers_for_revoked_relationship(
    state: &mut EngineState,
    relationship_id: &RelationshipId,
) -> Result<(), EngineStoreError> {
    let transfers = state
        .snapshot
        .transfers
        .values()
        .filter(|transfer| {
            &transfer.relationship_id == relationship_id && !transfer.state.is_terminal()
        })
        .map(|transfer| (transfer.id.clone(), transfer.state))
        .collect::<Vec<_>>();

    for (transfer_id, transfer_state) in transfers {
        let event = match transfer_state {
            TransferState::Offered => EngineEvent::TransferRejected {
                transfer_id,
                reason: TransferRejection::UserDeclined,
            },
            TransferState::Queued
            | TransferState::Connecting
            | TransferState::Transferring
            | TransferState::Paused => EngineEvent::TransferCanceled { transfer_id },
            TransferState::AwaitingDeliveryProof => EngineEvent::TransferFailed {
                transfer_id,
                failure: TransferFailure {
                    code: FailureCode::ReceiverFinalizationOutcomeUnknown,
                    phase: FailurePhase::Committing,
                    retryable: false,
                    recovery_action: RecoveryAction::None,
                },
            },
            TransferState::Delivered
            | TransferState::Rejected
            | TransferState::Failed
            | TransferState::Canceled => continue,
        };
        apply_product_event(state, event)?;
    }
    Ok(())
}

fn validate_device_selector(selector: &str) -> io::Result<()> {
    if selector.trim() != selector
        || selector.is_empty()
        || selector.len() > 128
        || selector.chars().any(char::is_control)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "device selector must be a bounded device ID or exact label",
        ));
    }
    Ok(())
}

fn validate_agent_status(status: &AgentStatus) -> io::Result<()> {
    if status.protocol_version != AGENT_PROTOCOL_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Agent status protocol version does not match its envelope",
        ));
    }
    if status.active_paths > MAX_AGENT_ACTIVE_PATHS
        || status.pending_offers > MAX_AGENT_PENDING_OFFERS
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Agent status contains too many active paths or pending offers",
        ));
    }
    validate_agent_directory_path(Path::new(&status.state_directory))?;
    validate_agent_directory_path(Path::new(&status.inbox_directory))?;
    crate::api::parse_broker_addr(&status.broker, status.relay.as_deref())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    Ok(())
}

fn validate_agent_offer_id(offer_id: &str) -> io::Result<()> {
    if offer_id.is_empty()
        || offer_id.len() > MAX_AGENT_OFFER_ID_BYTES
        || !offer_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Agent offer ID must be 1-128 ASCII letters, digits, '-' or '_'",
        ));
    }
    Ok(())
}

fn validate_agent_pending_offer(offer: &AgentPendingOffer) -> io::Result<()> {
    validate_agent_offer_id(&offer.offer_id)?;
    RelationshipId::parse(offer.from_device_id.clone())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    if validate_label(&offer.from_device_label)? != offer.from_device_label {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pending offer device label cannot have leading or trailing whitespace",
        ));
    }
    validate_agent_content_summary(&offer.root_names, offer.item_count, offer.directory_count)
}

fn validate_agent_content_summary(
    root_names: &[String],
    item_count: u32,
    directory_count: u32,
) -> io::Result<()> {
    if directory_count > item_count || root_names.len() > MAX_AGENT_OFFER_ROOTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Agent content summary contains inconsistent item counts or root previews",
        ));
    }
    for name in root_names {
        if name.is_empty()
            || name.len() > MAX_AGENT_OFFER_ROOT_NAME_BYTES
            || matches!(name.as_str(), "." | "..")
            || name
                .chars()
                .any(|character| character.is_control() || matches!(character, '/' | '\\'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Agent content summary contains an invalid root preview",
            ));
        }
    }
    Ok(())
}

fn validate_agent_pending_offers(offers: &[AgentPendingOffer]) -> io::Result<()> {
    if offers.len() > MAX_AGENT_PENDING_OFFERS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Agent response contains too many pending offers",
        ));
    }
    let mut ids = BTreeSet::new();
    for offer in offers {
        validate_agent_pending_offer(offer)?;
        if !ids.insert(&offer.offer_id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Agent response contains duplicate pending offers",
            ));
        }
    }
    Ok(())
}

fn validate_agent_device(device: &DeviceSummary) -> io::Result<()> {
    DeviceId::parse(device.id.clone())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    if validate_label(&device.label)? != device.label {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Agent device label cannot have leading or trailing whitespace",
        ));
    }
    if device
        .previous_generation
        .is_some_and(|previous| previous >= device.generation)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Agent Device has an inconsistent credential generation",
        ));
    }
    for (name, endpoint) in std::iter::once(("broker", device.broker.as_str()))
        .chain(device.relay.as_deref().map(|relay| ("relay", relay)))
    {
        if endpoint.trim().is_empty()
            || endpoint.len() > MAX_AGENT_ENDPOINT_BYTES
            || endpoint.chars().any(char::is_control)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Agent Device {name} must be a bounded visible endpoint"),
            ));
        }
    }
    Ok(())
}

fn validate_agent_transfer_paths(paths: &[AgentTransferPath]) -> io::Result<()> {
    if paths.len() > MAX_AGENT_ACTIVE_PATHS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Agent response contains too many active transfer paths",
        ));
    }
    let mut ids = BTreeSet::new();
    for path in paths {
        TransferId::parse(path.transfer_id.clone())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        let direction = match path.direction {
            TransferDirection::Send => 0,
            TransferDirection::Receive => 1,
        };
        if !ids.insert((path.transfer_id.as_str(), direction)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Agent response contains duplicate active transfer paths",
            ));
        }
    }
    Ok(())
}

fn validate_agent_transfer_telemetry(values: &[AgentTransferTelemetry]) -> io::Result<()> {
    if values.len() > MAX_AGENT_TRANSFER_TELEMETRY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Agent response contains too many live Transfer measurements",
        ));
    }
    let mut ids = BTreeSet::new();
    for value in values {
        TransferId::parse(value.transfer_id.clone())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        RelationshipId::parse(value.relationship_id.clone())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        validate_agent_content_summary(&value.root_names, value.item_count, value.directory_count)?;
        if !ids.insert(&value.transfer_id)
            || value.transferred_bytes > value.total_bytes
            || (value.current_bytes_per_second == 0 && value.eta_seconds.is_some())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Agent Transfer telemetry is duplicated or inconsistent",
            ));
        }
    }
    Ok(())
}

fn validate_agent_transfer(transfer: &Transfer) -> io::Result<()> {
    if transfer.transferred_bytes > transfer.total_bytes
        || (transfer.state == crate::model::TransferState::Failed) != transfer.failure.is_some()
        || (transfer.state == crate::model::TransferState::Rejected) != transfer.rejection.is_some()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Agent Transfer has inconsistent state or progress",
        ));
    }
    Ok(())
}

fn validate_agent_source_path(path: &Path) -> io::Result<()> {
    let value = path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Agent source path must be valid UTF-8",
        )
    })?;
    if value.is_empty() || value.len() > MAX_AGENT_PATH_BYTES || value.chars().any(char::is_control)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Agent source path must be bounded and contain no control characters",
        ));
    }
    Ok(())
}

fn validate_agent_directory_path(path: &Path) -> io::Result<()> {
    validate_agent_source_path(path)?;
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Agent directory path must be absolute",
        ));
    }
    Ok(())
}

pub fn default_agent_state_directory() -> io::Result<PathBuf> {
    if let Some(value) = env::var_os("ENVOIX_STATE_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    #[cfg(windows)]
    {
        env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(|value| PathBuf::from(value).join("Envoix"))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "cannot locate Envoix state directory; set ENVOIX_STATE_DIR or LOCALAPPDATA",
                )
            })
    }
    #[cfg(not(windows))]
    if let Some(value) = env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value).join("envoix"));
    }
    #[cfg(not(windows))]
    if let Some(value) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value).join(".local/state/envoix"));
    }
    #[cfg(not(windows))]
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "cannot locate Envoix state directory; set ENVOIX_STATE_DIR",
    ))
}

pub fn default_agent_control_endpoint() -> io::Result<PathBuf> {
    if let Some(value) = env::var_os("ENVOIX_AGENT_ENDPOINT").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    if let Some(value) = env::var_os("ENVOIX_AGENT_SOCKET").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    #[cfg(target_os = "macos")]
    {
        let home = env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "cannot locate the macOS Envoix helper; set HOME or ENVOIX_AGENT_ENDPOINT",
                )
            })?;
        macos_agent_control_endpoint(&home)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Ok(default_agent_state_directory()?.join("agent.sock"))
    }
    #[cfg(windows)]
    {
        windows_agent_pipe_name(&current_windows_user_sid()?)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the local Agent control transport is unsupported on this platform",
        ))
    }
}

#[cfg(any(target_os = "macos", test))]
fn macos_agent_control_endpoint(home: &Path) -> io::Result<PathBuf> {
    if !home.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "macOS home directory must be absolute",
        ));
    }
    Ok(home
        .join(MACOS_AGENT_STATE_RELATIVE_PATH)
        .join(AGENT_CONTROL_SOCKET_NAME))
}

#[cfg(any(windows, test))]
const WINDOWS_AGENT_PIPE_PREFIX: &str = r"\\.\pipe\envoix-agent-";

#[cfg(any(windows, test))]
fn windows_agent_pipe_name(user_sid: &str) -> io::Result<PathBuf> {
    if !is_canonical_windows_sid(user_sid) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows user SID is not canonical",
        ));
    }
    Ok(PathBuf::from(format!(
        "{WINDOWS_AGENT_PIPE_PREFIX}{user_sid}"
    )))
}

#[cfg(any(windows, test))]
fn is_canonical_windows_sid(value: &str) -> bool {
    if value.len() > 184 {
        return false;
    }
    let mut components = value.split('-');
    if components.next() != Some("S") || components.next() != Some("1") {
        return false;
    }
    let Some(authority) = components.next() else {
        return false;
    };
    if !is_canonical_decimal(authority)
        || !authority
            .parse::<u64>()
            .is_ok_and(|authority| authority <= 0x0000_ffff_ffff_ffff)
    {
        return false;
    }
    let subauthorities = components.collect::<Vec<_>>();
    !subauthorities.is_empty()
        && subauthorities.len() <= 15
        && subauthorities
            .iter()
            .all(|component| is_canonical_decimal(component) && component.parse::<u32>().is_ok())
}

#[cfg(any(windows, test))]
fn is_canonical_decimal(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

#[cfg(windows)]
pub fn current_windows_user_sid() -> io::Result<String> {
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: GetCurrentProcess returns a process pseudo-handle that is valid
    // for the lifetime of this process and must not be closed.
    windows_user_sid_for_process(unsafe { GetCurrentProcess() })
}

#[cfg(windows)]
pub fn windows_process_user_sid(process_id: u32) -> io::Result<String> {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    // SAFETY: OpenProcess receives a PID supplied by the kernel for a connected
    // pipe client. The returned handle is checked and owned by WinHandle.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    let process = WinHandle::new(process)?;
    windows_user_sid_for_process(process.0)
}

#[cfg(windows)]
fn windows_user_sid_for_process(
    process: windows_sys::Win32::Foundation::HANDLE,
) -> io::Result<String> {
    use std::ptr;

    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::OpenProcessToken;

    let mut token = ptr::null_mut();
    // SAFETY: process is a valid process handle or the documented current
    // process pseudo-handle; token points to writable HANDLE storage.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = WinHandle::new(token)?;
    let mut required = 0_u32;
    // SAFETY: A null buffer with length zero is the documented size query.
    let result =
        unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut required) };
    let size_query_error = io::Error::last_os_error();
    if result != 0
        || size_query_error.raw_os_error()
            != Some(windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER as i32)
    {
        return Err(size_query_error);
    }
    if required < u32::try_from(std::mem::size_of::<TOKEN_USER>()).unwrap_or(u32::MAX) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows token returned an invalid user SID size",
        ));
    }
    let word_size = std::mem::size_of::<usize>();
    let words = usize::try_from(required)
        .map_err(|_| io::Error::other("Windows token user SID is too large"))?
        .div_ceil(word_size);
    let mut buffer = vec![0_usize; words];
    // SAFETY: buffer is aligned for TOKEN_USER and contains at least `required`
    // writable bytes. The token and returned length remain valid for this call.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: GetTokenInformation successfully initialized a TOKEN_USER at the
    // aligned start of `buffer`, which remains alive while the SID is converted.
    let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    windows_sid_to_string(token_user.User.Sid)
}

#[cfg(windows)]
fn windows_sid_to_string(sid: windows_sys::Win32::Security::PSID) -> io::Result<String> {
    use std::ptr;

    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;

    let mut value = ptr::null_mut();
    // SAFETY: sid comes from a live TOKEN_USER buffer. The API writes one
    // LocalAlloc-owned, null-terminated UTF-16 pointer to `value`.
    if unsafe { ConvertSidToStringSidW(sid, &mut value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let value = LocalAllocation(value.cast());
    let mut length = 0_usize;
    // SAFETY: ConvertSidToStringSidW guarantees a null-terminated SID string.
    // Canonical Windows SID strings are at most 184 characters; the explicit
    // bound prevents an unbounded scan if the OS contract is violated.
    unsafe {
        while length <= 184 && *value.0.cast::<u16>().add(length) != 0 {
            length += 1;
        }
    }
    if length > 184 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows user SID exceeds the supported length",
        ));
    }
    // SAFETY: The preceding bounded scan established that these initialized
    // UTF-16 code units precede the terminating null.
    let units = unsafe { std::slice::from_raw_parts(value.0.cast::<u16>(), length) };
    let sid = String::from_utf16(units).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "Windows user SID is not UTF-16")
    })?;
    if !is_canonical_windows_sid(&sid) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned a non-canonical user SID",
        ));
    }
    Ok(sid)
}

#[cfg(windows)]
struct WinHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl WinHandle {
    fn new(handle: windows_sys::Win32::Foundation::HANDLE) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

#[cfg(windows)]
impl Drop for WinHandle {
    fn drop(&mut self) {
        // SAFETY: WinHandle exclusively owns a non-null kernel handle.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
struct LocalAllocation(windows_sys::Win32::Foundation::HLOCAL);

#[cfg(windows)]
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: This pointer was allocated by ConvertSidToStringSidW and is
        // released exactly once with LocalFree.
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.0);
        }
    }
}

fn validate_label(label: &str) -> io::Result<String> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 64 || label.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "device label must contain 1 to 64 visible characters",
        ));
    }
    Ok(label.to_string())
}

fn validate_request_id(request_id: String) -> io::Result<String> {
    if !is_valid_agent_request_id(&request_id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Agent request ID must be a bounded opaque identifier",
        ));
    }
    Ok(request_id)
}

pub fn is_valid_agent_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id.len() <= MAX_AGENT_REQUEST_ID_BYTES
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn random_identifier(prefix: &str) -> io::Result<String> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|error| io::Error::other(format!("identifier entropy unavailable: {error}")))?;
    Ok(format!("{prefix}_{}", URL_SAFE_NO_PAD.encode(random)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryVault {
        values: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
        load_error: std::sync::Mutex<Option<PlatformPortError>>,
    }

    impl SecureVaultPort for MemoryVault {
        fn contains(
            &self,
            reference: &VaultReference,
        ) -> Result<bool, crate::ports::PlatformPortError> {
            Ok(self.values.lock().unwrap().contains_key(reference.as_str()))
        }

        fn store(
            &self,
            reference: &VaultReference,
            secret: &SecretBytes,
        ) -> Result<(), crate::ports::PlatformPortError> {
            self.values
                .lock()
                .unwrap()
                .insert(reference.as_str().to_string(), secret.expose().to_vec());
            Ok(())
        }

        fn load(
            &self,
            reference: &VaultReference,
        ) -> Result<Option<SecretBytes>, crate::ports::PlatformPortError> {
            if let Some(error) = *self.load_error.lock().unwrap() {
                return Err(error);
            }
            self.values
                .lock()
                .unwrap()
                .get(reference.as_str())
                .cloned()
                .map(SecretBytes::new)
                .transpose()
        }

        fn delete(
            &self,
            reference: &VaultReference,
        ) -> Result<(), crate::ports::PlatformPortError> {
            self.values.lock().unwrap().remove(reference.as_str());
            Ok(())
        }
    }

    struct InteractionRequiredVault;

    impl SecureVaultPort for InteractionRequiredVault {
        fn contains(
            &self,
            _reference: &VaultReference,
        ) -> Result<bool, crate::ports::PlatformPortError> {
            Ok(false)
        }

        fn store(
            &self,
            _reference: &VaultReference,
            _secret: &SecretBytes,
        ) -> Result<(), crate::ports::PlatformPortError> {
            Err(crate::ports::PlatformPortError::InteractionRequired)
        }

        fn load(
            &self,
            _reference: &VaultReference,
        ) -> Result<Option<SecretBytes>, crate::ports::PlatformPortError> {
            Err(crate::ports::PlatformPortError::InteractionRequired)
        }

        fn delete(
            &self,
            _reference: &VaultReference,
        ) -> Result<(), crate::ports::PlatformPortError> {
            Err(crate::ports::PlatformPortError::InteractionRequired)
        }
    }

    fn opaque_credential() -> Vec<u8> {
        RememberedCredential::from_control_pairing(
            b"fixture control key",
            envoix_invite::Commitment::sha256(b"fixture transcript"),
        )
        .to_opaque()
    }

    fn inbox_item(id: &str, received_at: u64) -> InboxItem {
        InboxItem {
            id: id.into(),
            received_at_unix_ms: received_at,
            from_device_id: "dev_test".into(),
            from_device_label: "MacBook".into(),
            roots: vec![InboxRoot {
                name: "shot.png".into(),
                path: "/inbox/shot.png".into(),
            }],
            file_count: 1,
            directory_count: 0,
            total_bytes: 42,
        }
    }

    #[test]
    fn device_credentials_are_separate_and_generation_survives_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProductStore::open(directory.path()).unwrap();
        let credential = opaque_credential();
        let pending = store
            .prepare_device(" MacBook ", "broker", Some("https://relay"))
            .unwrap();
        let id = pending.id().to_string();
        store.commit_device(pending, &credential, 0).unwrap();
        store.rotate_device(&id, &credential, 1).unwrap();

        let metadata = fs::read(directory.path().join("engine-state-v2.json")).unwrap();
        assert!(
            !metadata
                .windows(credential.len())
                .any(|window| window == credential)
        );
        drop(store);
        let reopened = ProductStore::open(directory.path()).unwrap();
        let device = reopened.device_record(&id).unwrap();
        assert_eq!(device.label(), "MacBook");
        assert_eq!(device.generation(), 1);
        assert_eq!(device.previous_generation(), Some(0));
        assert_eq!(
            reopened.device_credential(&id).unwrap().expose(),
            credential.as_slice()
        );
    }

    #[test]
    fn device_route_update_preserves_credential_and_generation() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProductStore::open(directory.path()).unwrap();
        let credential = opaque_credential();
        let old_broker = format!(
            "{}@127.0.0.1:8555",
            crate::DEFAULT_RENDEZVOUS_BROKER.split('@').next().unwrap()
        );
        let pending = store
            .prepare_device(
                "MacBook",
                &old_broker,
                Some("https://old-relay.example.test"),
            )
            .unwrap();
        let id = pending.id().to_string();
        store.commit_device(pending, &credential, 7).unwrap();

        let updated = store
            .update_device_route(
                &id,
                crate::DEFAULT_RENDEZVOUS_BROKER,
                Some(crate::DEFAULT_RELAY_URL),
            )
            .unwrap();

        assert_eq!(updated.generation, 7);
        assert_eq!(updated.previous_generation, None);
        assert_eq!(updated.broker, crate::DEFAULT_RENDEZVOUS_BROKER);
        assert_eq!(updated.relay.as_deref(), Some(crate::DEFAULT_RELAY_URL));
        assert_eq!(store.device_credential(&id).unwrap().expose(), credential);
        assert!(store.update_device_route(&id, "invalid", None).is_err());
        drop(store);

        let reopened = ProductStore::open(directory.path()).unwrap();
        let device = reopened.device_record(&id).unwrap();
        assert_eq!(device.broker(), crate::DEFAULT_RENDEZVOUS_BROKER);
        assert_eq!(device.relay(), Some(crate::DEFAULT_RELAY_URL));
        assert_eq!(
            reopened.device_credential(&id).unwrap().expose(),
            credential
        );
    }

    #[test]
    fn product_store_uses_an_injected_vault_without_serializing_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let credential = opaque_credential();
        let mut store = ProductStore::open_with_vault(directory.path(), vault.clone()).unwrap();
        let pending = store.prepare_device("Android", "broker", None).unwrap();
        let id = pending.id().to_string();
        store.commit_device(pending, &credential, 0).unwrap();

        let serialized = fs::read(directory.path().join("engine-state-v2.json")).unwrap();
        assert!(
            !serialized
                .windows(credential.len())
                .any(|window| window == credential)
        );
        drop(store);

        let reopened = ProductStore::open_with_vault(directory.path(), vault).unwrap();
        assert_eq!(
            reopened.device_credential(&id).unwrap().expose(),
            credential.as_slice()
        );
    }

    #[test]
    fn corrupt_vault_material_is_typed_until_explicit_revoke() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let mut store = ProductStore::open_with_vault(directory.path(), vault.clone()).unwrap();
        let pending = store.prepare_device("Android", "broker", None).unwrap();
        let id = pending.id().to_string();
        store
            .commit_device(pending, &opaque_credential(), 0)
            .unwrap();
        *vault.values.lock().unwrap().values_mut().next().unwrap() = vec![0x5a; 37];

        assert!(matches!(
            store.device_credential(&id),
            Err(EngineStoreError::PlatformPort(
                crate::ports::PlatformPortError::CorruptData
            ))
        ));
        assert!(store.device_record(&id).is_some());

        *vault.load_error.lock().unwrap() = Some(PlatformPortError::CorruptData);
        store.forget_device(&id).unwrap();
        assert!(store.device_record(&id).is_none());
        assert!(vault.values.lock().unwrap().is_empty());
    }

    #[test]
    fn device_rename_is_engine_owned_and_survives_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProductStore::open(directory.path()).unwrap();
        let pending = store.prepare_device("Android", "broker", None).unwrap();
        let id = pending.id().to_string();
        store
            .commit_device(pending, &opaque_credential(), 0)
            .unwrap();

        let renamed = store.rename_device(&id, " Pixel Tablet ").unwrap();

        assert_eq!(renamed.label, "Pixel Tablet");
        drop(store);
        assert_eq!(
            ProductStore::open(directory.path())
                .unwrap()
                .device_record(&id)
                .unwrap()
                .label(),
            "Pixel Tablet"
        );
    }

    #[test]
    fn generic_application_events_are_persisted_or_rejected_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProductStore::open(directory.path()).unwrap();
        let observed = EventEnvelope {
            contract_version: crate::APPLICATION_CONTRACT_VERSION,
            sequence: 1,
            event: EngineEvent::DeviceObserved {
                device_id: DeviceId::parse("device_event_fixture").unwrap(),
                display_name: "Observed".into(),
            },
        };

        assert_eq!(
            store.apply_event(observed.clone()).unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(
            store.apply_event(observed).unwrap(),
            ApplyOutcome::IgnoredDuplicate
        );
        assert!(
            store
                .apply_event(EventEnvelope {
                    contract_version: crate::APPLICATION_CONTRACT_VERSION,
                    sequence: 2,
                    event: EngineEvent::RelationshipTrusted {
                        relationship_id: RelationshipId::parse("relationship_without_metadata")
                            .unwrap(),
                        device_id: DeviceId::parse("device_event_fixture").unwrap(),
                        generation: 0,
                    },
                })
                .is_err()
        );
        assert!(store.devices().is_empty());

        drop(store);
        let reopened = ProductStore::open(directory.path()).unwrap();
        assert_eq!(reopened.engine_snapshot().last_sequence, 1);
        assert!(reopened.engine_snapshot().relationships.is_empty());
    }

    #[test]
    fn vault_interaction_is_typed_and_never_partially_commits_a_relationship() {
        let directory = tempfile::tempdir().unwrap();
        let mut store =
            ProductStore::open_with_vault(directory.path(), Arc::new(InteractionRequiredVault))
                .unwrap();
        let pending = store.prepare_device("iPhone", "broker", None).unwrap();

        let error = store
            .commit_device(pending, &opaque_credential(), 0)
            .unwrap_err();

        assert!(matches!(
            error,
            EngineStoreError::PlatformPort(crate::ports::PlatformPortError::InteractionRequired)
        ));
        assert!(store.devices().is_empty());
        assert!(store.engine.state().snapshot.relationships.is_empty());
    }

    #[test]
    fn queued_transfer_is_decided_by_the_engine_and_survives_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProductStore::open(directory.path()).unwrap();
        let pending = store.prepare_device("MacBook", "broker", None).unwrap();
        let relationship_id = pending.id().to_string();
        store
            .commit_device(pending, &opaque_credential(), 0)
            .unwrap();
        let transfer_id = TransferId::parse("transfer_test").unwrap();
        let transfer = store
            .create_transfer(
                "MacBook",
                transfer_id.clone(),
                ContentId::parse("content_test").unwrap(),
                42,
            )
            .unwrap();
        assert_eq!(transfer.relationship_id.as_str(), relationship_id);
        assert_eq!(transfer.state, crate::model::TransferState::Queued);
        assert_eq!(transfer.total_bytes, 42);
        assert_eq!(store.transfers(), vec![transfer.clone()]);
        assert_eq!(store.transfer("transfer_test").unwrap(), Some(transfer));

        drop(store);
        let reopened = ProductStore::open(directory.path()).unwrap();
        assert_eq!(
            reopened.transfer("transfer_test").unwrap().unwrap().state,
            crate::model::TransferState::Queued
        );
    }

    #[test]
    fn outgoing_transfer_progress_and_completion_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProductStore::open(directory.path()).unwrap();
        let pending = store.prepare_device("MacBook", "broker", None).unwrap();
        let relationship_id = pending.id().to_string();
        store
            .commit_device(pending, &opaque_credential(), 0)
            .unwrap();
        let transfer_id = TransferId::parse("transfer_restart").unwrap();
        store
            .create_transfer(
                "MacBook",
                transfer_id.clone(),
                ContentId::parse("content_restart").unwrap(),
                42,
            )
            .unwrap();
        store.start_outgoing_transfer(&transfer_id).unwrap();
        store.progress_outgoing_transfer(&transfer_id, 21).unwrap();
        drop(store);

        let mut reopened = ProductStore::open(directory.path()).unwrap();
        let dispatchable = reopened.dispatchable_transfers(&relationship_id).unwrap();
        assert_eq!(dispatchable.len(), 1);
        assert_eq!(
            dispatchable[0].state,
            crate::model::TransferState::Transferring
        );
        assert_eq!(dispatchable[0].transferred_bytes, 21);

        let delivered = reopened.complete_outgoing_transfer(&transfer_id).unwrap();
        assert_eq!(delivered.state, crate::model::TransferState::Delivered);
        assert_eq!(delivered.transferred_bytes, 42);
        drop(reopened);

        assert_eq!(
            ProductStore::open(directory.path())
                .unwrap()
                .transfer(transfer_id.as_str())
                .unwrap()
                .unwrap()
                .state,
            crate::model::TransferState::Delivered
        );
    }

    #[test]
    fn transfer_controls_follow_the_engine_state_machine_and_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProductStore::open(directory.path()).unwrap();
        let pending = store.prepare_device("MacBook", "broker", None).unwrap();
        store
            .commit_device(pending, &opaque_credential(), 0)
            .unwrap();
        let transfer_id = TransferId::parse("transfer_control").unwrap();
        store
            .create_transfer(
                "MacBook",
                transfer_id.clone(),
                ContentId::parse("content_control").unwrap(),
                42,
            )
            .unwrap();

        assert!(store.pause_transfer(&transfer_id).is_err());
        store.start_outgoing_transfer(&transfer_id).unwrap();
        assert_eq!(
            store.pause_transfer(&transfer_id).unwrap().state,
            TransferState::Paused
        );
        assert!(
            store
                .dispatchable_transfers(
                    store
                        .transfer(transfer_id.as_str())
                        .unwrap()
                        .unwrap()
                        .relationship_id
                        .as_str()
                )
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.resume_transfer(&transfer_id).unwrap().state,
            TransferState::Connecting
        );
        assert_eq!(
            store.cancel_transfer(&transfer_id).unwrap().state,
            TransferState::Canceled
        );
        assert!(store.resume_transfer(&transfer_id).is_err());
        drop(store);

        let mut reopened = ProductStore::open(directory.path()).unwrap();
        assert_eq!(
            reopened
                .transfer(transfer_id.as_str())
                .unwrap()
                .unwrap()
                .state,
            TransferState::Canceled
        );
        reopened.remove_transfer(&transfer_id).unwrap();
        assert!(reopened.transfer(transfer_id.as_str()).unwrap().is_none());
    }

    #[test]
    fn forgetting_device_revokes_credential_and_preserves_inbox_history() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProductStore::open(directory.path()).unwrap();
        let credential = opaque_credential();
        let pending = store
            .prepare_device("MacBook", "broker", Some("https://relay"))
            .unwrap();
        let id = pending.id().to_string();
        let credential_path = directory
            .path()
            .join("vault")
            .join(&pending.credential_reference);
        store.commit_device(pending, &credential, 0).unwrap();
        store.append_inbox(inbox_item("received", 1)).unwrap();
        let transfer_id = TransferId::parse("transfer_to_forgotten_device").unwrap();
        store
            .create_transfer(
                &id,
                transfer_id.clone(),
                ContentId::parse("content_to_forgotten_device").unwrap(),
                42,
            )
            .unwrap();

        let forgotten = store.forget_device("macbook").unwrap();

        assert_eq!(forgotten.id, id);
        assert_eq!(forgotten.label, "MacBook");
        assert!(store.devices().is_empty());
        assert!(!credential_path.exists());
        assert_eq!(
            store.transfer(transfer_id.as_str()).unwrap().unwrap().state,
            crate::model::TransferState::Canceled
        );
        assert_eq!(store.latest_inbox().unwrap().id, "received");
        assert!(store.prepare_device("MacBook", "broker", None).is_ok());

        drop(store);
        let reopened = ProductStore::open(directory.path()).unwrap();
        assert!(reopened.devices().is_empty());
        assert_eq!(
            reopened
                .transfer(transfer_id.as_str())
                .unwrap()
                .unwrap()
                .state,
            crate::model::TransferState::Canceled
        );
        assert_eq!(reopened.latest_inbox().unwrap().id, "received");
    }

    #[test]
    fn inbox_is_newest_first_and_duplicate_jobs_are_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProductStore::open(directory.path()).unwrap();
        store.append_inbox(inbox_item("older", 1)).unwrap();
        store.append_inbox(inbox_item("newer", 2)).unwrap();
        store.append_inbox(inbox_item("newer", 3)).unwrap();

        assert_eq!(store.latest_inbox().unwrap().id, "newer");
        assert_eq!(
            store
                .inbox(10)
                .into_iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            ["newer", "older"]
        );
    }

    #[test]
    fn v0_2_product_state_is_rejected_without_touching_user_files() {
        let directory = tempfile::tempdir().unwrap();
        let legacy_directory = directory.path().join("product");
        fs::create_dir_all(&legacy_directory).unwrap();
        let legacy_state = br#"{"schema_version":1}"#;
        let legacy_path = legacy_directory.join("product-state-v1.json");
        fs::write(&legacy_path, legacy_state).unwrap();
        let inbox_file = directory.path().join("inbox/received.txt");
        fs::create_dir_all(inbox_file.parent().unwrap()).unwrap();
        fs::write(&inbox_file, b"received bytes").unwrap();

        let error = ProductStore::open(directory.path()).err().unwrap();

        assert!(matches!(
            error,
            EngineStoreError::UnsupportedLegacyState { path } if path == legacy_path
        ));
        assert_eq!(fs::read(legacy_path).unwrap(), legacy_state);
        assert_eq!(fs::read(inbox_file).unwrap(), b"received bytes");
    }

    #[test]
    fn current_engine_state_ignores_residual_v0_2_product_state() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProductStore::open(directory.path()).unwrap();
        store.append_inbox(inbox_item("received", 1)).unwrap();
        drop(store);
        let legacy_path = directory.path().join("product/product-state-v1.json");
        fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        fs::write(&legacy_path, b"legacy residue").unwrap();

        let reopened = ProductStore::open(directory.path()).unwrap();

        assert_eq!(reopened.latest_inbox().unwrap().id, "received");
        assert_eq!(fs::read(legacy_path).unwrap(), b"legacy residue");
    }

    #[test]
    fn legacy_agent_wire_fixture_remains_frozen() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.2/agent-control-v3.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], 3);
        assert_eq!(fixture["requests"][3]["command"], "forget_device");
        assert!(fixture["requests"][0].get("protocol_version").is_none());
    }

    #[test]
    fn agent_wire_v4_fixture_remains_frozen() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v4.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], 4);
        assert_eq!(AGENT_PROTOCOL_VERSION, 14);
        assert!(
            fixture["requests"]
                .as_array()
                .unwrap()
                .iter()
                .all(|request| {
                    request["protocol_version"] == 4 && request["request"]["command"] != "events"
                })
        );
        assert!(
            fixture["responses"][1]["response"]["snapshot"]
                .get("event_cursor")
                .is_none()
        );
    }

    #[test]
    fn agent_wire_v5_fixture_remains_frozen() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v5.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], 5);
        assert_eq!(AGENT_PROTOCOL_VERSION, 14);
        assert_eq!(fixture["requests"].as_array().unwrap().len(), 8);
        assert_eq!(fixture["responses"].as_array().unwrap().len(), 11);
        assert_eq!(
            fixture["responses"][2]["response"]["events"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
    }

    #[test]
    fn agent_wire_v6_fixture_remains_frozen() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v6.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], 6);
        assert_eq!(AGENT_PROTOCOL_VERSION, 14);
        assert_eq!(fixture["requests"].as_array().unwrap().len(), 12);
        assert_eq!(fixture["responses"].as_array().unwrap().len(), 15);
        assert!(
            fixture["responses"][1]["response"]["snapshot"]
                .get("pending_offers")
                .is_none()
        );
    }

    #[test]
    fn agent_wire_v7_fixture_remains_frozen() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v7.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], 7);
        assert_eq!(AGENT_PROTOCOL_VERSION, 14);
        assert_eq!(fixture["requests"].as_array().unwrap().len(), 15);
        assert_eq!(fixture["responses"].as_array().unwrap().len(), 18);
        assert!(
            fixture["responses"][1]["response"]["snapshot"]
                .get("active_paths")
                .is_none()
        );
    }

    #[test]
    fn agent_wire_v8_fixture_remains_frozen() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v8.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], 8);
        assert_eq!(AGENT_PROTOCOL_VERSION, 14);
        assert_eq!(fixture["requests"].as_array().unwrap().len(), 16);
        assert_eq!(fixture["responses"].as_array().unwrap().len(), 19);
        assert_eq!(
            fixture["responses"][17]["response"]["diagnostics"]["engine_schema_version"],
            1
        );
    }

    #[test]
    fn agent_wire_v9_fixture_remains_readable_and_frozen() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v9.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], 9);
        assert_eq!(AGENT_PROTOCOL_VERSION, 14);

        let settings: AgentSettings = serde_json::from_value(fixture["settings"].clone()).unwrap();
        settings.validate().unwrap();
        assert_eq!(settings.version, 1);
        assert_eq!(settings.broker, crate::DEFAULT_RENDEZVOUS_BROKER);
        assert_eq!(settings.relay.as_deref(), Some(crate::DEFAULT_RELAY_URL));

        let request_values = fixture["requests"].as_array().unwrap();
        let requests = request_values
            .iter()
            .cloned()
            .map(serde_json::from_value::<AgentRequestEnvelope>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(
            requests.as_slice(),
            [
                AgentRequestEnvelope {
                    request: AgentRequest::Status,
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::Snapshot { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::Events { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::Pair { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::ListDevices,
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::RevokeDevice { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::CreateTransfer { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::ListTransfers,
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::ListTransferPaths,
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::GetTransfer { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::ListPendingOffers,
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::DecidePendingOffer {
                        decision: AgentOfferDecision::Approve,
                        ..
                    },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::DecidePendingOffer {
                        decision: AgentOfferDecision::Reject,
                        ..
                    },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::ListInbox { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::LatestInbox,
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::Diagnostics,
                    ..
                },
            ]
        ));
        for (request, expected) in requests.iter().zip(request_values) {
            assert_eq!(serde_json::to_value(request).unwrap(), *expected);
        }

        let response_values = fixture["responses"].as_array().unwrap();
        let responses = response_values
            .iter()
            .cloned()
            .map(serde_json::from_value::<AgentResponseEnvelope>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(
            responses.as_slice(),
            [
                AgentResponseEnvelope {
                    response: AgentResponse::Status { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::Snapshot { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::Events { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::SnapshotRequired { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::Pairing { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::Devices { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::DeviceRevoked { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::TransferCreated { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::Transfers { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::TransferPaths { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::Transfer { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::PendingOffers { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::PendingOfferDecided {
                        decision: AgentOfferDecision::Approve,
                        ..
                    },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::PendingOfferDecided {
                        decision: AgentOfferDecision::Reject,
                        ..
                    },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::Inbox { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::Latest { item: Some(_) },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::Latest { item: None },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::Diagnostics { .. },
                    ..
                },
                AgentResponseEnvelope {
                    response: AgentResponse::Error { .. },
                    ..
                },
            ]
        ));
        for (response, expected) in responses.iter().zip(response_values) {
            assert_eq!(serde_json::to_value(response).unwrap(), *expected);
        }
        let AgentResponse::Events { events, .. } = &responses[2].response else {
            unreachable!("fixture order is checked above");
        };
        assert!(matches!(
            events.as_slice(),
            [
                AgentEventEnvelope {
                    event: AgentEvent::PairingChanged { .. },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::RelationshipChanged {
                        change: AgentRelationshipChange::Trusted,
                        ..
                    },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::RelationshipChanged {
                        change: AgentRelationshipChange::Rotated,
                        ..
                    },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::RelationshipChanged {
                        change: AgentRelationshipChange::Revoked,
                        ..
                    },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::InboxChanged { .. },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::TransferChanged { .. },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::TransferPathChanged {
                        path: Some(AgentPathKind::Lan),
                        ..
                    },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::TransferPathChanged { path: None, .. },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::PendingOfferChanged { pending: true, .. },
                    ..
                },
                AgentEventEnvelope {
                    event: AgentEvent::PendingOfferChanged { pending: false, .. },
                    ..
                },
            ]
        ));
        let AgentResponse::Pairing { pairing } = &responses[4].response else {
            unreachable!("fixture order is checked above");
        };
        assert_eq!(pairing.expires_at_unix_seconds, 1);
    }

    #[test]
    fn agent_wire_v10_fixture_freezes_apple_keychain_diagnostics() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v10.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], 10);

        let request: AgentRequestEnvelope =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        assert!(matches!(request.request, AgentRequest::Diagnostics));

        let response: AgentResponseEnvelope =
            serde_json::from_value(fixture["response"].clone()).unwrap();
        let AgentResponse::Diagnostics { diagnostics } = &response.response else {
            panic!("expected Agent diagnostics response")
        };
        assert_eq!(
            diagnostics.credential_protection,
            AgentCredentialProtection::AppleKeychain
        );
        assert_eq!(serde_json::to_value(response).unwrap(), fixture["response"]);
    }

    #[test]
    fn agent_wire_v11_fixture_freezes_agent_owned_pairing() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v11.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], 11);

        let request: AgentRequestEnvelope =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        let AgentRequest::JoinPairing { pairing } = &request.request else {
            panic!("expected Agent-owned pairing request")
        };
        assert_eq!(pairing.label, "Fixture WSL");
        assert!(!format!("{request:?}").contains("654321"));
        assert!(!format!("{request:?}").contains("fixture-room"));

        let response: AgentResponseEnvelope =
            serde_json::from_value(fixture["response"].clone()).unwrap();
        assert!(matches!(
            response.response,
            AgentResponse::DevicePaired { .. }
        ));
        assert_eq!(serde_json::to_value(response).unwrap(), fixture["response"]);
    }

    #[test]
    fn agent_wire_v12_fixture_freezes_route_migration() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v12.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], 12);

        let settings: AgentSettings = serde_json::from_value(fixture["settings"].clone()).unwrap();
        settings.validate().unwrap();
        assert_eq!(serde_json::to_value(settings).unwrap(), fixture["settings"]);

        let request: AgentRequestEnvelope =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        assert!(request.validate().is_err());
        assert!(matches!(
            request.request,
            AgentRequest::UpdateDeviceRoute { .. }
        ));

        let response: AgentResponseEnvelope =
            serde_json::from_value(fixture["response"].clone()).unwrap();
        assert!(response.validate_for(&request.request_id).is_err());
        assert!(matches!(
            response.response,
            AgentResponse::DeviceRouteUpdated { .. }
        ));

        let event: AgentEventEnvelope = serde_json::from_value(fixture["event"].clone()).unwrap();
        assert!(event.validate().is_err());
        assert!(matches!(
            event.event,
            AgentEvent::RelationshipChanged {
                change: AgentRelationshipChange::RouteUpdated,
                ..
            }
        ));
    }

    #[test]
    fn agent_wire_v13_fixture_freezes_transfer_controls() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v13.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], 13);

        let requests = fixture["requests"]
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(serde_json::from_value::<AgentRequestEnvelope>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(
            requests.as_slice(),
            [
                AgentRequestEnvelope {
                    request: AgentRequest::PauseTransfer { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::ResumeTransfer { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::RecoverTransfer { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::CancelTransfer { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::RemoveTransfer { .. },
                    ..
                },
            ]
        ));
        for (request, expected) in requests.iter().zip(fixture["requests"].as_array().unwrap()) {
            assert!(request.validate().is_err());
            assert_eq!(serde_json::to_value(request).unwrap(), *expected);
        }

        let responses = fixture["responses"]
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(serde_json::from_value::<AgentResponseEnvelope>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            responses[..4]
                .iter()
                .all(|response| matches!(response.response, AgentResponse::Transfer { .. }))
        );
        assert!(matches!(
            responses[4].response,
            AgentResponse::TransferRemoved { .. }
        ));
        for (response, expected) in responses
            .iter()
            .zip(fixture["responses"].as_array().unwrap())
        {
            assert!(response.validate_for(&response.request_id).is_err());
            assert_eq!(serde_json::to_value(response).unwrap(), *expected);
        }
    }

    #[test]
    fn agent_wire_v14_fixture_covers_preferences_and_live_telemetry() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v14.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], AGENT_PROTOCOL_VERSION);

        let request: AgentRequestEnvelope =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        assert!(matches!(
            request.request,
            AgentRequest::SetInboxDirectory { .. }
        ));
        request.validate().unwrap();
        assert_eq!(serde_json::to_value(&request).unwrap(), fixture["request"]);

        let response: AgentResponseEnvelope =
            serde_json::from_value(fixture["response"].clone()).unwrap();
        assert!(matches!(
            response.response,
            AgentResponse::PreferencesUpdated { .. }
        ));
        response.validate_for(&request.request_id).unwrap();
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            fixture["response"]
        );

        let telemetry: AgentTransferTelemetry =
            serde_json::from_value(fixture["telemetry"].clone()).unwrap();
        validate_agent_transfer_telemetry(std::slice::from_ref(&telemetry)).unwrap();
        assert_eq!(
            serde_json::to_value(&telemetry).unwrap(),
            fixture["telemetry"]
        );

        let event: AgentEventEnvelope = serde_json::from_value(fixture["event"].clone()).unwrap();
        event.validate().unwrap();
        assert!(matches!(event.event, AgentEvent::InboxDirectoryChanged));
        assert_eq!(serde_json::to_value(&event).unwrap(), fixture["event"]);
    }

    #[test]
    fn agent_envelopes_reject_invalid_and_mismatched_request_ids() {
        assert!(AgentRequestEnvelope::new("../request", AgentRequest::Status).is_err());
        let response = AgentResponseEnvelope::new(
            "request_1",
            AgentResponse::Status {
                status: AgentStatus {
                    protocol_version: AGENT_PROTOCOL_VERSION,
                    pid: 1,
                    device_name: "test".into(),
                    state_directory: "/state".into(),
                    inbox_directory: "/inbox".into(),
                    broker: "broker".into(),
                    relay: None,
                    paired_devices: 0,
                    active_receivers: 0,
                    active_pairings: 0,
                    active_paths: 0,
                    pending_offers: 0,
                },
            },
        )
        .unwrap();
        assert!(response.validate_for("request_2").is_err());
    }

    #[test]
    fn agent_device_responses_reject_malformed_and_duplicate_summaries() {
        let device = DeviceSummary {
            id: "dev_fixture".into(),
            label: "Fixture Mac".into(),
            generation: 1,
            previous_generation: Some(0),
            broker: "127.0.0.1:4000".into(),
            relay: None,
        };
        let paired = AgentResponseEnvelope::new(
            "request_1",
            AgentResponse::DevicePaired {
                device: device.clone(),
            },
        )
        .unwrap();
        paired.validate_for("request_1").unwrap();

        let mut malformed = device.clone();
        malformed.previous_generation = Some(malformed.generation);
        let malformed = AgentResponseEnvelope::new(
            "request_2",
            AgentResponse::DevicePaired { device: malformed },
        )
        .unwrap();
        assert!(malformed.validate_for("request_2").is_err());

        let duplicate = AgentResponseEnvelope::new(
            "request_3",
            AgentResponse::Devices {
                devices: vec![device.clone(), device],
            },
        )
        .unwrap();
        assert!(duplicate.validate_for("request_3").is_err());
    }

    #[test]
    fn agent_transfer_requests_require_bounded_paths() {
        for paths in [
            Vec::new(),
            vec![PathBuf::from("bad\npath")],
            vec![PathBuf::from("x".repeat(MAX_AGENT_PATH_BYTES + 1))],
        ] {
            let request = AgentRequestEnvelope::new(
                "request_1",
                AgentRequest::CreateTransfer {
                    device: "MacBook".into(),
                    paths,
                },
            )
            .unwrap();
            assert!(request.validate().is_err());
        }
        let request = AgentRequestEnvelope::new(
            "request_1",
            AgentRequest::CreateTransfer {
                device: "MacBook".into(),
                paths: vec![PathBuf::from("/tmp/hello.txt")],
            },
        )
        .unwrap();
        request.validate().unwrap();

        let request = AgentRequestEnvelope::new(
            "request_2",
            AgentRequest::CreateTransfer {
                device: "MacBook".into(),
                paths: vec![PathBuf::from("relative.txt")],
            },
        )
        .unwrap();
        request.validate().unwrap();
    }

    #[test]
    fn agent_pending_offers_are_bounded_and_secret_free() {
        let invalid_request = AgentRequestEnvelope::new(
            "request_1",
            AgentRequest::DecidePendingOffer {
                offer_id: "../offer".into(),
                decision: AgentOfferDecision::Approve,
            },
        )
        .unwrap();
        assert!(invalid_request.validate().is_err());

        let offer = AgentPendingOffer {
            offer_id: "offer_fixture".into(),
            from_device_id: "relationship_fixture".into(),
            from_device_label: "Fixture Mac".into(),
            root_names: vec!["fixture.bin".into()],
            item_count: 1,
            directory_count: 0,
            total_bytes: 43,
            allocatable_bytes: 84,
        };
        let response = AgentResponseEnvelope::new(
            "request_2",
            AgentResponse::PendingOffers {
                offers: vec![offer.clone()],
            },
        )
        .unwrap();
        response.validate_for("request_2").unwrap();
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(!encoded.contains("transfer_invite"));

        let mut invalid = offer;
        invalid.root_names = vec!["../secret".into()];
        let response = AgentResponseEnvelope::new(
            "request_3",
            AgentResponse::PendingOffers {
                offers: vec![invalid],
            },
        )
        .unwrap();
        assert!(response.validate_for("request_3").is_err());
    }

    #[test]
    fn agent_transfer_paths_are_bounded_and_unique_per_direction() {
        let path = AgentTransferPath {
            transfer_id: "transfer_fixture".into(),
            direction: TransferDirection::Send,
            path: AgentPathKind::Lan,
        };
        let response = AgentResponseEnvelope::new(
            "request_1",
            AgentResponse::TransferPaths {
                paths: vec![
                    path.clone(),
                    AgentTransferPath {
                        direction: TransferDirection::Receive,
                        path: AgentPathKind::Relay,
                        ..path.clone()
                    },
                ],
            },
        )
        .unwrap();
        response.validate_for("request_1").unwrap();

        let duplicate = AgentResponseEnvelope::new(
            "request_2",
            AgentResponse::TransferPaths {
                paths: vec![path.clone(), path.clone()],
            },
        )
        .unwrap();
        assert!(duplicate.validate_for("request_2").is_err());

        let oversized = AgentResponseEnvelope::new(
            "request_oversized",
            AgentResponse::TransferPaths {
                paths: (0..=MAX_AGENT_ACTIVE_PATHS)
                    .map(|index| AgentTransferPath {
                        transfer_id: format!("transfer_{index}"),
                        direction: TransferDirection::Send,
                        path: AgentPathKind::Direct,
                    })
                    .collect(),
            },
        )
        .unwrap();
        assert!(oversized.validate_for("request_oversized").is_err());

        let invalid = AgentResponseEnvelope::new(
            "request_3",
            AgentResponse::TransferPaths {
                paths: vec![AgentTransferPath {
                    transfer_id: "../transfer".into(),
                    ..path
                }],
            },
        )
        .unwrap();
        assert!(invalid.validate_for("request_3").is_err());
    }

    #[test]
    fn agent_settings_validate_version_name_and_inbox() {
        let mut settings = AgentSettings {
            version: AGENT_SETTINGS_VERSION,
            device_name: "WSL".into(),
            inbox_directory: PathBuf::from("/tmp/inbox"),
            broker: crate::DEFAULT_RENDEZVOUS_BROKER.into(),
            relay: Some(crate::DEFAULT_RELAY_URL.into()),
        };
        settings.validate().unwrap();

        settings.version += 1;
        assert_eq!(
            settings.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        settings.version = AGENT_SETTINGS_VERSION;
        settings.device_name = " WSL".into();
        assert_eq!(
            settings.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        settings.device_name = "WSL".into();
        settings.inbox_directory = PathBuf::from("relative/inbox");
        assert_eq!(
            settings.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn windows_agent_pipe_names_are_per_user_and_injection_safe() {
        assert_eq!(
            windows_agent_pipe_name("S-1-5-21-100-200-300-1001")
                .unwrap()
                .to_str(),
            Some(r"\\.\pipe\envoix-agent-S-1-5-21-100-200-300-1001")
        );
        for invalid in [
            "",
            "s-1-5-21-1",
            "S-",
            "S--1-5-21-1",
            "S-01-5-21-1",
            "S-1-05-21-1",
            "S-1-5-21-",
            "S-1-5-21-1\\other",
            "S-1-5-21-1;D:(A;;GA;;;WD)",
        ] {
            assert!(windows_agent_pipe_name(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn macos_agent_control_endpoint_matches_the_signed_helper_boundary() {
        assert_eq!(
            macos_agent_control_endpoint(Path::new("/Users/Test User")).unwrap(),
            PathBuf::from(
                "/Users/Test User/Library/Application Support/com.envoix.app/agent-v1/agent.sock"
            )
        );
        assert_eq!(
            macos_agent_control_endpoint(Path::new("relative-home"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_process_sid_matches_the_current_user() {
        assert_eq!(
            windows_process_user_sid(std::process::id()).unwrap(),
            current_windows_user_sid().unwrap()
        );
    }
}
