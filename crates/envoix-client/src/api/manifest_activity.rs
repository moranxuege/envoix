//! Durable Manifest activity state and persistence.
//!
//! This surface is additive: the established single-file context, record, and
//! driver types remain unchanged. A Manifest activity owns one aggregate
//! lifecycle plus its complete entry inventory and receiver-authoritative
//! per-entry results.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use envoix_protocol::{
    ManifestEntryKind, ManifestEntryResultStatus, ManifestEntryResultV1, ManifestV1,
    TransferProtocol, validate_manifest_relative_path,
};
use envoix_session::{ManifestSendRequest, ManifestTransferSummary, TransferDirection};
use serde::{Deserialize, Serialize};

use super::driver::ClientContext;
use super::error::{Phase, TransferError};
use super::machine::{AttemptEvent, Effect, Input, Session, State};
use super::record::unix_now_ms;
use super::{PeerSource, TransferOptions};

/// Current additive Manifest record schema.
pub const MANIFEST_RECORD_VERSION: u32 = 1;
const MANIFEST_CANCELLED_CODE: &str = "manifest.cancelled";

/// The shape-specific launch operation. The enum prevents a send without a
/// request or a receive without an output root.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "direction", rename_all = "snake_case")]
pub enum ManifestOperation {
    /// Send a validated Manifest and its durable source mapping.
    Send { request: ManifestSendRequest },
    /// Receive a negotiated Manifest into this root.
    Receive { output_dir: PathBuf },
}

impl ManifestOperation {
    /// Local transfer direction.
    pub const fn direction(&self) -> TransferDirection {
        match self {
            Self::Send { .. } => TransferDirection::Send,
            Self::Receive { .. } => TransferDirection::Receive,
        }
    }

    /// The sender's request, when this is a send activity.
    pub fn send_request(&self) -> Option<&ManifestSendRequest> {
        match self {
            Self::Send { request } => Some(request),
            Self::Receive { .. } => None,
        }
    }

    /// The receiver's output root, when this is a receive activity.
    pub fn output_dir(&self) -> Option<&Path> {
        match self {
            Self::Receive { output_dir } => Some(output_dir),
            Self::Send { .. } => None,
        }
    }
}

/// Everything needed to relaunch one Manifest activity.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestSessionParams {
    pub operation: ManifestOperation,
    pub sources: Vec<PeerSource>,
    pub options: TransferOptions,
    /// Receive lands in Activity-owned staging and must be published by the
    /// native platform before the aggregate lifecycle can complete.
    #[serde(default)]
    pub publication_required: bool,
}

impl ManifestSessionParams {
    pub const fn direction(&self) -> TransferDirection {
        self.operation.direction()
    }
}

/// Durable client policy plus shape-specific Manifest launch parameters.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestSessionContext {
    #[serde(default)]
    pub client: ClientContext,
    pub params: ManifestSessionParams,
}

impl ManifestSessionContext {
    /// Revalidates every persisted input before a start or restore can contact
    /// a peer.
    pub fn validate(&self) -> Result<(), TransferError> {
        self.client.client()?;
        super::validate_path_policy(&self.params.options)?;
        if self.params.sources.is_empty() {
            return Err(TransferError::input(
                "a Manifest activity needs at least one peer source",
            ));
        }
        if self.params.direction() == TransferDirection::Send && self.params.publication_required {
            return Err(TransferError::input(
                "Manifest publication is only valid for receives",
            ));
        }
        match &self.params.operation {
            ManifestOperation::Send { request } => request
                .validate()
                .map_err(|error| TransferError::from_core(error, Phase::Setup)),
            ManifestOperation::Receive { output_dir } if output_dir.as_os_str().is_empty() => Err(
                TransferError::input("a Manifest receive needs a non-empty output directory"),
            ),
            ManifestOperation::Receive { .. } => Ok(()),
        }
    }

    /// Static listener coordinates handed to a peer must keep the same
    /// identity across durable resumes.
    pub fn requires_stable_listener_identity(&self) -> bool {
        self.params.direction() == TransferDirection::Receive
            && self.params.sources.iter().any(|source| {
                matches!(
                    source,
                    PeerSource::ShowManual { .. }
                        | PeerSource::ShowInvite { .. }
                        | PeerSource::Mdns { .. }
                )
            })
    }
}

/// Per-entry phase rendered beside aggregate progress.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestEntryPhase {
    Preparing,
    Transferring,
}

/// The active Manifest entry, if one is being prepared or transferred.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestCurrentEntry {
    pub entry_id: u32,
    pub phase: ManifestEntryPhase,
    pub transfer_id: Option<String>,
    pub relative_path: String,
    pub bytes: u64,
    pub total: u64,
    pub bytes_resumed: u64,
}

/// Serializable aggregate lifecycle and Manifest-specific durable facts.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ManifestActivity {
    /// Explicit protocol discriminator for unified record/FFI projections.
    pub protocol: TransferProtocol,
    /// Aggregate lifecycle reused from the established pure state machine.
    pub session: Session,
    /// Complete validated inventory. Senders seed it at creation; receivers
    /// learn it from `ManifestPlanned` before payload starts.
    pub manifest: Option<ManifestV1>,
    pub current_entry: Option<ManifestCurrentEntry>,
    /// Receiver-authoritative terminal results, in Manifest entry order.
    pub entry_results: Vec<ManifestEntryResultV1>,
    /// Successful regular-file results; directories do not increment it.
    pub completed_files: u32,
}

impl ManifestActivity {
    pub fn new(context: &ManifestSessionContext) -> Result<Self, TransferError> {
        context.validate()?;
        let direction = context.params.direction();
        let manifest = context
            .params
            .operation
            .send_request()
            .map(|request| request.manifest.clone());
        let mut session = Session::new(direction);
        session.publication_required = context.params.publication_required;
        session.file_name = manifest
            .as_ref()
            .map(|manifest| manifest.manifest_id.to_string());
        Ok(Self {
            protocol: TransferProtocol::ManifestV1,
            session,
            manifest,
            current_entry: None,
            entry_results: Vec::new(),
            completed_files: 0,
        })
    }

    /// Checked Manifest counts for a durable snapshot. Before a receiver has
    /// accepted a plan all values are zero.
    pub fn root_count(&self) -> u32 {
        self.manifest.as_ref().map_or(0, |value| value.root_count)
    }

    pub fn file_count(&self) -> u32 {
        self.manifest.as_ref().map_or(0, |value| value.file_count)
    }

    pub fn directory_count(&self) -> u32 {
        self.manifest
            .as_ref()
            .map_or(0, |value| value.directory_count)
    }

    pub fn total_bytes(&self) -> u64 {
        self.manifest.as_ref().map_or(0, |value| value.total_bytes)
    }

    fn validate(&self) -> Result<(), TransferError> {
        if self.protocol != TransferProtocol::ManifestV1 {
            return Err(TransferError::input(
                "Manifest activity has a non-Manifest protocol",
            ));
        }
        let Some(manifest) = &self.manifest else {
            if self.current_entry.is_some()
                || !self.entry_results.is_empty()
                || self.completed_files != 0
            {
                return Err(TransferError::input(
                    "Manifest activity has entry facts before accepting a plan",
                ));
            }
            return Ok(());
        };
        manifest
            .validate_structure()
            .map_err(|error| TransferError::input(error.to_string()))?;
        let manifest_id = manifest.manifest_id.to_string();
        if self
            .session
            .transfer_id
            .as_deref()
            .is_some_and(|value| value != manifest_id)
            || self
                .session
                .file_name
                .as_deref()
                .is_some_and(|value| value != manifest_id)
            || (self.session.total != 0 && self.session.total != manifest.total_bytes)
            || self.session.bytes > manifest.total_bytes
        {
            return Err(TransferError::input(
                "Manifest aggregate lifecycle contradicts its accepted plan",
            ));
        }
        let mut result_ids = HashSet::new();
        for result in &self.entry_results {
            self.validate_result(result)?;
            if !result_ids.insert(result.entry_id) {
                return Err(TransferError::input(
                    "Manifest activity contains duplicate entry results",
                ));
            }
        }
        if let Some(current) = &self.current_entry {
            let entry = manifest
                .entries
                .iter()
                .find(|entry| entry.entry_id == current.entry_id)
                .ok_or_else(|| TransferError::input("current Manifest entry is unknown"))?;
            if result_ids.contains(&current.entry_id)
                || current.relative_path != entry.relative_path
                || current.total != entry.size
                || current.bytes > current.total
                || current.bytes_resumed > current.bytes
            {
                return Err(TransferError::input(
                    "current Manifest entry contradicts the accepted plan",
                ));
            }
        }
        if self.completed_files != self.successful_file_count() {
            return Err(TransferError::input(
                "Manifest completed-file count contradicts entry results",
            ));
        }
        if matches!(
            self.session.state,
            State::Completed | State::AwaitingPublication
        ) && (self.session.transfer_id.as_deref() != Some(&manifest_id)
            || self.session.total != manifest.total_bytes
            || self.session.bytes != manifest.total_bytes
            || self.entry_results.len() != manifest.entries.len()
            || self.entry_results.iter().any(|result| {
                matches!(
                    result.status,
                    ManifestEntryResultStatus::Failed | ManifestEntryResultStatus::Cancelled
                )
            }))
        {
            return Err(TransferError::input(
                "partial Manifest activity cannot be completed",
            ));
        }
        Ok(())
    }

    pub(crate) fn accept_plan(
        &mut self,
        direction: TransferDirection,
        manifest: ManifestV1,
    ) -> Result<(), TransferError> {
        if direction != self.session.direction {
            return Err(TransferError::input(
                "Manifest plan direction does not match the activity",
            ));
        }
        manifest
            .validate_structure()
            .map_err(|error| TransferError::input(error.to_string()))?;
        if let Some(existing) = &self.manifest
            && existing != &manifest
        {
            return Err(TransferError::input(
                "Manifest plan changed for an existing activity",
            ));
        }
        self.manifest = Some(manifest);
        Ok(())
    }

    pub(crate) fn preparing_entry(
        &mut self,
        entry_id: u32,
        relative_path: String,
        total: u64,
    ) -> Result<(), TransferError> {
        self.validate_entry_event(entry_id, &relative_path, total)?;
        self.current_entry = Some(ManifestCurrentEntry {
            entry_id,
            phase: ManifestEntryPhase::Preparing,
            transfer_id: None,
            relative_path,
            bytes: 0,
            total,
            bytes_resumed: 0,
        });
        Ok(())
    }

    pub(crate) fn started(&mut self) -> Result<Vec<Effect>, TransferError> {
        let manifest = self
            .manifest
            .as_ref()
            .ok_or_else(|| TransferError::input("Manifest started before its plan"))?;
        Ok(self.session.reduce(Input::Event {
            attempt: self.session.attempt,
            event: AttemptEvent::Started {
                transfer_id: manifest.manifest_id.to_string(),
                file_name: manifest.manifest_id.to_string(),
                total: manifest.total_bytes,
                bytes_resumed: 0,
            },
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn entry_started(
        &mut self,
        entry_id: u32,
        transfer_id: String,
        relative_path: String,
        total: u64,
        bytes_resumed: u64,
    ) -> Result<(), TransferError> {
        self.validate_entry_event(entry_id, &relative_path, total)?;
        if bytes_resumed > total {
            return Err(TransferError::input(
                "Manifest entry resumed beyond its expected size",
            ));
        }
        self.current_entry = Some(ManifestCurrentEntry {
            entry_id,
            phase: ManifestEntryPhase::Transferring,
            transfer_id: Some(transfer_id),
            relative_path,
            bytes: bytes_resumed,
            total,
            bytes_resumed,
        });
        Ok(())
    }

    pub(crate) fn progress(&mut self, entry_id: u32, entry_bytes: u64, aggregate: u64) {
        if let Some(current) = &mut self.current_entry
            && current.entry_id == entry_id
        {
            current.bytes = entry_bytes;
        }
        let _ = self.session.reduce(Input::Event {
            attempt: self.session.attempt,
            event: AttemptEvent::Progress { bytes: aggregate },
        });
    }

    pub(crate) fn entry_completed(
        &mut self,
        result: ManifestEntryResultV1,
    ) -> Result<(), TransferError> {
        self.validate_result(&result)?;
        if let Some(existing) = self
            .entry_results
            .iter()
            .find(|existing| existing.entry_id == result.entry_id)
        {
            return if existing == &result {
                Ok(())
            } else {
                Err(TransferError::input(
                    "Manifest entry produced contradictory terminal results",
                ))
            };
        }
        if self
            .current_entry
            .as_ref()
            .is_some_and(|current| current.entry_id == result.entry_id)
        {
            self.current_entry = None;
        }
        self.entry_results.push(result);
        self.sort_results();
        self.recount_completed_files();
        Ok(())
    }

    pub(crate) fn completed(
        &mut self,
        summary: ManifestTransferSummary,
        completed_root: Option<String>,
    ) -> Result<Vec<Effect>, TransferError> {
        let manifest = self
            .manifest
            .as_ref()
            .ok_or_else(|| TransferError::input("Manifest completed before its plan"))?;
        if summary.manifest_id != manifest.manifest_id
            || summary.file_count != manifest.file_count
            || summary.directory_count != manifest.directory_count
            || summary.total_bytes != manifest.total_bytes
            || summary.entries.len() != manifest.entries.len()
        {
            return Err(TransferError::input(
                "Manifest completion summary does not match its accepted plan",
            ));
        }
        let mut seen = HashSet::new();
        for result in &summary.entries {
            self.validate_result(result)?;
            if !seen.insert(result.entry_id)
                || matches!(
                    result.status,
                    ManifestEntryResultStatus::Failed | ManifestEntryResultStatus::Cancelled
                )
            {
                return Err(TransferError::input(
                    "Manifest completion contains a duplicate or unsuccessful entry",
                ));
            }
        }
        let manifest_id = manifest.manifest_id.to_string();
        let total_bytes = manifest.total_bytes;
        self.entry_results = summary.entries;
        self.sort_results();
        self.recount_completed_files();
        self.current_entry = None;
        Ok(self.session.reduce(Input::Event {
            attempt: self.session.attempt,
            event: AttemptEvent::Completed {
                transfer_id: manifest_id.clone(),
                file_name: manifest_id,
                bytes: total_bytes,
                completed_file_path: completed_root,
            },
        }))
    }

    pub(crate) fn cancel_unfinished(&mut self) -> Vec<Effect> {
        let effects = self.session.reduce(Input::Cancel);
        if self.session.state != State::Cancelled {
            return effects;
        }
        self.mark_unfinished_cancelled();
        effects
    }

    pub(crate) fn prepare_resume(&mut self, fresh: bool) {
        self.current_entry = None;
        if fresh {
            self.entry_results.clear();
        } else {
            self.entry_results.retain(|result| {
                !matches!(
                    result.status,
                    ManifestEntryResultStatus::Failed | ManifestEntryResultStatus::Cancelled
                )
            });
        }
        self.recount_completed_files();
    }

    pub(crate) fn fail_current(&mut self, failure_code: String) -> Result<(), TransferError> {
        let Some(current) = self.current_entry.take() else {
            return Ok(());
        };
        self.entry_completed(ManifestEntryResultV1 {
            entry_id: current.entry_id,
            status: ManifestEntryResultStatus::Failed,
            offered_relative_path: current.relative_path,
            final_relative_path: None,
            failure_code: Some(failure_code),
        })
    }

    pub(crate) fn mark_unfinished_cancelled(&mut self) {
        if let Some(manifest) = &self.manifest {
            for entry in &manifest.entries {
                if self
                    .entry_results
                    .iter()
                    .any(|result| result.entry_id == entry.entry_id)
                {
                    continue;
                }
                self.entry_results.push(ManifestEntryResultV1 {
                    entry_id: entry.entry_id,
                    status: ManifestEntryResultStatus::Cancelled,
                    offered_relative_path: entry.relative_path.clone(),
                    final_relative_path: None,
                    failure_code: Some(MANIFEST_CANCELLED_CODE.into()),
                });
            }
            self.sort_results();
        }
        self.current_entry = None;
    }

    fn validate_entry_event(
        &self,
        entry_id: u32,
        relative_path: &str,
        total: u64,
    ) -> Result<(), TransferError> {
        let entry = self
            .manifest
            .as_ref()
            .and_then(|manifest| {
                manifest
                    .entries
                    .iter()
                    .find(|entry| entry.entry_id == entry_id)
            })
            .ok_or_else(|| TransferError::input("Manifest event names an unknown entry"))?;
        if entry.relative_path != relative_path || entry.size != total {
            return Err(TransferError::input(
                "Manifest entry event contradicts the accepted plan",
            ));
        }
        Ok(())
    }

    fn validate_result(&self, result: &ManifestEntryResultV1) -> Result<(), TransferError> {
        let entry = self
            .manifest
            .as_ref()
            .and_then(|manifest| {
                manifest
                    .entries
                    .iter()
                    .find(|entry| entry.entry_id == result.entry_id)
            })
            .ok_or_else(|| TransferError::input("Manifest result names an unknown entry"))?;
        if result.offered_relative_path != entry.relative_path {
            return Err(TransferError::input(
                "Manifest result changed the offered entry path",
            ));
        }
        if let Some(path) = &result.final_relative_path {
            validate_manifest_relative_path(path)
                .map_err(|error| TransferError::input(error.to_string()))?;
        }
        match result.status {
            ManifestEntryResultStatus::Completed
            | ManifestEntryResultStatus::SkippedIdentical
            | ManifestEntryResultStatus::Renamed
                if result.final_relative_path.is_none() =>
            {
                return Err(TransferError::input(
                    "successful Manifest result has no final path",
                ));
            }
            ManifestEntryResultStatus::Failed
                if result
                    .failure_code
                    .as_deref()
                    .is_none_or(|code| code.trim().is_empty()) =>
            {
                return Err(TransferError::input(
                    "failed Manifest result has no failure code",
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn sort_results(&mut self) {
        let order = self.manifest.as_ref().map(|manifest| {
            manifest
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.entry_id, index))
                .collect::<BTreeMap<_, _>>()
        });
        self.entry_results.sort_by_key(|result| {
            order
                .as_ref()
                .and_then(|value| value.get(&result.entry_id))
                .copied()
                .unwrap_or(usize::MAX)
        });
    }

    fn recount_completed_files(&mut self) {
        self.completed_files = self.successful_file_count();
    }

    fn successful_file_count(&self) -> u32 {
        self.entry_results
            .iter()
            .filter(|result| {
                !matches!(
                    result.status,
                    ManifestEntryResultStatus::Failed | ManifestEntryResultStatus::Cancelled
                ) && self.manifest.as_ref().is_some_and(|manifest| {
                    manifest.entries.iter().any(|entry| {
                        entry.entry_id == result.entry_id
                            && entry.kind == ManifestEntryKind::RegularFile
                    })
                })
            })
            .count() as u32
    }
}

/// One persisted Manifest activity.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestTransferRecord {
    #[serde(default)]
    pub version: u32,
    pub id: u64,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub context: ManifestSessionContext,
    pub activity: ManifestActivity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_extras: Option<serde_json::Value>,
}

impl ManifestTransferRecord {
    pub fn validate(&self) -> Result<(), TransferError> {
        self.context.validate()?;
        if self.version > MANIFEST_RECORD_VERSION {
            return Err(TransferError::input(
                "Manifest record uses an unsupported future schema",
            ));
        }
        if self.activity.protocol != TransferProtocol::ManifestV1
            || self.activity.session.direction != self.context.params.direction()
        {
            return Err(TransferError::input(
                "Manifest record lifecycle does not match its context",
            ));
        }
        if let Some(expected) = self
            .context
            .params
            .operation
            .send_request()
            .map(|request| &request.manifest)
        {
            match self.activity.manifest.as_ref() {
                Some(actual) if actual == expected => {}
                _ => {
                    return Err(TransferError::input(
                        "Manifest record plan changed from its send request",
                    ));
                }
            }
        }
        self.activity.validate()?;
        Ok(())
    }
}

/// Atomic JSON store using a separate filename namespace from compatible
/// single-file records.
#[derive(Clone, Debug)]
pub struct ManifestRecordStore {
    dir: PathBuf,
}

impl ManifestRecordStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, id: u64) -> PathBuf {
        self.dir.join(format!("manifest-record-{id}.json"))
    }

    pub fn identity_path(&self, id: u64) -> PathBuf {
        self.dir
            .join("identities")
            .join(format!("manifest-identity-{id}.json"))
    }

    pub async fn save(&self, record: &ManifestTransferRecord) -> std::io::Result<()> {
        record
            .validate()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        tokio::fs::create_dir_all(&self.dir).await?;
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let tmp = self
            .dir
            .join(format!(".manifest-record-{}.json.tmp", record.id));
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(tmp, self.path(record.id)).await
    }

    pub async fn load(&self, id: u64) -> Option<ManifestTransferRecord> {
        let bytes = tokio::fs::read(self.path(id)).await.ok()?;
        match serde_json::from_slice::<ManifestTransferRecord>(&bytes) {
            Ok(record) => match record.validate() {
                Ok(()) if record.id == id => Some(record),
                Ok(()) => {
                    tracing::warn!(
                        expected_id = id,
                        actual_id = record.id,
                        "Manifest record id does not match its filename"
                    );
                    None
                }
                Err(error) => {
                    tracing::warn!(%error, id, "invalid Manifest record");
                    None
                }
            },
            Err(error) => {
                tracing::warn!(%error, id, "unparseable Manifest record");
                None
            }
        }
    }

    pub async fn load_all(&self) -> Vec<ManifestTransferRecord> {
        let mut records = BTreeMap::new();
        let Ok(mut entries) = tokio::fs::read_dir(&self.dir).await else {
            return Vec::new();
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(id) = name
                .strip_prefix("manifest-record-")
                .and_then(|value| value.strip_suffix(".json"))
                .and_then(|value| value.parse::<u64>().ok())
            else {
                continue;
            };
            if let Some(record) = self.load(id).await {
                records.insert(record.id, record);
            }
        }
        records.into_values().collect()
    }

    pub async fn delete(&self, id: u64) {
        let _ = tokio::fs::remove_file(self.path(id)).await;
        let _ = tokio::fs::remove_file(self.identity_path(id)).await;
    }
}

pub(crate) fn new_manifest_record(
    id: u64,
    context: ManifestSessionContext,
    activity: ManifestActivity,
    platform_extras: Option<serde_json::Value>,
) -> ManifestTransferRecord {
    let now = unix_now_ms();
    ManifestTransferRecord {
        version: MANIFEST_RECORD_VERSION,
        id,
        created_ms: now,
        updated_ms: now,
        context,
        activity,
        platform_extras,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use envoix_protocol::{ManifestEntryV1, ManifestHashAlgorithm, ManifestId};
    use tempfile::tempdir;

    fn manifest() -> ManifestV1 {
        ManifestV1 {
            manifest_id: ManifestId::new("durable-manifest"),
            entries: vec![
                ManifestEntryV1 {
                    entry_id: 0,
                    relative_path: "Album".into(),
                    kind: ManifestEntryKind::Directory,
                    size: 0,
                    hash: None,
                    modified_at_unix_ms: None,
                },
                ManifestEntryV1 {
                    entry_id: 1,
                    relative_path: "Album/a.jpg".into(),
                    kind: ManifestEntryKind::RegularFile,
                    size: 10,
                    hash: Some([1; 32]),
                    modified_at_unix_ms: None,
                },
                ManifestEntryV1 {
                    entry_id: 2,
                    relative_path: "b.txt".into(),
                    kind: ManifestEntryKind::RegularFile,
                    size: 5,
                    hash: Some([2; 32]),
                    modified_at_unix_ms: None,
                },
            ],
            file_count: 2,
            directory_count: 1,
            root_count: 2,
            total_bytes: 15,
            hash_algorithm: ManifestHashAlgorithm::Blake3_256,
        }
    }

    fn receive_context(publication_required: bool) -> ManifestSessionContext {
        ManifestSessionContext {
            client: ClientContext::default(),
            params: ManifestSessionParams {
                operation: ManifestOperation::Receive {
                    output_dir: "/tmp/envoix-manifest-staging".into(),
                },
                sources: vec![PeerSource::ShowManual {
                    token: Some("token".into()),
                }],
                options: TransferOptions::default(),
                publication_required,
            },
        }
    }

    fn result(entry_id: u32, path: &str) -> ManifestEntryResultV1 {
        ManifestEntryResultV1 {
            entry_id,
            status: ManifestEntryResultStatus::Completed,
            offered_relative_path: path.into(),
            final_relative_path: Some(path.into()),
            failure_code: None,
        }
    }

    #[test]
    fn publication_gates_aggregate_completion() {
        let context = receive_context(true);
        let mut activity = ManifestActivity::new(&context).unwrap();
        let plan = manifest();
        activity
            .accept_plan(TransferDirection::Receive, plan.clone())
            .unwrap();
        activity.started().unwrap();
        activity
            .entry_started(1, "entry-1".into(), "Album/a.jpg".into(), 10, 2)
            .unwrap();
        activity.progress(1, 8, 8);
        activity.entry_completed(result(0, "Album")).unwrap();
        activity.entry_completed(result(1, "Album/a.jpg")).unwrap();
        activity.entry_completed(result(2, "b.txt")).unwrap();
        let summary = ManifestTransferSummary {
            manifest_id: plan.manifest_id,
            file_count: 2,
            directory_count: 1,
            total_bytes: 15,
            entries: activity.entry_results.clone(),
        };
        activity
            .completed(summary, Some("/tmp/envoix-manifest-staging".into()))
            .unwrap();

        assert_eq!(activity.session.state, State::AwaitingPublication);
        assert_eq!(activity.completed_files, 2);
        assert_eq!(activity.root_count(), 2);
        activity.session.reduce(Input::Published {
            path: "file:///Downloads/Envoix".into(),
        });
        assert_eq!(activity.session.state, State::Completed);
    }

    #[test]
    fn cancel_preserves_committed_results_and_marks_only_unfinished_entries() {
        let context = receive_context(false);
        let mut activity = ManifestActivity::new(&context).unwrap();
        activity
            .accept_plan(TransferDirection::Receive, manifest())
            .unwrap();
        activity.started().unwrap();
        activity.entry_completed(result(0, "Album")).unwrap();
        activity.entry_completed(result(1, "Album/a.jpg")).unwrap();
        activity
            .entry_started(2, "entry-2".into(), "b.txt".into(), 5, 0)
            .unwrap();

        activity.cancel_unfinished();

        assert_eq!(activity.session.state, State::Cancelled);
        assert_eq!(activity.entry_results.len(), 3);
        assert_eq!(activity.completed_files, 1);
        assert_eq!(
            activity.entry_results[1].status,
            ManifestEntryResultStatus::Completed
        );
        assert_eq!(
            activity.entry_results[2].status,
            ManifestEntryResultStatus::Cancelled
        );
    }

    #[test]
    fn partial_result_cannot_report_aggregate_completion() {
        let context = receive_context(false);
        let mut activity = ManifestActivity::new(&context).unwrap();
        let plan = manifest();
        activity
            .accept_plan(TransferDirection::Receive, plan.clone())
            .unwrap();
        activity.started().unwrap();
        let failed = ManifestEntryResultV1 {
            entry_id: 1,
            status: ManifestEntryResultStatus::Failed,
            offered_relative_path: "Album/a.jpg".into(),
            final_relative_path: None,
            failure_code: Some("manifest.receive_failed".into()),
        };
        let summary = ManifestTransferSummary {
            manifest_id: plan.manifest_id,
            file_count: 2,
            directory_count: 1,
            total_bytes: 15,
            entries: vec![result(0, "Album"), failed, result(2, "b.txt")],
        };

        assert!(activity.completed(summary, None).is_err());
        assert_eq!(activity.session.state, State::Transferring);
    }

    #[tokio::test]
    async fn record_store_round_trips_manifest_facts() {
        let dir = tempdir().unwrap();
        let store = ManifestRecordStore::new(dir.path());
        let context = receive_context(true);
        let mut activity = ManifestActivity::new(&context).unwrap();
        activity
            .accept_plan(TransferDirection::Receive, manifest())
            .unwrap();
        activity
            .preparing_entry(1, "Album/a.jpg".into(), 10)
            .unwrap();
        let record = new_manifest_record(7, context, activity, Some(serde_json::json!({"ui": 1})));

        let mut invalid = record.clone();
        invalid.activity.completed_files = 9;
        assert!(store.save(&invalid).await.is_err());

        store.save(&record).await.unwrap();
        let loaded = store.load(7).await.unwrap();
        assert_eq!(loaded.activity.protocol, TransferProtocol::ManifestV1);
        assert_eq!(loaded.activity.root_count(), 2);
        assert_eq!(loaded.activity.current_entry.unwrap().entry_id, 1);
        assert_eq!(loaded.platform_extras, Some(serde_json::json!({"ui": 1})));
        assert_eq!(store.load_all().await.len(), 1);

        store.delete(7).await;
        assert!(store.load(7).await.is_none());
    }

    #[test]
    fn deserialized_send_request_is_revalidated() {
        let request = ManifestSendRequest::new(
            manifest(),
            [
                (1, PathBuf::from("/private/a.jpg")),
                (2, PathBuf::from("/private/b.txt")),
            ],
        )
        .unwrap();
        let context = ManifestSessionContext {
            client: ClientContext::default(),
            params: ManifestSessionParams {
                operation: ManifestOperation::Send { request },
                sources: vec![PeerSource::Mdns {
                    token: Some("token".into()),
                }],
                options: TransferOptions::default(),
                publication_required: false,
            },
        };
        let mut value = serde_json::to_value(context).unwrap();
        value["params"]["operation"]["request"]["source_paths"] = serde_json::json!({});
        let context: ManifestSessionContext = serde_json::from_value(value).unwrap();
        assert!(context.validate().is_err());
    }
}
