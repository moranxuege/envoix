//! Receiver-owned local destination planning and same-storage staging.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::ffi::CString;

use async_trait::async_trait;
use envoix_protocol::manifest_v2::{
    ContentDigestV2, JobIdV2, ManifestEntryKindV2, ManifestEntryV2, ManifestOfferV2, ManifestV2,
    build_manifest_offer_v2,
};
use envoix_protocol::manifest_v2_frames::{
    EntryBlockV2, EntryCompletionChoiceV2, EntryDispositionV2, EntryPlanV2, EntryResultKindV2,
    EntryStartV2, JobGenerationV2, ManifestAcceptV2, ProofCapabilityV2, RootPlanV2,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::{ManifestV2DataError, ManifestV2PayloadSink, SavedEntryV2, VerifiedEntryV2};

pub const POST_SAVE_RESERVE_BYTES: u64 = 64 * 1024 * 1024;
pub const AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_FINALIZATION_NAME_REPLANS: u32 = 32;
const LOCAL_SAVE_LEDGER_SCHEMA_VERSION: u16 = 1;
static NEXT_RENAME_PROBE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DestinationModeV2 {
    DirectSave,
    CopyAfterVerify,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DestinationDecisionV2 {
    UseDirectSave,
    ContinueWithCopyAfterVerify,
}

/// Returns bytes currently allocatable by an ordinary user on the storage
/// domain containing `path`. The provider must already have authorized and
/// created the directory; an unknown result is never treated as unlimited.
pub fn local_allocatable_bytes(path: &Path) -> Result<u64, DestinationPlanErrorV2> {
    let stats = rustix::fs::statvfs(path)?;
    stats
        .f_bavail
        .checked_mul(stats.f_frsize)
        .ok_or(DestinationPlanErrorV2::SpaceOverflow)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StorageDomainIdentityV2 {
    pub provider: String,
    pub opaque_volume_id: String,
    pub stable_object_identity: bool,
}

#[derive(Clone)]
pub struct DestinationRequestV2 {
    pub target_directory: PathBuf,
    pub copy_staging_directory: Option<PathBuf>,
    pub decision: DestinationDecisionV2,
    pub target_allocatable_bytes: Option<u64>,
    pub staging_allocatable_bytes: Option<u64>,
    pub stable_object_identity: bool,
    pub exceptional_transfer_approved: bool,
}

impl fmt::Debug for DestinationRequestV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DestinationRequestV2")
            .field("target_directory", &"<redacted>")
            .field(
                "copy_staging_directory",
                &self.copy_staging_directory.as_ref().map(|_| "<redacted>"),
            )
            .field("decision", &self.decision)
            .field("target_allocatable_bytes", &self.target_allocatable_bytes)
            .field("staging_allocatable_bytes", &self.staging_allocatable_bytes)
            .field("stable_object_identity", &self.stable_object_identity)
            .field(
                "exceptional_transfer_approved",
                &self.exceptional_transfer_approved,
            )
            .finish()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationWritePlanV2 {
    pub job_id: JobIdV2,
    pub generation: u32,
    pub mode: DestinationModeV2,
    pub storage_domain: StorageDomainIdentityV2,
    pub plan_revision: u32,
    pub root_plans: Vec<RootPlanV2>,
    pub exceptional_transfer_approved: bool,
    target_directory: PathBuf,
    staging_directory: PathBuf,
    reservations: Vec<PathBuf>,
    reuse_entry_ids: Vec<u32>,
}

impl fmt::Debug for DestinationWritePlanV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DestinationWritePlanV2")
            .field("job_id", &self.job_id)
            .field("generation", &self.generation)
            .field("mode", &self.mode)
            .field("storage_domain", &self.storage_domain)
            .field("plan_revision", &self.plan_revision)
            .field("root_plan_count", &self.root_plans.len())
            .field("target_directory", &"<redacted>")
            .field("staging_directory", &"<redacted>")
            .field("reservation_count", &self.reservations.len())
            .finish()
    }
}

#[derive(Debug, Error)]
pub enum DestinationPlanErrorV2 {
    #[error("destination requires explicit CopyAfterVerify approval")]
    CopyDecisionRequired,
    #[error("CopyAfterVerify requires an app-owned staging directory")]
    MissingCopyStaging,
    #[error("destination provider cannot guarantee no-overwrite finalization")]
    UnsupportedProvider,
    #[error("destination capacity is unknown")]
    UnknownCapacity,
    #[error("exceptional transfer requires receiver approval before payload")]
    ExceptionalTransferApprovalRequired,
    #[error("destination requires {required} bytes but only {available} are allocatable")]
    InsufficientSpace { required: u64, available: u64 },
    #[error("destination space projection overflowed")]
    SpaceOverflow,
    #[error("destination name space is exhausted")]
    NameExhausted,
    #[error("destination plan no longer owns its reserved name")]
    ReservationLost,
    #[error("an external object took the reserved destination name")]
    LateCollision,
    #[error("destination namespace remained contended after bounded replanning")]
    DestinationContended,
    #[error("destination source object changed during reuse")]
    ReusedObjectLost,
    #[error("destination entry state is invalid")]
    InvalidEntryState,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl DestinationWritePlanV2 {
    pub async fn create(
        offer: &ManifestOfferV2,
        mut request: DestinationRequestV2,
    ) -> Result<Self, DestinationPlanErrorV2> {
        fs::create_dir_all(&request.target_directory).await?;
        request.target_directory = fs::canonicalize(&request.target_directory).await?;
        if let Some(staging) = &request.copy_staging_directory {
            fs::create_dir_all(staging).await?;
            request.copy_staging_directory = Some(fs::canonicalize(staging).await?);
        }
        let target_available = request
            .target_allocatable_bytes
            .ok_or(DestinationPlanErrorV2::UnknownCapacity)?;
        let exceptional_transfer = offer.manifest.totals.total_plaintext_bytes
            > AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES
            || offer.manifest.totals.total_plaintext_bytes > target_available / 2;
        if exceptional_transfer && !request.exceptional_transfer_approved {
            return Err(DestinationPlanErrorV2::ExceptionalTransferApprovalRequired);
        }
        let direct_supported = exclusive_rename_supported()
            && exclusive_rename_probe(&request.target_directory).await?;
        let mode = match request.decision {
            DestinationDecisionV2::UseDirectSave if direct_supported => {
                DestinationModeV2::DirectSave
            }
            DestinationDecisionV2::UseDirectSave => {
                return Err(DestinationPlanErrorV2::CopyDecisionRequired);
            }
            DestinationDecisionV2::ContinueWithCopyAfterVerify => {
                DestinationModeV2::CopyAfterVerify
            }
        };
        if mode == DestinationModeV2::CopyAfterVerify && !direct_supported {
            // Copy still ends in a destination-local no-overwrite rename.
            return Err(DestinationPlanErrorV2::UnsupportedProvider);
        }
        let copy_staging_same_domain = if mode == DestinationModeV2::CopyAfterVerify {
            let staging = request
                .copy_staging_directory
                .as_ref()
                .ok_or(DestinationPlanErrorV2::MissingCopyStaging)?;
            fs::create_dir_all(staging).await?;
            same_storage_domain(&request.target_directory, staging).await?
        } else {
            false
        };
        validate_space(&offer.manifest, &request, mode, copy_staging_same_domain)?;
        let job_hex = encode_job_id(offer.manifest.job_id);
        let staging_directory = match mode {
            DestinationModeV2::DirectSave => request
                .target_directory
                .join(".envoix-staging-v2")
                .join(&job_hex),
            DestinationModeV2::CopyAfterVerify => request
                .copy_staging_directory
                .ok_or(DestinationPlanErrorV2::MissingCopyStaging)?
                .join("envoix-staging-v2")
                .join(&job_hex),
        };
        fs::create_dir_all(&staging_directory).await?;
        restrict_private_directory(&staging_directory).await?;
        let reservation_directory = request.target_directory.join(".envoix-reservations-v2");
        fs::create_dir_all(&reservation_directory).await?;
        restrict_private_directory(&reservation_directory).await?;
        let mut occupied_names = destination_names(&request.target_directory).await?;
        let mut root_plans = Vec::with_capacity(offer.manifest.roots.len());
        let mut reservations = Vec::with_capacity(offer.manifest.roots.len());
        for root in &offer.manifest.roots {
            let base = provider_safe_component(&root.requested_name);
            let (planned_name, reservation) = reserve_keep_both_name(
                &reservation_directory,
                &base,
                &mut occupied_names,
                offer.manifest.job_id,
                root.root_id,
            )
            .await?;
            root_plans.push(RootPlanV2 {
                root_id: root.root_id,
                planned_name,
            });
            reservations.push(reservation);
        }
        let reuse_entry_ids = discover_known_reuse_entries(
            offer,
            &request.target_directory,
            request.stable_object_identity,
        )
        .await?;
        Ok(Self {
            job_id: offer.manifest.job_id,
            generation: offer.manifest.generation,
            mode,
            storage_domain: local_storage_domain(
                &request.target_directory,
                request.stable_object_identity,
            )
            .await?,
            plan_revision: 1,
            root_plans,
            exceptional_transfer_approved: exceptional_transfer,
            target_directory: request.target_directory,
            staging_directory,
            reservations,
            reuse_entry_ids,
        })
    }

    pub fn target_path_for_root(&self, root_id: u32) -> Option<PathBuf> {
        self.root_plans
            .get(root_id as usize)
            .filter(|plan| plan.root_id == root_id)
            .map(|plan| self.target_directory.join(&plan.planned_name))
    }

    pub async fn validate_resume_request(
        &self,
        offer: &ManifestOfferV2,
        request: &DestinationRequestV2,
    ) -> Result<(), DestinationPlanErrorV2> {
        self.validate_shape()?;
        let target = fs::canonicalize(&request.target_directory).await?;
        let mode_matches = matches!(
            (self.mode, request.decision),
            (
                DestinationModeV2::DirectSave,
                DestinationDecisionV2::UseDirectSave
            ) | (
                DestinationModeV2::CopyAfterVerify,
                DestinationDecisionV2::ContinueWithCopyAfterVerify
            )
        );
        if self.job_id != offer.manifest.job_id
            || self.generation != offer.manifest.generation
            || self.root_plans.len() != offer.manifest.roots.len()
            || self.target_directory != target
            || self.storage_domain.stable_object_identity != request.stable_object_identity
            || !mode_matches
            || self.exceptional_transfer_approved && !request.exceptional_transfer_approved
        {
            return Err(DestinationPlanErrorV2::InvalidEntryState);
        }
        if self.mode == DestinationModeV2::CopyAfterVerify {
            let staging = request
                .copy_staging_directory
                .as_ref()
                .ok_or(DestinationPlanErrorV2::MissingCopyStaging)?;
            let staging = fs::canonicalize(staging).await?;
            if !self
                .staging_directory
                .starts_with(staging.join("envoix-staging-v2"))
            {
                return Err(DestinationPlanErrorV2::InvalidEntryState);
            }
        }
        let current_domain = local_storage_domain(
            &self.target_directory,
            self.storage_domain.stable_object_identity,
        )
        .await?;
        if current_domain != self.storage_domain {
            return Err(DestinationPlanErrorV2::InvalidEntryState);
        }
        let copy_staging_same_domain = if self.mode == DestinationModeV2::CopyAfterVerify {
            same_storage_domain(
                &self.target_directory,
                request
                    .copy_staging_directory
                    .as_deref()
                    .ok_or(DestinationPlanErrorV2::MissingCopyStaging)?,
            )
            .await?
        } else {
            false
        };
        validate_space(
            &offer.manifest,
            request,
            self.mode,
            copy_staging_same_domain,
        )
    }

    fn validate_shape(&self) -> Result<(), DestinationPlanErrorV2> {
        let unique_root_names = self
            .root_plans
            .iter()
            .map(|root| name_equivalence_key(&root.planned_name))
            .collect::<HashSet<_>>();
        let unique_reservations = self.reservations.iter().collect::<HashSet<_>>();
        if self.job_id.0 == [0; 16]
            || self.generation == 0
            || self.plan_revision == 0
            || self.root_plans.is_empty()
            || self.root_plans.len() != self.reservations.len()
            || unique_root_names.len() != self.root_plans.len()
            || unique_reservations.len() != self.reservations.len()
            || self
                .reuse_entry_ids
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || self.root_plans.iter().enumerate().any(|(index, root)| {
                root.root_id != index as u32
                    || root.planned_name.is_empty()
                    || provider_safe_component(&root.planned_name) != root.planned_name
            })
        {
            return Err(DestinationPlanErrorV2::InvalidEntryState);
        }
        let reservation_directory = self.target_directory.join(".envoix-reservations-v2");
        if self.reservations.iter().any(|reservation| {
            reservation.parent() != Some(reservation_directory.as_path())
                || reservation.extension().and_then(|value| value.to_str()) != Some("reservation")
        }) {
            return Err(DestinationPlanErrorV2::InvalidEntryState);
        }
        if self.mode == DestinationModeV2::DirectSave
            && !self
                .staging_directory
                .starts_with(self.target_directory.join(".envoix-staging-v2"))
        {
            return Err(DestinationPlanErrorV2::InvalidEntryState);
        }
        Ok(())
    }

    pub fn create_initial_accept(
        &self,
        offer: &ManifestOfferV2,
    ) -> Result<ManifestAcceptV2, DestinationPlanErrorV2> {
        if self.job_id != offer.manifest.job_id
            || self.generation != offer.manifest.generation
            || self.root_plans.len() != offer.manifest.roots.len()
            || self.reuse_entry_ids.iter().any(|entry_id| {
                let Some(entry) = offer.manifest.entries.get(*entry_id as usize) else {
                    return true;
                };
                let root = &offer.manifest.roots[entry.root_id as usize];
                entry.entry_id != *entry_id
                    || entry.entry_id != root.root_entry_id
                    || entry.kind != ManifestEntryKindV2::RegularFile
                    || !matches!(
                        entry.content_digest,
                        envoix_protocol::manifest_v2::EntryContentDigestV2::Known(_)
                    )
            })
        {
            return Err(DestinationPlanErrorV2::InvalidEntryState);
        }
        let mut accept_nonce = [0_u8; 32];
        let mut proof_capability = [0_u8; 32];
        getrandom::fill(&mut accept_nonce)
            .map_err(|_| std::io::Error::other("receiver entropy unavailable"))?;
        getrandom::fill(&mut proof_capability)
            .map_err(|_| std::io::Error::other("receiver entropy unavailable"))?;
        if proof_capability == [0; 32] {
            return Err(std::io::Error::other("receiver entropy unavailable").into());
        }
        Ok(ManifestAcceptV2 {
            identity: JobGenerationV2 {
                job_id: self.job_id,
                generation: self.generation,
            },
            manifest_digest: offer.structural_digest,
            accept_nonce,
            proof_capability: ProofCapabilityV2(proof_capability),
            plan_revision: self.plan_revision,
            root_plans: self.root_plans.clone(),
            entry_plans: offer
                .manifest
                .entries
                .iter()
                .map(|entry| EntryPlanV2 {
                    entry_id: entry.entry_id,
                    disposition: if self.reuse_entry_ids.binary_search(&entry.entry_id).is_ok() {
                        EntryDispositionV2::ReuseExisting
                    } else {
                        EntryDispositionV2::ReceivePayload
                    },
                    next_plaintext_block: 0,
                })
                .collect(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct DestinationPlanStoreV2 {
    directory: PathBuf,
}

impl DestinationPlanStoreV2 {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub async fn save(&self, plan: &DestinationWritePlanV2) -> Result<(), DestinationPlanErrorV2> {
        plan.validate_shape()?;
        fs::create_dir_all(&self.directory).await?;
        restrict_private_directory(&self.directory).await?;
        let final_path = self.path(plan.job_id, plan.generation);
        let temporary_path = final_path.with_extension("tmp");
        let bytes = serde_json::to_vec(plan)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        crate::persistence_v2::replace_file(temporary_path, final_path).await?;
        Ok(())
    }

    pub async fn load(
        &self,
        job_id: JobIdV2,
        generation: u32,
    ) -> Result<Option<DestinationWritePlanV2>, DestinationPlanErrorV2> {
        let bytes = match fs::read(self.path(job_id, generation)).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let plan: DestinationWritePlanV2 = serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        plan.validate_shape()?;
        Ok(Some(plan))
    }

    fn path(&self, job_id: JobIdV2, generation: u32) -> PathBuf {
        self.directory.join(format!(
            "destination-{}-{generation}.json",
            encode_job_id(job_id)
        ))
    }
}

struct OpenPayload {
    file: fs::File,
    hasher: blake3::Hasher,
    bytes: u64,
}

struct ReuseObject {
    file: fs::File,
    size: u64,
    digest: ContentDigestV2,
    final_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum SavedRootResultV2 {
    Saved,
    ReusedExisting,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum RootSaveStateV2 {
    Pending,
    FinalizeIntent {
        expected_object: ObjectIdentityV2,
        plan_revision: u32,
        planned_name: String,
        reservation: PathBuf,
        idempotency_key: [u8; 16],
    },
    Saved {
        result: SavedRootResultV2,
        final_name: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum ObjectIdentityV2 {
    Stable { volume: u64, object: u64 },
    ExactContent { root_digest: ContentDigestV2 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalSaveLedgerV2 {
    schema_version: u16,
    job_id: JobIdV2,
    generation: u32,
    manifest_digest: ContentDigestV2,
    roots: Vec<RootSaveStateV2>,
}

pub struct LocalDestinationProviderV2 {
    plan: DestinationWritePlanV2,
    manifest: ManifestV2,
    entry_paths: Vec<PathBuf>,
    entry_overrides: Vec<Option<String>>,
    payloads: HashMap<u32, OpenPayload>,
    reuse_objects: HashMap<u32, ReuseObject>,
    save_ledger_path: PathBuf,
    plan_store: DestinationPlanStoreV2,
    save_ledger: LocalSaveLedgerV2,
}

impl fmt::Debug for LocalDestinationProviderV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalDestinationProviderV2")
            .field("plan", &self.plan)
            .field("entry_count", &self.manifest.entries.len())
            .field("open_payload_count", &self.payloads.len())
            .field("reuse_count", &self.reuse_objects.len())
            .finish()
    }
}

impl LocalDestinationProviderV2 {
    pub async fn new(
        mut plan: DestinationWritePlanV2,
        manifest: ManifestV2,
    ) -> Result<Self, DestinationPlanErrorV2> {
        plan.validate_shape()?;
        if plan.job_id != manifest.job_id
            || plan.generation != manifest.generation
            || plan.root_plans.len() != manifest.roots.len()
            || plan.reuse_entry_ids.iter().any(|entry_id| {
                let Some(entry) = manifest.entries.get(*entry_id as usize) else {
                    return true;
                };
                let root = &manifest.roots[entry.root_id as usize];
                entry.entry_id != *entry_id
                    || entry.entry_id != root.root_entry_id
                    || entry.kind != ManifestEntryKindV2::RegularFile
                    || !matches!(
                        entry.content_digest,
                        envoix_protocol::manifest_v2::EntryContentDigestV2::Known(_)
                    )
            })
        {
            return Err(DestinationPlanErrorV2::InvalidEntryState);
        }
        let current_domain = local_storage_domain(
            &plan.target_directory,
            plan.storage_domain.stable_object_identity,
        )
        .await?;
        if current_domain != plan.storage_domain {
            return Err(DestinationPlanErrorV2::InvalidEntryState);
        }
        if fs::canonicalize(&plan.target_directory).await? != plan.target_directory {
            return Err(DestinationPlanErrorV2::InvalidEntryState);
        }
        let plan_store =
            DestinationPlanStoreV2::new(plan.target_directory.join(".envoix-ledgers-v2"));
        if let Some(durable_plan) = plan_store.load(plan.job_id, plan.generation).await? {
            if durable_plan.plan_revision < plan.plan_revision
                || durable_plan.storage_domain != plan.storage_domain
                || durable_plan.plan_revision == plan.plan_revision && durable_plan != plan
            {
                return Err(DestinationPlanErrorV2::InvalidEntryState);
            }
            plan = durable_plan;
        } else {
            plan_store.save(&plan).await?;
        }
        fs::create_dir_all(&plan.staging_directory).await?;
        if fs::canonicalize(&plan.staging_directory).await? != plan.staging_directory {
            return Err(DestinationPlanErrorV2::InvalidEntryState);
        }
        restrict_private_directory(&plan.staging_directory).await?;
        let (entry_paths, entry_overrides) = build_entry_paths(&plan, &manifest)?;
        let save_ledger_path = plan
            .target_directory
            .join(".envoix-ledgers-v2")
            .join(format!(
                "save-{}-{}.json",
                encode_job_id(plan.job_id),
                plan.generation
            ));
        let manifest_digest = build_manifest_offer_v2(manifest.clone())
            .map_err(|_| DestinationPlanErrorV2::InvalidEntryState)?
            .structural_digest;
        let save_ledger = match fs::read(&save_ledger_path).await {
            Ok(bytes) => serde_json::from_slice::<LocalSaveLedgerV2>(&bytes)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => LocalSaveLedgerV2 {
                schema_version: LOCAL_SAVE_LEDGER_SCHEMA_VERSION,
                job_id: plan.job_id,
                generation: plan.generation,
                manifest_digest,
                roots: vec![RootSaveStateV2::Pending; manifest.roots.len()],
            },
            Err(error) => return Err(error.into()),
        };
        if save_ledger.schema_version != LOCAL_SAVE_LEDGER_SCHEMA_VERSION
            || save_ledger.job_id != plan.job_id
            || save_ledger.generation != plan.generation
            || save_ledger.manifest_digest != manifest_digest
            || save_ledger.roots.len() != manifest.roots.len()
            || save_ledger.roots.iter().any(|state| match state {
                RootSaveStateV2::Pending => false,
                RootSaveStateV2::FinalizeIntent {
                    plan_revision,
                    planned_name,
                    reservation,
                    idempotency_key,
                    ..
                } => {
                    *plan_revision == 0
                        || planned_name.is_empty()
                        || reservation.extension().and_then(|value| value.to_str())
                            != Some("reservation")
                        || *idempotency_key == [0; 16]
                }
                RootSaveStateV2::Saved { final_name, .. } => {
                    final_name.is_empty()
                        || provider_safe_component(final_name) != final_name.as_str()
                }
            })
        {
            return Err(DestinationPlanErrorV2::InvalidEntryState);
        }
        Ok(Self {
            plan,
            manifest,
            entry_paths,
            entry_overrides,
            payloads: HashMap::new(),
            reuse_objects: HashMap::new(),
            save_ledger_path,
            plan_store,
            save_ledger,
        })
    }

    pub fn plan(&self) -> &DestinationWritePlanV2 {
        &self.plan
    }

    /// Reconciles durable block boundaries with receiver-owned staging before
    /// advertising ResumeStatus. A missing/inconsistent incomplete payload is
    /// reset to block zero; an already-finalized root is left for save-intent
    /// adoption and is never downloaded again.
    pub async fn reconcile_resume(
        &self,
        ledger: &mut crate::ReceiverDataPlaneLedgerV2,
        store: &crate::ReceiverDataPlaneStoreV2,
    ) -> Result<(), ManifestV2DataError> {
        for (entry_id, next_plaintext_block, plaintext_bytes, payload_complete) in
            ledger.pending_payload_boundaries()
        {
            if next_plaintext_block == 0 && !payload_complete {
                continue;
            }
            let entry = &self.manifest.entries[entry_id as usize];
            let root_state = &self.save_ledger.roots[entry.root_id as usize];
            if matches!(
                root_state,
                RootSaveStateV2::Saved { .. } | RootSaveStateV2::FinalizeIntent { .. }
            ) && fs::try_exists(
                self.plan
                    .target_path_for_root(entry.root_id)
                    .ok_or(DestinationPlanErrorV2::InvalidEntryState)?,
            )
            .await?
            {
                continue;
            }
            let path = &self.entry_paths[entry_id as usize];
            let boundary_is_owned = match fs::metadata(path).await {
                Ok(metadata) => metadata.is_file() && metadata.len() == plaintext_bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            };
            if boundary_is_owned {
                continue;
            }
            match fs::remove_file(path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            ledger
                .reset_payload_checkpoint(&self.manifest, entry_id, store)
                .await?;
        }
        Ok(())
    }

    async fn try_open_reuse(
        &self,
        entry: &ManifestEntryV2,
        digest: ContentDigestV2,
    ) -> Result<Option<ReuseObject>, DestinationPlanErrorV2> {
        if !self.plan.storage_domain.stable_object_identity {
            return Ok(None);
        }
        let root = &self.manifest.roots[entry.root_id as usize];
        if entry.entry_id != root.root_entry_id || entry.kind != ManifestEntryKindV2::RegularFile {
            return Ok(None);
        }
        let candidate_name = provider_safe_component(&root.requested_name);
        let candidate = self.plan.target_directory.join(&candidate_name);
        let metadata = match fs::metadata(&candidate).await {
            Ok(metadata) if metadata.is_file() && metadata.len() == entry.plaintext_size => {
                metadata
            }
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut file = fs::File::open(&candidate).await?;
        let candidate_digest = hash_open_file(&mut file).await?;
        if candidate_digest != digest {
            return Ok(None);
        }
        file.seek(std::io::SeekFrom::Start(0)).await?;
        Ok(Some(ReuseObject {
            file,
            size: metadata.len(),
            digest,
            final_name: candidate_name,
        }))
    }

    async fn prepare_finalization_source(
        &self,
        root_id: u32,
    ) -> Result<PathBuf, DestinationPlanErrorV2> {
        let source = self.root_staging_path(root_id);
        if self.plan.mode != DestinationModeV2::CopyAfterVerify {
            return Ok(source);
        }
        let destination_local = self
            .plan
            .target_directory
            .join(".envoix-staging-v2")
            .join(encode_job_id(self.plan.job_id))
            .join(format!("copy-root-{root_id}"));
        if !fs::try_exists(&destination_local).await? {
            copy_tree(&source, &destination_local).await?;
        }
        Ok(destination_local)
    }

    fn finalization_source_path(&self, root_id: u32) -> PathBuf {
        if self.plan.mode == DestinationModeV2::CopyAfterVerify {
            self.plan
                .target_directory
                .join(".envoix-staging-v2")
                .join(encode_job_id(self.plan.job_id))
                .join(format!("copy-root-{root_id}"))
        } else {
            self.root_staging_path(root_id)
        }
    }

    async fn finalize_root_source(
        &self,
        root_id: u32,
        source: &Path,
    ) -> Result<(), DestinationPlanErrorV2> {
        let final_path = self
            .plan
            .target_path_for_root(root_id)
            .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
        if !fs::try_exists(&self.plan.reservations[root_id as usize]).await? {
            return Err(DestinationPlanErrorV2::ReservationLost);
        }
        match exclusive_rename(source, &final_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(DestinationPlanErrorV2::LateCollision)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn root_staging_path(&self, root_id: u32) -> PathBuf {
        self.plan.staging_directory.join(format!("root-{root_id}"))
    }

    async fn persist_new_save_intent(
        &mut self,
        root_id: u32,
        expected_object: ObjectIdentityV2,
    ) -> Result<(), DestinationPlanErrorV2> {
        let index = root_id as usize;
        let root_plan = self
            .plan
            .root_plans
            .get(index)
            .filter(|plan| plan.root_id == root_id)
            .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
        let reservation = self
            .plan
            .reservations
            .get(index)
            .ok_or(DestinationPlanErrorV2::InvalidEntryState)?
            .clone();
        let mut idempotency_key = [0_u8; 16];
        getrandom::fill(&mut idempotency_key)
            .map_err(|_| std::io::Error::other("receiver entropy unavailable"))?;
        self.save_ledger.roots[index] = RootSaveStateV2::FinalizeIntent {
            expected_object,
            plan_revision: self.plan.plan_revision,
            planned_name: root_plan.planned_name.clone(),
            reservation,
            idempotency_key,
        };
        self.save_effect_ledger().await
    }

    fn current_intent_object(
        &self,
        root_id: u32,
    ) -> Result<ObjectIdentityV2, DestinationPlanErrorV2> {
        let index = root_id as usize;
        match self.save_ledger.roots.get(index) {
            Some(RootSaveStateV2::FinalizeIntent {
                expected_object,
                plan_revision,
                planned_name,
                reservation,
                idempotency_key,
            }) if *plan_revision == self.plan.plan_revision
                && planned_name == &self.plan.root_plans[index].planned_name
                && reservation == &self.plan.reservations[index]
                && *idempotency_key != [0; 16] =>
            {
                Ok(expected_object.clone())
            }
            _ => Err(DestinationPlanErrorV2::InvalidEntryState),
        }
    }

    async fn replan_root_name(&mut self, root_id: u32) -> Result<(), DestinationPlanErrorV2> {
        let index = root_id as usize;
        let root = self
            .manifest
            .roots
            .get(index)
            .filter(|root| root.root_id == root_id)
            .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
        let mut occupied_names = destination_names(&self.plan.target_directory).await?;
        let reservation_directory = self.plan.target_directory.join(".envoix-reservations-v2");
        let (planned_name, reservation) = reserve_keep_both_name(
            &reservation_directory,
            &provider_safe_component(&root.requested_name),
            &mut occupied_names,
            self.plan.job_id,
            root_id,
        )
        .await?;
        let previous_reservation = self.plan.reservations[index].clone();
        self.plan.plan_revision = self
            .plan
            .plan_revision
            .checked_add(1)
            .ok_or(DestinationPlanErrorV2::SpaceOverflow)?;
        self.plan.root_plans[index].planned_name = planned_name.clone();
        self.plan.reservations[index] = reservation;
        self.plan_store.save(&self.plan).await?;
        self.entry_overrides[root.root_entry_id as usize] =
            (planned_name != root.requested_name).then_some(planned_name);
        match fs::remove_file(previous_reservation).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    async fn save_effect_ledger(&self) -> Result<(), DestinationPlanErrorV2> {
        let parent = self
            .save_ledger_path
            .parent()
            .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
        fs::create_dir_all(parent).await?;
        restrict_private_directory(parent).await?;
        let temporary = self.save_ledger_path.with_extension("tmp");
        let bytes = serde_json::to_vec(&self.save_ledger)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        crate::persistence_v2::replace_file(temporary, self.save_ledger_path.clone()).await?;
        Ok(())
    }
}

#[async_trait]
impl ManifestV2PayloadSink for LocalDestinationProviderV2 {
    async fn begin_entry(
        &mut self,
        entry: &ManifestEntryV2,
        start: EntryStartV2,
        next_plaintext_block: u64,
    ) -> Result<(), ManifestV2DataError> {
        if entry.kind != ManifestEntryKindV2::RegularFile {
            return Err(DestinationPlanErrorV2::InvalidEntryState.into());
        }
        let path = self.entry_paths[entry.entry_id as usize].clone();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let expected_bytes = next_plaintext_block
            .checked_mul(start.plaintext_block_bytes as u64)
            .ok_or(ManifestV2DataError::ArithmeticOverflow)?
            .min(entry.plaintext_size);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .await?;
        let metadata = file.metadata().await?;
        if metadata.len() != expected_bytes {
            return Err(DestinationPlanErrorV2::InvalidEntryState.into());
        }
        let mut hasher = blake3::Hasher::new();
        let mut remaining = expected_bytes;
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
        file.seek(std::io::SeekFrom::Start(0)).await?;
        while remaining > 0 {
            let length = remaining.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..length]).await?;
            hasher.update(&buffer[..length]);
            remaining -= length as u64;
        }
        file.seek(std::io::SeekFrom::Start(expected_bytes)).await?;
        self.payloads.insert(
            entry.entry_id,
            OpenPayload {
                file,
                hasher,
                bytes: expected_bytes,
            },
        );
        Ok(())
    }

    async fn write_block(
        &mut self,
        entry: &ManifestEntryV2,
        block: &EntryBlockV2,
    ) -> Result<(), ManifestV2DataError> {
        let payload = self
            .payloads
            .get_mut(&entry.entry_id)
            .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
        if payload.bytes != block.plaintext_offset
            || block.plaintext_length as usize != block.encoded_bytes.len()
        {
            return Err(DestinationPlanErrorV2::InvalidEntryState.into());
        }
        payload.file.write_all(&block.encoded_bytes).await?;
        payload.file.sync_data().await?;
        payload.hasher.update(&block.encoded_bytes);
        payload.bytes = payload
            .bytes
            .checked_add(block.plaintext_length as u64)
            .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
        Ok(())
    }

    async fn try_choose_reuse(
        &mut self,
        entry: &ManifestEntryV2,
        digest: ContentDigestV2,
    ) -> Result<bool, ManifestV2DataError> {
        let Some(reuse) = self.try_open_reuse(entry, digest).await? else {
            return Ok(false);
        };
        self.payloads.remove(&entry.entry_id);
        let staging_path = &self.entry_paths[entry.entry_id as usize];
        match fs::remove_file(staging_path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        self.reuse_objects.insert(entry.entry_id, reuse);
        Ok(true)
    }

    async fn verify_payload(
        &mut self,
        entry: &ManifestEntryV2,
        final_digest: ContentDigestV2,
    ) -> Result<(), ManifestV2DataError> {
        let payload = self
            .payloads
            .get_mut(&entry.entry_id)
            .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
        payload.file.sync_all().await?;
        if payload.bytes != entry.plaintext_size
            || ContentDigestV2(*payload.hasher.clone().finalize().as_bytes()) != final_digest
        {
            return Err(ManifestV2DataError::FinalMismatch);
        }
        Ok(())
    }

    async fn stage_directory(
        &mut self,
        entry: &ManifestEntryV2,
    ) -> Result<(), ManifestV2DataError> {
        let path = &self.entry_paths[entry.entry_id as usize];
        fs::create_dir_all(path).await?;
        Ok(())
    }

    async fn commit_job(
        &mut self,
        manifest: &ManifestV2,
        verified_entries: &[VerifiedEntryV2],
    ) -> Result<Vec<SavedEntryV2>, ManifestV2DataError> {
        if manifest.job_id != self.manifest.job_id
            || verified_entries.len() != manifest.entries.len()
        {
            return Err(DestinationPlanErrorV2::InvalidEntryState.into());
        }
        for verified in verified_entries {
            if verified.completion_choice == EntryCompletionChoiceV2::ReuseChosen {
                let reuse = self
                    .reuse_objects
                    .get_mut(&verified.entry_id)
                    .ok_or(DestinationPlanErrorV2::ReusedObjectLost)?;
                let metadata = reuse.file.metadata().await?;
                reuse.file.seek(std::io::SeekFrom::Start(0)).await?;
                if metadata.len() != reuse.size
                    || hash_open_file(&mut reuse.file).await? != reuse.digest
                {
                    return Err(DestinationPlanErrorV2::ReusedObjectLost.into());
                }
            }
        }
        self.payloads.clear();
        for root in &manifest.roots {
            let root_verified = verified_entries[root.root_entry_id as usize];
            if root_verified.completion_choice == EntryCompletionChoiceV2::ReuseChosen {
                let reuse = self
                    .reuse_objects
                    .get(&root.root_entry_id)
                    .ok_or(DestinationPlanErrorV2::ReusedObjectLost)?;
                if !matches!(
                    self.save_ledger.roots[root.root_id as usize],
                    RootSaveStateV2::Saved {
                        result: SavedRootResultV2::ReusedExisting,
                        ..
                    }
                ) {
                    self.save_ledger.roots[root.root_id as usize] = RootSaveStateV2::Saved {
                        result: SavedRootResultV2::ReusedExisting,
                        final_name: reuse.final_name.clone(),
                    };
                    self.save_effect_ledger().await?;
                }
                continue;
            }
            if matches!(
                self.save_ledger.roots[root.root_id as usize],
                RootSaveStateV2::Saved {
                    result: SavedRootResultV2::Saved,
                    ..
                }
            ) {
                continue;
            }
            let expected_root_digest =
                root_content_digest(manifest, verified_entries, root.root_id)?;
            if matches!(
                self.save_ledger.roots[root.root_id as usize],
                RootSaveStateV2::Pending
            ) {
                let source = self.prepare_finalization_source(root.root_id).await?;
                sync_tree_directories(&source).await?;
                let expected_object = object_identity(
                    &source,
                    expected_root_digest,
                    self.plan.storage_domain.stable_object_identity,
                )
                .await?;
                self.persist_new_save_intent(root.root_id, expected_object)
                    .await?;
            } else {
                let refresh_object = match &self.save_ledger.roots[root.root_id as usize] {
                    RootSaveStateV2::FinalizeIntent {
                        expected_object,
                        plan_revision,
                        ..
                    } if *plan_revision < self.plan.plan_revision => Some(expected_object.clone()),
                    RootSaveStateV2::FinalizeIntent { .. } => {
                        self.current_intent_object(root.root_id)?;
                        None
                    }
                    _ => return Err(DestinationPlanErrorV2::InvalidEntryState.into()),
                };
                if let Some(expected_object) = refresh_object {
                    self.persist_new_save_intent(root.root_id, expected_object)
                        .await?;
                }
            }
            for replan_attempt in 0..=MAX_FINALIZATION_NAME_REPLANS {
                let expected_object = self.current_intent_object(root.root_id)?;
                let source = self.finalization_source_path(root.root_id);
                let final_path = self
                    .plan
                    .target_path_for_root(root.root_id)
                    .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
                let outcome = if fs::try_exists(&source).await? {
                    if object_identity(
                        &source,
                        expected_root_digest,
                        self.plan.storage_domain.stable_object_identity,
                    )
                    .await?
                        != expected_object
                    {
                        return Err(DestinationPlanErrorV2::InvalidEntryState.into());
                    }
                    self.finalize_root_source(root.root_id, &source).await
                } else if fs::try_exists(&final_path).await? {
                    verify_adopted_root(
                        &final_path,
                        &expected_object,
                        manifest,
                        verified_entries,
                        root.root_id,
                        &self.entry_overrides,
                        self.plan.storage_domain.stable_object_identity,
                    )
                    .await
                } else if self.plan.mode == DestinationModeV2::CopyAfterVerify {
                    let source = self.prepare_finalization_source(root.root_id).await?;
                    sync_tree_directories(&source).await?;
                    let expected_object = object_identity(
                        &source,
                        expected_root_digest,
                        self.plan.storage_domain.stable_object_identity,
                    )
                    .await?;
                    self.persist_new_save_intent(root.root_id, expected_object)
                        .await?;
                    self.finalize_root_source(root.root_id, &source).await
                } else {
                    return Err(DestinationPlanErrorV2::InvalidEntryState.into());
                };
                match outcome {
                    Ok(()) => break,
                    Err(DestinationPlanErrorV2::LateCollision)
                        if replan_attempt < MAX_FINALIZATION_NAME_REPLANS =>
                    {
                        let expected_object = self.current_intent_object(root.root_id)?;
                        self.replan_root_name(root.root_id).await?;
                        self.persist_new_save_intent(root.root_id, expected_object)
                            .await?;
                    }
                    Err(DestinationPlanErrorV2::LateCollision) => {
                        return Err(DestinationPlanErrorV2::DestinationContended.into());
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let final_name = self.plan.root_plans[root.root_id as usize]
                .planned_name
                .clone();
            self.save_ledger.roots[root.root_id as usize] = RootSaveStateV2::Saved {
                result: SavedRootResultV2::Saved,
                final_name,
            };
            self.save_effect_ledger().await?;
        }
        let mut results = Vec::with_capacity(manifest.entries.len());
        for (entry, verified) in manifest.entries.iter().zip(verified_entries) {
            let result = if verified.completion_choice == EntryCompletionChoiceV2::ReuseChosen {
                EntryResultKindV2::ReusedExisting
            } else {
                EntryResultKindV2::Saved
            };
            let root = &manifest.roots[entry.root_id as usize];
            let override_name = if entry.entry_id == root.root_entry_id {
                match &self.save_ledger.roots[root.root_id as usize] {
                    RootSaveStateV2::Saved { final_name, .. } => {
                        (final_name != &entry.component).then(|| final_name.clone())
                    }
                    _ => return Err(DestinationPlanErrorV2::InvalidEntryState.into()),
                }
            } else {
                self.entry_overrides[entry.entry_id as usize].clone()
            };
            results.push(SavedEntryV2 {
                entry_id: entry.entry_id,
                result,
                final_component_override: override_name,
            });
        }
        for reservation in &self.plan.reservations {
            match fs::remove_file(reservation).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(results)
    }

    async fn retire_payload(&mut self, entry: &ManifestEntryV2) -> Result<(), ManifestV2DataError> {
        self.payloads.remove(&entry.entry_id);
        let path = &self.entry_paths[entry.entry_id as usize];
        match fs::remove_file(path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
}

fn validate_space(
    manifest: &ManifestV2,
    request: &DestinationRequestV2,
    mode: DestinationModeV2,
    copy_staging_same_domain: bool,
) -> Result<(), DestinationPlanErrorV2> {
    let plaintext = manifest.totals.total_plaintext_bytes;
    let target_payload = if mode == DestinationModeV2::CopyAfterVerify && copy_staging_same_domain {
        plaintext
            .checked_mul(2)
            .ok_or(DestinationPlanErrorV2::SpaceOverflow)?
    } else {
        plaintext
    };
    let target_required = target_payload
        .checked_add(POST_SAVE_RESERVE_BYTES)
        .ok_or(DestinationPlanErrorV2::SpaceOverflow)?;
    let target_available = request
        .target_allocatable_bytes
        .ok_or(DestinationPlanErrorV2::UnknownCapacity)?;
    if target_available < target_required {
        return Err(DestinationPlanErrorV2::InsufficientSpace {
            required: target_required,
            available: target_available,
        });
    }
    if mode == DestinationModeV2::CopyAfterVerify && !copy_staging_same_domain {
        let staging_available = request
            .staging_allocatable_bytes
            .ok_or(DestinationPlanErrorV2::UnknownCapacity)?;
        if staging_available < target_required {
            return Err(DestinationPlanErrorV2::InsufficientSpace {
                required: target_required,
                available: staging_available,
            });
        }
    }
    Ok(())
}

fn build_entry_paths(
    plan: &DestinationWritePlanV2,
    manifest: &ManifestV2,
) -> Result<(Vec<PathBuf>, Vec<Option<String>>), DestinationPlanErrorV2> {
    let mut paths = Vec::with_capacity(manifest.entries.len());
    let mut overrides = Vec::with_capacity(manifest.entries.len());
    let mut occupied: BTreeMap<Option<u32>, HashSet<String>> = BTreeMap::new();
    for entry in &manifest.entries {
        let root = &manifest.roots[entry.root_id as usize];
        if entry.entry_id == root.root_entry_id {
            paths.push(
                plan.staging_directory
                    .join(format!("root-{}", root.root_id)),
            );
            overrides.push(
                (plan.root_plans[root.root_id as usize].planned_name != root.requested_name)
                    .then(|| plan.root_plans[root.root_id as usize].planned_name.clone()),
            );
            continue;
        }
        let parent_id = entry
            .parent_entry_id
            .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
        let parent = paths
            .get(parent_id as usize)
            .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
        let safe = provider_safe_component(&entry.component);
        let siblings = occupied.entry(Some(parent_id)).or_default();
        let final_name = allocate_in_memory_name(&safe, siblings)?;
        paths.push(parent.join(&final_name));
        overrides.push((final_name != entry.component).then_some(final_name));
    }
    Ok((paths, overrides))
}

async fn destination_names(directory: &Path) -> Result<HashSet<String>, std::io::Error> {
    let mut names = HashSet::new();
    let mut entries = fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        if let Some(name) = entry.file_name().to_str() {
            names.insert(name_equivalence_key(name));
        }
    }
    Ok(names)
}

async fn discover_known_reuse_entries(
    offer: &ManifestOfferV2,
    target_directory: &Path,
    stable_object_identity: bool,
) -> Result<Vec<u32>, DestinationPlanErrorV2> {
    if !stable_object_identity {
        return Ok(Vec::new());
    }
    let mut reusable = Vec::new();
    for root in &offer.manifest.roots {
        let entry = &offer.manifest.entries[root.root_entry_id as usize];
        let envoix_protocol::manifest_v2::EntryContentDigestV2::Known(expected_digest) =
            entry.content_digest
        else {
            continue;
        };
        if entry.kind != ManifestEntryKindV2::RegularFile {
            continue;
        }
        let path = target_directory.join(provider_safe_component(&root.requested_name));
        let metadata = match fs::symlink_metadata(&path).await {
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() == entry.plaintext_size =>
            {
                metadata
            }
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let mut file = fs::File::open(&path).await?;
        if file.metadata().await?.len() == metadata.len()
            && hash_open_file(&mut file).await? == expected_digest
        {
            reusable.push(entry.entry_id);
        }
    }
    Ok(reusable)
}

async fn reserve_keep_both_name(
    reservation_directory: &Path,
    base: &str,
    occupied: &mut HashSet<String>,
    job_id: JobIdV2,
    root_id: u32,
) -> Result<(String, PathBuf), DestinationPlanErrorV2> {
    for suffix in 0_u32..10_000 {
        let candidate = if suffix == 0 {
            base.to_owned()
        } else {
            component_with_suffix(base, suffix)
        };
        let key = name_equivalence_key(&candidate);
        if occupied.contains(&key) {
            continue;
        }
        let reservation_key = blake3::hash(key.as_bytes()).to_hex().to_string();
        let reservation = reservation_directory.join(format!("{reservation_key}.reservation"));
        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&reservation)
            .await
        {
            Ok(mut file) => {
                file.write_all(format!("{}:{root_id}", encode_job_id(job_id)).as_bytes())
                    .await?;
                file.sync_all().await?;
                occupied.insert(key);
                return Ok((candidate, reservation));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(DestinationPlanErrorV2::NameExhausted)
}

fn allocate_in_memory_name(
    base: &str,
    occupied: &mut HashSet<String>,
) -> Result<String, DestinationPlanErrorV2> {
    for suffix in 0_u32..10_000 {
        let candidate = if suffix == 0 {
            base.to_owned()
        } else {
            component_with_suffix(base, suffix)
        };
        if occupied.insert(name_equivalence_key(&candidate)) {
            return Ok(candidate);
        }
    }
    Err(DestinationPlanErrorV2::NameExhausted)
}

fn provider_safe_component(component: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        let mut value = component
            .chars()
            .map(|character| {
                if matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                ) {
                    '_'
                } else {
                    character
                }
            })
            .collect::<String>();
        while value.ends_with(' ') || value.ends_with('.') {
            value.pop();
        }
        let stem = value
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || stem
                .strip_prefix("COM")
                .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
            || stem
                .strip_prefix("LPT")
                .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
        {
            value.insert(0, '_');
        }
        if value.is_empty() { "_".into() } else { value }
    }
    #[cfg(not(target_os = "windows"))]
    {
        component.to_owned()
    }
}

fn component_with_suffix(base: &str, suffix: u32) -> String {
    let suffix = format!(" ({suffix})");
    let maximum_base_bytes = 255_usize.saturating_sub(suffix.len());
    let mut end = base.len().min(maximum_base_bytes);
    while !base.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &base[..end], suffix)
}

fn name_equivalence_key(component: &str) -> String {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        component.to_lowercase()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        component.to_owned()
    }
}

async fn hash_open_file(file: &mut fs::File) -> Result<ContentDigestV2, std::io::Error> {
    file.seek(std::io::SeekFrom::Start(0)).await?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(ContentDigestV2(*hasher.finalize().as_bytes()))
}

fn root_content_digest(
    manifest: &ManifestV2,
    verified_entries: &[VerifiedEntryV2],
    root_id: u32,
) -> Result<ContentDigestV2, DestinationPlanErrorV2> {
    let mut hasher = blake3::Hasher::new_derive_key("envoix/manifest/v2/root-content");
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.root_id == root_id)
    {
        let verified = verified_entries
            .get(entry.entry_id as usize)
            .filter(|verified| verified.entry_id == entry.entry_id)
            .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
        hasher.update(&entry.entry_id.to_be_bytes());
        hasher.update(&[entry.kind as u8]);
        hasher.update(&entry.plaintext_size.to_be_bytes());
        match verified.final_digest {
            Some(digest) => {
                hasher.update(&[1]);
                hasher.update(&digest.0);
            }
            None => {
                hasher.update(&[0]);
            }
        }
    }
    Ok(ContentDigestV2(*hasher.finalize().as_bytes()))
}

async fn object_identity(
    path: &Path,
    exact_content: ContentDigestV2,
    stable_object_identity: bool,
) -> Result<ObjectIdentityV2, DestinationPlanErrorV2> {
    let metadata = fs::symlink_metadata(path).await?;
    if metadata.file_type().is_symlink() {
        return Err(DestinationPlanErrorV2::InvalidEntryState);
    }
    if !stable_object_identity {
        return Ok(ObjectIdentityV2::ExactContent {
            root_digest: exact_content,
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return Ok(ObjectIdentityV2::Stable {
            volume: metadata.dev(),
            object: metadata.ino(),
        });
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        if let (Some(volume), Some(object)) =
            (metadata.volume_serial_number(), metadata.file_index())
        {
            return Ok(ObjectIdentityV2::Stable {
                volume: volume as u64,
                object,
            });
        }
    }
    Ok(ObjectIdentityV2::ExactContent {
        root_digest: exact_content,
    })
}

async fn verify_adopted_root(
    final_path: &Path,
    expected_object: &ObjectIdentityV2,
    manifest: &ManifestV2,
    verified_entries: &[VerifiedEntryV2],
    root_id: u32,
    entry_overrides: &[Option<String>],
    stable_object_identity: bool,
) -> Result<(), DestinationPlanErrorV2> {
    match expected_object {
        ObjectIdentityV2::Stable { .. } => {
            if &object_identity(
                final_path,
                root_content_digest(manifest, verified_entries, root_id)?,
                stable_object_identity,
            )
            .await?
                != expected_object
            {
                return Err(DestinationPlanErrorV2::LateCollision);
            }
        }
        ObjectIdentityV2::ExactContent { root_digest } => {
            verify_exact_root(
                final_path,
                manifest,
                verified_entries,
                root_id,
                entry_overrides,
            )
            .await?;
            if &root_content_digest(manifest, verified_entries, root_id)? != root_digest {
                return Err(DestinationPlanErrorV2::LateCollision);
            }
        }
    }
    Ok(())
}

async fn verify_exact_root(
    final_root: &Path,
    manifest: &ManifestV2,
    verified_entries: &[VerifiedEntryV2],
    root_id: u32,
    entry_overrides: &[Option<String>],
) -> Result<(), DestinationPlanErrorV2> {
    let root = manifest
        .roots
        .get(root_id as usize)
        .filter(|root| root.root_id == root_id)
        .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
    let mut physical_paths = vec![PathBuf::new(); manifest.entries.len()];
    let mut expected_paths = HashSet::new();
    physical_paths[root.root_entry_id as usize] = final_root.to_path_buf();
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.root_id == root_id)
    {
        let path = if entry.entry_id == root.root_entry_id {
            final_root.to_path_buf()
        } else {
            let parent = entry
                .parent_entry_id
                .and_then(|parent| physical_paths.get(parent as usize))
                .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
            parent.join(
                entry_overrides[entry.entry_id as usize]
                    .as_deref()
                    .unwrap_or(&entry.component),
            )
        };
        let metadata = fs::symlink_metadata(&path).await?;
        if metadata.file_type().is_symlink()
            || entry.kind == ManifestEntryKindV2::Directory && !metadata.is_dir()
            || entry.kind == ManifestEntryKindV2::RegularFile
                && (!metadata.is_file() || metadata.len() != entry.plaintext_size)
        {
            return Err(DestinationPlanErrorV2::LateCollision);
        }
        if entry.kind == ManifestEntryKindV2::RegularFile {
            let expected = verified_entries[entry.entry_id as usize]
                .final_digest
                .ok_or(DestinationPlanErrorV2::InvalidEntryState)?;
            let mut file = fs::File::open(&path).await?;
            if hash_open_file(&mut file).await? != expected {
                return Err(DestinationPlanErrorV2::LateCollision);
            }
        }
        expected_paths.insert(path.clone());
        physical_paths[entry.entry_id as usize] = path;
    }
    if manifest.entries[root.root_entry_id as usize].kind == ManifestEntryKindV2::Directory {
        let mut pending = vec![final_root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let mut children = fs::read_dir(&directory).await?;
            while let Some(child) = children.next_entry().await? {
                let path = child.path();
                let metadata = fs::symlink_metadata(&path).await?;
                if metadata.file_type().is_symlink() || !expected_paths.contains(&path) {
                    return Err(DestinationPlanErrorV2::LateCollision);
                }
                if metadata.is_dir() {
                    pending.push(path);
                } else if !metadata.is_file() {
                    return Err(DestinationPlanErrorV2::LateCollision);
                }
            }
        }
    }
    Ok(())
}

async fn copy_tree(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    let metadata = fs::metadata(source).await?;
    if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::copy(source, destination).await?;
        let file = fs::OpenOptions::new().write(true).open(destination).await?;
        file.sync_all().await?;
        return Ok(());
    }
    fs::create_dir_all(destination).await?;
    let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((from, to)) = pending.pop() {
        let mut entries = fs::read_dir(&from).await?;
        while let Some(entry) = entries.next_entry().await? {
            let target = to.join(entry.file_name());
            let metadata = entry.metadata().await?;
            if metadata.is_dir() {
                fs::create_dir(&target).await?;
                pending.push((entry.path(), target));
            } else if metadata.is_file() {
                fs::copy(entry.path(), &target).await?;
                let file = fs::OpenOptions::new().write(true).open(target).await?;
                file.sync_all().await?;
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "staging tree contains an unsupported object",
                ));
            }
        }
    }
    Ok(())
}

async fn sync_tree_directories(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        let metadata = fs::symlink_metadata(path).await?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "staging root is a symbolic link",
            ));
        }
        if metadata.is_dir() {
            let mut pending = vec![path.to_path_buf()];
            let mut directories = Vec::new();
            while let Some(directory) = pending.pop() {
                directories.push(directory.clone());
                let mut children = fs::read_dir(&directory).await?;
                while let Some(child) = children.next_entry().await? {
                    let metadata = fs::symlink_metadata(child.path()).await?;
                    if metadata.file_type().is_symlink() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "staging tree contains a symbolic link",
                        ));
                    }
                    if metadata.is_dir() {
                        pending.push(child.path());
                    }
                }
            }
            for directory in directories.into_iter().rev() {
                fs::File::open(directory).await?.sync_all().await?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

async fn restrict_private_directory(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

async fn exclusive_rename_probe(directory: &Path) -> Result<bool, std::io::Error> {
    let token = format!(
        "{}-{}",
        std::process::id(),
        NEXT_RENAME_PROBE.fetch_add(1, Ordering::Relaxed)
    );
    let source = directory.join(format!(".envoix-rename-probe-{token}.source"));
    let target = directory.join(format!(".envoix-rename-probe-{token}.target"));
    let _ = fs::remove_file(&source).await;
    let _ = fs::remove_file(&target).await;
    fs::write(&source, b"source").await?;
    fs::write(&target, b"target").await?;
    let collision_safe = exclusive_rename(&source, &target)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists);
    let source_retained = fs::try_exists(&source).await?;
    let target_unchanged = fs::read(&target).await? == b"target";
    fs::remove_file(&target).await?;
    let success = exclusive_rename(&source, &target).is_ok();
    let _ = fs::remove_file(&source).await;
    let _ = fs::remove_file(&target).await;
    Ok(collision_safe && source_retained && target_unchanged && success)
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
const fn exclusive_rename_supported() -> bool {
    true
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
const fn exclusive_rename_supported() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn exclusive_rename(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::ffi::OsStrExt;
    const RENAME_EXCL: u32 = 0x0000_0004;
    unsafe extern "C" {
        fn renamex_np(
            from: *const std::ffi::c_char,
            to: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: both pointers reference live NUL-terminated path buffers for the
    // duration of the synchronous Darwin syscall; no Rust memory is aliased.
    let result = unsafe { renamex_np(source.as_ptr(), destination.as_ptr(), RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn exclusive_rename(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::ffi::OsStrExt;
    const AT_FDCWD: i32 = -100;
    const RENAME_NOREPLACE: u32 = 1;
    unsafe extern "C" {
        fn renameat2(
            olddirfd: i32,
            oldpath: *const std::ffi::c_char,
            newdirfd: i32,
            newpath: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: both pointers reference live NUL-terminated path buffers for the
    // duration of the synchronous Linux syscall; descriptors are AT_FDCWD.
    let result = unsafe {
        renameat2(
            AT_FDCWD,
            source.as_ptr(),
            AT_FDCWD,
            destination.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "windows")]
fn exclusive_rename(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::ffi::OsStrExt;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both pointers reference live NUL-terminated UTF-16 buffers for
    // the duration of the synchronous Win32 call; replace-existing is absent.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if result != 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(80) | Some(183)) {
            Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
        } else {
            Err(error)
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn exclusive_rename(_source: &Path, _destination: &Path) -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "exclusive rename is unavailable",
    ))
}

async fn local_storage_domain(
    target_directory: &Path,
    stable_object_identity: bool,
) -> Result<StorageDomainIdentityV2, std::io::Error> {
    let canonical = fs::canonicalize(target_directory).await?;
    #[cfg(unix)]
    let opaque_volume_id = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&canonical).await?.dev().to_string()
    };
    #[cfg(target_os = "windows")]
    let opaque_volume_id = windows_volume_guid(&canonical)?;
    #[cfg(not(any(unix, target_os = "windows")))]
    let opaque_volume_id = canonical
        .components()
        .next()
        .map(|component| format!("{component:?}"))
        .unwrap_or_default();
    Ok(StorageDomainIdentityV2 {
        provider: "local_path".into(),
        opaque_volume_id,
        stable_object_identity,
    })
}

#[cfg(target_os = "windows")]
fn windows_volume_guid(path: &Path) -> Result<String, std::io::Error> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    unsafe extern "system" {
        fn GetVolumePathNameW(file_name: *const u16, volume_path: *mut u16, length: u32) -> i32;
        fn GetVolumeNameForVolumeMountPointW(
            volume_path: *const u16,
            volume_name: *mut u16,
            length: u32,
        ) -> i32;
    }
    let path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut volume_path = vec![0_u16; 1024];
    // SAFETY: all pointers reference live, writable NUL-terminated UTF-16
    // buffers with the exact capacities passed to Win32.
    if unsafe {
        GetVolumePathNameW(
            path.as_ptr(),
            volume_path.as_mut_ptr(),
            volume_path.len() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let mut volume_name = vec![0_u16; 1024];
    // SAFETY: volume_path was initialized by Win32 above and both buffers stay
    // live for this synchronous call.
    if unsafe {
        GetVolumeNameForVolumeMountPointW(
            volume_path.as_ptr(),
            volume_name.as_mut_ptr(),
            volume_name.len() as u32,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let length = volume_name
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(volume_name.len());
    Ok(std::ffi::OsString::from_wide(&volume_name[..length])
        .to_string_lossy()
        .into_owned())
}

async fn same_storage_domain(left: &Path, right: &Path) -> Result<bool, std::io::Error> {
    Ok(local_storage_domain(left, false).await?.opaque_volume_id
        == local_storage_domain(right, false).await?.opaque_volume_id)
}

fn encode_job_id(job_id: JobIdV2) -> String {
    job_id.0.iter().map(|byte| format!("{byte:02x}")).collect()
}
