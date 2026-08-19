//! Typed platform-owned Manifest-v2 destination boundary.
//!
//! Rust owns the verified private staging tree and delivery authority. A
//! platform implementation freezes public root names before Accept, then
//! commits those roots before receiver results or delivery proof are emitted.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use envoix_client::api::{
    DestinationDecisionV2, DestinationRequestV2, ManifestV2DataError, ManifestV2ResultGate,
    RootPlanV2, SavedEntryV2, SessionError, local_allocatable_bytes,
};
use envoix_error::TransferCause;
use envoix_protocol::manifest_v2::{
    MAX_MANIFEST_V2_COMPONENT_BYTES, ManifestEntryKindV2, ManifestV2,
};

use crate::FfiManifestEntryKindV2;

const RESERVED_DESTINATION_NAMES: [&str; 2] = [".envoix-staging-v2", ".envoix-reservations-v2"];

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiPlatformReceiveDestinationV2 {
    pub verified_staging_directory: String,
    pub verified_staging_allocatable_bytes: Option<u64>,
    pub exceptional_transfer_approved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiDestinationRootRequestV2 {
    pub root_id: u32,
    pub requested_name: String,
    pub kind: FfiManifestEntryKindV2,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiDestinationPlanRequestV2 {
    pub job_id: String,
    pub generation: u32,
    pub reserved_names: Vec<String>,
    pub roots: Vec<FfiDestinationRootRequestV2>,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiDestinationPlannedRootV2 {
    pub root_id: u32,
    pub planned_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiDestinationPlanReplyV2 {
    pub roots: Vec<FfiDestinationPlannedRootV2>,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiDestinationCommitRootV2 {
    pub root_id: u32,
    pub local_path: String,
    pub planned_name: String,
    pub kind: FfiManifestEntryKindV2,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiDestinationCommitRequestV2 {
    pub job_id: String,
    pub generation: u32,
    pub roots: Vec<FfiDestinationCommitRootV2>,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiDestinationSavedRootV2 {
    pub root_id: u32,
    pub final_name: String,
    pub uri: String,
}

#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct FfiDestinationCommitReplyV2 {
    pub roots: Vec<FfiDestinationSavedRootV2>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiManifestV2DestinationError {
    #[error("{reason}")]
    Operation { reason: String },
}

#[uniffi::export(with_foreign)]
#[async_trait]
pub trait ManifestV2PlatformDestination: Send + Sync {
    async fn plan(
        &self,
        request: FfiDestinationPlanRequestV2,
    ) -> Result<FfiDestinationPlanReplyV2, FfiManifestV2DestinationError>;

    async fn commit(
        &self,
        request: FfiDestinationCommitRequestV2,
    ) -> Result<FfiDestinationCommitReplyV2, FfiManifestV2DestinationError>;
}

pub(crate) struct PlatformDestinationGate {
    destination: Arc<dyn ManifestV2PlatformDestination>,
    verified_staging_directory: PathBuf,
    planned_roots: Vec<FfiDestinationPlannedRootV2>,
    committed_roots: Mutex<Option<Vec<FfiDestinationSavedRootV2>>>,
}

impl PlatformDestinationGate {
    pub(crate) fn committed_roots(&self) -> Result<Vec<FfiDestinationSavedRootV2>, SessionError> {
        self.committed_roots
            .lock()
            .map_err(|_| {
                SessionError::Storage("platform destination result lock is poisoned".into())
            })?
            .clone()
            .ok_or_else(|| {
                SessionError::Storage("platform destination did not commit final roots".into())
            })
    }
}

#[async_trait]
impl ManifestV2ResultGate for PlatformDestinationGate {
    async fn commit_results(
        &self,
        manifest: &ManifestV2,
        saved_entries: &mut [SavedEntryV2],
    ) -> Result<(), ManifestV2DataError> {
        let planned_by_root = self
            .planned_roots
            .iter()
            .map(|root| (root.root_id, root))
            .collect::<BTreeMap<_, _>>();
        let roots = manifest
            .roots
            .iter()
            .map(|root| {
                let planned = planned_by_root.get(&root.root_id).ok_or_else(|| {
                    destination_contract("platform destination plan omitted a root")
                })?;
                let saved = saved_entries
                    .get(root.root_entry_id as usize)
                    .ok_or_else(|| destination_contract("root result is missing before save"))?;
                let private_name = saved
                    .final_component_override
                    .as_deref()
                    .unwrap_or(root.requested_name.as_str());
                let entry = manifest
                    .entries
                    .get(root.root_entry_id as usize)
                    .ok_or_else(|| destination_contract("manifest root entry is missing"))?;
                Ok(FfiDestinationCommitRootV2 {
                    root_id: root.root_id,
                    local_path: self
                        .verified_staging_directory
                        .join(private_name)
                        .to_string_lossy()
                        .into_owned(),
                    planned_name: planned.planned_name.clone(),
                    kind: ffi_entry_kind(entry.kind),
                })
            })
            .collect::<Result<Vec<_>, ManifestV2DataError>>()?;
        let reply = self
            .destination
            .commit(FfiDestinationCommitRequestV2 {
                job_id: encode_job_id(manifest),
                generation: manifest.generation,
                roots,
            })
            .await
            .map_err(|error| {
                destination_contract(format!("platform destination save failed: {error}"))
            })?;
        validate_committed_roots(manifest, &self.planned_roots, &reply.roots)?;
        for root in &manifest.roots {
            let saved = reply
                .roots
                .iter()
                .find(|saved| saved.root_id == root.root_id)
                .expect("validated platform destination result contains every root");
            saved_entries[root.root_entry_id as usize].final_component_override =
                Some(saved.final_name.clone());
        }
        *self.committed_roots.lock().map_err(|_| {
            ManifestV2DataError::Internal("platform destination result lock is poisoned".into())
        })? = Some(reply.roots);
        Ok(())
    }
}

pub(crate) async fn prepare_platform_destination(
    manifest: &ManifestV2,
    request: FfiPlatformReceiveDestinationV2,
    destination: Arc<dyn ManifestV2PlatformDestination>,
) -> Result<(DestinationRequestV2, PlatformDestinationGate), SessionError> {
    if request.verified_staging_directory.trim().is_empty() {
        return Err(destination_unavailable(
            "verified staging directory must not be empty",
        ));
    }
    let staging_directory = PathBuf::from(request.verified_staging_directory);
    let actual_capacity = local_allocatable_bytes(&staging_directory)
        .map_err(|error| destination_unavailable(error.to_string()))?;
    let capacity = request
        .verified_staging_allocatable_bytes
        .map_or(actual_capacity, |reported| reported.min(actual_capacity));
    let plan = destination
        .plan(destination_plan_request(manifest))
        .await
        .map_err(|error| {
            destination_unavailable(format!("platform destination plan failed: {error}"))
        })?;
    validate_planned_roots(manifest, &plan.roots)
        .map_err(|error| destination_unavailable(error.to_string()))?;
    let core_request = DestinationRequestV2 {
        target_directory: staging_directory.clone(),
        copy_staging_directory: None,
        decision: DestinationDecisionV2::UseDirectSave,
        target_allocatable_bytes: Some(capacity),
        staging_allocatable_bytes: None,
        stable_object_identity: false,
        exceptional_transfer_approved: request.exceptional_transfer_approved,
        preplanned_root_names: Some(
            plan.roots
                .iter()
                .map(|root| RootPlanV2 {
                    root_id: root.root_id,
                    planned_name: root.planned_name.clone(),
                })
                .collect(),
        ),
    };
    let gate = PlatformDestinationGate {
        destination,
        verified_staging_directory: staging_directory,
        planned_roots: plan.roots,
        committed_roots: Mutex::new(None),
    };
    Ok((core_request, gate))
}

fn destination_plan_request(manifest: &ManifestV2) -> FfiDestinationPlanRequestV2 {
    FfiDestinationPlanRequestV2 {
        job_id: encode_job_id(manifest),
        generation: manifest.generation,
        reserved_names: RESERVED_DESTINATION_NAMES.map(str::to_string).to_vec(),
        roots: manifest
            .roots
            .iter()
            .map(|root| {
                let entry = &manifest.entries[root.root_entry_id as usize];
                FfiDestinationRootRequestV2 {
                    root_id: root.root_id,
                    requested_name: root.requested_name.clone(),
                    kind: ffi_entry_kind(entry.kind),
                }
            })
            .collect(),
    }
}

fn validate_planned_roots(
    manifest: &ManifestV2,
    roots: &[FfiDestinationPlannedRootV2],
) -> Result<(), ManifestV2DataError> {
    let mut names = HashSet::new();
    if roots.len() != manifest.roots.len()
        || roots.iter().enumerate().any(|(index, root)| {
            root.root_id != index as u32
                || !valid_component(&root.planned_name)
                || !names.insert(root.planned_name.to_lowercase())
        })
    {
        return Err(destination_contract(
            "platform destination returned an invalid root name plan",
        ));
    }
    Ok(())
}

fn validate_committed_roots(
    manifest: &ManifestV2,
    planned_roots: &[FfiDestinationPlannedRootV2],
    saved_roots: &[FfiDestinationSavedRootV2],
) -> Result<(), ManifestV2DataError> {
    if saved_roots.len() != manifest.roots.len() {
        return Err(destination_contract(
            "platform destination did not save every root",
        ));
    }
    let saved_by_root = saved_roots
        .iter()
        .map(|root| (root.root_id, root))
        .collect::<BTreeMap<_, _>>();
    if saved_by_root.len() != manifest.roots.len() {
        return Err(destination_contract(
            "platform destination returned duplicate roots",
        ));
    }
    for root in &manifest.roots {
        let saved = saved_by_root
            .get(&root.root_id)
            .ok_or_else(|| destination_contract("platform destination omitted a manifest root"))?;
        let planned = planned_roots
            .get(root.root_id as usize)
            .filter(|planned| planned.root_id == root.root_id)
            .ok_or_else(|| destination_contract("platform destination plan omitted a root"))?;
        if !valid_component(&saved.final_name) || saved.uri.trim().is_empty() {
            return Err(destination_contract(
                "platform destination returned an invalid final name or URI",
            ));
        }
        if saved.final_name != planned.planned_name {
            return Err(destination_contract(
                "platform destination changed a frozen public name",
            ));
        }
    }
    Ok(())
}

fn ffi_entry_kind(kind: ManifestEntryKindV2) -> FfiManifestEntryKindV2 {
    match kind {
        ManifestEntryKindV2::RegularFile => FfiManifestEntryKindV2::File,
        ManifestEntryKindV2::Directory => FfiManifestEntryKindV2::Directory,
    }
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.len() <= MAX_MANIFEST_V2_COMPONENT_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
}

fn destination_contract(message: impl Into<String>) -> ManifestV2DataError {
    ManifestV2DataError::DestinationContract(message.into())
}

fn destination_unavailable(message: impl Into<String>) -> SessionError {
    SessionError::Cause {
        cause: TransferCause::ReceiverDestinationUnavailable,
        detail: message.into(),
    }
}

fn encode_job_id(manifest: &ManifestV2) -> String {
    manifest
        .job_id
        .0
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use envoix_client::api::SavedEntryV2;
    use envoix_protocol::manifest_v2::{
        CompressionPolicyV2, ContentDigestV2, EntryContentDigestV2, JobIdV2, ManifestEntryV2,
        ManifestRootV2, ManifestTotalsV2, SourceCompletenessV2,
    };
    use envoix_protocol::manifest_v2_frames::EntryResultKindV2;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn core_info_advertises_platform_destination_in_ffi_v21() {
        let info = crate::envoix_core_info();

        assert_eq!(info.ffi_api_version, 21);
        assert!(
            info.capabilities
                .iter()
                .any(|capability| capability == "platform_manifest_v2_destination_v1")
        );
    }

    struct RecordingDestination {
        plan: FfiDestinationPlanReplyV2,
        commit: FfiDestinationCommitReplyV2,
        plan_request: StdMutex<Option<FfiDestinationPlanRequestV2>>,
        commit_request: StdMutex<Option<FfiDestinationCommitRequestV2>>,
    }

    #[async_trait]
    impl ManifestV2PlatformDestination for RecordingDestination {
        async fn plan(
            &self,
            request: FfiDestinationPlanRequestV2,
        ) -> Result<FfiDestinationPlanReplyV2, FfiManifestV2DestinationError> {
            *self.plan_request.lock().unwrap() = Some(request);
            Ok(self.plan.clone())
        }

        async fn commit(
            &self,
            request: FfiDestinationCommitRequestV2,
        ) -> Result<FfiDestinationCommitReplyV2, FfiManifestV2DestinationError> {
            *self.commit_request.lock().unwrap() = Some(request);
            Ok(self.commit.clone())
        }
    }

    #[tokio::test]
    async fn typed_platform_destination_freezes_and_commits_public_names() {
        let staging = tempdir().unwrap();
        let destination = Arc::new(RecordingDestination {
            plan: plan("report (1).txt"),
            commit: commit("report (1).txt"),
            plan_request: StdMutex::new(None),
            commit_request: StdMutex::new(None),
        });
        let manifest = manifest();

        let (request, gate) = prepare_platform_destination(
            &manifest,
            platform_request(staging.path().to_string_lossy().into_owned()),
            destination.clone(),
        )
        .await
        .unwrap();

        assert_eq!(request.target_directory, staging.path());
        assert!(!request.stable_object_identity);
        assert_eq!(
            request.preplanned_root_names.as_ref().unwrap()[0].planned_name,
            "report (1).txt"
        );
        let planned = destination.plan_request.lock().unwrap().clone().unwrap();
        assert_eq!(planned.job_id, "21212121212121212121212121212121");
        assert_eq!(planned.roots[0].requested_name, "report.txt");
        assert_eq!(planned.roots[0].kind, FfiManifestEntryKindV2::File);

        let mut saved_entries = vec![SavedEntryV2 {
            entry_id: 0,
            result: EntryResultKindV2::Saved,
            final_component_override: Some("report (1).txt".into()),
        }];
        gate.commit_results(&manifest, &mut saved_entries)
            .await
            .unwrap();

        let committed = destination.commit_request.lock().unwrap().clone().unwrap();
        assert_eq!(committed.roots[0].planned_name, "report (1).txt");
        assert_eq!(
            committed.roots[0].local_path,
            staging.path().join("report (1).txt").to_string_lossy()
        );
        assert_eq!(
            gate.committed_roots().unwrap(),
            commit("report (1).txt").roots
        );
        assert_eq!(
            saved_entries[0].final_component_override.as_deref(),
            Some("report (1).txt")
        );
    }

    #[tokio::test]
    async fn duplicate_public_names_fail_before_accept() {
        let staging = tempdir().unwrap();
        let destination = Arc::new(RecordingDestination {
            plan: FfiDestinationPlanReplyV2 {
                roots: vec![
                    FfiDestinationPlannedRootV2 {
                        root_id: 0,
                        planned_name: "report.txt".into(),
                    },
                    FfiDestinationPlannedRootV2 {
                        root_id: 1,
                        planned_name: "REPORT.TXT".into(),
                    },
                ],
            },
            commit: commit("report.txt"),
            plan_request: StdMutex::new(None),
            commit_request: StdMutex::new(None),
        });
        let mut manifest = manifest();
        manifest.roots.push(ManifestRootV2 {
            root_id: 1,
            root_entry_id: 1,
            requested_name: "second.txt".into(),
            completeness: SourceCompletenessV2::Complete,
        });
        manifest.entries.push(ManifestEntryV2 {
            entry_id: 1,
            root_id: 1,
            parent_entry_id: None,
            component: "second.txt".into(),
            kind: ManifestEntryKindV2::RegularFile,
            plaintext_size: 1,
            content_digest: EntryContentDigestV2::Known(ContentDigestV2([2; 32])),
        });
        manifest.totals.file_count = 2;
        manifest.totals.total_plaintext_bytes = 2;

        let result = prepare_platform_destination(
            &manifest,
            platform_request(staging.path().to_string_lossy().into_owned()),
            destination,
        )
        .await;
        let Err(error) = result else {
            panic!("duplicate public roots must fail");
        };

        assert!(matches!(
            error,
            SessionError::Cause {
                cause: TransferCause::ReceiverDestinationUnavailable,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn final_save_cannot_change_the_frozen_name() {
        let staging = tempdir().unwrap();
        let destination = Arc::new(RecordingDestination {
            plan: plan("report.txt"),
            commit: commit("renamed.txt"),
            plan_request: StdMutex::new(None),
            commit_request: StdMutex::new(None),
        });
        let manifest = manifest();
        let (_, gate) = prepare_platform_destination(
            &manifest,
            platform_request(staging.path().to_string_lossy().into_owned()),
            destination,
        )
        .await
        .unwrap();
        let mut saved_entries = vec![SavedEntryV2 {
            entry_id: 0,
            result: EntryResultKindV2::Saved,
            final_component_override: None,
        }];

        let error = gate
            .commit_results(&manifest, &mut saved_entries)
            .await
            .expect_err("final name mutation must fail");

        assert!(matches!(error, ManifestV2DataError::DestinationContract(_)));
        assert!(gate.committed_roots().is_err());
    }

    fn platform_request(directory: String) -> FfiPlatformReceiveDestinationV2 {
        FfiPlatformReceiveDestinationV2 {
            verified_staging_directory: directory,
            verified_staging_allocatable_bytes: Some(u64::MAX),
            exceptional_transfer_approved: true,
        }
    }

    fn plan(name: &str) -> FfiDestinationPlanReplyV2 {
        FfiDestinationPlanReplyV2 {
            roots: vec![FfiDestinationPlannedRootV2 {
                root_id: 0,
                planned_name: name.into(),
            }],
        }
    }

    fn commit(name: &str) -> FfiDestinationCommitReplyV2 {
        FfiDestinationCommitReplyV2 {
            roots: vec![FfiDestinationSavedRootV2 {
                root_id: 0,
                final_name: name.into(),
                uri: "content://downloads/report".into(),
            }],
        }
    }

    fn manifest() -> ManifestV2 {
        ManifestV2 {
            job_id: JobIdV2([0x21; 16]),
            generation: 1,
            selection_revision: 1,
            compression_policy: CompressionPolicyV2::Never,
            roots: vec![ManifestRootV2 {
                root_id: 0,
                root_entry_id: 0,
                requested_name: "report.txt".into(),
                completeness: SourceCompletenessV2::Complete,
            }],
            entries: vec![ManifestEntryV2 {
                entry_id: 0,
                root_id: 0,
                parent_entry_id: None,
                component: "report.txt".into(),
                kind: ManifestEntryKindV2::RegularFile,
                plaintext_size: 1,
                content_digest: EntryContentDigestV2::Known(ContentDigestV2([1; 32])),
            }],
            totals: ManifestTotalsV2 {
                file_count: 1,
                directory_count: 0,
                total_plaintext_bytes: 1,
            },
        }
    }
}
