//! Canonical local transfer preparation and immutable Manifest v2 Seal.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use envoix_protocol::manifest_v2::{
    CompressionPolicyV2, ContentDigestV2, EntryContentDigestV2, JobIdV2,
    MAX_MANIFEST_V2_COMPONENT_BYTES, MAX_MANIFEST_V2_ENTRIES, MAX_MANIFEST_V2_PATH_BYTES,
    MAX_MANIFEST_V2_PATH_DEPTH, MAX_MANIFEST_V2_ROOTS, ManifestEntryKindV2, ManifestEntryV2,
    ManifestRootV2, ManifestTotalsV2, ManifestV2, SourceCompletenessV2, build_manifest_offer_v2,
    encode_manifest_offer_v2,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use unicode_normalization::UnicodeNormalization;

const TRANSFER_JOB_SCHEMA_VERSION: u16 = 1;
const FIRST_SELECTION_REVISION: u64 = 1;
const FIRST_GENERATION: u32 = 1;
const FIRST_SOURCE_ITEM_ID: u64 = 1;
const DERIVED_ITEM_ID_MASK: u64 = 1 << 63;
const HASH_READ_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_COMPRESSION_SAMPLE_BYTES: usize = 256 * 1024;
pub const DEFAULT_INVENTORY_PAGE_SIZE: usize = 128;
pub const MAX_INVENTORY_PAGE_SIZE: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SourceItemId(pub u64);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalSourceOrigin {
    Filesystem,
    PhotosStaging,
    ShareStaging,
    ContentUriStaging,
    FileProviderStaging,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobLifecycle {
    Preparing,
    NeedsSourceDecision,
    ReadyToSend,
    Sealed,
    Canceled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSelectionState {
    Pending,
    Enumerating,
    NeedsDecision,
    Ready,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceIssueKind {
    PermissionDenied,
    Unavailable,
    InvalidName,
    SymbolicLink,
    SpecialFile,
    SourceChanged,
    DepthLimit,
    EntryLimit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceIssue {
    pub issue_id: u64,
    pub root_item_id: SourceItemId,
    pub relative_components: Vec<String>,
    pub kind: SourceIssueKind,
}

/// A platform SourceProvider issue discovered while stabilizing an opaque
/// tree. The core assigns durable issue/root identities when it attaches this
/// fact to a canonical job.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderSourceIssue {
    pub relative_components: Vec<String>,
    pub kind: SourceIssueKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSelectionInfo {
    pub root_item_id: SourceItemId,
    pub requested_name: String,
    pub state: SourceSelectionState,
    pub partial_approved: bool,
    pub issues: Vec<SourceIssue>,
}

#[derive(Clone, Eq, PartialEq)]
pub enum SourceDecision {
    Reauthorize { local_path: PathBuf },
    ApprovePartial,
    RemoveSelection,
    CancelJob,
}

impl fmt::Debug for SourceDecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reauthorize { .. } => formatter
                .debug_struct("Reauthorize")
                .field("local_path", &"<redacted>")
                .finish(),
            Self::ApprovePartial => formatter.write_str("ApprovePartial"),
            Self::RemoveSelection => formatter.write_str("RemoveSelection"),
            Self::CancelJob => formatter.write_str("CancelJob"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AddSourceResult {
    pub root_item_id: SourceItemId,
    pub folded_into_existing_selection: bool,
    pub removed_covered_roots: Vec<SourceItemId>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InventoryCursor {
    pub revision: u64,
    pub offset: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryItem {
    pub item_id: SourceItemId,
    pub root_item_id: SourceItemId,
    pub parent_item_id: Option<SourceItemId>,
    pub name: String,
    pub kind: ManifestEntryKindV2,
    pub plaintext_size: u64,
    pub digest_known: bool,
    pub has_warning: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryPage {
    pub revision: u64,
    pub items: Vec<InventoryItem>,
    pub next_cursor: Option<InventoryCursor>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InventorySummary {
    pub root_count: u32,
    pub file_count: u32,
    pub directory_count: u32,
    pub total_plaintext_bytes: u64,
    pub warning_count: u32,
}

#[derive(Clone)]
pub struct PreparedFileSource {
    path: PathBuf,
    fingerprint: SourceFingerprint,
}

impl fmt::Debug for PreparedFileSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedFileSource")
            .field("path", &"<redacted>")
            .finish()
    }
}

impl PreparedFileSource {
    pub async fn open(&self) -> Result<fs::File, TransferJobError> {
        verify_source_fingerprint(&self.path, &self.fingerprint).await?;
        Ok(fs::File::open(&self.path).await?)
    }

    pub async fn hash(&self) -> Result<ContentDigestV2, TransferJobError> {
        let mut file = self.open().await?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0_u8; HASH_READ_BUFFER_BYTES];
        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        verify_source_fingerprint(&self.path, &self.fingerprint).await?;
        Ok(ContentDigestV2(*hasher.finalize().as_bytes()))
    }

    pub async fn verify_unchanged(&self) -> Result<(), TransferJobError> {
        verify_source_fingerprint(&self.path, &self.fingerprint).await
    }
}

#[derive(Debug, Error)]
pub enum TransferJobError {
    #[error("entropy source unavailable")]
    Entropy,
    #[error("job is sealed; cancel it and create a replacement job")]
    SealedMutation,
    #[error("job is canceled")]
    Canceled,
    #[error("source selection {0:?} was not found")]
    UnknownSelection(SourceItemId),
    #[error("inventory item {0:?} was not found")]
    UnknownItem(SourceItemId),
    #[error("source selection is not awaiting a user decision")]
    DecisionNotRequired,
    #[error("an entirely inaccessible root cannot be approved as partial")]
    EmptyPartialRoot,
    #[error("this source issue cannot be resolved by sending a partial root")]
    PartialNotAllowed,
    #[error("reauthorized source overlaps another selected root")]
    OverlappingSelection,
    #[error("job still has unresolved source decisions")]
    UnresolvedSourceDecision,
    #[error("job preparation is incomplete")]
    PreparationIncomplete,
    #[error("inventory cursor belongs to revision {actual}, expected {expected}")]
    StaleInventoryCursor { expected: u64, actual: u64 },
    #[error("inventory page limit must be between 1 and {MAX_INVENTORY_PAGE_SIZE}")]
    InvalidPageLimit,
    #[error("source component is invalid")]
    InvalidComponent,
    #[error("source root count exceeds {MAX_MANIFEST_V2_ROOTS}")]
    RootLimit,
    #[error("source entry count exceeds {MAX_MANIFEST_V2_ENTRIES}")]
    EntryLimit,
    #[error("source changed after preparation")]
    SourceChanged,
    #[error("source is not a regular file")]
    NotRegularFile,
    #[error("compression sample limit exceeds {MAX_COMPRESSION_SAMPLE_BYTES} bytes")]
    SampleLimit,
    #[error("durable job schema {0} is unsupported")]
    UnsupportedSchema(u16),
    #[error("durable job record is inconsistent: {0}")]
    InvalidRecord(String),
    #[error("protocol contract rejected the sealed job: {0}")]
    Protocol(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceFingerprint {
    plaintext_size: u64,
    modified_unix_nanos: Option<u64>,
    canonical_path_digest: [u8; 32],
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalSourceBinding {
    path: PathBuf,
    origin: LocalSourceOrigin,
    job_owned_staging: bool,
    fingerprint: SourceFingerprint,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceSelection {
    root_item_id: SourceItemId,
    selection_order: u32,
    requested_name: String,
    path: PathBuf,
    canonical_path: Option<PathBuf>,
    is_directory_hint: Option<bool>,
    origin: LocalSourceOrigin,
    job_owned_staging: bool,
    state: SourceSelectionState,
    root_inventory_item_id: Option<SourceItemId>,
    completeness: SourceCompletenessV2,
    issues: Vec<SourceIssue>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreparedInventoryEntry {
    item_id: SourceItemId,
    root_item_id: SourceItemId,
    parent_item_id: Option<SourceItemId>,
    relative_components: Vec<String>,
    name: String,
    kind: ManifestEntryKindV2,
    plaintext_size: u64,
    modified_unix_nanos: Option<u64>,
    digest: Option<ContentDigestV2>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SealedTransferJob {
    manifest: ManifestV2,
    structural_digest: ContentDigestV2,
    offer_bytes: Vec<u8>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalTransferJob {
    schema_version: u16,
    job_id: JobIdV2,
    created_unix_ms: u64,
    updated_unix_ms: u64,
    selection_revision: u64,
    generation: u32,
    lifecycle: JobLifecycle,
    compression_policy: CompressionPolicyV2,
    next_source_item_id: u64,
    next_issue_id: u64,
    selections: Vec<SourceSelection>,
    inventory: Vec<PreparedInventoryEntry>,
    source_bindings: BTreeMap<SourceItemId, LocalSourceBinding>,
    cleanup_pending: Vec<PathBuf>,
    sealed: Option<SealedTransferJob>,
}

impl fmt::Debug for CanonicalTransferJob {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalTransferJob")
            .field("job_id", &self.job_id)
            .field("selection_revision", &self.selection_revision)
            .field("generation", &self.generation)
            .field("lifecycle", &self.lifecycle)
            .field("root_count", &self.selections.len())
            .field("entry_count", &self.inventory.len())
            .field("sealed", &self.sealed.is_some())
            .finish()
    }
}

impl CanonicalTransferJob {
    pub fn new(compression_policy: CompressionPolicyV2) -> Result<Self, TransferJobError> {
        let mut job_id = [0_u8; 16];
        getrandom::fill(&mut job_id).map_err(|_| TransferJobError::Entropy)?;
        if job_id == [0; 16] {
            return Err(TransferJobError::Entropy);
        }
        let now = unix_time_ms();
        Ok(Self {
            schema_version: TRANSFER_JOB_SCHEMA_VERSION,
            job_id: JobIdV2(job_id),
            created_unix_ms: now,
            updated_unix_ms: now,
            selection_revision: FIRST_SELECTION_REVISION,
            generation: FIRST_GENERATION,
            lifecycle: JobLifecycle::Preparing,
            compression_policy,
            next_source_item_id: FIRST_SOURCE_ITEM_ID,
            next_issue_id: 1,
            selections: Vec::new(),
            inventory: Vec::new(),
            source_bindings: BTreeMap::new(),
            cleanup_pending: Vec::new(),
            sealed: None,
        })
    }

    pub fn job_id(&self) -> JobIdV2 {
        self.job_id
    }

    pub fn created_unix_ms(&self) -> u64 {
        self.created_unix_ms
    }

    pub fn updated_unix_ms(&self) -> u64 {
        self.updated_unix_ms
    }

    pub fn selection_revision(&self) -> u64 {
        self.selection_revision
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn lifecycle(&self) -> JobLifecycle {
        self.lifecycle
    }

    pub fn compression_policy(&self) -> CompressionPolicyV2 {
        self.compression_policy
    }

    pub fn set_compression_policy(
        &mut self,
        compression_policy: CompressionPolicyV2,
    ) -> Result<(), TransferJobError> {
        self.ensure_mutable()?;
        if self.compression_policy != compression_policy {
            self.compression_policy = compression_policy;
            self.bump_revision()?;
        }
        Ok(())
    }

    pub fn manifest(&self) -> Option<&ManifestV2> {
        self.sealed.as_ref().map(|sealed| &sealed.manifest)
    }

    pub fn structural_digest(&self) -> Option<ContentDigestV2> {
        self.sealed.as_ref().map(|sealed| sealed.structural_digest)
    }

    pub fn sealed_offer_bytes(&self) -> Option<&[u8]> {
        self.sealed
            .as_ref()
            .map(|sealed| sealed.offer_bytes.as_slice())
    }

    pub fn source_issues(&self, root_item_id: SourceItemId) -> Option<&[SourceIssue]> {
        self.selections
            .iter()
            .find(|selection| selection.root_item_id == root_item_id)
            .map(|selection| selection.issues.as_slice())
    }

    pub fn source_selections(&self) -> Vec<SourceSelectionInfo> {
        self.selections
            .iter()
            .map(|selection| SourceSelectionInfo {
                root_item_id: selection.root_item_id,
                requested_name: selection.requested_name.clone(),
                state: selection.state,
                partial_approved: matches!(
                    selection.completeness,
                    SourceCompletenessV2::UserApprovedPartial { .. }
                ),
                issues: selection.issues.clone(),
            })
            .collect()
    }

    pub async fn add_local_path(
        &mut self,
        path: PathBuf,
    ) -> Result<AddSourceResult, TransferJobError> {
        let requested_name = source_name(&path)?;
        self.add_local_source(path, requested_name, LocalSourceOrigin::Filesystem, false)
            .await
    }

    /// Adds a platform-stabilized file or directory while preserving provider
    /// completeness facts. Platform staging remains platform-owned; explicit
    /// Remove lets the platform delete it after this durable mutation commits.
    pub async fn add_provider_path(
        &mut self,
        path: PathBuf,
        requested_name: String,
        origin: LocalSourceOrigin,
        provider_issues: Vec<ProviderSourceIssue>,
    ) -> Result<AddSourceResult, TransferJobError> {
        if origin == LocalSourceOrigin::Filesystem {
            return Err(TransferJobError::InvalidRecord(
                "provider source must retain its platform origin".into(),
            ));
        }
        let requested_name = canonical_component(&requested_name);
        let provider_issues = canonical_provider_issues(provider_issues)?;
        validate_component(&requested_name)?;
        self.validate_provider_issues(&provider_issues)?;
        let added = self
            .add_local_source(path, requested_name, origin, false)
            .await?;
        if added.folded_into_existing_selection {
            return Ok(added);
        }
        self.prepare_selection(added.root_item_id).await?;
        self.attach_provider_issues(added.root_item_id, provider_issues)?;
        Ok(added)
    }

    /// Replaces one opaque provider root after the user grants access again.
    /// Platform-discovered inaccessible boundaries are committed with the new
    /// stabilized root so reauthorization cannot silently claim completeness.
    pub async fn reauthorize_provider_source(
        &mut self,
        root_item_id: SourceItemId,
        local_path: PathBuf,
        provider_issues: Vec<ProviderSourceIssue>,
    ) -> Result<(), TransferJobError> {
        let selection_index = self.selection_index(root_item_id)?;
        if self.selections[selection_index].origin == LocalSourceOrigin::Filesystem {
            return Err(TransferJobError::InvalidRecord(
                "filesystem sources must use filesystem reauthorization".into(),
            ));
        }
        let provider_issues = canonical_provider_issues(provider_issues)?;
        self.validate_provider_issues(&provider_issues)?;
        self.resolve_source_decision(root_item_id, SourceDecision::Reauthorize { local_path })?;
        self.prepare_selection(root_item_id).await?;
        self.attach_provider_issues(root_item_id, provider_issues)
    }

    async fn add_staged_source(
        &mut self,
        path: PathBuf,
        requested_name: String,
        origin: LocalSourceOrigin,
    ) -> Result<AddSourceResult, TransferJobError> {
        if origin == LocalSourceOrigin::Filesystem {
            return Err(TransferJobError::InvalidRecord(
                "staged source must retain its platform origin".into(),
            ));
        }
        let requested_name = canonical_component(&requested_name);
        validate_component(&requested_name)?;
        self.add_local_source(path, requested_name, origin, true)
            .await
    }

    async fn add_local_source(
        &mut self,
        path: PathBuf,
        requested_name: String,
        origin: LocalSourceOrigin,
        job_owned_staging: bool,
    ) -> Result<AddSourceResult, TransferJobError> {
        self.ensure_mutable()?;
        let requested_name = canonical_component(&requested_name);
        validate_component(&requested_name)?;

        let metadata = fs::symlink_metadata(&path).await;
        let (canonical_path, is_directory_hint, initial_issue) = match metadata {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                (None, None, Some(SourceIssueKind::SymbolicLink))
            }
            Ok(metadata) if metadata.is_file() || metadata.is_dir() => (
                fs::canonicalize(&path).await.ok(),
                Some(metadata.is_dir()),
                None,
            ),
            Ok(_) => (None, None, Some(SourceIssueKind::SpecialFile)),
            Err(error) => (None, None, Some(issue_kind(&error))),
        };

        if let Some(canonical_path) = canonical_path.as_ref() {
            if let Some(existing) = self.selections.iter().find(|selection| {
                selection.canonical_path.as_ref() == Some(canonical_path)
                    || selection.is_directory_hint == Some(true)
                        && selection
                            .canonical_path
                            .as_ref()
                            .is_some_and(|root| canonical_path.starts_with(root))
            }) {
                return Ok(AddSourceResult {
                    root_item_id: existing.root_item_id,
                    folded_into_existing_selection: true,
                    removed_covered_roots: Vec::new(),
                });
            }
        }
        let covers_existing_root = is_directory_hint == Some(true)
            && canonical_path.as_ref().is_some_and(|root| {
                self.selections.iter().any(|selection| {
                    selection
                        .canonical_path
                        .as_ref()
                        .is_some_and(|existing| existing.starts_with(root))
                })
            });
        if self.selections.len() >= MAX_MANIFEST_V2_ROOTS && !covers_existing_root {
            return Err(TransferJobError::RootLimit);
        }

        let root_item_id = self.allocate_source_item_id()?;
        let mut removed_covered_roots = Vec::new();
        if is_directory_hint == Some(true)
            && let Some(canonical_path) = canonical_path.as_ref()
        {
            self.selections.retain(|selection| {
                let covered = selection
                    .canonical_path
                    .as_ref()
                    .is_some_and(|existing| existing.starts_with(canonical_path));
                if covered {
                    removed_covered_roots.push(selection.root_item_id);
                }
                !covered
            });
            let removed = removed_covered_roots
                .iter()
                .copied()
                .collect::<HashSet<_>>();
            self.inventory
                .retain(|entry| !removed.contains(&entry.root_item_id));
            self.retain_current_bindings();
        }

        let mut issues = Vec::new();
        if let Some(kind) = initial_issue {
            issues.push(self.issue(root_item_id, Vec::new(), kind)?);
        }
        self.selections.push(SourceSelection {
            root_item_id,
            selection_order: 0,
            requested_name,
            path,
            canonical_path,
            is_directory_hint,
            origin,
            job_owned_staging,
            state: if issues.is_empty() {
                SourceSelectionState::Pending
            } else {
                SourceSelectionState::NeedsDecision
            },
            root_inventory_item_id: None,
            completeness: SourceCompletenessV2::Complete,
            issues,
        });
        self.reindex_selections();
        self.bump_revision()?;
        self.refresh_lifecycle();
        Ok(AddSourceResult {
            root_item_id,
            folded_into_existing_selection: false,
            removed_covered_roots,
        })
    }

    pub async fn prepare_all(&mut self) -> Result<(), TransferJobError> {
        self.ensure_mutable()?;
        let pending = self
            .selections
            .iter()
            .filter(|selection| selection.state == SourceSelectionState::Pending)
            .map(|selection| selection.root_item_id)
            .collect::<Vec<_>>();
        for root_item_id in pending {
            self.prepare_selection(root_item_id).await?;
        }
        self.refresh_lifecycle();
        Ok(())
    }

    pub async fn prepare_selection(
        &mut self,
        root_item_id: SourceItemId,
    ) -> Result<(), TransferJobError> {
        self.ensure_mutable()?;
        let selection_index = self.selection_index(root_item_id)?;
        if self.selections[selection_index].state == SourceSelectionState::NeedsDecision {
            return Err(TransferJobError::UnresolvedSourceDecision);
        }
        self.selections[selection_index].state = SourceSelectionState::Enumerating;
        self.refresh_lifecycle();

        self.inventory
            .retain(|entry| entry.root_item_id != root_item_id);
        self.retain_current_bindings();

        let selection = self.selections[selection_index].clone();
        let occupied_ids = self
            .inventory
            .iter()
            .map(|entry| entry.item_id)
            .chain(
                self.selections
                    .iter()
                    .map(|selection| selection.root_item_id),
            )
            .collect();
        let remaining_entries = MAX_MANIFEST_V2_ENTRIES.saturating_sub(self.inventory.len());
        let outcome =
            enumerate_local_selection(&selection, self.job_id, occupied_ids, remaining_entries)
                .await;
        match outcome {
            Ok(mut outcome) => {
                for pending_issue in outcome.issues.drain(..) {
                    outcome.resolved_issues.push(self.issue(
                        root_item_id,
                        pending_issue.relative_components,
                        pending_issue.kind,
                    )?);
                }
                self.inventory.extend(outcome.entries);
                self.source_bindings.extend(outcome.bindings);
                let selection = &mut self.selections[selection_index];
                selection.root_inventory_item_id = outcome.root_inventory_item_id;
                selection.issues = outcome.resolved_issues;
                selection.completeness = SourceCompletenessV2::Complete;
                selection.state = if selection.issues.is_empty() {
                    SourceSelectionState::Ready
                } else {
                    SourceSelectionState::NeedsDecision
                };
            }
            Err(kind) => {
                let issue = self.issue(root_item_id, Vec::new(), kind)?;
                let selection = &mut self.selections[selection_index];
                selection.issues = vec![issue];
                selection.root_inventory_item_id = None;
                selection.state = SourceSelectionState::NeedsDecision;
            }
        }
        self.bump_revision()?;
        self.refresh_lifecycle();
        Ok(())
    }

    fn resolve_source_decision(
        &mut self,
        root_item_id: SourceItemId,
        decision: SourceDecision,
    ) -> Result<Option<PathBuf>, TransferJobError> {
        self.ensure_mutable()?;
        let selection_index = self.selection_index(root_item_id)?;
        if self.selections[selection_index].state != SourceSelectionState::NeedsDecision
            && !matches!(
                &decision,
                SourceDecision::RemoveSelection | SourceDecision::CancelJob
            )
        {
            return Err(TransferJobError::DecisionNotRequired);
        }
        match decision {
            SourceDecision::Reauthorize { local_path } => {
                let canonical_path = std::fs::canonicalize(&local_path).ok();
                if canonical_path.as_ref().is_some_and(|candidate| {
                    self.selections.iter().any(|selection| {
                        selection.root_item_id != root_item_id
                            && selection.canonical_path.as_ref().is_some_and(|existing| {
                                candidate == existing
                                    || candidate.starts_with(existing)
                                    || existing.starts_with(candidate)
                            })
                    })
                }) {
                    return Err(TransferJobError::OverlappingSelection);
                }
                let selection = &mut self.selections[selection_index];
                selection.path = local_path;
                selection.job_owned_staging = false;
                selection.canonical_path = canonical_path;
                selection.is_directory_hint = selection
                    .canonical_path
                    .as_ref()
                    .and_then(|path| std::fs::metadata(path).ok())
                    .map(|metadata| metadata.is_dir());
                selection.root_inventory_item_id = None;
                selection.completeness = SourceCompletenessV2::Complete;
                selection.issues.clear();
                selection.state = SourceSelectionState::Pending;
                self.inventory
                    .retain(|entry| entry.root_item_id != root_item_id);
                self.retain_current_bindings();
            }
            SourceDecision::ApprovePartial => {
                let selection = &mut self.selections[selection_index];
                if selection.root_inventory_item_id.is_none()
                    || selection
                        .issues
                        .iter()
                        .any(|issue| issue.relative_components.is_empty())
                {
                    return Err(TransferJobError::EmptyPartialRoot);
                }
                if selection
                    .issues
                    .iter()
                    .any(|issue| issue.kind == SourceIssueKind::EntryLimit)
                {
                    return Err(TransferJobError::PartialNotAllowed);
                }
                selection.completeness = SourceCompletenessV2::UserApprovedPartial {
                    inaccessible_boundary_count: selection.issues.len() as u64,
                    omitted_entry_count: None,
                };
                selection.state = SourceSelectionState::Ready;
            }
            SourceDecision::RemoveSelection => {
                let removed = self.selections.remove(selection_index);
                self.inventory
                    .retain(|entry| entry.root_item_id != root_item_id);
                self.retain_current_bindings();
                self.reindex_selections();
                let owned_artifact = removed.job_owned_staging.then_some(removed.path);
                if let Some(path) = owned_artifact.as_ref() {
                    self.cleanup_pending.push(path.clone());
                }
                self.bump_revision()?;
                self.refresh_lifecycle();
                return Ok(owned_artifact);
            }
            SourceDecision::CancelJob => {
                self.lifecycle = JobLifecycle::Canceled;
                self.bump_revision()?;
                return Ok(None);
            }
        }
        self.bump_revision()?;
        self.refresh_lifecycle();
        Ok(None)
    }

    pub async fn hash_entry(
        &mut self,
        item_id: SourceItemId,
    ) -> Result<ContentDigestV2, TransferJobError> {
        if self.lifecycle == JobLifecycle::Canceled {
            return Err(TransferJobError::Canceled);
        }
        let binding = self
            .source_bindings
            .get(&item_id)
            .cloned()
            .ok_or(TransferJobError::UnknownItem(item_id))?;
        let entry_index = self
            .inventory
            .iter()
            .position(|entry| entry.item_id == item_id)
            .ok_or(TransferJobError::UnknownItem(item_id))?;
        if self.inventory[entry_index].kind != ManifestEntryKindV2::RegularFile {
            return Err(TransferJobError::NotRegularFile);
        }
        verify_fingerprint(&binding).await?;
        let mut file = fs::File::open(&binding.path).await?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0_u8; HASH_READ_BUFFER_BYTES];
        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        verify_fingerprint(&binding).await?;
        let digest = ContentDigestV2(*hasher.finalize().as_bytes());
        self.inventory[entry_index].digest = Some(digest);
        if self.lifecycle != JobLifecycle::Sealed {
            self.bump_revision()?;
        } else {
            self.updated_unix_ms = unix_time_ms().max(self.updated_unix_ms);
        }
        Ok(digest)
    }

    pub async fn read_compression_sample(
        &self,
        item_id: SourceItemId,
        limit: usize,
    ) -> Result<Vec<u8>, TransferJobError> {
        if limit == 0 || limit > MAX_COMPRESSION_SAMPLE_BYTES {
            return Err(TransferJobError::SampleLimit);
        }
        let binding = self
            .source_bindings
            .get(&item_id)
            .ok_or(TransferJobError::UnknownItem(item_id))?;
        verify_fingerprint(binding).await?;
        let mut file = fs::File::open(&binding.path).await?;
        let mut sample = vec![0_u8; limit];
        let read = file.read(&mut sample).await?;
        sample.truncate(read);
        Ok(sample)
    }

    pub fn inventory_summary(&self) -> InventorySummary {
        let mut summary = InventorySummary {
            root_count: self.selections.len() as u32,
            warning_count: self
                .selections
                .iter()
                .map(|selection| selection.issues.len() as u32)
                .sum(),
            ..InventorySummary::default()
        };
        for entry in &self.inventory {
            match entry.kind {
                ManifestEntryKindV2::RegularFile => {
                    summary.file_count = summary.file_count.saturating_add(1);
                    summary.total_plaintext_bytes = summary
                        .total_plaintext_bytes
                        .saturating_add(entry.plaintext_size);
                }
                ManifestEntryKindV2::Directory => {
                    summary.directory_count = summary.directory_count.saturating_add(1);
                }
            }
        }
        summary
    }

    pub fn list_roots(&self) -> Vec<InventoryItem> {
        self.selections
            .iter()
            .filter_map(|selection| selection.root_inventory_item_id)
            .filter_map(|item_id| self.inventory_item(item_id))
            .collect()
    }

    pub fn list_children(
        &self,
        parent_item_id: SourceItemId,
        cursor: Option<InventoryCursor>,
        limit: usize,
    ) -> Result<InventoryPage, TransferJobError> {
        self.page_inventory(
            self.inventory
                .iter()
                .filter(|entry| entry.parent_item_id == Some(parent_item_id)),
            cursor,
            limit,
        )
    }

    pub fn get_item(&self, item_id: SourceItemId) -> Option<InventoryItem> {
        self.inventory_item(item_id)
    }

    pub fn local_path_for_item(&self, item_id: SourceItemId) -> Option<&Path> {
        self.source_bindings
            .get(&item_id)
            .map(|binding| binding.path.as_path())
            .or_else(|| {
                self.selections
                    .iter()
                    .find(|selection| selection.root_item_id == item_id)
                    .map(|selection| selection.path.as_path())
            })
    }

    pub fn content_digest_for_item(&self, item_id: SourceItemId) -> Option<ContentDigestV2> {
        self.inventory
            .iter()
            .find(|entry| entry.item_id == item_id)
            .and_then(|entry| entry.digest)
    }

    pub fn source_for_sealed_entry(
        &self,
        entry_id: u32,
    ) -> Result<PreparedFileSource, TransferJobError> {
        if self.lifecycle != JobLifecycle::Sealed {
            return Err(TransferJobError::PreparationIncomplete);
        }
        let entry = self
            .selections
            .iter()
            .flat_map(|selection| {
                self.inventory
                    .iter()
                    .filter(move |entry| entry.root_item_id == selection.root_item_id)
            })
            .nth(entry_id as usize)
            .ok_or(TransferJobError::UnknownItem(SourceItemId(entry_id as u64)))?;
        if entry.kind != ManifestEntryKindV2::RegularFile {
            return Err(TransferJobError::NotRegularFile);
        }
        let binding = self
            .source_bindings
            .get(&entry.item_id)
            .ok_or(TransferJobError::UnknownItem(entry.item_id))?;
        Ok(PreparedFileSource {
            path: binding.path.clone(),
            fingerprint: binding.fingerprint.clone(),
        })
    }

    pub fn seal_for_send(&mut self) -> Result<&ManifestV2, TransferJobError> {
        self.ensure_mutable()?;
        self.refresh_lifecycle();
        match self.lifecycle {
            JobLifecycle::NeedsSourceDecision => {
                return Err(TransferJobError::UnresolvedSourceDecision);
            }
            JobLifecycle::ReadyToSend => {}
            _ => return Err(TransferJobError::PreparationIncomplete),
        }

        let mut roots = Vec::with_capacity(self.selections.len());
        let mut entries = Vec::with_capacity(self.inventory.len());
        let mut item_to_entry = BTreeMap::new();
        for (root_index, selection) in self.selections.iter().enumerate() {
            let root_id = root_index as u32;
            let root_entries = self
                .inventory
                .iter()
                .filter(|entry| entry.root_item_id == selection.root_item_id);
            let root_entry_id = entries.len() as u32;
            roots.push(ManifestRootV2 {
                root_id,
                root_entry_id,
                requested_name: selection.requested_name.clone(),
                completeness: selection.completeness,
            });
            for inventory_entry in root_entries {
                let entry_id = entries.len() as u32;
                let parent_entry_id = inventory_entry
                    .parent_item_id
                    .map(|parent| {
                        item_to_entry.get(&parent).copied().ok_or_else(|| {
                            TransferJobError::InvalidRecord(
                                "canonical inventory parent appears after its child".into(),
                            )
                        })
                    })
                    .transpose()?;
                item_to_entry.insert(inventory_entry.item_id, entry_id);
                entries.push(ManifestEntryV2 {
                    entry_id,
                    root_id,
                    parent_entry_id,
                    component: inventory_entry.name.clone(),
                    kind: inventory_entry.kind,
                    plaintext_size: inventory_entry.plaintext_size,
                    content_digest: inventory_entry
                        .digest
                        .map(EntryContentDigestV2::Known)
                        .unwrap_or(EntryContentDigestV2::Deferred),
                });
            }
        }
        let summary = self.inventory_summary();
        let manifest = ManifestV2 {
            job_id: self.job_id,
            generation: self.generation,
            selection_revision: self.selection_revision,
            compression_policy: self.compression_policy,
            roots,
            entries,
            totals: ManifestTotalsV2 {
                file_count: summary.file_count,
                directory_count: summary.directory_count,
                total_plaintext_bytes: summary.total_plaintext_bytes,
            },
        };
        let offer = build_manifest_offer_v2(manifest)
            .map_err(|error| TransferJobError::Protocol(error.to_string()))?;
        let offer_bytes = encode_manifest_offer_v2(&offer.manifest)
            .map_err(|error| TransferJobError::Protocol(error.to_string()))?;
        self.sealed = Some(SealedTransferJob {
            manifest: offer.manifest,
            structural_digest: offer.structural_digest,
            offer_bytes,
        });
        self.lifecycle = JobLifecycle::Sealed;
        self.updated_unix_ms = unix_time_ms().max(self.updated_unix_ms);
        self.sealed
            .as_ref()
            .map(|sealed| &sealed.manifest)
            .ok_or_else(|| TransferJobError::InvalidRecord("sealed job facts are missing".into()))
    }

    pub fn cancel(&mut self) -> Result<(), TransferJobError> {
        if self.lifecycle == JobLifecycle::Canceled {
            return Ok(());
        }
        self.lifecycle = JobLifecycle::Canceled;
        self.bump_revision()
    }

    pub fn validate_durable(&self) -> Result<(), TransferJobError> {
        if self.schema_version != TRANSFER_JOB_SCHEMA_VERSION {
            return Err(TransferJobError::UnsupportedSchema(self.schema_version));
        }
        if self.job_id.0 == [0; 16]
            || self.created_unix_ms == 0
            || self.updated_unix_ms < self.created_unix_ms
            || self.selection_revision == 0
            || self.generation == 0
            || self.selections.len() > MAX_MANIFEST_V2_ROOTS
            || self.inventory.len() > MAX_MANIFEST_V2_ENTRIES
        {
            return Err(TransferJobError::InvalidRecord(
                "identity, revision, generation, or bounds are invalid".into(),
            ));
        }
        let unique_items = self
            .inventory
            .iter()
            .map(|entry| entry.item_id)
            .collect::<HashSet<_>>();
        if unique_items.len() != self.inventory.len() {
            return Err(TransferJobError::InvalidRecord(
                "inventory item IDs are not unique".into(),
            ));
        }
        let root_ids = self
            .selections
            .iter()
            .map(|selection| selection.root_item_id)
            .collect::<HashSet<_>>();
        if root_ids.len() != self.selections.len()
            || self
                .selections
                .iter()
                .enumerate()
                .any(|(index, selection)| selection.selection_order != index as u32)
        {
            return Err(TransferJobError::InvalidRecord(
                "selection IDs or ordering are invalid".into(),
            ));
        }
        for selection in &self.selections {
            validate_component(&selection.requested_name)
                .map_err(|_| TransferJobError::InvalidRecord("selection name is invalid".into()))?;
            if selection
                .issues
                .iter()
                .any(|issue| issue.root_item_id != selection.root_item_id)
            {
                return Err(TransferJobError::InvalidRecord(
                    "source issue belongs to another root".into(),
                ));
            }
            let root_entry = selection
                .root_inventory_item_id
                .and_then(|item_id| self.inventory.iter().find(|entry| entry.item_id == item_id));
            if root_entry.is_some_and(|entry| {
                entry.root_item_id != selection.root_item_id
                    || entry.parent_item_id.is_some()
                    || !entry.relative_components.is_empty()
            }) || selection.root_inventory_item_id.is_some() && root_entry.is_none()
            {
                return Err(TransferJobError::InvalidRecord(
                    "selection root inventory reference is invalid".into(),
                ));
            }
        }
        let mut seen = HashSet::new();
        for entry in &self.inventory {
            if !root_ids.contains(&entry.root_item_id)
                || validate_component(&entry.name).is_err()
                || entry.relative_components.len() > MAX_MANIFEST_V2_PATH_DEPTH
                || logical_path_bytes(&entry.relative_components) > MAX_MANIFEST_V2_PATH_BYTES
                || entry.kind == ManifestEntryKindV2::Directory && entry.plaintext_size != 0
            {
                return Err(TransferJobError::InvalidRecord(
                    "inventory entry violates canonical bounds".into(),
                ));
            }
            if entry
                .relative_components
                .last()
                .is_some_and(|name| name != &entry.name)
            {
                return Err(TransferJobError::InvalidRecord(
                    "inventory component and logical path disagree".into(),
                ));
            }
            if let Some(parent_item_id) = entry.parent_item_id {
                let Some(parent) = self
                    .inventory
                    .iter()
                    .find(|candidate| candidate.item_id == parent_item_id)
                else {
                    return Err(TransferJobError::InvalidRecord(
                        "inventory parent is missing".into(),
                    ));
                };
                if !seen.contains(&parent_item_id)
                    || parent.root_item_id != entry.root_item_id
                    || parent.kind != ManifestEntryKindV2::Directory
                {
                    return Err(TransferJobError::InvalidRecord(
                        "inventory parent ordering or kind is invalid".into(),
                    ));
                }
            }
            seen.insert(entry.item_id);
        }
        if self.source_bindings.len() != self.inventory.len()
            || self
                .source_bindings
                .keys()
                .any(|item_id| !unique_items.contains(item_id))
        {
            return Err(TransferJobError::InvalidRecord(
                "source bindings do not match inventory".into(),
            ));
        }
        if self.cleanup_pending.iter().any(|pending| {
            self.selections
                .iter()
                .any(|selection| selection.path == *pending)
        }) {
            return Err(TransferJobError::InvalidRecord(
                "active sources cannot also be pending cleanup".into(),
            ));
        }
        if self.lifecycle != JobLifecycle::Canceled {
            let expected = if self.sealed.is_some() {
                JobLifecycle::Sealed
            } else if self
                .selections
                .iter()
                .any(|selection| selection.state == SourceSelectionState::NeedsDecision)
            {
                JobLifecycle::NeedsSourceDecision
            } else if !self.selections.is_empty()
                && self
                    .selections
                    .iter()
                    .all(|selection| selection.state == SourceSelectionState::Ready)
            {
                JobLifecycle::ReadyToSend
            } else {
                JobLifecycle::Preparing
            };
            if self.lifecycle != expected {
                return Err(TransferJobError::InvalidRecord(
                    "job lifecycle does not match durable facts".into(),
                ));
            }
        }
        if let Some(sealed) = &self.sealed {
            if !matches!(
                self.lifecycle,
                JobLifecycle::Sealed | JobLifecycle::Canceled
            ) {
                return Err(TransferJobError::InvalidRecord(
                    "only a sealed or canceled job may contain sealed bytes".into(),
                ));
            }
            if sealed.manifest.job_id != self.job_id
                || sealed.manifest.generation != self.generation
                || sealed.manifest.selection_revision != self.selection_revision
                || sealed.manifest.compression_policy != self.compression_policy
            {
                return Err(TransferJobError::InvalidRecord(
                    "sealed Manifest identity does not match its job".into(),
                ));
            }
            let rebuilt = build_manifest_offer_v2(sealed.manifest.clone())
                .map_err(|error| TransferJobError::Protocol(error.to_string()))?;
            let encoded = encode_manifest_offer_v2(&rebuilt.manifest)
                .map_err(|error| TransferJobError::Protocol(error.to_string()))?;
            if rebuilt.structural_digest != sealed.structural_digest
                || encoded != sealed.offer_bytes
            {
                return Err(TransferJobError::InvalidRecord(
                    "sealed Manifest bytes or digest changed".into(),
                ));
            }
        }
        Ok(())
    }

    fn page_inventory<'a>(
        &self,
        entries: impl Iterator<Item = &'a PreparedInventoryEntry>,
        cursor: Option<InventoryCursor>,
        limit: usize,
    ) -> Result<InventoryPage, TransferJobError> {
        if limit == 0 || limit > MAX_INVENTORY_PAGE_SIZE {
            return Err(TransferJobError::InvalidPageLimit);
        }
        let offset = if let Some(cursor) = cursor {
            if cursor.revision != self.selection_revision {
                return Err(TransferJobError::StaleInventoryCursor {
                    expected: self.selection_revision,
                    actual: cursor.revision,
                });
            }
            cursor.offset as usize
        } else {
            0
        };
        let entries = entries.collect::<Vec<_>>();
        let items = entries
            .iter()
            .skip(offset)
            .take(limit)
            .map(|entry| self.project_inventory_item(entry))
            .collect::<Vec<_>>();
        let next_offset = offset.saturating_add(items.len());
        Ok(InventoryPage {
            revision: self.selection_revision,
            items,
            next_cursor: (next_offset < entries.len()).then_some(InventoryCursor {
                revision: self.selection_revision,
                offset: next_offset as u32,
            }),
        })
    }

    fn inventory_item(&self, item_id: SourceItemId) -> Option<InventoryItem> {
        self.inventory
            .iter()
            .find(|entry| entry.item_id == item_id)
            .map(|entry| self.project_inventory_item(entry))
    }

    fn project_inventory_item(&self, entry: &PreparedInventoryEntry) -> InventoryItem {
        let has_warning = self
            .selections
            .iter()
            .find(|selection| selection.root_item_id == entry.root_item_id)
            .is_some_and(|selection| !selection.issues.is_empty());
        InventoryItem {
            item_id: entry.item_id,
            root_item_id: entry.root_item_id,
            parent_item_id: entry.parent_item_id,
            name: entry.name.clone(),
            kind: entry.kind,
            plaintext_size: entry.plaintext_size,
            digest_known: entry.digest.is_some(),
            has_warning,
        }
    }

    fn selection_index(&self, root_item_id: SourceItemId) -> Result<usize, TransferJobError> {
        self.selections
            .iter()
            .position(|selection| selection.root_item_id == root_item_id)
            .ok_or(TransferJobError::UnknownSelection(root_item_id))
    }

    fn issue(
        &mut self,
        root_item_id: SourceItemId,
        relative_components: Vec<String>,
        kind: SourceIssueKind,
    ) -> Result<SourceIssue, TransferJobError> {
        let issue_id = self.next_issue_id;
        self.next_issue_id = self
            .next_issue_id
            .checked_add(1)
            .ok_or_else(|| TransferJobError::InvalidRecord("issue ID overflow".into()))?;
        Ok(SourceIssue {
            issue_id,
            root_item_id,
            relative_components,
            kind,
        })
    }

    fn validate_provider_issues(
        &self,
        provider_issues: &[ProviderSourceIssue],
    ) -> Result<(), TransferJobError> {
        for issue in provider_issues {
            for component in &issue.relative_components {
                validate_component(component)?;
            }
        }
        Ok(())
    }

    fn attach_provider_issues(
        &mut self,
        root_item_id: SourceItemId,
        provider_issues: Vec<ProviderSourceIssue>,
    ) -> Result<(), TransferJobError> {
        if provider_issues.is_empty() {
            return Ok(());
        }
        self.validate_provider_issues(&provider_issues)?;
        let mut resolved = Vec::with_capacity(provider_issues.len());
        for issue in provider_issues {
            resolved.push(self.issue(root_item_id, issue.relative_components, issue.kind)?);
        }
        let selection_index = self.selection_index(root_item_id)?;
        self.selections[selection_index].issues.extend(resolved);
        self.selections[selection_index].state = SourceSelectionState::NeedsDecision;
        self.bump_revision()?;
        self.refresh_lifecycle();
        Ok(())
    }

    fn allocate_source_item_id(&mut self) -> Result<SourceItemId, TransferJobError> {
        let id = self.next_source_item_id;
        self.next_source_item_id = self
            .next_source_item_id
            .checked_add(1)
            .ok_or_else(|| TransferJobError::InvalidRecord("source item ID overflow".into()))?;
        Ok(SourceItemId(id))
    }

    fn reindex_selections(&mut self) {
        for (index, selection) in self.selections.iter_mut().enumerate() {
            selection.selection_order = index as u32;
        }
    }

    fn bump_revision(&mut self) -> Result<(), TransferJobError> {
        self.selection_revision = self
            .selection_revision
            .checked_add(1)
            .ok_or_else(|| TransferJobError::InvalidRecord("selection revision overflow".into()))?;
        self.updated_unix_ms = unix_time_ms().max(self.updated_unix_ms);
        Ok(())
    }

    fn retain_current_bindings(&mut self) {
        let retained = self
            .inventory
            .iter()
            .map(|entry| entry.item_id)
            .collect::<HashSet<_>>();
        self.source_bindings
            .retain(|item_id, _| retained.contains(item_id));
    }

    fn ensure_mutable(&self) -> Result<(), TransferJobError> {
        match self.lifecycle {
            JobLifecycle::Sealed => Err(TransferJobError::SealedMutation),
            JobLifecycle::Canceled => Err(TransferJobError::Canceled),
            _ => Ok(()),
        }
    }

    fn refresh_lifecycle(&mut self) {
        if matches!(
            self.lifecycle,
            JobLifecycle::Sealed | JobLifecycle::Canceled
        ) {
            return;
        }
        self.lifecycle = if self
            .selections
            .iter()
            .any(|selection| selection.state == SourceSelectionState::NeedsDecision)
        {
            JobLifecycle::NeedsSourceDecision
        } else if !self.selections.is_empty()
            && self
                .selections
                .iter()
                .all(|selection| selection.state == SourceSelectionState::Ready)
        {
            JobLifecycle::ReadyToSend
        } else {
            JobLifecycle::Preparing
        };
    }
}

#[derive(Clone, Debug)]
struct PendingIssue {
    relative_components: Vec<String>,
    kind: SourceIssueKind,
}

struct EnumerationOutcome {
    root_inventory_item_id: Option<SourceItemId>,
    entries: Vec<PreparedInventoryEntry>,
    bindings: BTreeMap<SourceItemId, LocalSourceBinding>,
    issues: Vec<PendingIssue>,
    resolved_issues: Vec<SourceIssue>,
}

async fn enumerate_local_selection(
    selection: &SourceSelection,
    job_id: JobIdV2,
    mut occupied_ids: HashSet<SourceItemId>,
    entry_budget: usize,
) -> Result<EnumerationOutcome, SourceIssueKind> {
    let mut entries = Vec::new();
    let mut bindings = BTreeMap::new();
    let mut issues = Vec::new();
    let mut pending = vec![(
        selection.path.clone(),
        Vec::<String>::new(),
        None::<SourceItemId>,
    )];
    let mut root_inventory_item_id = None;

    while let Some((path, relative_components, parent_item_id)) = pending.pop() {
        if entries.len() >= entry_budget {
            issues.push(PendingIssue {
                relative_components,
                kind: SourceIssueKind::EntryLimit,
            });
            break;
        }
        if relative_components.len() > MAX_MANIFEST_V2_PATH_DEPTH
            || logical_path_bytes(&relative_components) > MAX_MANIFEST_V2_PATH_BYTES
        {
            issues.push(PendingIssue {
                relative_components,
                kind: SourceIssueKind::DepthLimit,
            });
            continue;
        }
        let metadata = match fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) => {
                issues.push(PendingIssue {
                    relative_components,
                    kind: issue_kind(&error),
                });
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            issues.push(PendingIssue {
                relative_components,
                kind: SourceIssueKind::SymbolicLink,
            });
            continue;
        }
        if !metadata.is_file() && !metadata.is_dir() {
            issues.push(PendingIssue {
                relative_components,
                kind: SourceIssueKind::SpecialFile,
            });
            continue;
        }

        let name = relative_components
            .last()
            .cloned()
            .unwrap_or_else(|| selection.requested_name.clone());
        if validate_component(&name).is_err() {
            issues.push(PendingIssue {
                relative_components,
                kind: SourceIssueKind::InvalidName,
            });
            continue;
        }
        let item_id = stable_inventory_item_id(
            job_id,
            selection.root_item_id,
            &relative_components,
            &mut occupied_ids,
        );
        if root_inventory_item_id.is_none() {
            root_inventory_item_id = Some(item_id);
        }
        let canonical_path = fs::canonicalize(&path)
            .await
            .unwrap_or_else(|_| path.clone());
        let fingerprint = fingerprint(&canonical_path, &metadata);
        let kind = if metadata.is_dir() {
            ManifestEntryKindV2::Directory
        } else {
            ManifestEntryKindV2::RegularFile
        };
        entries.push(PreparedInventoryEntry {
            item_id,
            root_item_id: selection.root_item_id,
            parent_item_id,
            relative_components: relative_components.clone(),
            name,
            kind,
            plaintext_size: if metadata.is_file() {
                metadata.len()
            } else {
                0
            },
            modified_unix_nanos: modified_unix_nanos(&metadata),
            digest: None,
        });
        bindings.insert(
            item_id,
            LocalSourceBinding {
                path: canonical_path,
                origin: selection.origin,
                job_owned_staging: selection.job_owned_staging,
                fingerprint,
            },
        );

        if metadata.is_dir() {
            let mut children = Vec::new();
            let mut directory = match fs::read_dir(&path).await {
                Ok(directory) => directory,
                Err(error) => {
                    issues.push(PendingIssue {
                        relative_components,
                        kind: issue_kind(&error),
                    });
                    continue;
                }
            };
            loop {
                match directory.next_entry().await {
                    Ok(Some(child)) => {
                        let Some(child_name) = child.file_name().to_str().map(canonical_component)
                        else {
                            issues.push(PendingIssue {
                                relative_components: relative_components.clone(),
                                kind: SourceIssueKind::InvalidName,
                            });
                            continue;
                        };
                        if validate_component(&child_name).is_err() {
                            let mut child_components = relative_components.clone();
                            child_components.push(child_name);
                            issues.push(PendingIssue {
                                relative_components: child_components,
                                kind: SourceIssueKind::InvalidName,
                            });
                            continue;
                        }
                        let mut child_components = relative_components.clone();
                        child_components.push(child_name);
                        children.push((child.path(), child_components, Some(item_id)));
                    }
                    Ok(None) => break,
                    Err(error) => {
                        issues.push(PendingIssue {
                            relative_components: relative_components.clone(),
                            kind: issue_kind(&error),
                        });
                        break;
                    }
                }
            }
            remove_component_collisions(&mut children, &mut issues);
            children.sort_unstable_by(|left, right| left.1.cmp(&right.1));
            pending.extend(children.into_iter().rev());
        }
    }

    Ok(EnumerationOutcome {
        root_inventory_item_id,
        entries,
        bindings,
        issues,
        resolved_issues: Vec::new(),
    })
}

fn remove_component_collisions(
    children: &mut Vec<(PathBuf, Vec<String>, Option<SourceItemId>)>,
    issues: &mut Vec<PendingIssue>,
) {
    let mut seen_components = HashSet::new();
    let mut collided_components = HashSet::new();
    for (_, components, _) in children.iter() {
        if !seen_components.insert(components.clone()) {
            collided_components.insert(components.clone());
        }
    }
    if collided_components.is_empty() {
        return;
    }
    children.retain(|(_, components, _)| !collided_components.contains(components));
    issues.extend(
        collided_components
            .into_iter()
            .map(|relative_components| PendingIssue {
                relative_components,
                kind: SourceIssueKind::InvalidName,
            }),
    );
}

fn stable_inventory_item_id(
    job_id: JobIdV2,
    root_item_id: SourceItemId,
    components: &[String],
    occupied: &mut HashSet<SourceItemId>,
) -> SourceItemId {
    let mut salt = 0_u64;
    loop {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&job_id.0);
        hasher.update(&root_item_id.0.to_be_bytes());
        hasher.update(&salt.to_be_bytes());
        for component in components {
            hasher.update(&(component.len() as u32).to_be_bytes());
            hasher.update(component.as_bytes());
        }
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&hasher.finalize().as_bytes()[..8]);
        let id = SourceItemId(u64::from_be_bytes(bytes) | DERIVED_ITEM_ID_MASK);
        if occupied.insert(id) {
            return id;
        }
        salt = salt.wrapping_add(1);
    }
}

fn source_name(path: &Path) -> Result<String, TransferJobError> {
    let name: String = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(TransferJobError::InvalidComponent)?
        .nfc()
        .collect();
    validate_component(&name)?;
    Ok(name)
}

fn canonical_component(component: &str) -> String {
    component.nfc().collect()
}

fn canonical_provider_issues(
    mut issues: Vec<ProviderSourceIssue>,
) -> Result<Vec<ProviderSourceIssue>, TransferJobError> {
    for issue in &mut issues {
        for component in &mut issue.relative_components {
            *component = canonical_component(component);
            validate_component(component)?;
        }
    }
    Ok(issues)
}

fn validate_component(component: &str) -> Result<(), TransferJobError> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.len() > MAX_MANIFEST_V2_COMPONENT_BYTES
        || component
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
        || !unicode_normalization::is_nfc(component)
    {
        return Err(TransferJobError::InvalidComponent);
    }
    Ok(())
}

fn logical_path_bytes(components: &[String]) -> usize {
    components
        .iter()
        .map(String::len)
        .sum::<usize>()
        .saturating_add(components.len().saturating_sub(1))
}

fn modified_unix_nanos(metadata: &std::fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
}

fn fingerprint(path: &Path, metadata: &std::fs::Metadata) -> SourceFingerprint {
    SourceFingerprint {
        plaintext_size: if metadata.is_file() {
            metadata.len()
        } else {
            0
        },
        modified_unix_nanos: modified_unix_nanos(metadata),
        canonical_path_digest: *blake3::hash(path.as_os_str().as_encoded_bytes()).as_bytes(),
    }
}

async fn verify_fingerprint(binding: &LocalSourceBinding) -> Result<(), TransferJobError> {
    verify_source_fingerprint(&binding.path, &binding.fingerprint).await
}

async fn verify_source_fingerprint(
    path: &Path,
    expected: &SourceFingerprint,
) -> Result<(), TransferJobError> {
    let metadata = fs::symlink_metadata(path).await?;
    if metadata.file_type().is_symlink() || fingerprint(path, &metadata) != *expected {
        return Err(TransferJobError::SourceChanged);
    }
    Ok(())
}

fn issue_kind(error: &std::io::Error) -> SourceIssueKind {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => SourceIssueKind::PermissionDenied,
        std::io::ErrorKind::NotFound => SourceIssueKind::Unavailable,
        _ => SourceIssueKind::Unavailable,
    }
}

#[derive(Clone, Debug)]
pub struct TransferJobStore {
    directory: PathBuf,
}

impl TransferJobStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub async fn save(&self, job: &CanonicalTransferJob) -> Result<(), TransferJobError> {
        job.validate_durable()?;
        fs::create_dir_all(&self.directory).await?;
        let final_path = self.job_path(job.job_id);
        let temporary_path = self.temporary_job_path(job.job_id);
        let bytes = serde_json::to_vec(job)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        fs::rename(&temporary_path, final_path).await?;
        Ok(())
    }

    /// Copies a transient platform file into store-owned staging before it is
    /// attached to the job. Only paths created here acquire cleanup ownership.
    pub async fn import_staged_file(
        &self,
        job: &mut CanonicalTransferJob,
        source_path: &Path,
        requested_name: String,
        origin: LocalSourceOrigin,
    ) -> Result<AddSourceResult, TransferJobError> {
        if origin == LocalSourceOrigin::Filesystem {
            return Err(TransferJobError::InvalidRecord(
                "staged import must retain its platform origin".into(),
            ));
        }
        let requested_name = canonical_component(&requested_name);
        validate_component(&requested_name)?;
        let source_metadata = fs::symlink_metadata(source_path).await?;
        if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
            return Err(TransferJobError::NotRegularFile);
        }
        let staging_directory = self
            .directory
            .join(".envoix-staging")
            .join(encode_job_id(job.job_id()));
        fs::create_dir_all(&staging_directory).await?;
        let mut source = fs::File::open(source_path).await?;
        let staged_path = loop {
            let mut random_name = [0_u8; 16];
            getrandom::fill(&mut random_name).map_err(|_| TransferJobError::Entropy)?;
            let candidate = staging_directory.join(encode_hex(&random_name));
            match fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&candidate)
                .await
            {
                Ok(mut destination) => {
                    if let Err(error) = tokio::io::copy(&mut source, &mut destination).await {
                        drop(destination);
                        let _ = fs::remove_file(&candidate).await;
                        return Err(error.into());
                    }
                    if let Err(error) = destination.sync_all().await {
                        drop(destination);
                        let _ = fs::remove_file(&candidate).await;
                        return Err(error.into());
                    }
                    break candidate;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        match job
            .add_staged_source(staged_path.clone(), requested_name, origin)
            .await
        {
            Ok(result) => Ok(result),
            Err(error) => {
                let _ = fs::remove_file(staged_path).await;
                Err(error)
            }
        }
    }

    /// Persists the source decision before releasing any store-owned staging.
    pub async fn apply_source_decision(
        &self,
        job: &mut CanonicalTransferJob,
        root_item_id: SourceItemId,
        decision: SourceDecision,
    ) -> Result<(), TransferJobError> {
        let owned_artifact = job.resolve_source_decision(root_item_id, decision)?;
        self.save(job).await?;
        if let Some(path) = owned_artifact {
            self.remove_staged_artifact(job.job_id(), &path).await?;
            job.cleanup_pending.retain(|pending| pending != &path);
            self.save(job).await?;
        }
        Ok(())
    }

    pub async fn reconcile_pending_cleanup(
        &self,
        job: &mut CanonicalTransferJob,
    ) -> Result<(), TransferJobError> {
        let pending = job.cleanup_pending.clone();
        if pending.is_empty() {
            return Ok(());
        }
        for path in pending {
            self.remove_staged_artifact(job.job_id(), &path).await?;
            job.cleanup_pending.retain(|candidate| candidate != &path);
            self.save(job).await?;
        }
        Ok(())
    }

    async fn remove_staged_artifact(
        &self,
        job_id: JobIdV2,
        path: &Path,
    ) -> Result<(), TransferJobError> {
        let staging_directory = self
            .directory
            .join(".envoix-staging")
            .join(encode_job_id(job_id));
        let relative = path.strip_prefix(&staging_directory).map_err(|_| {
            TransferJobError::InvalidRecord("owned staging escaped its job directory".into())
        })?;
        if relative.components().count() != 1 {
            return Err(TransferJobError::InvalidRecord(
                "owned staging path has an invalid shape".into(),
            ));
        }
        match fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    pub async fn load(
        &self,
        job_id: JobIdV2,
    ) -> Result<Option<CanonicalTransferJob>, TransferJobError> {
        let bytes = match fs::read(self.job_path(job_id)).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let job = serde_json::from_slice::<CanonicalTransferJob>(&bytes)?;
        job.validate_durable()?;
        Ok(Some(job))
    }

    pub async fn load_all(&self) -> Result<Vec<CanonicalTransferJob>, TransferJobError> {
        let mut jobs = Vec::new();
        let mut directory = match fs::read_dir(&self.directory).await {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(jobs),
            Err(error) => return Err(error.into()),
        };
        while let Some(entry) = directory.next_entry().await? {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with("job-") || !name.ends_with(".json") {
                continue;
            }
            let bytes = match fs::read(entry.path()).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!(kind = ?error.kind(), "skipping unreadable transfer job");
                    continue;
                }
            };
            match serde_json::from_slice::<CanonicalTransferJob>(&bytes) {
                Ok(job) if job.validate_durable().is_ok() => jobs.push(job),
                Ok(_) => tracing::warn!("skipping inconsistent transfer job"),
                Err(error) => tracing::warn!(%error, "skipping unparseable transfer job"),
            }
        }
        jobs.sort_unstable_by_key(|job| (job.created_unix_ms(), job.job_id().0));
        Ok(jobs)
    }

    fn job_path(&self, job_id: JobIdV2) -> PathBuf {
        self.directory
            .join(format!("job-{}.json", encode_job_id(job_id)))
    }

    fn temporary_job_path(&self, job_id: JobIdV2) -> PathBuf {
        self.directory
            .join(format!(".job-{}.tmp", encode_job_id(job_id)))
    }
}

#[cfg(test)]
#[path = "job_tests.rs"]
mod tests;

fn encode_job_id(job_id: JobIdV2) -> String {
    encode_hex(&job_id.0)
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}
