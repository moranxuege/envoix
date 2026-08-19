//! Bounded native projection for canonical Manifest v2 preparation.

use std::path::PathBuf;
use std::sync::Arc;

use envoix_client::api::{
    CanonicalTransferJob, CompressionPolicyV2, InventoryCursor, InventoryItem, JobIdV2,
    JobLifecycle, LocalSourceOrigin, ManifestEntryKindV2, ProviderSourceIssue, SourceDecision,
    SourceIssue, SourceIssueKind, SourceItemId, SourceSelectionInfo, SourceSelectionState,
    TransferJobStore,
};
use tokio::sync::Mutex;

use super::{EnvoixError, on_ffi_runtime, op_err};

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiCompressionPolicyV2 {
    Never,
    Always,
    Smart,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiTransferJobStateV2 {
    Preparing,
    NeedsSourceDecision,
    ReadyToSend,
    Sealed,
    Canceled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiSourceSelectionStateV2 {
    Pending,
    Enumerating,
    NeedsDecision,
    Ready,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiSourceOriginV2 {
    Photos,
    Share,
    ContentUri,
    FileProvider,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiProviderSourceIssueKindV2 {
    PermissionDenied,
    Unavailable,
    InvalidName,
    SpecialFile,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiProviderSourceIssueV2 {
    pub relative_components: Vec<String>,
    pub kind: FfiProviderSourceIssueKindV2,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiStagedProviderRootV2 {
    pub path: String,
    pub requested_name: String,
    pub origin: FfiSourceOriginV2,
    pub issues: Vec<FfiProviderSourceIssueV2>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiSourceIssueKindV2 {
    PermissionDenied,
    Unavailable,
    InvalidName,
    SymbolicLink,
    SpecialFile,
    SourceChanged,
    DepthLimit,
    EntryLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiSourceDecisionV2 {
    Reauthorize,
    ApprovePartial,
    RemoveSelection,
    CancelJob,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum FfiInventoryItemKindV2 {
    File,
    Directory,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiSourceIssueV2 {
    pub issue_id: u64,
    pub relative_components: Vec<String>,
    pub kind: FfiSourceIssueKindV2,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiSourceSelectionV2 {
    pub root_item_id: u64,
    pub requested_name: String,
    pub state: FfiSourceSelectionStateV2,
    pub partial_approved: bool,
    pub issues: Vec<FfiSourceIssueV2>,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiInventorySummaryV2 {
    pub root_count: u32,
    pub file_count: u32,
    pub directory_count: u32,
    pub total_plaintext_bytes: u64,
    pub warning_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiTransferJobSnapshotV2 {
    pub job_id: String,
    pub selection_revision: u64,
    pub state: FfiTransferJobStateV2,
    pub compression_policy: FfiCompressionPolicyV2,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
    pub inventory: FfiInventorySummaryV2,
    pub selections: Vec<FfiSourceSelectionV2>,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiInventoryItemV2 {
    pub item_id: u64,
    pub root_item_id: u64,
    pub parent_item_id: Option<u64>,
    pub name: String,
    pub kind: FfiInventoryItemKindV2,
    pub plaintext_size: u64,
    pub digest_known: bool,
    pub has_warning: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiInventoryPageV2 {
    pub revision: u64,
    pub items: Vec<FfiInventoryItemV2>,
    pub next_offset: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiManifestSealV2 {
    pub job_id: String,
    pub selection_revision: u64,
    pub structural_digest: Vec<u8>,
    pub offer_bytes: Vec<u8>,
}

#[derive(uniffi::Object)]
pub struct FfiTransferJobV2 {
    job: Mutex<CanonicalTransferJob>,
    store: TransferJobStore,
}

impl FfiTransferJobV2 {
    pub(crate) async fn clone_sealed_job(&self) -> Result<CanonicalTransferJob, EnvoixError> {
        let job = self.job.lock().await;
        if job.manifest().is_none() {
            return Err(EnvoixError::Operation {
                reason: "transfer job must be sealed by an explicit Send action".into(),
            });
        }
        Ok(job.clone())
    }
}

#[uniffi::export]
impl FfiTransferJobV2 {
    pub async fn snapshot(&self) -> FfiTransferJobSnapshotV2 {
        let job = self.job.lock().await;
        snapshot(&job)
    }

    /// Adds any mixture of local files and folders, then starts local-only
    /// preparation. No session or other network object is reachable here.
    pub async fn add_local_paths(
        &self,
        paths: Vec<String>,
    ) -> Result<FfiTransferJobSnapshotV2, EnvoixError> {
        on_ffi_runtime(async {
            if paths.is_empty() || paths.iter().any(|path| path.trim().is_empty()) {
                return Err(EnvoixError::Operation {
                    reason: "paths must contain at least one non-empty local path".into(),
                });
            }
            let mut job = self.job.lock().await;
            for path in paths {
                job.add_local_path(PathBuf::from(path))
                    .await
                    .map_err(op_err)?;
                self.store.save(&job).await.map_err(op_err)?;
            }
            job.prepare_all().await.map_err(op_err)?;
            self.store.save(&job).await.map_err(op_err)?;
            Ok(snapshot(&job))
        })
        .await
    }

    /// Imports Photos/Share/provider data into job-owned staging and prepares
    /// it immediately. The native source remains outside Rust ownership.
    pub async fn import_transient_file(
        &self,
        source_path: String,
        requested_name: String,
        origin: FfiSourceOriginV2,
    ) -> Result<FfiTransferJobSnapshotV2, EnvoixError> {
        on_ffi_runtime(async {
            if source_path.trim().is_empty() || requested_name.trim().is_empty() {
                return Err(EnvoixError::Operation {
                    reason: "source_path and requested_name must not be empty".into(),
                });
            }
            let mut job = self.job.lock().await;
            let added = self
                .store
                .import_staged_file(
                    &mut job,
                    &PathBuf::from(source_path),
                    requested_name,
                    core_source_origin(origin),
                )
                .await
                .map_err(op_err)?;
            self.store.save(&job).await.map_err(op_err)?;
            job.prepare_selection(added.root_item_id)
                .await
                .map_err(op_err)?;
            self.store.save(&job).await.map_err(op_err)?;
            Ok(snapshot(&job))
        })
        .await
    }

    /// Attaches provider content that a trusted platform port has already
    /// stabilized in private storage. Provider completeness facts are retained
    /// so a copied partial directory cannot be mistaken for a complete source.
    pub async fn add_staged_provider_roots(
        &self,
        roots: Vec<FfiStagedProviderRootV2>,
    ) -> Result<FfiTransferJobSnapshotV2, EnvoixError> {
        on_ffi_runtime(async {
            if roots.is_empty()
                || roots.iter().any(|root| {
                    root.path.trim().is_empty() || root.requested_name.trim().is_empty()
                })
            {
                return Err(EnvoixError::Operation {
                    reason: "staged roots must contain a path and requested name".into(),
                });
            }
            let mut job = self.job.lock().await;
            for root in roots {
                job.add_provider_path(
                    PathBuf::from(root.path),
                    root.requested_name,
                    core_source_origin(root.origin),
                    root.issues.into_iter().map(core_provider_issue).collect(),
                )
                .await
                .map_err(op_err)?;
                self.store.save(&job).await.map_err(op_err)?;
            }
            Ok(snapshot(&job))
        })
        .await
    }

    pub async fn reauthorize_staged_provider_source(
        &self,
        root_item_id: u64,
        source_path: String,
        issues: Vec<FfiProviderSourceIssueV2>,
    ) -> Result<FfiTransferJobSnapshotV2, EnvoixError> {
        on_ffi_runtime(async {
            if source_path.trim().is_empty() {
                return Err(EnvoixError::Operation {
                    reason: "reauthorized provider path must not be empty".into(),
                });
            }
            let mut job = self.job.lock().await;
            job.reauthorize_provider_source(
                SourceItemId(root_item_id),
                PathBuf::from(source_path),
                issues.into_iter().map(core_provider_issue).collect(),
            )
            .await
            .map_err(op_err)?;
            self.store.save(&job).await.map_err(op_err)?;
            Ok(snapshot(&job))
        })
        .await
    }

    pub async fn resolve_source_issue(
        &self,
        root_item_id: u64,
        decision: FfiSourceDecisionV2,
        reauthorized_path: Option<String>,
    ) -> Result<FfiTransferJobSnapshotV2, EnvoixError> {
        on_ffi_runtime(async {
            let should_reprepare = decision == FfiSourceDecisionV2::Reauthorize;
            let decision = match decision {
                FfiSourceDecisionV2::Reauthorize => {
                    let path = reauthorized_path
                        .filter(|path| !path.trim().is_empty())
                        .ok_or_else(|| EnvoixError::Operation {
                            reason: "reauthorized_path is required for Reauthorize".into(),
                        })?;
                    SourceDecision::Reauthorize {
                        local_path: PathBuf::from(path),
                    }
                }
                FfiSourceDecisionV2::ApprovePartial => SourceDecision::ApprovePartial,
                FfiSourceDecisionV2::RemoveSelection => SourceDecision::RemoveSelection,
                FfiSourceDecisionV2::CancelJob => SourceDecision::CancelJob,
            };
            let mut job = self.job.lock().await;
            self.store
                .apply_source_decision(&mut job, SourceItemId(root_item_id), decision)
                .await
                .map_err(op_err)?;
            if should_reprepare {
                job.prepare_selection(SourceItemId(root_item_id))
                    .await
                    .map_err(op_err)?;
                self.store.save(&job).await.map_err(op_err)?;
            }
            Ok(snapshot(&job))
        })
        .await
    }

    pub async fn set_compression_policy(
        &self,
        policy: FfiCompressionPolicyV2,
    ) -> Result<FfiTransferJobSnapshotV2, EnvoixError> {
        on_ffi_runtime(async {
            let mut job = self.job.lock().await;
            job.set_compression_policy(core_compression_policy(policy))
                .map_err(op_err)?;
            self.store.save(&job).await.map_err(op_err)?;
            Ok(snapshot(&job))
        })
        .await
    }

    pub async fn cancel_job(&self) -> Result<FfiTransferJobSnapshotV2, EnvoixError> {
        on_ffi_runtime(async {
            let mut job = self.job.lock().await;
            job.cancel().map_err(op_err)?;
            self.store.save(&job).await.map_err(op_err)?;
            Ok(snapshot(&job))
        })
        .await
    }

    /// This explicit call is the Send boundary. It freezes the entry forest;
    /// the networking layer may consume the returned canonical offer afterwards.
    pub async fn seal_for_send(&self) -> Result<FfiManifestSealV2, EnvoixError> {
        on_ffi_runtime(async {
            let mut job = self.job.lock().await;
            if job.lifecycle() != JobLifecycle::Sealed {
                job.seal_for_send().map_err(op_err)?;
            }
            self.store.save(&job).await.map_err(op_err)?;
            let digest = job
                .structural_digest()
                .ok_or_else(|| EnvoixError::Operation {
                    reason: "sealed transfer job is missing its structural digest".into(),
                })?;
            let offer_bytes = job
                .sealed_offer_bytes()
                .ok_or_else(|| EnvoixError::Operation {
                    reason: "sealed transfer job is missing its canonical offer".into(),
                })?
                .to_vec();
            Ok(FfiManifestSealV2 {
                job_id: encode_job_id(job.job_id()),
                selection_revision: job.selection_revision(),
                structural_digest: digest.0.to_vec(),
                offer_bytes,
            })
        })
        .await
    }

    pub async fn list_roots(&self) -> Vec<FfiInventoryItemV2> {
        self.job
            .lock()
            .await
            .list_roots()
            .into_iter()
            .map(ffi_inventory_item)
            .collect()
    }

    pub async fn list_children(
        &self,
        parent_item_id: u64,
        cursor_revision: Option<u64>,
        cursor_offset: Option<u32>,
        limit: u32,
    ) -> Result<FfiInventoryPageV2, EnvoixError> {
        let cursor = match (cursor_revision, cursor_offset) {
            (None, None) => None,
            (Some(revision), Some(offset)) => Some(InventoryCursor { revision, offset }),
            _ => {
                return Err(EnvoixError::Operation {
                    reason: "cursor_revision and cursor_offset must be both present or absent"
                        .into(),
                });
            }
        };
        let limit = usize::try_from(limit).map_err(op_err)?;
        let page = self
            .job
            .lock()
            .await
            .list_children(SourceItemId(parent_item_id), cursor, limit)
            .map_err(op_err)?;
        Ok(FfiInventoryPageV2 {
            revision: page.revision,
            items: page.items.into_iter().map(ffi_inventory_item).collect(),
            next_offset: page.next_cursor.map(|cursor| cursor.offset),
        })
    }

    pub async fn get_item(&self, item_id: u64) -> Option<FfiInventoryItemV2> {
        self.job
            .lock()
            .await
            .get_item(SourceItemId(item_id))
            .map(ffi_inventory_item)
    }

    /// Returns a source path only for an explicit native preview/reveal action.
    pub async fn source_path_for_preview(&self, item_id: u64) -> Option<String> {
        self.job
            .lock()
            .await
            .local_path_for_item(SourceItemId(item_id))
            .map(|path| path.to_string_lossy().into_owned())
    }
}

#[uniffi::export]
pub async fn create_transfer_job_v2(
    store_directory: String,
    compression_policy: FfiCompressionPolicyV2,
) -> Result<Arc<FfiTransferJobV2>, EnvoixError> {
    on_ffi_runtime(async move {
        if store_directory.trim().is_empty() {
            return Err(EnvoixError::Operation {
                reason: "store_directory must not be empty".into(),
            });
        }
        let store = TransferJobStore::new(PathBuf::from(store_directory));
        let job = CanonicalTransferJob::new(core_compression_policy(compression_policy))
            .map_err(op_err)?;
        store.save(&job).await.map_err(op_err)?;
        Ok(Arc::new(FfiTransferJobV2 {
            job: Mutex::new(job),
            store,
        }))
    })
    .await
}

#[uniffi::export]
pub async fn restore_transfer_job_v2(
    store_directory: String,
    job_id: String,
) -> Result<Arc<FfiTransferJobV2>, EnvoixError> {
    on_ffi_runtime(async move {
        if store_directory.trim().is_empty() {
            return Err(EnvoixError::Operation {
                reason: "store_directory must not be empty".into(),
            });
        }
        let store = TransferJobStore::new(PathBuf::from(store_directory));
        let job_id = decode_job_id(&job_id)?;
        let mut job =
            store
                .load(job_id)
                .await
                .map_err(op_err)?
                .ok_or_else(|| EnvoixError::Operation {
                    reason: "transfer job was not found".into(),
                })?;
        store
            .reconcile_pending_cleanup(&mut job)
            .await
            .map_err(op_err)?;
        Ok(Arc::new(FfiTransferJobV2 {
            job: Mutex::new(job),
            store,
        }))
    })
    .await
}

/// Bounded durable index for native unsent-job restoration. Sealed/canceled
/// records remain on disk for their owning session/GC policy but are not
/// returned as editable preparations.
#[uniffi::export]
pub async fn list_preparing_transfer_jobs_v2(
    store_directory: String,
) -> Result<Vec<FfiTransferJobSnapshotV2>, EnvoixError> {
    on_ffi_runtime(async move {
        if store_directory.trim().is_empty() {
            return Err(EnvoixError::Operation {
                reason: "store_directory must not be empty".into(),
            });
        }
        let jobs = TransferJobStore::new(PathBuf::from(store_directory))
            .load_all()
            .await
            .map_err(op_err)?;
        Ok(jobs
            .iter()
            .filter(|job| {
                !job.source_selections().is_empty()
                    && matches!(
                        job.lifecycle(),
                        JobLifecycle::Preparing
                            | JobLifecycle::NeedsSourceDecision
                            | JobLifecycle::ReadyToSend
                    )
            })
            .map(snapshot)
            .collect())
    })
    .await
}

fn snapshot(job: &CanonicalTransferJob) -> FfiTransferJobSnapshotV2 {
    let summary = job.inventory_summary();
    FfiTransferJobSnapshotV2 {
        job_id: encode_job_id(job.job_id()),
        selection_revision: job.selection_revision(),
        state: ffi_job_state(job.lifecycle()),
        compression_policy: ffi_compression_policy(job.compression_policy()),
        created_unix_ms: job.created_unix_ms(),
        updated_unix_ms: job.updated_unix_ms(),
        inventory: FfiInventorySummaryV2 {
            root_count: summary.root_count,
            file_count: summary.file_count,
            directory_count: summary.directory_count,
            total_plaintext_bytes: summary.total_plaintext_bytes,
            warning_count: summary.warning_count,
        },
        selections: job
            .source_selections()
            .into_iter()
            .map(ffi_source_selection)
            .collect(),
    }
}

fn ffi_source_selection(selection: SourceSelectionInfo) -> FfiSourceSelectionV2 {
    FfiSourceSelectionV2 {
        root_item_id: selection.root_item_id.0,
        requested_name: selection.requested_name,
        state: match selection.state {
            SourceSelectionState::Pending => FfiSourceSelectionStateV2::Pending,
            SourceSelectionState::Enumerating => FfiSourceSelectionStateV2::Enumerating,
            SourceSelectionState::NeedsDecision => FfiSourceSelectionStateV2::NeedsDecision,
            SourceSelectionState::Ready => FfiSourceSelectionStateV2::Ready,
        },
        partial_approved: selection.partial_approved,
        issues: selection.issues.into_iter().map(ffi_source_issue).collect(),
    }
}

fn ffi_source_issue(issue: SourceIssue) -> FfiSourceIssueV2 {
    FfiSourceIssueV2 {
        issue_id: issue.issue_id,
        relative_components: issue.relative_components,
        kind: match issue.kind {
            SourceIssueKind::PermissionDenied => FfiSourceIssueKindV2::PermissionDenied,
            SourceIssueKind::Unavailable => FfiSourceIssueKindV2::Unavailable,
            SourceIssueKind::InvalidName => FfiSourceIssueKindV2::InvalidName,
            SourceIssueKind::SymbolicLink => FfiSourceIssueKindV2::SymbolicLink,
            SourceIssueKind::SpecialFile => FfiSourceIssueKindV2::SpecialFile,
            SourceIssueKind::SourceChanged => FfiSourceIssueKindV2::SourceChanged,
            SourceIssueKind::DepthLimit => FfiSourceIssueKindV2::DepthLimit,
            SourceIssueKind::EntryLimit => FfiSourceIssueKindV2::EntryLimit,
        },
    }
}

fn ffi_inventory_item(item: InventoryItem) -> FfiInventoryItemV2 {
    FfiInventoryItemV2 {
        item_id: item.item_id.0,
        root_item_id: item.root_item_id.0,
        parent_item_id: item.parent_item_id.map(|item_id| item_id.0),
        name: item.name,
        kind: match item.kind {
            ManifestEntryKindV2::RegularFile => FfiInventoryItemKindV2::File,
            ManifestEntryKindV2::Directory => FfiInventoryItemKindV2::Directory,
        },
        plaintext_size: item.plaintext_size,
        digest_known: item.digest_known,
        has_warning: item.has_warning,
    }
}

fn ffi_job_state(state: JobLifecycle) -> FfiTransferJobStateV2 {
    match state {
        JobLifecycle::Preparing => FfiTransferJobStateV2::Preparing,
        JobLifecycle::NeedsSourceDecision => FfiTransferJobStateV2::NeedsSourceDecision,
        JobLifecycle::ReadyToSend => FfiTransferJobStateV2::ReadyToSend,
        JobLifecycle::Sealed => FfiTransferJobStateV2::Sealed,
        JobLifecycle::Canceled => FfiTransferJobStateV2::Canceled,
    }
}

fn core_source_origin(origin: FfiSourceOriginV2) -> LocalSourceOrigin {
    match origin {
        FfiSourceOriginV2::Photos => LocalSourceOrigin::PhotosStaging,
        FfiSourceOriginV2::Share => LocalSourceOrigin::ShareStaging,
        FfiSourceOriginV2::ContentUri => LocalSourceOrigin::ContentUriStaging,
        FfiSourceOriginV2::FileProvider => LocalSourceOrigin::FileProviderStaging,
    }
}

fn core_provider_issue(issue: FfiProviderSourceIssueV2) -> ProviderSourceIssue {
    ProviderSourceIssue {
        relative_components: issue.relative_components,
        kind: match issue.kind {
            FfiProviderSourceIssueKindV2::PermissionDenied => SourceIssueKind::PermissionDenied,
            FfiProviderSourceIssueKindV2::Unavailable => SourceIssueKind::Unavailable,
            FfiProviderSourceIssueKindV2::InvalidName => SourceIssueKind::InvalidName,
            FfiProviderSourceIssueKindV2::SpecialFile => SourceIssueKind::SpecialFile,
        },
    }
}

fn core_compression_policy(policy: FfiCompressionPolicyV2) -> CompressionPolicyV2 {
    match policy {
        FfiCompressionPolicyV2::Never => CompressionPolicyV2::Never,
        FfiCompressionPolicyV2::Always => CompressionPolicyV2::Always,
        FfiCompressionPolicyV2::Smart => CompressionPolicyV2::Smart,
    }
}

fn ffi_compression_policy(policy: CompressionPolicyV2) -> FfiCompressionPolicyV2 {
    match policy {
        CompressionPolicyV2::Never => FfiCompressionPolicyV2::Never,
        CompressionPolicyV2::Always => FfiCompressionPolicyV2::Always,
        CompressionPolicyV2::Smart => FfiCompressionPolicyV2::Smart,
    }
}

fn encode_job_id(job_id: JobIdV2) -> String {
    job_id.0.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_job_id(value: &str) -> Result<JobIdV2, EnvoixError> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(EnvoixError::Operation {
            reason: "job_id must contain exactly 32 hexadecimal characters".into(),
        });
    }
    let mut bytes = [0_u8; 16];
    for (index, output) in bytes.iter_mut().enumerate() {
        let offset = index * 2;
        *output = u8::from_str_radix(&value[offset..offset + 2], 16).map_err(op_err)?;
    }
    if bytes == [0; 16] {
        return Err(EnvoixError::Operation {
            reason: "job_id must not be all zero".into(),
        });
    }
    Ok(JobIdV2(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_info_advertises_typed_staged_provider_jobs_in_ffi_v21() {
        let info = crate::envoix_core_info();
        assert_eq!(info.ffi_api_version, 21);
        assert!(
            info.capabilities
                .contains(&"typed_staged_provider_job_v1".to_string())
        );
    }

    #[test]
    fn staged_provider_issues_survive_the_typed_boundary_and_reauthorization() {
        let temporary = tempfile::tempdir().expect("temporary provider job store");
        let original = temporary.path().join("original");
        let replacement = temporary.path().join("replacement");
        std::fs::create_dir(&original).expect("create original root");
        std::fs::create_dir(&replacement).expect("create replacement root");
        std::fs::write(original.join("visible.txt"), b"visible").expect("write original");
        std::fs::write(replacement.join("complete.txt"), b"complete").expect("write replacement");

        crate::ffi_runtime().block_on(async {
            let job = create_transfer_job_v2(
                temporary.path().join("jobs").to_string_lossy().into_owned(),
                FfiCompressionPolicyV2::Smart,
            )
            .await
            .expect("create typed job");
            let snapshot = job
                .add_staged_provider_roots(vec![FfiStagedProviderRootV2 {
                    path: original.to_string_lossy().into_owned(),
                    requested_name: "Shared folder".into(),
                    origin: FfiSourceOriginV2::ContentUri,
                    issues: vec![FfiProviderSourceIssueV2 {
                        relative_components: vec!["hidden.txt".into()],
                        kind: FfiProviderSourceIssueKindV2::PermissionDenied,
                    }],
                }])
                .await
                .expect("attach staged provider root");

            assert_eq!(snapshot.state, FfiTransferJobStateV2::NeedsSourceDecision);
            assert_eq!(snapshot.selections.len(), 1);
            assert_eq!(snapshot.selections[0].issues.len(), 1);
            assert_eq!(
                snapshot.selections[0].issues[0].kind,
                FfiSourceIssueKindV2::PermissionDenied
            );

            let root_item_id = snapshot.selections[0].root_item_id;
            let repaired = job
                .reauthorize_staged_provider_source(
                    root_item_id,
                    replacement.to_string_lossy().into_owned(),
                    Vec::new(),
                )
                .await
                .expect("reauthorize staged provider root");
            assert_eq!(repaired.state, FfiTransferJobStateV2::ReadyToSend);
            assert_eq!(repaired.selections[0].root_item_id, root_item_id);
            assert!(repaired.selections[0].issues.is_empty());
        });
    }

    #[test]
    fn typed_seal_is_idempotent_after_a_lost_response() {
        let temporary = tempfile::tempdir().expect("temporary provider job store");
        let source = temporary.path().join("payload.txt");
        std::fs::write(&source, b"payload").expect("write source");

        crate::ffi_runtime().block_on(async {
            let job = create_transfer_job_v2(
                temporary.path().join("jobs").to_string_lossy().into_owned(),
                FfiCompressionPolicyV2::Never,
            )
            .await
            .expect("create typed job");
            job.add_staged_provider_roots(vec![FfiStagedProviderRootV2 {
                path: source.to_string_lossy().into_owned(),
                requested_name: "payload.txt".into(),
                origin: FfiSourceOriginV2::Share,
                issues: Vec::new(),
            }])
            .await
            .expect("attach staged file");

            let first = job.seal_for_send().await.expect("seal typed job");
            let repeated = job.seal_for_send().await.expect("repeat typed seal");
            assert_eq!(repeated.job_id, first.job_id);
            assert_eq!(repeated.selection_revision, first.selection_revision);
            assert_eq!(repeated.structural_digest, first.structural_digest);
            assert_eq!(repeated.offer_bytes, first.offer_bytes);
        });
    }
}
