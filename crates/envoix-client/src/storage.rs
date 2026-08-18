//! Durable, non-secret Engine state owned by one process.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::model::{RelationshipId, RelationshipState, RoomState, TransferState};
use crate::product::InboxItem;
use crate::snapshot::EngineSnapshot;

pub const ENGINE_STATE_SCHEMA_VERSION: u16 = 1;
pub const MAX_ENGINE_STATE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_DURABLE_ENTITIES: usize = 1_000;

const ENGINE_STATE_FILE: &str = "engine-state-v1.json";
const PREVIOUS_ENGINE_STATE_FILE: &str = "engine-state-v1.previous.json";
const ENGINE_LOCK_FILE: &str = "engine.lock";
const MAX_REFERENCE_BYTES: usize = 128;
const MAX_LABEL_CHARS: usize = 256;
const MAX_ENDPOINT_BYTES: usize = 2_048;
const MAX_PATH_BYTES: usize = 4_096;

#[derive(Debug, Error)]
pub enum EngineStoreError {
    #[error("Engine state directory is already owned: {directory}")]
    AlreadyOwned { directory: PathBuf },
    #[error("Engine state is {actual} bytes; maximum is {maximum} bytes")]
    StateTooLarge { actual: u64, maximum: u64 },
    #[error("unsupported Engine state schema {actual}; expected {expected}")]
    UnsupportedSchema { expected: u16, actual: u16 },
    #[error("invalid Engine state: {0}")]
    InvalidState(String),
    #[error("decode Engine state: {0}")]
    Decode(#[source] serde_json::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineStoreOrigin {
    Empty,
    Current,
    RecoveredPrevious,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct VaultReference(String);

impl VaultReference {
    pub fn parse(value: impl Into<String>) -> Result<Self, EngineStoreError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_REFERENCE_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(invalid(
                "vault reference is not a bounded opaque identifier",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for VaultReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VaultReference(<redacted>)")
    }
}

impl<'de> Deserialize<'de> for VaultReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurableRelationship {
    pub vault_reference: Option<VaultReference>,
    pub broker: String,
    pub relay: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RePairReason {
    InvalidMetadata,
    MissingCredential,
    UnsupportedCredential,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RePairRequiredRelationship {
    pub legacy_device_id: String,
    pub label: String,
    pub reason: RePairReason,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct V02ImportRecord {
    pub backup_file: String,
    pub imported_relationships: u32,
    pub imported_inbox_items: u32,
    pub re_pair_required: Vec<RePairRequiredRelationship>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationMetadata {
    pub v0_2_import: Option<V02ImportRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EngineState {
    schema_version: u16,
    pub snapshot: EngineSnapshot,
    pub durable_relationships: BTreeMap<RelationshipId, DurableRelationship>,
    pub inbox: Vec<InboxItem>,
    pub migration: MigrationMetadata,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            schema_version: ENGINE_STATE_SCHEMA_VERSION,
            snapshot: EngineSnapshot::new(),
            durable_relationships: BTreeMap::new(),
            inbox: Vec::new(),
            migration: MigrationMetadata::default(),
        }
    }
}

impl EngineState {
    pub fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub fn validate(&self) -> Result<(), EngineStoreError> {
        if self.schema_version != ENGINE_STATE_SCHEMA_VERSION {
            return Err(EngineStoreError::UnsupportedSchema {
                expected: ENGINE_STATE_SCHEMA_VERSION,
                actual: self.schema_version,
            });
        }
        self.snapshot
            .validate_contract()
            .map_err(|error| invalid(error.to_string()))?;
        validate_entity_count("devices", self.snapshot.devices.len())?;
        validate_entity_count("relationships", self.snapshot.relationships.len())?;
        validate_entity_count("rooms", self.snapshot.rooms.len())?;
        validate_entity_count("transfers", self.snapshot.transfers.len())?;
        validate_entity_count("Inbox", self.inbox.len())?;
        validate_entity_count("durable relationships", self.durable_relationships.len())?;

        for (device_id, device) in &self.snapshot.devices {
            if device_id != &device.id {
                return Err(invalid(format!(
                    "device map key {device_id} does not match its record"
                )));
            }
            validate_text("device display name", &device.display_name, MAX_LABEL_CHARS)?;
        }

        for (relationship_id, relationship) in &self.snapshot.relationships {
            if relationship_id != &relationship.id {
                return Err(invalid(format!(
                    "relationship map key {relationship_id} does not match its record"
                )));
            }
            if !self.snapshot.devices.contains_key(&relationship.device_id) {
                return Err(invalid(format!(
                    "relationship {relationship_id} references a missing device"
                )));
            }
            let durable = self
                .durable_relationships
                .get(relationship_id)
                .ok_or_else(|| {
                    invalid(format!(
                        "relationship {relationship_id} has no durable metadata"
                    ))
                })?;
            validate_endpoint("relationship broker", &durable.broker)?;
            if let Some(relay) = &durable.relay {
                validate_endpoint("relationship relay", relay)?;
            }
            match relationship.state {
                RelationshipState::Trusted if durable.vault_reference.is_none() => {
                    return Err(invalid(format!(
                        "trusted relationship {relationship_id} has no vault reference"
                    )));
                }
                RelationshipState::Revoked if durable.vault_reference.is_some() => {
                    return Err(invalid(format!(
                        "revoked relationship {relationship_id} retains a vault reference"
                    )));
                }
                RelationshipState::Trusted | RelationshipState::Revoked => {}
            }
        }
        for relationship_id in self.durable_relationships.keys() {
            if !self.snapshot.relationships.contains_key(relationship_id) {
                return Err(invalid(format!(
                    "durable relationship {relationship_id} has no application record"
                )));
            }
        }

        for (room_id, room) in &self.snapshot.rooms {
            if room_id != &room.id {
                return Err(invalid(format!(
                    "Room map key {room_id} does not match its record"
                )));
            }
            if let Some(relationship_id) = &room.relationship_id
                && !self.snapshot.relationships.contains_key(relationship_id)
            {
                return Err(invalid(format!(
                    "Room {room_id} references a missing relationship"
                )));
            }
            if let Some(replacement_id) = &room.replacement_room_id
                && !self.snapshot.rooms.contains_key(replacement_id)
            {
                return Err(invalid(format!(
                    "Room {room_id} references a missing replacement"
                )));
            }
            match room.state {
                RoomState::Closed if room.close_reason.is_none() => {
                    return Err(invalid(format!(
                        "closed Room {room_id} has no close reason"
                    )));
                }
                RoomState::Connecting | RoomState::Authenticating | RoomState::Connected
                    if room.close_reason.is_some() =>
                {
                    return Err(invalid(format!("open Room {room_id} has a close reason")));
                }
                RoomState::Connecting
                | RoomState::Authenticating
                | RoomState::Connected
                | RoomState::Closed => {}
            }
        }

        for (transfer_id, transfer) in &self.snapshot.transfers {
            if transfer_id != &transfer.id {
                return Err(invalid(format!(
                    "Transfer map key {transfer_id} does not match its record"
                )));
            }
            if !self
                .snapshot
                .relationships
                .contains_key(&transfer.relationship_id)
            {
                return Err(invalid(format!(
                    "Transfer {transfer_id} references a missing relationship"
                )));
            }
            if let Some(room_id) = &transfer.room_id
                && !self.snapshot.rooms.contains_key(room_id)
            {
                return Err(invalid(format!(
                    "Transfer {transfer_id} references a missing Room"
                )));
            }
            if transfer.transferred_bytes > transfer.total_bytes {
                return Err(invalid(format!(
                    "Transfer {transfer_id} progress exceeds its total"
                )));
            }
            if (transfer.state == TransferState::Failed) != transfer.failure.is_some() {
                return Err(invalid(format!(
                    "Transfer {transfer_id} failure does not match its state"
                )));
            }
            if (transfer.state == TransferState::Rejected) != transfer.rejection.is_some() {
                return Err(invalid(format!(
                    "Transfer {transfer_id} rejection does not match its state"
                )));
            }
        }

        let mut inbox_ids = BTreeSet::new();
        for item in &self.inbox {
            validate_identifier_text("Inbox item ID", &item.id)?;
            validate_identifier_text("Inbox sender device ID", &item.from_device_id)?;
            validate_text(
                "Inbox sender label",
                &item.from_device_label,
                MAX_LABEL_CHARS,
            )?;
            if item.roots.len() > 3 {
                return Err(invalid(format!(
                    "Inbox item {} has more than three roots",
                    item.id
                )));
            }
            if u64::try_from(item.roots.len()).unwrap_or(u64::MAX)
                > u64::from(item.file_count) + u64::from(item.directory_count)
            {
                return Err(invalid(format!(
                    "Inbox item {} has more roots than items",
                    item.id
                )));
            }
            for root in &item.roots {
                validate_text("Inbox root name", &root.name, MAX_LABEL_CHARS)?;
                validate_text_bytes("Inbox root path", &root.path, MAX_PATH_BYTES)?;
            }
            if !inbox_ids.insert(&item.id) {
                return Err(invalid(format!("duplicate Inbox item {}", item.id)));
            }
        }

        if let Some(import) = &self.migration.v0_2_import {
            validate_backup_file(&import.backup_file)?;
            validate_entity_count(
                "relationships requiring re-pair",
                import.re_pair_required.len(),
            )?;
            let mut legacy_ids = BTreeSet::new();
            for relationship in &import.re_pair_required {
                validate_identifier_text("legacy device ID", &relationship.legacy_device_id)?;
                validate_text("legacy device label", &relationship.label, MAX_LABEL_CHARS)?;
                if !legacy_ids.insert(&relationship.legacy_device_id) {
                    return Err(invalid(format!(
                        "duplicate legacy device {} in migration metadata",
                        relationship.legacy_device_id
                    )));
                }
            }
        }
        Ok(())
    }
}

pub struct EngineStore {
    directory: PathBuf,
    state: EngineState,
    origin: EngineStoreOrigin,
    has_current_snapshot: bool,
    _owner_lock: File,
}

impl EngineStore {
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self, EngineStoreError> {
        let directory = directory.into();
        create_private_directory(&directory)?;
        let owner_lock = acquire_owner_lock(&directory)?;
        let current_path = directory.join(ENGINE_STATE_FILE);
        let previous_path = directory.join(PREVIOUS_ENGINE_STATE_FILE);

        let loaded_current = load_state(&current_path);
        let (state, origin, has_current_snapshot) = match loaded_current {
            Ok(Some(state)) => (state, EngineStoreOrigin::Current, true),
            Ok(None) => match load_state(&previous_path)? {
                Some(state) => (state, EngineStoreOrigin::RecoveredPrevious, false),
                None => (EngineState::default(), EngineStoreOrigin::Empty, false),
            },
            Err(error) if error.allows_previous_recovery() => match load_state(&previous_path) {
                Ok(Some(state)) => (state, EngineStoreOrigin::RecoveredPrevious, false),
                Ok(None) | Err(_) => return Err(error),
            },
            Err(error) => return Err(error),
        };

        Ok(Self {
            directory,
            state,
            origin,
            has_current_snapshot,
            _owner_lock: owner_lock,
        })
    }

    pub fn origin(&self) -> EngineStoreOrigin {
        self.origin
    }

    pub fn state(&self) -> &EngineState {
        &self.state
    }

    pub fn replace(&mut self, state: EngineState) -> Result<(), EngineStoreError> {
        state.validate()?;
        let bytes = encode_state(&state)?;
        if self.has_current_snapshot {
            let previous = encode_state(&self.state)?;
            write_atomic(&self.directory, PREVIOUS_ENGINE_STATE_FILE, &previous)?;
        }

        let target = self.directory.join(ENGINE_STATE_FILE);
        if let Err(error) = write_atomic(&self.directory, ENGINE_STATE_FILE, &bytes) {
            if load_state(&target).ok().flatten().as_ref() == Some(&state) {
                self.state = state;
                self.origin = EngineStoreOrigin::Current;
                self.has_current_snapshot = true;
            }
            return Err(error);
        }
        self.state = state;
        self.origin = EngineStoreOrigin::Current;
        self.has_current_snapshot = true;
        Ok(())
    }
}

impl EngineStoreError {
    fn allows_previous_recovery(&self) -> bool {
        matches!(
            self,
            Self::StateTooLarge { .. } | Self::InvalidState(_) | Self::Decode(_)
        )
    }
}

#[derive(Deserialize)]
struct SchemaHeader {
    schema_version: u16,
}

fn load_state(path: &Path) -> Result<Option<EngineState>, EngineStoreError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let length = file.metadata()?.len();
    if length > MAX_ENGINE_STATE_BYTES {
        return Err(EngineStoreError::StateTooLarge {
            actual: length,
            maximum: MAX_ENGINE_STATE_BYTES,
        });
    }
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or(0));
    file.take(MAX_ENGINE_STATE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ENGINE_STATE_BYTES {
        return Err(EngineStoreError::StateTooLarge {
            actual: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            maximum: MAX_ENGINE_STATE_BYTES,
        });
    }
    let header: SchemaHeader = serde_json::from_slice(&bytes).map_err(EngineStoreError::Decode)?;
    if header.schema_version != ENGINE_STATE_SCHEMA_VERSION {
        return Err(EngineStoreError::UnsupportedSchema {
            expected: ENGINE_STATE_SCHEMA_VERSION,
            actual: header.schema_version,
        });
    }
    let state: EngineState = serde_json::from_slice(&bytes).map_err(EngineStoreError::Decode)?;
    state.validate()?;
    Ok(Some(state))
}

fn encode_state(state: &EngineState) -> Result<Vec<u8>, EngineStoreError> {
    let bytes = serde_json::to_vec_pretty(state).map_err(EngineStoreError::Decode)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ENGINE_STATE_BYTES {
        return Err(EngineStoreError::StateTooLarge {
            actual: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            maximum: MAX_ENGINE_STATE_BYTES,
        });
    }
    Ok(bytes)
}

fn acquire_owner_lock(directory: &Path) -> Result<File, EngineStoreError> {
    let path = directory.join(ENGINE_LOCK_FILE);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    fs::set_permissions(
        directory.join(ENGINE_LOCK_FILE),
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err(EngineStoreError::AlreadyOwned {
            directory: directory.to_path_buf(),
        }),
        Err(fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

fn write_atomic(directory: &Path, target_name: &str, bytes: &[u8]) -> Result<(), EngineStoreError> {
    create_private_directory(directory)?;
    let temporary = temporary_path(directory, target_name)?;
    let target = directory.join(target_name);
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        replace_file(&temporary, &target)?;
        #[cfg(unix)]
        {
            fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
            File::open(directory)?.sync_all()?;
        }
        Ok::<(), io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(Into::into)
}

fn temporary_path(directory: &Path, target_name: &str) -> Result<PathBuf, EngineStoreError> {
    let mut nonce = [0_u8; 12];
    getrandom::fill(&mut nonce).map_err(|error| {
        io::Error::other(format!("temporary name entropy unavailable: {error}"))
    })?;
    Ok(directory.join(format!(
        ".{target_name}.{}.tmp",
        URL_SAFE_NO_PAD.encode(nonce)
    )))
}

fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, target: &Path) -> io::Result<()> {
    fs::rename(temporary, target)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let temporary = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both pointers reference live, NUL-terminated UTF-16 buffers for
    // the duration of the call. Windows exposes replace-existing plus
    // write-through semantics only through this system API.
    let result = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn validate_entity_count(name: &str, count: usize) -> Result<(), EngineStoreError> {
    if count > MAX_DURABLE_ENTITIES {
        return Err(invalid(format!(
            "{name} has {count} records; maximum is {MAX_DURABLE_ENTITIES}"
        )));
    }
    Ok(())
}

fn validate_endpoint(name: &str, value: &str) -> Result<(), EngineStoreError> {
    validate_text_bytes(name, value, MAX_ENDPOINT_BYTES)
}

fn validate_identifier_text(name: &str, value: &str) -> Result<(), EngineStoreError> {
    if value.is_empty()
        || value.len() > MAX_REFERENCE_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid(format!("{name} is not a bounded identifier")));
    }
    Ok(())
}

fn validate_text(name: &str, value: &str, max_chars: usize) -> Result<(), EngineStoreError> {
    if value.trim().is_empty()
        || value.chars().count() > max_chars
        || value.chars().any(char::is_control)
    {
        return Err(invalid(format!(
            "{name} must contain 1 to {max_chars} visible characters"
        )));
    }
    Ok(())
}

fn validate_text_bytes(name: &str, value: &str, max_bytes: usize) -> Result<(), EngineStoreError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(invalid(format!(
            "{name} must contain 1 to {max_bytes} visible bytes"
        )));
    }
    Ok(())
}

fn validate_backup_file(value: &str) -> Result<(), EngineStoreError> {
    if value.is_empty()
        || value.len() > MAX_REFERENCE_BYTES
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(invalid("migration backup must be a bounded file name"));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> EngineStoreError {
    EngineStoreError::InvalidState(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::DesktopCredentialStore;
    use crate::model::{
        ContentId, Device, DeviceId, Relationship, Transfer, TransferDirection, TransferId,
    };

    fn durable_state(transferred_bytes: u64) -> EngineState {
        let device_id = DeviceId::parse("device_test").unwrap();
        let relationship_id = RelationshipId::parse("relationship_test").unwrap();
        let transfer_id = TransferId::parse("transfer_test").unwrap();
        let mut state = EngineState::default();
        state.snapshot.devices.insert(
            device_id.clone(),
            Device {
                id: device_id.clone(),
                display_name: "Test device".into(),
            },
        );
        state.snapshot.relationships.insert(
            relationship_id.clone(),
            Relationship {
                id: relationship_id.clone(),
                device_id,
                generation: 2,
                previous_generation: Some(1),
                state: RelationshipState::Trusted,
            },
        );
        state.durable_relationships.insert(
            relationship_id.clone(),
            DurableRelationship {
                vault_reference: Some(VaultReference::parse("vault_test").unwrap()),
                broker: "broker.invalid:8445".into(),
                relay: Some("https://relay.invalid".into()),
            },
        );
        state.snapshot.transfers.insert(
            transfer_id.clone(),
            Transfer {
                id: transfer_id,
                relationship_id,
                room_id: None,
                content_id: ContentId::parse("content_test").unwrap(),
                direction: TransferDirection::Send,
                state: TransferState::Transferring,
                transferred_bytes,
                total_bytes: 100,
                failure: None,
                rejection: None,
            },
        );
        state
    }

    #[test]
    fn active_transfer_and_relationship_survive_restart_without_secret_material() {
        let directory = tempfile::tempdir().unwrap();
        DesktopCredentialStore::new(directory.path().join("vault"))
            .put("vault_test", b"actual-secret")
            .unwrap();
        let mut store = EngineStore::open(directory.path()).unwrap();
        let state = durable_state(42);
        store.replace(state.clone()).unwrap();
        drop(store);

        let bytes = fs::read(directory.path().join(ENGINE_STATE_FILE)).unwrap();
        assert!(
            !bytes
                .windows(b"actual-secret".len())
                .any(|window| window == b"actual-secret")
        );
        let reopened = EngineStore::open(directory.path()).unwrap();
        assert_eq!(reopened.origin(), EngineStoreOrigin::Current);
        assert_eq!(reopened.state(), &state);
    }

    #[test]
    fn a_second_owner_cannot_open_the_same_state_directory() {
        let directory = tempfile::tempdir().unwrap();
        let owner = EngineStore::open(directory.path()).unwrap();
        let error = EngineStore::open(directory.path()).err().unwrap();
        assert!(matches!(error, EngineStoreError::AlreadyOwned { .. }));
        drop(owner);
        EngineStore::open(directory.path()).unwrap();
    }

    #[test]
    fn corrupt_current_state_recovers_the_last_known_good_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = EngineStore::open(directory.path()).unwrap();
        let previous = durable_state(10);
        store.replace(previous.clone()).unwrap();
        store.replace(durable_state(20)).unwrap();
        drop(store);
        fs::write(directory.path().join(ENGINE_STATE_FILE), b"{truncated").unwrap();

        let recovered = EngineStore::open(directory.path()).unwrap();
        assert_eq!(recovered.origin(), EngineStoreOrigin::RecoveredPrevious);
        assert_eq!(recovered.state(), &previous);
    }

    #[test]
    fn invalid_references_are_rejected_before_activation() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = EngineStore::open(directory.path()).unwrap();
        let mut state = durable_state(42);
        state.durable_relationships.clear();

        let error = store.replace(state).unwrap_err();
        assert!(matches!(error, EngineStoreError::InvalidState(_)));
        assert_eq!(store.state(), &EngineState::default());
        assert!(!directory.path().join(ENGINE_STATE_FILE).exists());
    }

    #[test]
    fn unsupported_schema_does_not_silently_fall_back() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = EngineStore::open(directory.path()).unwrap();
        store.replace(durable_state(10)).unwrap();
        store.replace(durable_state(20)).unwrap();
        drop(store);

        let target = directory.path().join(ENGINE_STATE_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
        value["schema_version"] = serde_json::json!(2);
        fs::write(&target, serde_json::to_vec(&value).unwrap()).unwrap();

        let error = EngineStore::open(directory.path()).err().unwrap();
        assert!(matches!(
            error,
            EngineStoreError::UnsupportedSchema { actual: 2, .. }
        ));
    }

    #[test]
    fn oversized_state_is_rejected_before_reading_it() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join(ENGINE_STATE_FILE);
        File::create(&target)
            .unwrap()
            .set_len(MAX_ENGINE_STATE_BYTES + 1)
            .unwrap();

        let error = EngineStore::open(directory.path()).err().unwrap();
        assert!(matches!(
            error,
            EngineStoreError::StateTooLarge {
                actual,
                maximum: MAX_ENGINE_STATE_BYTES
            } if actual == MAX_ENGINE_STATE_BYTES + 1
        ));
    }

    #[test]
    fn vault_references_are_validated_and_redacted() {
        assert!(VaultReference::parse("../credential").is_err());
        let reference = VaultReference::parse("credential_1").unwrap();
        assert_eq!(reference.as_str(), "credential_1");
        assert_eq!(format!("{reference:?}"), "VaultReference(<redacted>)");
    }

    #[test]
    fn empty_fixture_freezes_schema_one() {
        let fixture = include_bytes!("../../../tests/fixtures/v0.3/engine-state-v1.json");
        let state: EngineState = serde_json::from_slice(fixture).unwrap();
        state.validate().unwrap();
        assert_eq!(state, EngineState::default());
        assert_eq!(serde_json::to_value(state).unwrap()["schema_version"], 1);
        assert_eq!(crate::APPLICATION_CONTRACT_VERSION, 6);
    }

    #[cfg(unix)]
    #[test]
    fn state_files_and_lock_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let mut store = EngineStore::open(directory.path()).unwrap();
        store.replace(EngineState::default()).unwrap();
        assert_eq!(
            fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in [ENGINE_LOCK_FILE, ENGINE_STATE_FILE] {
            assert_eq!(
                fs::metadata(directory.path().join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
