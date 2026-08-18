//! Shared product-level state and the local Agent wire contract.
//!
//! Transfer bytes still flow through the canonical Manifest v2 session APIs.
//! This module names the user-facing concepts above that protocol: remembered
//! devices, a durable Inbox, and commands exchanged with a local Agent.

use std::collections::BTreeSet;
use std::env;
#[cfg(test)]
use std::fs::{self, File, OpenOptions};
use std::io;
#[cfg(test)]
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::api::DesktopCredentialStore;
use crate::api::RememberedCredential;
use crate::event::{EngineEvent, EventEnvelope};
use crate::model::{Device, DeviceId, Relationship, RelationshipId, RelationshipState, TransferId};
use crate::snapshot::EngineSnapshot;
use crate::storage::{
    DurableRelationship, EngineState, EngineStore, EngineStoreError, EngineStoreOrigin,
    MAX_DURABLE_ENTITIES, RePairReason, RePairRequiredRelationship, V02ImportRecord,
    VaultReference, read_bounded_file,
};

pub const AGENT_PROTOCOL_VERSION: u16 = 5;
pub const AGENT_SETTINGS_VERSION: u16 = 1;
pub const MAX_AGENT_REQUEST_BYTES: u64 = 64 * 1024;
pub const MAX_AGENT_RESPONSE_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_AGENT_EVENT_BATCH: usize = 256;
const MAX_AGENT_REQUEST_ID_BYTES: usize = 64;
const PRODUCT_STATE_SCHEMA_VERSION: u16 = 1;
const PRODUCT_STATE_FILE: &str = "product-state-v1.json";
const MAX_INBOX_ITEMS: usize = 1_000;
const V02_PRODUCT_STATE_BACKUP: &str = "v0.2-product-state-v1.backup.json";

/// User-owned settings loaded by a managed Agent process.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSettings {
    pub version: u16,
    pub device_name: String,
    pub inbox_directory: PathBuf,
}

impl AgentSettings {
    pub fn validate(&self) -> io::Result<()> {
        if self.version != AGENT_SETTINGS_VERSION {
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
        Ok(())
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
    ListDevices,
    RevokeDevice {
        device: String,
    },
    ListInbox {
        limit: usize,
    },
    LatestInbox,
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
    Devices {
        devices: Vec<DeviceSummary>,
    },
    DeviceRevoked {
        device: DeviceSummary,
    },
    Inbox {
        items: Vec<InboxItem>,
    },
    Latest {
        item: Option<InboxItem>,
    },
    Snapshot {
        snapshot: AgentSnapshot,
    },
    Events {
        cursor: AgentEventCursor,
        events: Vec<AgentEventEnvelope>,
    },
    SnapshotRequired {
        cursor: AgentEventCursor,
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
        if let AgentRequest::Events { after, .. } = &self.request {
            after.validate()?;
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
            AgentResponse::Snapshot { snapshot } => snapshot.event_cursor.validate()?,
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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSnapshot {
    pub status: AgentStatus,
    pub engine: EngineSnapshot,
    pub inbox: Vec<InboxItem>,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairingInvitation {
    pub label: String,
    pub room_code: String,
    pub verification_code: String,
    pub expires_at_unix_seconds: u64,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistentProductState {
    schema_version: u16,
    devices: Vec<RememberedDeviceRecord>,
    inbox: Vec<InboxItem>,
}

impl Default for PersistentProductState {
    fn default() -> Self {
        Self {
            schema_version: PRODUCT_STATE_SCHEMA_VERSION,
            devices: Vec::new(),
            inbox: Vec::new(),
        }
    }
}

#[cfg(test)]
struct LegacyProductStore {
    directory: PathBuf,
    credentials: DesktopCredentialStore,
    state: PersistentProductState,
}

#[cfg(test)]
impl LegacyProductStore {
    pub fn open(directory: impl Into<PathBuf>) -> io::Result<Self> {
        let directory = directory.into();
        create_private_directory(&directory)?;
        let path = directory.join(PRODUCT_STATE_FILE);
        let state = match fs::read(&path) {
            Ok(bytes) => {
                let state: PersistentProductState = serde_json::from_slice(&bytes)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                if state.schema_version != PRODUCT_STATE_SCHEMA_VERSION {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("unsupported product state schema {}", state.schema_version),
                    ));
                }
                state
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                PersistentProductState::default()
            }
            Err(error) => return Err(error),
        };
        Ok(Self {
            credentials: DesktopCredentialStore::new(directory.join("credentials")),
            directory,
            state,
        })
    }

    pub fn prepare_device(
        &self,
        label: &str,
        broker: &str,
        relay: Option<&str>,
    ) -> io::Result<PreparedRememberedDevice> {
        let label = validate_label(label)?;
        if self
            .state
            .devices
            .iter()
            .any(|device| device.label.eq_ignore_ascii_case(&label))
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("device label {label:?} is already paired"),
            ));
        }
        if broker.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "remembered device broker cannot be empty",
            ));
        }
        Ok(PreparedRememberedDevice {
            id: random_identifier("dev")?,
            label,
            credential_reference: random_identifier("cred")?,
            broker: broker.to_string(),
            relay: relay.map(str::to_string),
        })
    }

    pub fn commit_device(
        &mut self,
        prepared: PreparedRememberedDevice,
        opaque_credential: &[u8],
        generation: u64,
    ) -> io::Result<DeviceSummary> {
        if self.state.devices.iter().any(|device| {
            device.id == prepared.id || device.label.eq_ignore_ascii_case(&prepared.label)
        }) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "remembered device already exists",
            ));
        }
        self.credentials
            .put(&prepared.credential_reference, opaque_credential)?;
        let record = RememberedDeviceRecord {
            id: prepared.id,
            label: prepared.label,
            credential_reference: prepared.credential_reference,
            generation,
            previous_generation: None,
            broker: prepared.broker,
            relay: prepared.relay,
        };
        self.state.devices.push(record);
        if let Err(error) = self.save() {
            let record = self.state.devices.pop().expect("new device was appended");
            let _ = self.credentials.delete(&record.credential_reference);
            return Err(error);
        }
        Ok(self
            .state
            .devices
            .last()
            .expect("new device remains stored")
            .summary())
    }

    pub fn device_records(&self) -> Vec<RememberedDeviceRecord> {
        self.state.devices.clone()
    }

    pub fn device_credential(&self, id: &str) -> io::Result<Vec<u8>> {
        let device = self
            .state
            .devices
            .iter()
            .find(|device| device.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "remembered device missing"))?;
        self.credentials
            .get(&device.credential_reference)?
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "remembered device credential missing",
                )
            })
    }

    pub fn append_inbox(&mut self, item: InboxItem) -> io::Result<()> {
        if self
            .state
            .inbox
            .iter()
            .any(|existing| existing.id == item.id)
        {
            return Ok(());
        }
        let previous = self.state.inbox.clone();
        self.state.inbox.push(item);
        self.state
            .inbox
            .sort_by_key(|item| item.received_at_unix_ms);
        let overflow = self.state.inbox.len().saturating_sub(MAX_INBOX_ITEMS);
        if overflow > 0 {
            self.state.inbox.drain(..overflow);
        }
        if let Err(error) = self.save() {
            self.state.inbox = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn latest_inbox(&self) -> Option<InboxItem> {
        self.state.inbox.last().cloned()
    }

    fn save(&self) -> io::Result<()> {
        create_private_directory(&self.directory)?;
        let target = self.directory.join(PRODUCT_STATE_FILE);
        let temporary = self
            .directory
            .join(format!(".{PRODUCT_STATE_FILE}.{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec_pretty(&self.state)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            replace_file(&temporary, &target)?;
            #[cfg(unix)]
            fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
            File::open(&self.directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

/// Agent-facing projection of the unified Engine store and desktop vault.
pub struct ProductStore {
    engine: EngineStore,
    vault: DesktopCredentialStore,
}

impl ProductStore {
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self, EngineStoreError> {
        let directory = directory.into();
        let vault = DesktopCredentialStore::new(directory.join("vault"));
        let mut engine = EngineStore::open(&directory)?;
        import_v0_2_product_state(&mut engine, &directory.join("product"), &vault)?;
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
        if self.vault.contains(&prepared.credential_reference)? {
            return Err(EngineStoreError::InvalidState(
                "prepared vault reference already exists".into(),
            ));
        }

        let device_id = DeviceId::parse(prepared.id.clone())
            .map_err(|error| EngineStoreError::InvalidState(error.to_string()))?;
        let relationship_id = RelationshipId::parse(prepared.id.clone())
            .expect("Device and Relationship identifiers share validation");
        let vault_reference = VaultReference::parse(prepared.credential_reference.clone())?;
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
                vault_reference: Some(vault_reference),
                broker: prepared.broker,
                relay: prepared.relay,
            },
        );

        self.vault
            .put(&prepared.credential_reference, opaque_credential)?;
        if let Err(error) = self.engine.replace(state) {
            self.vault
                .delete(&prepared.credential_reference)
                .map_err(|rollback| {
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
        let old_credential = Zeroizing::new(self.device_credential(id)?);
        let changed = old_credential.as_slice() != opaque_credential;
        if changed {
            self.vault
                .put(&current.credential_reference, opaque_credential)?;
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
                    .put(&current.credential_reference, &old_credential)
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

    pub fn device_credential(&self, id: &str) -> Result<Vec<u8>, EngineStoreError> {
        let device = self
            .device_record(id)
            .ok_or_else(|| EngineStoreError::InvalidState("remembered device is missing".into()))?;
        self.vault
            .get(&device.credential_reference)?
            .ok_or_else(|| {
                EngineStoreError::InvalidState("remembered device credential is missing".into())
            })
    }

    pub fn forget_device(&mut self, selector: &str) -> Result<DeviceSummary, EngineStoreError> {
        let selector = selector.trim();
        if selector.is_empty() || selector.len() > 128 || selector.chars().any(char::is_control) {
            return Err(EngineStoreError::InvalidState(
                "device selector must be a device ID or exact label".into(),
            ));
        }
        let record = self
            .device_records()
            .into_iter()
            .find(|device| device.id == selector || device.label.eq_ignore_ascii_case(selector))
            .ok_or_else(|| EngineStoreError::InvalidState("remembered device is missing".into()))?;
        let credential = self
            .vault
            .get(&record.credential_reference)?
            .map(Zeroizing::new);
        self.vault.delete(&record.credential_reference)?;

        let relationship_id =
            RelationshipId::parse(record.id.clone()).expect("stored device ID was validated");
        let mut state = self.engine.state().clone();
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
                    .put(&record.credential_reference, &credential)
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

    pub fn engine_snapshot(&self) -> EngineSnapshot {
        self.engine.state().snapshot.clone()
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum V02ProductImportOutcome {
    NoLegacyState,
    AlreadyImported(V02ImportRecord),
    Imported(V02ImportRecord),
}

/// Imports the desktop v0.2 ProductStore into an empty Engine store.
///
/// The source JSON and credential directory are read-only inputs. Usable
/// credentials are copied through the Agent-owned vault adapter before the new
/// Engine state is activated; unusable records become explicit re-pair items.
pub fn import_v0_2_product_state(
    engine: &mut EngineStore,
    legacy_directory: &Path,
    target_vault: &DesktopCredentialStore,
) -> Result<V02ProductImportOutcome, EngineStoreError> {
    if let Some(record) = &engine.state().migration.v0_2_import {
        return Ok(V02ProductImportOutcome::AlreadyImported(record.clone()));
    }
    let source_path = legacy_directory.join(PRODUCT_STATE_FILE);
    let Some(source_bytes) = read_bounded_file(&source_path)? else {
        return Ok(V02ProductImportOutcome::NoLegacyState);
    };
    if engine.origin() != EngineStoreOrigin::Empty {
        return Err(EngineStoreError::InvalidState(
            "cannot import v0.2 state into an initialized Engine store".into(),
        ));
    }

    let legacy: PersistentProductState =
        serde_json::from_slice(&source_bytes).map_err(EngineStoreError::Decode)?;
    if legacy.schema_version != PRODUCT_STATE_SCHEMA_VERSION {
        return Err(EngineStoreError::InvalidState(format!(
            "unsupported v0.2 product schema {}",
            legacy.schema_version
        )));
    }
    if legacy.devices.len() > MAX_DURABLE_ENTITIES || legacy.inbox.len() > MAX_INBOX_ITEMS {
        return Err(EngineStoreError::InvalidState(
            "v0.2 product state exceeds collection limits".into(),
        ));
    }

    let source_vault = DesktopCredentialStore::new(legacy_directory.join("credentials"));
    let mut candidate = EngineState::default();
    candidate.inbox = legacy.inbox;
    let mut credential_copies = Vec::new();
    let mut re_pair_required = Vec::new();
    let mut labels = BTreeSet::new();

    for record in legacy.devices {
        let label = validate_label(&record.label).map_err(EngineStoreError::Io)?;
        if !labels.insert(label.to_ascii_lowercase()) {
            return Err(EngineStoreError::InvalidState(format!(
                "duplicate v0.2 device label {label:?}"
            )));
        }
        let device_id = match DeviceId::parse(record.id.clone()) {
            Ok(device_id) => device_id,
            Err(_) => {
                re_pair_required.push(RePairRequiredRelationship {
                    legacy_device_id: record.id,
                    label,
                    reason: RePairReason::InvalidMetadata,
                });
                continue;
            }
        };
        let relationship_id = RelationshipId::parse(device_id.as_str().to_string())
            .expect("Device and Relationship identifiers share validation");
        if candidate.snapshot.devices.contains_key(&device_id) {
            return Err(EngineStoreError::InvalidState(format!(
                "duplicate v0.2 device ID {device_id}"
            )));
        }
        if record
            .previous_generation
            .is_some_and(|previous| previous >= record.generation)
        {
            re_pair_required.push(RePairRequiredRelationship {
                legacy_device_id: record.id,
                label,
                reason: RePairReason::InvalidMetadata,
            });
            continue;
        }
        let vault_reference = match VaultReference::parse(record.credential_reference.clone()) {
            Ok(reference) => reference,
            Err(_) => {
                re_pair_required.push(RePairRequiredRelationship {
                    legacy_device_id: record.id,
                    label,
                    reason: RePairReason::InvalidMetadata,
                });
                continue;
            }
        };
        let durable = DurableRelationship {
            vault_reference: Some(vault_reference.clone()),
            broker: record.broker,
            relay: record.relay,
        };
        if durable.validate(RelationshipState::Trusted).is_err() {
            re_pair_required.push(RePairRequiredRelationship {
                legacy_device_id: record.id,
                label,
                reason: RePairReason::InvalidMetadata,
            });
            continue;
        }
        let Some(opaque) = source_vault
            .get(&record.credential_reference)
            .map_err(EngineStoreError::Io)?
        else {
            re_pair_required.push(RePairRequiredRelationship {
                legacy_device_id: record.id,
                label,
                reason: RePairReason::MissingCredential,
            });
            continue;
        };
        let opaque = Zeroizing::new(opaque);
        if RememberedCredential::from_opaque(&opaque).is_err() {
            re_pair_required.push(RePairRequiredRelationship {
                legacy_device_id: record.id,
                label,
                reason: RePairReason::UnsupportedCredential,
            });
            continue;
        }

        candidate.snapshot.devices.insert(
            device_id.clone(),
            Device {
                id: device_id.clone(),
                display_name: label,
            },
        );
        candidate.snapshot.relationships.insert(
            relationship_id.clone(),
            Relationship {
                id: relationship_id.clone(),
                device_id,
                generation: record.generation,
                previous_generation: record.previous_generation,
                state: RelationshipState::Trusted,
            },
        );
        candidate
            .durable_relationships
            .insert(relationship_id, durable);
        credential_copies.push((vault_reference, opaque));
    }

    let imported_relationships =
        u32::try_from(candidate.snapshot.relationships.len()).expect("collection limit fits u32");
    let imported_inbox_items = u32::try_from(candidate.inbox.len()).expect("Inbox limit fits u32");
    let import_record = V02ImportRecord {
        backup_file: V02_PRODUCT_STATE_BACKUP.into(),
        imported_relationships,
        imported_inbox_items,
        re_pair_required,
    };
    candidate.migration.v0_2_import = Some(import_record.clone());
    candidate.validate()?;

    engine.install_migration_backup(V02_PRODUCT_STATE_BACKUP, &source_bytes)?;
    for (reference, credential) in credential_copies {
        if let Some(existing) = target_vault
            .get(reference.as_str())
            .map_err(EngineStoreError::Io)?
        {
            let existing = Zeroizing::new(existing);
            if existing.as_slice() != credential.as_slice() {
                return Err(EngineStoreError::InvalidState(format!(
                    "target vault reference {} already contains different data",
                    reference.as_str()
                )));
            }
        } else {
            target_vault
                .put(reference.as_str(), &credential)
                .map_err(EngineStoreError::Io)?;
        }
    }
    engine.replace(candidate)?;
    Ok(V02ProductImportOutcome::Imported(import_record))
}

pub fn default_agent_state_directory() -> io::Result<PathBuf> {
    if let Some(value) = env::var_os("ENVOIX_STATE_DIR").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    if let Some(value) = env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value).join("envoix"));
    }
    if let Some(value) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value).join(".local/state/envoix"));
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "cannot locate Envoix state directory; set ENVOIX_STATE_DIR",
    ))
}

pub fn default_agent_socket_path() -> io::Result<PathBuf> {
    if let Some(value) = env::var_os("ENVOIX_AGENT_SOCKET").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    Ok(default_agent_state_directory()?.join("agent.sock"))
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
fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(windows))]
#[cfg(test)]
fn replace_file(temporary: &Path, target: &Path) -> io::Result<()> {
    fs::rename(temporary, target)
}

#[cfg(windows)]
#[cfg(test)]
fn replace_file(temporary: &Path, target: &Path) -> io::Result<()> {
    tempfile::TempPath::try_from_path(temporary.to_path_buf())?
        .persist(target)
        .map_err(io::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let metadata = fs::read(directory.path().join("engine-state-v1.json")).unwrap();
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
        assert_eq!(reopened.device_credential(&id).unwrap(), credential);
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

        let forgotten = store.forget_device("macbook").unwrap();

        assert_eq!(forgotten.id, id);
        assert_eq!(forgotten.label, "MacBook");
        assert!(store.devices().is_empty());
        assert!(!credential_path.exists());
        assert_eq!(store.latest_inbox().unwrap().id, "received");
        assert!(store.prepare_device("MacBook", "broker", None).is_ok());

        drop(store);
        let reopened = ProductStore::open(directory.path()).unwrap();
        assert!(reopened.devices().is_empty());
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
    fn product_state_fixtures_load_or_fail_closed() {
        let valid = include_bytes!("../../../tests/fixtures/v0.2/product-state-v1.json");
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(PRODUCT_STATE_FILE), valid).unwrap();

        let store = LegacyProductStore::open(directory.path()).unwrap();
        let device = &store.device_records()[0];
        assert_eq!(device.id(), "dev_fixture_not_a_real_identity");
        assert_eq!(device.generation(), 4);
        assert_eq!(device.previous_generation(), Some(3));
        assert_eq!(store.latest_inbox().unwrap().id, "job_fixture_delivered");
        assert_eq!(
            store.device_credential(device.id()).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );

        for (name, invalid) in [
            (
                "corrupt",
                include_bytes!("../../../tests/fixtures/v0.2/product-state-v1-corrupt.json")
                    as &[u8],
            ),
            (
                "truncated",
                include_bytes!("../../../tests/fixtures/v0.2/product-state-v1-truncated.json"),
            ),
            (
                "unknown-version",
                include_bytes!("../../../tests/fixtures/v0.2/product-state-unknown-version.json"),
            ),
            (
                "partial-migration",
                include_bytes!("../../../tests/fixtures/v0.2/product-state-partial-migration.json"),
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            fs::write(directory.path().join(PRODUCT_STATE_FILE), invalid).unwrap();
            let error = LegacyProductStore::open(directory.path())
                .err()
                .unwrap_or_else(|| panic!("{name} fixture unexpectedly loaded"));
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{name}");
        }
    }

    #[test]
    fn v0_2_import_is_atomic_restartable_and_keeps_secrets_out_of_engine_state() {
        let directory = tempfile::tempdir().unwrap();
        let legacy_directory = directory.path().join("product");
        let credential = RememberedCredential::from_control_pairing(
            b"fixture control key",
            envoix_invite::Commitment::sha256(b"fixture transcript"),
        )
        .to_opaque();
        let mut legacy = LegacyProductStore::open(&legacy_directory).unwrap();
        let pending = legacy
            .prepare_device(
                "MacBook",
                "broker.invalid:8445",
                Some("https://relay.invalid"),
            )
            .unwrap();
        let device_id = pending.id().to_string();
        let credential_reference = pending.credential_reference.clone();
        legacy.commit_device(pending, &credential, 4).unwrap();
        legacy.append_inbox(inbox_item("received", 1)).unwrap();
        drop(legacy);
        let source_path = legacy_directory.join(PRODUCT_STATE_FILE);
        let source_bytes = fs::read(&source_path).unwrap();

        let target_vault = DesktopCredentialStore::new(directory.path().join("vault"));
        let mut engine = EngineStore::open(directory.path()).unwrap();
        let outcome =
            import_v0_2_product_state(&mut engine, &legacy_directory, &target_vault).unwrap();
        let V02ProductImportOutcome::Imported(record) = outcome else {
            panic!("expected a new import");
        };
        assert_eq!(record.imported_relationships, 1);
        assert_eq!(record.imported_inbox_items, 1);
        assert!(record.re_pair_required.is_empty());
        let relationship_id = RelationshipId::parse(device_id.clone()).unwrap();
        assert_eq!(
            engine
                .state()
                .snapshot
                .relationships
                .get(&relationship_id)
                .unwrap()
                .generation,
            4
        );
        assert_eq!(
            target_vault.get(&credential_reference).unwrap().unwrap(),
            credential
        );
        assert_eq!(fs::read(&source_path).unwrap(), source_bytes);
        assert_eq!(
            fs::read(
                directory
                    .path()
                    .join("migration")
                    .join(V02_PRODUCT_STATE_BACKUP)
            )
            .unwrap(),
            source_bytes
        );
        let serialized = serde_json::to_vec(engine.state()).unwrap();
        assert!(
            !serialized
                .windows(credential.len())
                .any(|value| value == credential)
        );

        assert!(matches!(
            import_v0_2_product_state(&mut engine, &legacy_directory, &target_vault).unwrap(),
            V02ProductImportOutcome::AlreadyImported(_)
        ));
        drop(engine);
        let reopened = EngineStore::open(directory.path()).unwrap();
        assert!(
            reopened
                .state()
                .snapshot
                .relationships
                .contains_key(&relationship_id)
        );
    }

    #[test]
    fn v0_2_import_records_missing_credentials_as_re_pair_required() {
        let directory = tempfile::tempdir().unwrap();
        let legacy_directory = directory.path().join("product");
        fs::create_dir_all(&legacy_directory).unwrap();
        let source = include_bytes!("../../../tests/fixtures/v0.2/product-state-v1.json");
        fs::write(legacy_directory.join(PRODUCT_STATE_FILE), source).unwrap();
        let migration_directory = directory.path().join("migration");
        fs::create_dir_all(&migration_directory).unwrap();
        fs::write(migration_directory.join(V02_PRODUCT_STATE_BACKUP), source).unwrap();

        let target_vault = DesktopCredentialStore::new(directory.path().join("vault"));
        let mut engine = EngineStore::open(directory.path()).unwrap();
        let V02ProductImportOutcome::Imported(record) =
            import_v0_2_product_state(&mut engine, &legacy_directory, &target_vault).unwrap()
        else {
            panic!("expected a resumed import");
        };

        assert_eq!(record.imported_relationships, 0);
        assert_eq!(record.imported_inbox_items, 1);
        assert_eq!(record.re_pair_required.len(), 1);
        assert_eq!(
            record.re_pair_required[0].reason,
            RePairReason::MissingCredential
        );
        assert!(engine.state().snapshot.relationships.is_empty());
        assert_eq!(
            fs::read(legacy_directory.join(PRODUCT_STATE_FILE)).unwrap(),
            source
        );
        assert_eq!(
            fs::read(migration_directory.join(V02_PRODUCT_STATE_BACKUP)).unwrap(),
            source
        );
    }

    #[test]
    fn failed_v0_2_import_preserves_source_and_received_files() {
        for (name, source) in [
            (
                "corrupt",
                include_bytes!("../../../tests/fixtures/v0.2/product-state-v1-corrupt.json")
                    as &[u8],
            ),
            (
                "truncated",
                include_bytes!("../../../tests/fixtures/v0.2/product-state-v1-truncated.json"),
            ),
            (
                "unknown-version",
                include_bytes!("../../../tests/fixtures/v0.2/product-state-unknown-version.json"),
            ),
            (
                "partial-migration",
                include_bytes!("../../../tests/fixtures/v0.2/product-state-partial-migration.json"),
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let legacy_directory = directory.path().join("product");
            fs::create_dir_all(&legacy_directory).unwrap();
            let source_path = legacy_directory.join(PRODUCT_STATE_FILE);
            fs::write(&source_path, source).unwrap();
            let inbox_directory = directory.path().join("inbox");
            fs::create_dir_all(&inbox_directory).unwrap();
            let received_path = inbox_directory.join("keep.txt");
            fs::write(&received_path, b"received bytes").unwrap();
            let target_vault = DesktopCredentialStore::new(directory.path().join("vault"));
            let mut engine = EngineStore::open(directory.path()).unwrap();

            assert!(
                import_v0_2_product_state(&mut engine, &legacy_directory, &target_vault).is_err(),
                "{name} unexpectedly imported"
            );
            assert_eq!(engine.origin(), EngineStoreOrigin::Empty, "{name}");
            assert_eq!(engine.state(), &EngineState::default(), "{name}");
            assert_eq!(fs::read(&source_path).unwrap(), source, "{name}");
            assert_eq!(
                fs::read(&received_path).unwrap(),
                b"received bytes",
                "{name}"
            );
        }
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
        assert_eq!(AGENT_PROTOCOL_VERSION, 5);
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
    fn agent_wire_v5_fixture_round_trips_every_variant() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.3/agent-control-v5.json"
        ))
        .unwrap();
        assert_eq!(fixture["fixture_version"], 1);
        assert_eq!(fixture["protocol_version"], AGENT_PROTOCOL_VERSION);

        let settings: AgentSettings = serde_json::from_value(fixture["settings"].clone()).unwrap();
        settings.validate().unwrap();
        assert_eq!(serde_json::to_value(settings).unwrap(), fixture["settings"]);

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
                    request: AgentRequest::ListInbox { .. },
                    ..
                },
                AgentRequestEnvelope {
                    request: AgentRequest::LatestInbox,
                    ..
                },
            ]
        ));
        for (request, expected) in requests.iter().zip(request_values) {
            request.validate().unwrap();
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
                    response: AgentResponse::Error { .. },
                    ..
                },
            ]
        ));
        for (response, expected) in responses.iter().zip(response_values) {
            response.validate_for(&response.request_id).unwrap();
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
            ]
        ));
        let AgentResponse::Pairing { pairing } = &responses[4].response else {
            unreachable!("fixture order is checked above");
        };
        assert_eq!(pairing.expires_at_unix_seconds, 1);
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
                },
            },
        )
        .unwrap();
        assert!(response.validate_for("request_2").is_err());
    }

    #[test]
    fn agent_settings_validate_version_name_and_inbox() {
        let mut settings = AgentSettings {
            version: AGENT_SETTINGS_VERSION,
            device_name: "WSL".into(),
            inbox_directory: PathBuf::from("/tmp/inbox"),
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
}
