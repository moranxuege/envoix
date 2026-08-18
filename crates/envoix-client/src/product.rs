//! Shared product-level state and the local Agent wire contract.
//!
//! Transfer bytes still flow through the canonical Manifest v2 session APIs.
//! This module names the user-facing concepts above that protocol: remembered
//! devices, a durable Inbox, and commands exchanged with a local Agent.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::api::DesktopCredentialStore;

pub const AGENT_PROTOCOL_VERSION: u16 = 3;
pub const AGENT_SETTINGS_VERSION: u16 = 1;
const PRODUCT_STATE_SCHEMA_VERSION: u16 = 1;
const PRODUCT_STATE_FILE: &str = "product-state-v1.json";
const MAX_INBOX_ITEMS: usize = 1_000;

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
    Pair { label: String },
    ListDevices,
    ForgetDevice { device: String },
    ListInbox { limit: usize },
    LatestInbox,
}

/// One response is returned as one JSON line over the local Agent socket.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentResponse {
    Status { status: AgentStatus },
    Pairing { pairing: PairingInvitation },
    Devices { devices: Vec<DeviceSummary> },
    DeviceForgotten { device: DeviceSummary },
    Inbox { items: Vec<InboxItem> },
    Latest { item: Option<InboxItem> },
    Error { code: String, message: String },
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

/// Durable non-secret product metadata plus a separate owner-only credential
/// directory. Raw remembered credentials never enter the JSON state file.
pub struct ProductStore {
    directory: PathBuf,
    credentials: DesktopCredentialStore,
    state: PersistentProductState,
}

impl ProductStore {
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

    pub fn rotate_device(
        &mut self,
        id: &str,
        opaque_credential: &[u8],
        generation: u64,
    ) -> io::Result<()> {
        let index = self
            .state
            .devices
            .iter()
            .position(|device| device.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "remembered device missing"))?;
        let old = self.state.devices[index].clone();
        if generation < old.generation {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "remembered generation moved backwards",
            ));
        }
        if generation == old.generation {
            return Ok(());
        }
        let old_credential = self
            .credentials
            .get(&old.credential_reference)?
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "remembered device credential missing",
                )
            })?;
        let credential_changed = old_credential != opaque_credential;
        if credential_changed {
            self.credentials
                .put(&old.credential_reference, opaque_credential)?;
        }
        self.state.devices[index].previous_generation = Some(old.generation);
        self.state.devices[index].generation = generation;
        if let Err(error) = self.save() {
            self.state.devices[index] = old;
            if credential_changed {
                self.credentials
                    .put(
                        &self.state.devices[index].credential_reference,
                        &old_credential,
                    )
                    .map_err(|rollback_error| {
                        io::Error::other(format!(
                            "{error}; credential rollback also failed: {rollback_error}"
                        ))
                    })?;
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn devices(&self) -> Vec<DeviceSummary> {
        let mut devices = self
            .state
            .devices
            .iter()
            .map(RememberedDeviceRecord::summary)
            .collect::<Vec<_>>();
        devices.sort_by(|left, right| {
            left.label
                .to_ascii_lowercase()
                .cmp(&right.label.to_ascii_lowercase())
        });
        devices
    }

    pub fn device_records(&self) -> Vec<RememberedDeviceRecord> {
        self.state.devices.clone()
    }

    pub fn device_record(&self, id: &str) -> Option<RememberedDeviceRecord> {
        self.state
            .devices
            .iter()
            .find(|device| device.id == id)
            .cloned()
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

    pub fn forget_device(&mut self, selector: &str) -> io::Result<DeviceSummary> {
        let selector = selector.trim();
        if selector.is_empty() || selector.len() > 128 || selector.chars().any(char::is_control) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "device selector must be a device ID or exact label",
            ));
        }
        let index = self
            .state
            .devices
            .iter()
            .position(|device| device.id == selector || device.label.eq_ignore_ascii_case(selector))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "remembered device missing"))?;
        let record = self.state.devices[index].clone();
        let credential = self.credentials.get(&record.credential_reference)?;
        self.credentials.delete(&record.credential_reference)?;
        self.state.devices.remove(index);
        if let Err(error) = self.save() {
            self.state.devices.insert(index, record.clone());
            if let Some(credential) = credential
                && let Err(rollback_error) = self
                    .credentials
                    .put(&record.credential_reference, &credential)
            {
                return Err(io::Error::other(format!(
                    "{error}; credential rollback also failed: {rollback_error}"
                )));
            }
            return Err(error);
        }
        Ok(record.summary())
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

    pub fn inbox(&self, limit: usize) -> Vec<InboxItem> {
        self.state
            .inbox
            .iter()
            .rev()
            .take(limit.min(MAX_INBOX_ITEMS))
            .cloned()
            .collect()
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

fn random_identifier(prefix: &str) -> io::Result<String> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|error| io::Error::other(format!("identifier entropy unavailable: {error}")))?;
    Ok(format!("{prefix}_{}", URL_SAFE_NO_PAD.encode(random)))
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
    tempfile::TempPath::try_from_path(temporary.to_path_buf())?
        .persist(target)
        .map_err(io::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let pending = store
            .prepare_device(" MacBook ", "broker", Some("https://relay"))
            .unwrap();
        let id = pending.id().to_string();
        store.commit_device(pending, b"opaque", 0).unwrap();
        store.rotate_device(&id, b"opaque", 1).unwrap();

        let metadata = fs::read_to_string(directory.path().join(PRODUCT_STATE_FILE)).unwrap();
        assert!(!metadata.contains("opaque"));
        let reopened = ProductStore::open(directory.path()).unwrap();
        let device = reopened.device_record(&id).unwrap();
        assert_eq!(device.label(), "MacBook");
        assert_eq!(device.generation(), 1);
        assert_eq!(device.previous_generation(), Some(0));
        assert_eq!(reopened.device_credential(&id).unwrap(), b"opaque");
    }

    #[test]
    fn forgetting_device_revokes_credential_and_preserves_inbox_history() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ProductStore::open(directory.path()).unwrap();
        let pending = store
            .prepare_device("MacBook", "broker", Some("https://relay"))
            .unwrap();
        let id = pending.id().to_string();
        let credential_path = directory
            .path()
            .join("credentials")
            .join(&pending.credential_reference);
        store.commit_device(pending, b"opaque", 0).unwrap();
        store.append_inbox(inbox_item("received", 1)).unwrap();

        let forgotten = store.forget_device("macbook").unwrap();

        assert_eq!(forgotten.id, id);
        assert_eq!(forgotten.label, "MacBook");
        assert!(store.devices().is_empty());
        assert!(!credential_path.exists());
        assert_eq!(store.latest_inbox().unwrap().id, "received");
        assert!(store.prepare_device("MacBook", "broker", None).is_ok());

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

        let store = ProductStore::open(directory.path()).unwrap();
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
            let error = ProductStore::open(directory.path())
                .err()
                .unwrap_or_else(|| panic!("{name} fixture unexpectedly loaded"));
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{name}");
        }
    }

    #[test]
    fn agent_wire_fixture_round_trips_every_variant() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/v0.2/agent-control-v3.json"
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
            .map(serde_json::from_value::<AgentRequest>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(
            requests.as_slice(),
            [
                AgentRequest::Status,
                AgentRequest::Pair { .. },
                AgentRequest::ListDevices,
                AgentRequest::ForgetDevice { .. },
                AgentRequest::ListInbox { .. },
                AgentRequest::LatestInbox,
            ]
        ));
        for (request, expected) in requests.iter().zip(request_values) {
            assert_eq!(serde_json::to_value(request).unwrap(), *expected);
        }

        let response_values = fixture["responses"].as_array().unwrap();
        let responses = response_values
            .iter()
            .cloned()
            .map(serde_json::from_value::<AgentResponse>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(
            responses.as_slice(),
            [
                AgentResponse::Status { .. },
                AgentResponse::Pairing { .. },
                AgentResponse::Devices { .. },
                AgentResponse::DeviceForgotten { .. },
                AgentResponse::Inbox { .. },
                AgentResponse::Latest { item: Some(_) },
                AgentResponse::Latest { item: None },
                AgentResponse::Error { .. },
            ]
        ));
        for (response, expected) in responses.iter().zip(response_values) {
            assert_eq!(serde_json::to_value(response).unwrap(), *expected);
        }
        let AgentResponse::Pairing { pairing } = &responses[1] else {
            unreachable!("fixture order is checked above");
        };
        assert_eq!(pairing.expires_at_unix_seconds, 1);
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
