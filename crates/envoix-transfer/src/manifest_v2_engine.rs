//! Sequential Manifest v2 identity data plane.

use std::path::PathBuf;

use async_trait::async_trait;
use envoix_error::CoreError;
use envoix_protocol::manifest_v2::{
    CompressionPolicyV2, ContentDigestV2, DEFAULT_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES,
    EntryContentDigestV2, MAX_MANIFEST_V2_BLOCK_ENCODED_BYTES, ManifestEntryKindV2,
    ManifestEntryV2, ManifestOfferV2, ManifestV2, build_manifest_offer_v2,
};
use envoix_protocol::manifest_v2_frames::{
    EntryArbiterV2, EntryBlockV2, EntryCompleteV2, EntryCompletionChoiceV2,
    EntryContentDigestFrameV2, EntryDigestDecisionV2, EntryDispositionV2, EntryEncodingV2,
    EntryResultKindV2, EntryResultV2, EntryStartV2, JobCompleteV2, JobGenerationV2,
    ManifestAcceptV2, ManifestV2Frame, ManifestV2FrameCodecError, ManifestV2FrameConnection,
    ResumeEntryV2, ResumeRequestV2, ResumeStatusV2, canonical_manifest_v2_frame_body_digest,
    encode_manifest_v2_frame,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    CanonicalTransferJob, DeliveryAuthorityErrorV2, DestinationPlanErrorV2,
    ManifestV2DeliveryAuthority, PreparedFileSource, SenderDeliveryRecordV2, SenderDeliveryStoreV2,
    SenderTransferPhaseV2, TransferJobError,
};

const RECEIVER_DATA_PLANE_SCHEMA_VERSION: u16 = 1;
const SMART_COMPRESSION_SAMPLE_BYTES: usize = 64 * 1024;
const ZSTD_COMPRESSION_LEVEL: i32 = 3;
#[derive(Debug, Error)]
pub enum ManifestV2DataError {
    #[error("Manifest v2 job must be sealed before sending")]
    JobNotSealed,
    #[error("unexpected Manifest v2 frame while {0}")]
    UnexpectedFrame(&'static str),
    #[error("Manifest v2 frame belongs to another job or generation")]
    IdentityMismatch,
    #[error("Manifest v2 Accept does not match the sealed offer")]
    AcceptMismatch,
    #[error("Manifest v2 entry order is invalid")]
    EntryOrder,
    #[error("Manifest v2 block order, offset, or size is invalid")]
    BlockOrder,
    #[error("Manifest v2 digest changed after it was committed")]
    DigestConflict,
    #[error("Manifest v2 final size or digest did not verify")]
    FinalMismatch,
    #[error("Manifest v2 entry encoding is not available")]
    UnsupportedEncoding,
    #[error("Manifest v2 compressed block is invalid or exceeds its plaintext bound")]
    InvalidCompressedBlock,
    #[error("Manifest v2 reuse requires a stable receiver object and known digest")]
    ReuseUnavailable,
    #[error("Manifest v2 durable ledger is inconsistent: {0}")]
    InvalidLedger(String),
    #[error("Manifest v2 arithmetic overflow")]
    ArithmeticOverflow,
    #[error(transparent)]
    Job(#[from] TransferJobError),
    #[error(transparent)]
    Codec(#[from] ManifestV2FrameCodecError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Destination(#[from] DestinationPlanErrorV2),
    #[error("destination provider violated its result contract: {0}")]
    DestinationContract(String),
    #[error(transparent)]
    Delivery(#[from] DeliveryAuthorityErrorV2),
    #[error("internal data-plane task failed: {0}")]
    Internal(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedEntryV2 {
    pub entry_id: u32,
    pub result: EntryResultKindV2,
    pub final_component_override: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifiedEntryV2 {
    pub entry_id: u32,
    pub final_digest: Option<ContentDigestV2>,
    pub completion_choice: EntryCompletionChoiceV2,
}

#[async_trait]
pub trait ManifestV2PayloadSink: Send {
    /// Reopens or creates the receiver-owned incomplete object and reconstructs
    /// any transient verification state through `next_plaintext_block`.
    async fn begin_entry(
        &mut self,
        entry: &ManifestEntryV2,
        start: EntryStartV2,
        next_plaintext_block: u64,
    ) -> Result<(), ManifestV2DataError>;

    /// Writes, decode-verifies, hashes, and durably flushes exactly one complete
    /// plaintext block before returning.
    async fn write_block(
        &mut self,
        entry: &ManifestEntryV2,
        block: &EntryBlockV2,
    ) -> Result<(), ManifestV2DataError>;

    /// Returns true only after a stable opened existing object has been chosen
    /// and that arbiter effect is durable in the destination provider.
    async fn try_choose_reuse(
        &mut self,
        entry: &ManifestEntryV2,
        digest: ContentDigestV2,
    ) -> Result<bool, ManifestV2DataError>;

    async fn verify_payload(
        &mut self,
        entry: &ManifestEntryV2,
        final_digest: ContentDigestV2,
    ) -> Result<(), ManifestV2DataError>;

    async fn stage_directory(&mut self, entry: &ManifestEntryV2)
    -> Result<(), ManifestV2DataError>;

    /// Finalizes every root only after all entry payloads are verified. The
    /// returned dense result set is not sent until the caller durably records it.
    async fn commit_job(
        &mut self,
        manifest: &ManifestV2,
        verified_entries: &[VerifiedEntryV2],
    ) -> Result<Vec<SavedEntryV2>, ManifestV2DataError>;

    async fn retire_payload(&mut self, entry: &ManifestEntryV2) -> Result<(), ManifestV2DataError>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ReceiverEntryCheckpointV2 {
    entry_id: u32,
    start: Option<EntryStartV2>,
    arbiter: EntryArbiterV2,
    next_plaintext_block: u64,
    plaintext_bytes: u64,
    content_digest: Option<ContentDigestV2>,
    completion: Option<EntryCompleteV2>,
    result: Option<EntryResultV2>,
    payload_retired: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverDataPlaneLedgerV2 {
    schema_version: u16,
    identity: JobGenerationV2,
    manifest_digest: ContentDigestV2,
    accept: ManifestAcceptV2,
    accept_body_digest: ContentDigestV2,
    entries: Vec<ReceiverEntryCheckpointV2>,
    sender_completion_set_digest: Option<ContentDigestV2>,
}

impl std::fmt::Debug for ReceiverDataPlaneLedgerV2 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReceiverDataPlaneLedgerV2")
            .field("identity", &self.identity)
            .field("manifest_digest", &self.manifest_digest)
            .field("entry_count", &self.entries.len())
            .field(
                "completed_entries",
                &self
                    .entries
                    .iter()
                    .filter(|entry| entry.result.is_some())
                    .count(),
            )
            .finish()
    }
}

impl ReceiverDataPlaneLedgerV2 {
    pub fn new(
        offer: &ManifestOfferV2,
        accept: ManifestAcceptV2,
    ) -> Result<Self, ManifestV2DataError> {
        validate_accept(offer, &accept)?;
        let accept_body_digest =
            canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::Accept(accept.clone()))?;
        let entries = offer
            .manifest
            .entries
            .iter()
            .map(|entry| ReceiverEntryCheckpointV2 {
                entry_id: entry.entry_id,
                start: None,
                arbiter: EntryArbiterV2::PayloadOpen,
                next_plaintext_block: accept.entry_plans[entry.entry_id as usize]
                    .next_plaintext_block,
                plaintext_bytes: accept.entry_plans[entry.entry_id as usize]
                    .next_plaintext_block
                    .saturating_mul(DEFAULT_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES as u64)
                    .min(entry.plaintext_size),
                content_digest: match entry.content_digest {
                    EntryContentDigestV2::Known(digest) => Some(digest),
                    EntryContentDigestV2::Deferred => None,
                },
                completion: None,
                result: None,
                payload_retired: false,
            })
            .collect();
        Ok(Self {
            schema_version: RECEIVER_DATA_PLANE_SCHEMA_VERSION,
            identity: accept.identity,
            manifest_digest: offer.structural_digest,
            accept,
            accept_body_digest,
            entries,
            sender_completion_set_digest: None,
        })
    }

    pub fn accept(&self) -> &ManifestAcceptV2 {
        &self.accept
    }

    pub fn accept_body_digest(&self) -> ContentDigestV2 {
        self.accept_body_digest
    }

    pub fn requires_authenticated_resume(&self) -> bool {
        self.sender_completion_set_digest.is_some()
            || self.entries.iter().any(|entry| {
                entry.start.is_some()
                    || entry.completion.is_some()
                    || entry.result.is_some()
                    || entry.next_plaintext_block > 0
                    || entry.plaintext_bytes > 0
            })
    }

    pub fn validate(&self, manifest: &ManifestV2) -> Result<(), ManifestV2DataError> {
        let rebuilt_offer = build_manifest_offer_v2(manifest.clone()).map_err(|error| {
            ManifestV2DataError::Codec(ManifestV2FrameCodecError::Offer(error.to_string()))
        })?;
        if self.schema_version != RECEIVER_DATA_PLANE_SCHEMA_VERSION
            || self.identity.job_id != manifest.job_id
            || self.identity.generation != manifest.generation
            || self.manifest_digest != rebuilt_offer.structural_digest
            || self.entries.len() != manifest.entries.len()
            || self.accept.identity != self.identity
            || self.accept.manifest_digest != self.manifest_digest
        {
            return Err(ManifestV2DataError::InvalidLedger(
                "identity, schema, or entry count changed".into(),
            ));
        }
        validate_accept(&rebuilt_offer, &self.accept)?;
        for (index, checkpoint) in self.entries.iter().enumerate() {
            if checkpoint.entry_id != index as u32
                || checkpoint.result.as_ref().is_some_and(|result| {
                    result.identity != self.identity || result.entry_id != checkpoint.entry_id
                })
                || checkpoint.start.is_some_and(|start| {
                    start.identity != self.identity || start.entry_id != checkpoint.entry_id
                })
                || checkpoint.completion.is_some_and(|completion| {
                    completion.identity != self.identity
                        || completion.entry_id != checkpoint.entry_id
                })
            {
                return Err(ManifestV2DataError::InvalidLedger(
                    "entry checkpoint identity changed".into(),
                ));
            }
        }
        let digest =
            canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::Accept(self.accept.clone()))?;
        if digest != self.accept_body_digest {
            return Err(ManifestV2DataError::InvalidLedger(
                "Accept bytes changed after commit".into(),
            ));
        }
        Ok(())
    }

    pub fn resume_status(&self) -> ResumeStatusV2 {
        ResumeStatusV2 {
            identity: self.identity,
            accept_body_digest: self.accept_body_digest,
            plan_revision: self.accept.plan_revision,
            entries: self
                .entries
                .iter()
                .map(|checkpoint| ResumeEntryV2 {
                    entry_id: checkpoint.entry_id,
                    arbiter: checkpoint.arbiter,
                    next_plaintext_block: checkpoint.next_plaintext_block,
                    content_digest: checkpoint.content_digest,
                    entry_result: checkpoint.result.clone(),
                })
                .collect(),
            challenge_nonce: [0; 32],
            challenge_mac: [0; 32],
        }
    }

    pub fn validate_resume_request(
        &self,
        request: &ResumeRequestV2,
    ) -> Result<(), ManifestV2DataError> {
        if request.identity != self.identity
            || request.offer.structural_digest != self.manifest_digest
            || request.accept_body_digest != self.accept_body_digest
        {
            return Err(ManifestV2DataError::AcceptMismatch);
        }
        Ok(())
    }

    pub fn completed_summary(&self) -> Option<ReceiverDataPlaneSummaryV2> {
        let sender_completion_set_digest = self.sender_completion_set_digest?;
        let entry_results = self
            .entries
            .iter()
            .map(|entry| entry.result.clone())
            .collect::<Option<Vec<_>>>()?;
        Some(ReceiverDataPlaneSummaryV2 {
            identity: self.identity,
            sender_completion_set_digest,
            entry_results,
        })
    }

    pub(crate) fn pending_payload_boundaries(&self) -> Vec<(u32, u64, u64, bool)> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.result.is_none()
                    && self.accept.entry_plans[entry.entry_id as usize].disposition
                        == EntryDispositionV2::ReceivePayload
            })
            .map(|entry| {
                (
                    entry.entry_id,
                    entry.next_plaintext_block,
                    entry.plaintext_bytes,
                    entry.completion.is_some(),
                )
            })
            .collect()
    }

    pub(crate) fn pending_reuse_entries(&self) -> Vec<(u32, ContentDigestV2)> {
        self.entries
            .iter()
            .filter(|entry| entry.result.is_none() && entry.arbiter == EntryArbiterV2::ReuseChosen)
            .filter_map(|entry| entry.content_digest.map(|digest| (entry.entry_id, digest)))
            .collect()
    }

    pub(crate) async fn reset_payload_checkpoint(
        &mut self,
        manifest: &ManifestV2,
        entry_id: u32,
        store: &ReceiverDataPlaneStoreV2,
    ) -> Result<(), ManifestV2DataError> {
        let entry = manifest
            .entries
            .get(entry_id as usize)
            .filter(|entry| entry.entry_id == entry_id)
            .ok_or(ManifestV2DataError::EntryOrder)?;
        let disposition = self
            .accept
            .entry_plans
            .get(entry_id as usize)
            .filter(|plan| plan.entry_id == entry_id)
            .ok_or(ManifestV2DataError::EntryOrder)?
            .disposition;
        let checkpoint = self
            .entries
            .get_mut(entry_id as usize)
            .filter(|checkpoint| checkpoint.entry_id == entry_id)
            .ok_or(ManifestV2DataError::EntryOrder)?;
        if checkpoint.result.is_some() || disposition != EntryDispositionV2::ReceivePayload {
            return Err(ManifestV2DataError::InvalidLedger(
                "cannot reset a terminal or reuse payload checkpoint".into(),
            ));
        }
        checkpoint.start = None;
        checkpoint.arbiter = EntryArbiterV2::PayloadOpen;
        checkpoint.next_plaintext_block = 0;
        checkpoint.plaintext_bytes = 0;
        checkpoint.content_digest = match entry.content_digest {
            EntryContentDigestV2::Known(digest) => Some(digest),
            EntryContentDigestV2::Deferred => None,
        };
        checkpoint.completion = None;
        checkpoint.payload_retired = true;
        self.sender_completion_set_digest = None;
        store.save(self).await
    }
}

#[derive(Clone, Debug)]
pub struct ReceiverDataPlaneStoreV2 {
    directory: PathBuf,
}

impl ReceiverDataPlaneStoreV2 {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub async fn save(
        &self,
        ledger: &ReceiverDataPlaneLedgerV2,
    ) -> Result<(), ManifestV2DataError> {
        fs::create_dir_all(&self.directory).await?;
        let final_path = self.ledger_path(ledger.identity);
        let temporary_path = final_path.with_extension("tmp");
        let bytes = serde_json::to_vec(ledger)?;
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
        identity: JobGenerationV2,
    ) -> Result<Option<ReceiverDataPlaneLedgerV2>, ManifestV2DataError> {
        let bytes = match fs::read(self.ledger_path(identity)).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let ledger = serde_json::from_slice(&bytes)?;
        Ok(Some(ledger))
    }

    fn ledger_path(&self, identity: JobGenerationV2) -> PathBuf {
        let job_id = identity
            .job_id
            .0
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.directory
            .join(format!("receiver-{job_id}-{}.json", identity.generation))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderDataPlaneSummaryV2 {
    pub identity: JobGenerationV2,
    pub accept_body_digest: ContentDigestV2,
    pub sender_completion_set_digest: ContentDigestV2,
    pub entry_results: Vec<EntryResultV2>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiverDataPlaneSummaryV2 {
    pub identity: JobGenerationV2,
    pub sender_completion_set_digest: ContentDigestV2,
    pub entry_results: Vec<EntryResultV2>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SenderResumeIntentV2 {
    ContinueData,
    AwaitDelivery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestV2ProgressPhase {
    Transferring,
    Verifying,
    Saving,
    WaitingForReceiverSave,
    FinalizingDelivery,
}

pub trait ManifestV2ProgressSink: Send + Sync {
    fn on_progress(&self, completed_plaintext_bytes: u64, total_plaintext_bytes: u64);
    fn on_phase(&self, phase: ManifestV2ProgressPhase);
}

#[async_trait]
pub trait ManifestV2ResultGate: Send + Sync {
    /// Completes a platform-owned save (for example SAF/MediaStore
    /// CopyAfterVerify) before result frames or delivery proof can be emitted.
    /// The gate may replace final component overrides with provider-assigned
    /// names, but cannot change entry identity, result kind, size, or digest.
    async fn commit_results(
        &self,
        manifest: &ManifestV2,
        saved_entries: &mut [SavedEntryV2],
    ) -> Result<(), ManifestV2DataError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopManifestV2ResultGate;

#[async_trait]
impl ManifestV2ResultGate for NoopManifestV2ResultGate {
    async fn commit_results(
        &self,
        _manifest: &ManifestV2,
        _saved_entries: &mut [SavedEntryV2],
    ) -> Result<(), ManifestV2DataError> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ManifestV2DataPlane;

impl ManifestV2DataPlane {
    /// Re-establishes a previously committed Accept without sending its raw
    /// proof capability again. ResumeRequest is the connection's first frame
    /// and carries the immutable Offer plus a fresh challenge.
    pub async fn establish_sender_reconnect<C>(
        job: &CanonicalTransferJob,
        delivery_record: &SenderDeliveryRecordV2,
        connection: &mut C,
    ) -> Result<ResumeStatusV2, ManifestV2DataError>
    where
        C: ManifestV2FrameConnection,
    {
        let manifest = job.manifest().ok_or(ManifestV2DataError::JobNotSealed)?;
        let offer = build_manifest_offer_v2(manifest.clone()).map_err(|error| {
            ManifestV2DataError::Codec(ManifestV2FrameCodecError::Offer(error.to_string()))
        })?;
        delivery_record
            .validate_offer(&offer)
            .map_err(ManifestV2DataError::Delivery)?;
        let accept_digest = delivery_record.accept_body_digest().ok_or_else(|| {
            ManifestV2DataError::InvalidLedger("sender Accept is not committed".into())
        })?;
        let capability = delivery_record.proof_capability().ok_or_else(|| {
            ManifestV2DataError::InvalidLedger("sender proof capability is missing".into())
        })?;
        let challenge_nonce = ManifestV2DeliveryAuthority::new_challenge_nonce()
            .map_err(ManifestV2DataError::Delivery)?;
        let request = ResumeRequestV2 {
            identity: delivery_record.identity(),
            offer: offer.clone(),
            accept_body_digest: accept_digest,
            sender_checkpoint_digest: sender_checkpoint_digest(
                offer.structural_digest,
                accept_digest,
                match delivery_record.phase() {
                    SenderTransferPhaseV2::Transferring => SenderResumeIntentV2::ContinueData,
                    SenderTransferPhaseV2::WaitingForReceiverSave
                    | SenderTransferPhaseV2::Delivered => SenderResumeIntentV2::AwaitDelivery,
                    _ => {
                        return Err(ManifestV2DataError::InvalidLedger(
                            "sender phase cannot establish a reconnect".into(),
                        ));
                    }
                },
            ),
            challenge_nonce,
        };
        connection
            .send_manifest_v2_frame(ManifestV2Frame::ResumeRequest(request))
            .await?;
        let status = match connection.recv_manifest_v2_frame().await? {
            ManifestV2Frame::ResumeStatus(status) => status,
            _ => {
                return Err(ManifestV2DataError::UnexpectedFrame(
                    "waiting for receiver resume status",
                ));
            }
        };
        if status.challenge_nonce != challenge_nonce {
            return Err(ManifestV2DataError::AcceptMismatch);
        }
        ManifestV2DeliveryAuthority::verify_resume_challenge(
            delivery_record.identity(),
            challenge_nonce,
            status.challenge_mac,
            capability,
        )
        .map_err(ManifestV2DataError::Delivery)?;
        validate_resume_status(manifest, delivery_record, &status)?;
        Ok(status)
    }

    pub async fn send<C>(
        job: &CanonicalTransferJob,
        delivery_record: &mut SenderDeliveryRecordV2,
        delivery_store: &SenderDeliveryStoreV2,
        connection: &mut C,
        progress: &dyn ManifestV2ProgressSink,
    ) -> Result<SenderDataPlaneSummaryV2, ManifestV2DataError>
    where
        C: ManifestV2FrameConnection,
    {
        let manifest = job.manifest().ok_or(ManifestV2DataError::JobNotSealed)?;
        let offer = build_manifest_offer_v2(manifest.clone()).map_err(|error| {
            ManifestV2DataError::Codec(ManifestV2FrameCodecError::Offer(error.to_string()))
        })?;
        delivery_record
            .validate_offer(&offer)
            .map_err(ManifestV2DataError::Delivery)?;
        delivery_store
            .save(delivery_record)
            .await
            .map_err(ManifestV2DataError::Delivery)?;
        let (accept, resume_status) = if delivery_record.phase()
            == SenderTransferPhaseV2::Transferring
        {
            let status = Self::establish_sender_reconnect(job, delivery_record, connection).await?;
            let mut accept = delivery_record.accept().cloned().ok_or_else(|| {
                ManifestV2DataError::InvalidLedger("sender Accept is missing".into())
            })?;
            for (plan, resume) in accept.entry_plans.iter_mut().zip(&status.entries) {
                plan.next_plaintext_block = resume.next_plaintext_block;
                if resume.arbiter == EntryArbiterV2::ReuseChosen {
                    plan.disposition = EntryDispositionV2::ReuseExisting;
                }
            }
            (accept, Some(status))
        } else {
            connection
                .send_manifest_v2_frame(ManifestV2Frame::Offer(offer.clone()))
                .await?;
            let accept = match connection.recv_manifest_v2_frame().await? {
                ManifestV2Frame::Accept(accept) => accept,
                _ => return Err(ManifestV2DataError::UnexpectedFrame("waiting for Accept")),
            };
            validate_accept(&offer, &accept)?;
            (accept, None)
        };
        let accept_body_digest = match &resume_status {
            Some(_) => delivery_record.accept_body_digest().ok_or_else(|| {
                ManifestV2DataError::InvalidLedger("sender Accept digest is missing".into())
            })?,
            None => {
                canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::Accept(accept.clone()))?
            }
        };
        let identity = accept.identity;
        if resume_status.is_none() {
            delivery_record
                .commit_accept(&accept, accept_body_digest)
                .map_err(ManifestV2DataError::Delivery)?;
            delivery_store
                .save(delivery_record)
                .await
                .map_err(ManifestV2DataError::Delivery)?;
        }

        progress.on_phase(ManifestV2ProgressPhase::Transferring);
        let total_plaintext_bytes = manifest.totals.total_plaintext_bytes;
        let mut completed_plaintext_bytes = resume_status
            .as_ref()
            .map(|status| resume_plaintext_bytes(manifest, status))
            .transpose()?
            .unwrap_or(0);
        progress.on_progress(completed_plaintext_bytes, total_plaintext_bytes);
        let mut completion_hasher = blake3::Hasher::new();
        let mut completions = Vec::with_capacity(manifest.entries.len());
        for entry in &manifest.entries {
            if let Some(result) = resume_status.as_ref().and_then(|status| {
                status.entries[entry.entry_id as usize]
                    .entry_result
                    .as_ref()
            }) {
                let completion = completion_from_result(entry, result)?;
                update_completion_set(&mut completion_hasher, completion)?;
                completions.push(completion);
                continue;
            }
            if let Some(checkpoint) = resume_status
                .as_ref()
                .map(|status| &status.entries[entry.entry_id as usize])
                .filter(|checkpoint| checkpoint.arbiter != EntryArbiterV2::PayloadOpen)
            {
                let completion = completion_from_resume_checkpoint(identity, entry, checkpoint)?;
                update_completion_set(&mut completion_hasher, completion)?;
                completions.push(completion);
                continue;
            }
            let plan = accept.entry_plans[entry.entry_id as usize];
            let completion = if entry.kind == ManifestEntryKindV2::Directory {
                send_directory(connection, identity, entry).await?
            } else {
                let source = job.source_for_sealed_entry(entry.entry_id)?;
                let resumed_entry_bytes = resume_status
                    .as_ref()
                    .map(|status| {
                        resume_entry_plaintext_bytes(
                            entry,
                            &status.entries[entry.entry_id as usize],
                        )
                    })
                    .transpose()?
                    .unwrap_or(0);
                send_file(
                    connection,
                    identity,
                    entry,
                    plan,
                    source,
                    manifest.compression_policy,
                    &mut completed_plaintext_bytes,
                    total_plaintext_bytes,
                    progress,
                    resumed_entry_bytes,
                )
                .await?
            };
            update_completion_set(&mut completion_hasher, completion)?;
            completions.push(completion);
        }
        let sender_completion_set_digest =
            ContentDigestV2(*completion_hasher.finalize().as_bytes());
        connection
            .send_manifest_v2_frame(ManifestV2Frame::JobComplete(JobCompleteV2 {
                identity,
                sender_completion_set_digest,
            }))
            .await?;
        let mut entry_results = Vec::with_capacity(manifest.entries.len());
        for (entry, completion) in manifest.entries.iter().zip(completions) {
            let result = recv_entry_result(connection, identity, entry.entry_id).await?;
            validate_entry_result(entry, identity, completion, &result)?;
            entry_results.push(result);
        }
        let summary = SenderDataPlaneSummaryV2 {
            identity,
            accept_body_digest,
            sender_completion_set_digest,
            entry_results,
        };
        delivery_record
            .commit_results(&summary)
            .map_err(ManifestV2DataError::Delivery)?;
        delivery_store
            .save(delivery_record)
            .await
            .map_err(ManifestV2DataError::Delivery)?;
        progress.on_phase(ManifestV2ProgressPhase::WaitingForReceiverSave);
        Ok(summary)
    }

    pub async fn receive<C, S>(
        offer: &ManifestOfferV2,
        ledger: &mut ReceiverDataPlaneLedgerV2,
        store: &ReceiverDataPlaneStoreV2,
        sink: &mut S,
        connection: &mut C,
        progress: &dyn ManifestV2ProgressSink,
        result_gate: &dyn ManifestV2ResultGate,
    ) -> Result<ReceiverDataPlaneSummaryV2, ManifestV2DataError>
    where
        C: ManifestV2FrameConnection,
        S: ManifestV2PayloadSink,
    {
        ledger.validate(&offer.manifest)?;
        let identity = ledger.identity;
        let total_plaintext_bytes = offer.manifest.totals.total_plaintext_bytes;
        let mut completed_plaintext_bytes =
            resume_plaintext_bytes(&offer.manifest, &ledger.resume_status())?;
        progress.on_phase(ManifestV2ProgressPhase::Transferring);
        progress.on_progress(completed_plaintext_bytes, total_plaintext_bytes);
        let mut completion_hasher = blake3::Hasher::new();
        for entry in &offer.manifest.entries {
            let checkpoint_index = entry.entry_id as usize;
            if ledger.entries[checkpoint_index].result.is_some() {
                let completion = ledger.entries[checkpoint_index].completion.ok_or_else(|| {
                    ManifestV2DataError::InvalidLedger(
                        "completed entry is missing its completion fact".into(),
                    )
                })?;
                update_completion_set(&mut completion_hasher, completion)?;
                continue;
            }
            if ledger.entries[checkpoint_index].arbiter != EntryArbiterV2::PayloadOpen {
                let completion = match ledger.entries[checkpoint_index].completion {
                    Some(completion) => completion,
                    None if ledger.entries[checkpoint_index].arbiter
                        == EntryArbiterV2::ReuseChosen =>
                    {
                        EntryCompleteV2 {
                            identity,
                            entry_id: entry.entry_id,
                            final_size: entry.plaintext_size,
                            final_digest: ledger.entries[checkpoint_index].content_digest.ok_or(
                                ManifestV2DataError::InvalidLedger(
                                    "reused entry is missing its content digest".into(),
                                ),
                            )?,
                            completion_choice: EntryCompletionChoiceV2::ReuseChosen,
                        }
                    }
                    None => {
                        return Err(ManifestV2DataError::InvalidLedger(
                            "completed payload is missing its completion fact".into(),
                        ));
                    }
                };
                ledger.entries[checkpoint_index].completion = Some(completion);
                store.save(ledger).await?;
                update_completion_set(&mut completion_hasher, completion)?;
                continue;
            }
            let plan = ledger.accept.entry_plans[checkpoint_index];
            let start = recv_entry_start(connection, identity, entry.entry_id).await?;
            {
                let checkpoint = &mut ledger.entries[checkpoint_index];
                match checkpoint.start {
                    Some(existing) if existing != start => {
                        return Err(ManifestV2DataError::DigestConflict);
                    }
                    None => checkpoint.start = Some(start),
                    Some(_) => {}
                }
                checkpoint.payload_retired = false;
            }
            store.save(ledger).await?;
            if entry.kind == ManifestEntryKindV2::Directory {
                receive_directory(
                    entry,
                    identity,
                    checkpoint_index,
                    ledger,
                    store,
                    sink,
                    connection,
                )
                .await?;
            } else {
                if plan.disposition == EntryDispositionV2::ReceivePayload {
                    sink.begin_entry(
                        entry,
                        start,
                        ledger.entries[checkpoint_index].next_plaintext_block,
                    )
                    .await?;
                }
                receive_file(
                    entry,
                    plan,
                    checkpoint_index,
                    ledger,
                    store,
                    sink,
                    connection,
                    &mut completed_plaintext_bytes,
                    total_plaintext_bytes,
                    progress,
                )
                .await?;
            }
            let completion = ledger.entries[checkpoint_index].completion.ok_or_else(|| {
                ManifestV2DataError::InvalidLedger("entry completion missing".into())
            })?;
            update_completion_set(&mut completion_hasher, completion)?;
        }
        let sender_digest = match connection.recv_manifest_v2_frame().await? {
            ManifestV2Frame::JobComplete(complete) if complete.identity == identity => {
                complete.sender_completion_set_digest
            }
            _ => {
                return Err(ManifestV2DataError::UnexpectedFrame(
                    "waiting for JobComplete",
                ));
            }
        };
        let expected = ContentDigestV2(*completion_hasher.finalize().as_bytes());
        if sender_digest != expected {
            return Err(ManifestV2DataError::DigestConflict);
        }
        match ledger.sender_completion_set_digest {
            Some(existing) if existing != sender_digest => {
                return Err(ManifestV2DataError::DigestConflict);
            }
            Some(_) => {}
            None => ledger.sender_completion_set_digest = Some(sender_digest),
        }
        store.save(ledger).await?;
        if ledger.entries.iter().all(|entry| entry.result.is_some()) {
            let entry_results = ledger
                .entries
                .iter()
                .map(|entry| {
                    entry.result.clone().ok_or_else(|| {
                        ManifestV2DataError::InvalidLedger("completed result set is sparse".into())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            for result in &entry_results {
                connection
                    .send_manifest_v2_frame(ManifestV2Frame::EntryResult(result.clone()))
                    .await?;
            }
            progress.on_phase(ManifestV2ProgressPhase::FinalizingDelivery);
            return Ok(ReceiverDataPlaneSummaryV2 {
                identity,
                sender_completion_set_digest: sender_digest,
                entry_results,
            });
        }
        let verified_entries = ledger
            .entries
            .iter()
            .map(|checkpoint| {
                let completion = checkpoint.completion.ok_or_else(|| {
                    ManifestV2DataError::InvalidLedger("entry completion missing".into())
                })?;
                Ok(VerifiedEntryV2 {
                    entry_id: checkpoint.entry_id,
                    final_digest: (offer.manifest.entries[checkpoint.entry_id as usize].kind
                        == ManifestEntryKindV2::RegularFile)
                        .then_some(completion.final_digest),
                    completion_choice: completion.completion_choice,
                })
            })
            .collect::<Result<Vec<_>, ManifestV2DataError>>()?;
        progress.on_phase(ManifestV2ProgressPhase::Saving);
        let mut saved_entries = sink.commit_job(&offer.manifest, &verified_entries).await?;
        result_gate
            .commit_results(&offer.manifest, &mut saved_entries)
            .await?;
        if saved_entries.len() != offer.manifest.entries.len() {
            return Err(ManifestV2DataError::DestinationContract(
                "destination returned an incomplete result set".into(),
            ));
        }
        let mut entry_results = Vec::with_capacity(saved_entries.len());
        for (entry, (verified, saved)) in offer
            .manifest
            .entries
            .iter()
            .zip(verified_entries.iter().zip(saved_entries))
        {
            let expected_result = match verified.completion_choice {
                EntryCompletionChoiceV2::PayloadComplete => EntryResultKindV2::Saved,
                EntryCompletionChoiceV2::ReuseChosen => EntryResultKindV2::ReusedExisting,
            };
            if saved.entry_id != entry.entry_id || saved.result != expected_result {
                return Err(ManifestV2DataError::DestinationContract(
                    "destination result set is noncanonical".into(),
                ));
            }
            let result = EntryResultV2 {
                identity,
                entry_id: entry.entry_id,
                result: saved.result,
                final_size: entry.plaintext_size,
                final_digest: verified.final_digest,
                final_component_override: saved.final_component_override,
            };
            ledger.entries[entry.entry_id as usize].result = Some(result.clone());
            entry_results.push(result);
        }
        store.save(ledger).await?;
        for result in &entry_results {
            connection
                .send_manifest_v2_frame(ManifestV2Frame::EntryResult(result.clone()))
                .await?;
        }
        progress.on_phase(ManifestV2ProgressPhase::FinalizingDelivery);
        Ok(ReceiverDataPlaneSummaryV2 {
            identity,
            sender_completion_set_digest: sender_digest,
            entry_results,
        })
    }
}

async fn send_directory<C>(
    connection: &mut C,
    identity: JobGenerationV2,
    entry: &ManifestEntryV2,
) -> Result<EntryCompleteV2, ManifestV2DataError>
where
    C: ManifestV2FrameConnection,
{
    connection
        .send_manifest_v2_frame(ManifestV2Frame::EntryStart(EntryStartV2 {
            identity,
            entry_id: entry.entry_id,
            encoding: EntryEncodingV2::Identity,
            plaintext_block_bytes: DEFAULT_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES,
        }))
        .await?;
    let completion = EntryCompleteV2 {
        identity,
        entry_id: entry.entry_id,
        final_size: 0,
        final_digest: empty_digest(),
        completion_choice: EntryCompletionChoiceV2::PayloadComplete,
    };
    connection
        .send_manifest_v2_frame(ManifestV2Frame::EntryComplete(completion))
        .await?;
    Ok(completion)
}

async fn send_file<C>(
    connection: &mut C,
    identity: JobGenerationV2,
    entry: &ManifestEntryV2,
    plan: envoix_protocol::manifest_v2_frames::EntryPlanV2,
    source: PreparedFileSource,
    compression_policy: CompressionPolicyV2,
    completed_plaintext_bytes: &mut u64,
    total_plaintext_bytes: u64,
    progress: &dyn ManifestV2ProgressSink,
    resumed_entry_bytes: u64,
) -> Result<EntryCompleteV2, ManifestV2DataError>
where
    C: ManifestV2FrameConnection,
{
    let block_bytes = DEFAULT_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES;
    let encoding = select_entry_encoding(&source, compression_policy).await?;
    connection
        .send_manifest_v2_frame(ManifestV2Frame::EntryStart(EntryStartV2 {
            identity,
            entry_id: entry.entry_id,
            encoding,
            plaintext_block_bytes: block_bytes,
        }))
        .await?;
    let known_digest = match entry.content_digest {
        EntryContentDigestV2::Known(digest) => Some(digest),
        EntryContentDigestV2::Deferred => None,
    };
    if plan.disposition == EntryDispositionV2::ReuseExisting {
        let digest = known_digest.ok_or(ManifestV2DataError::ReuseUnavailable)?;
        source.verify_unchanged().await?;
        let completion = EntryCompleteV2 {
            identity,
            entry_id: entry.entry_id,
            final_size: entry.plaintext_size,
            final_digest: digest,
            completion_choice: EntryCompletionChoiceV2::ReuseChosen,
        };
        connection
            .send_manifest_v2_frame(ManifestV2Frame::EntryComplete(completion))
            .await?;
        *completed_plaintext_bytes = completed_plaintext_bytes
            .checked_add(entry.plaintext_size.saturating_sub(resumed_entry_bytes))
            .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
        progress.on_progress(*completed_plaintext_bytes, total_plaintext_bytes);
        return Ok(completion);
    }

    let mut file = source.open().await?;
    let mut payload_hasher = blake3::Hasher::new();
    let mut hash_task = if known_digest.is_none() {
        let hash_source = source.clone();
        PendingHashTask::new(tokio::spawn(async move { hash_source.hash().await }))
    } else {
        PendingHashTask::empty()
    };
    let mut late_digest = None;
    let mut reuse_chosen = false;
    let mut block_index = 0_u64;
    let mut plaintext_offset = 0_u64;
    while plaintext_offset < entry.plaintext_size {
        if late_digest.is_none() && hash_task.is_finished() {
            let digest = await_hash_task(hash_task.take()).await?;
            late_digest = Some(digest);
            reuse_chosen =
                send_late_digest_and_receive_status(connection, identity, entry.entry_id, digest)
                    .await?;
            if reuse_chosen {
                break;
            }
        }
        let remaining = entry.plaintext_size - plaintext_offset;
        let length = remaining.min(block_bytes as u64) as usize;
        let mut bytes = vec![0_u8; length];
        file.read_exact(&mut bytes).await?;
        payload_hasher.update(&bytes);
        if block_index >= plan.next_plaintext_block {
            let response = connection
                .send_entry_block_or_recv_frame(EntryBlockV2 {
                    identity,
                    entry_id: entry.entry_id,
                    block_index,
                    plaintext_offset,
                    plaintext_length: length as u32,
                    encoded_bytes: encode_block(encoding, &bytes)?,
                })
                .await?;
            if let Some(frame) = response {
                handle_sender_control(frame, identity)?;
            }
            *completed_plaintext_bytes = completed_plaintext_bytes
                .checked_add(length as u64)
                .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
            progress.on_progress(*completed_plaintext_bytes, total_plaintext_bytes);
        }
        block_index = block_index
            .checked_add(1)
            .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
        plaintext_offset = plaintext_offset
            .checked_add(length as u64)
            .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
    }

    let final_digest = if reuse_chosen {
        source.verify_unchanged().await?;
        let entry_progress = resumed_entry_bytes.max(plaintext_offset).max(
            plan.next_plaintext_block
                .saturating_mul(block_bytes as u64)
                .min(entry.plaintext_size),
        );
        *completed_plaintext_bytes = completed_plaintext_bytes
            .checked_add(entry.plaintext_size.saturating_sub(entry_progress))
            .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
        progress.on_progress(*completed_plaintext_bytes, total_plaintext_bytes);
        late_digest.ok_or(ManifestV2DataError::DigestConflict)?
    } else {
        let payload_digest = ContentDigestV2(*payload_hasher.finalize().as_bytes());
        source.verify_unchanged().await?;
        match known_digest {
            Some(expected) if expected != payload_digest => {
                return Err(ManifestV2DataError::FinalMismatch);
            }
            Some(expected) => expected,
            None => {
                let digest = match late_digest {
                    Some(digest) => digest,
                    None => await_hash_task(hash_task.take()).await?,
                };
                if digest != payload_digest {
                    return Err(ManifestV2DataError::FinalMismatch);
                }
                if late_digest.is_none() {
                    reuse_chosen = send_late_digest_and_receive_status(
                        connection,
                        identity,
                        entry.entry_id,
                        digest,
                    )
                    .await?;
                }
                digest
            }
        }
    };
    let completion = EntryCompleteV2 {
        identity,
        entry_id: entry.entry_id,
        final_size: entry.plaintext_size,
        final_digest,
        completion_choice: if reuse_chosen {
            EntryCompletionChoiceV2::ReuseChosen
        } else {
            EntryCompletionChoiceV2::PayloadComplete
        },
    };
    connection
        .send_manifest_v2_frame(ManifestV2Frame::EntryComplete(completion))
        .await?;
    Ok(completion)
}

async fn send_late_digest_and_receive_status<C>(
    connection: &mut C,
    identity: JobGenerationV2,
    entry_id: u32,
    digest: ContentDigestV2,
) -> Result<bool, ManifestV2DataError>
where
    C: ManifestV2FrameConnection,
{
    connection
        .send_manifest_v2_frame(ManifestV2Frame::EntryContentDigest(
            EntryContentDigestFrameV2 {
                identity,
                entry_id,
                digest,
                decision: EntryDigestDecisionV2::Proposed,
            },
        ))
        .await?;
    let response = match connection.recv_manifest_v2_frame().await? {
        ManifestV2Frame::EntryContentDigest(response) => response,
        frame => {
            handle_sender_control(frame, identity)?;
            return Err(ManifestV2DataError::UnexpectedFrame(
                "waiting for late-digest decision",
            ));
        }
    };
    if response.identity != identity
        || response.entry_id != entry_id
        || response.digest != digest
        || response.decision == EntryDigestDecisionV2::Proposed
    {
        return Err(ManifestV2DataError::IdentityMismatch);
    }
    Ok(response.decision == EntryDigestDecisionV2::ReuseExisting)
}

async fn await_hash_task(
    task: Option<tokio::task::JoinHandle<Result<ContentDigestV2, TransferJobError>>>,
) -> Result<ContentDigestV2, ManifestV2DataError> {
    task.ok_or(ManifestV2DataError::DigestConflict)?
        .await
        .map_err(|error| ManifestV2DataError::Internal(error.to_string()))?
        .map_err(ManifestV2DataError::Job)
}

struct PendingHashTask {
    task: Option<tokio::task::JoinHandle<Result<ContentDigestV2, TransferJobError>>>,
}

impl PendingHashTask {
    fn new(task: tokio::task::JoinHandle<Result<ContentDigestV2, TransferJobError>>) -> Self {
        Self { task: Some(task) }
    }

    fn empty() -> Self {
        Self { task: None }
    }

    fn is_finished(&self) -> bool {
        self.task
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished)
    }

    fn take(
        &mut self,
    ) -> Option<tokio::task::JoinHandle<Result<ContentDigestV2, TransferJobError>>> {
        self.task.take()
    }
}

impl Drop for PendingHashTask {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn recv_entry_start<C>(
    connection: &mut C,
    identity: JobGenerationV2,
    entry_id: u32,
) -> Result<EntryStartV2, ManifestV2DataError>
where
    C: ManifestV2FrameConnection,
{
    match connection.recv_manifest_v2_frame().await? {
        ManifestV2Frame::EntryStart(start)
            if start.identity == identity && start.entry_id == entry_id =>
        {
            Ok(start)
        }
        _ => Err(ManifestV2DataError::UnexpectedFrame(
            "waiting for EntryStart",
        )),
    }
}

async fn receive_directory<C, S>(
    entry: &ManifestEntryV2,
    identity: JobGenerationV2,
    checkpoint_index: usize,
    ledger: &mut ReceiverDataPlaneLedgerV2,
    store: &ReceiverDataPlaneStoreV2,
    sink: &mut S,
    connection: &mut C,
) -> Result<(), ManifestV2DataError>
where
    C: ManifestV2FrameConnection,
    S: ManifestV2PayloadSink,
{
    let completion = match connection.recv_manifest_v2_frame().await? {
        ManifestV2Frame::EntryComplete(completion) => completion,
        _ => {
            return Err(ManifestV2DataError::UnexpectedFrame(
                "waiting for directory completion",
            ));
        }
    };
    if completion.identity != identity
        || completion.entry_id != entry.entry_id
        || completion.final_size != 0
        || completion.final_digest != empty_digest()
        || completion.completion_choice != EntryCompletionChoiceV2::PayloadComplete
    {
        return Err(ManifestV2DataError::FinalMismatch);
    }
    sink.stage_directory(entry).await?;
    let checkpoint = &mut ledger.entries[checkpoint_index];
    checkpoint.arbiter = EntryArbiterV2::PayloadCompleteChosen;
    checkpoint.completion = Some(completion);
    store.save(ledger).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn receive_file<C, S>(
    entry: &ManifestEntryV2,
    plan: envoix_protocol::manifest_v2_frames::EntryPlanV2,
    checkpoint_index: usize,
    ledger: &mut ReceiverDataPlaneLedgerV2,
    store: &ReceiverDataPlaneStoreV2,
    sink: &mut S,
    connection: &mut C,
    completed_plaintext_bytes: &mut u64,
    total_plaintext_bytes: u64,
    progress: &dyn ManifestV2ProgressSink,
) -> Result<(), ManifestV2DataError>
where
    C: ManifestV2FrameConnection,
    S: ManifestV2PayloadSink,
{
    let identity = ledger.identity;
    let resumed_as_reuse = ledger.entries[checkpoint_index].arbiter == EntryArbiterV2::ReuseChosen;
    if plan.disposition == EntryDispositionV2::ReuseExisting {
        let digest = ledger.entries[checkpoint_index]
            .content_digest
            .ok_or(ManifestV2DataError::ReuseUnavailable)?;
        if !sink.try_choose_reuse(entry, digest).await? {
            return Err(ManifestV2DataError::ReuseUnavailable);
        }
        ledger.entries[checkpoint_index].arbiter = EntryArbiterV2::ReuseChosen;
        store.save(ledger).await?;
        if !resumed_as_reuse {
            *completed_plaintext_bytes = completed_plaintext_bytes
                .checked_add(entry.plaintext_size)
                .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
            progress.on_progress(*completed_plaintext_bytes, total_plaintext_bytes);
        }
    }

    loop {
        match connection.recv_manifest_v2_frame().await? {
            ManifestV2Frame::EntryContentDigest(digest_frame) => {
                if digest_frame.identity != identity || digest_frame.entry_id != entry.entry_id {
                    return Err(ManifestV2DataError::IdentityMismatch);
                }
                if digest_frame.decision != EntryDigestDecisionV2::Proposed {
                    return Err(ManifestV2DataError::UnexpectedFrame(
                        "receiver expected a proposed content digest",
                    ));
                }
                if set_or_equal_digest(&mut ledger.entries[checkpoint_index], digest_frame.digest)
                    .is_err()
                {
                    retire_mismatch(entry, checkpoint_index, ledger, store, sink).await?;
                    return Err(ManifestV2DataError::DigestConflict);
                }
                if ledger.entries[checkpoint_index].arbiter == EntryArbiterV2::PayloadOpen
                    && sink.try_choose_reuse(entry, digest_frame.digest).await?
                {
                    let received_plaintext = ledger.entries[checkpoint_index].plaintext_bytes;
                    ledger.entries[checkpoint_index].arbiter = EntryArbiterV2::ReuseChosen;
                    ledger.entries[checkpoint_index].next_plaintext_block = 0;
                    ledger.entries[checkpoint_index].plaintext_bytes = 0;
                    *completed_plaintext_bytes = completed_plaintext_bytes
                        .checked_add(entry.plaintext_size.saturating_sub(received_plaintext))
                        .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
                    progress.on_progress(*completed_plaintext_bytes, total_plaintext_bytes);
                }
                store.save(ledger).await?;
                let decision =
                    if ledger.entries[checkpoint_index].arbiter == EntryArbiterV2::ReuseChosen {
                        EntryDigestDecisionV2::ReuseExisting
                    } else {
                        EntryDigestDecisionV2::ContinuePayload
                    };
                connection
                    .send_manifest_v2_frame(ManifestV2Frame::EntryContentDigest(
                        EntryContentDigestFrameV2 {
                            identity,
                            entry_id: entry.entry_id,
                            digest: digest_frame.digest,
                            decision,
                        },
                    ))
                    .await?;
            }
            ManifestV2Frame::EntryBlock(block) => {
                validate_block(entry, &ledger.entries[checkpoint_index], &block)?;
                if ledger.entries[checkpoint_index].arbiter != EntryArbiterV2::PayloadOpen {
                    return Err(ManifestV2DataError::BlockOrder);
                }
                let start = ledger.entries[checkpoint_index].start.ok_or_else(|| {
                    ManifestV2DataError::InvalidLedger("entry start is missing".into())
                })?;
                let plaintext_block = decode_block(start.encoding, &block)?;
                sink.write_block(entry, &plaintext_block).await?;
                let checkpoint = &mut ledger.entries[checkpoint_index];
                checkpoint.next_plaintext_block = checkpoint
                    .next_plaintext_block
                    .checked_add(1)
                    .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
                checkpoint.plaintext_bytes = block
                    .plaintext_offset
                    .checked_add(block.plaintext_length as u64)
                    .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
                store.save(ledger).await?;
                *completed_plaintext_bytes = completed_plaintext_bytes
                    .checked_add(block.plaintext_length as u64)
                    .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
                progress.on_progress(*completed_plaintext_bytes, total_plaintext_bytes);
            }
            ManifestV2Frame::EntryComplete(completion) => {
                if completion.identity != identity || completion.entry_id != entry.entry_id {
                    return Err(ManifestV2DataError::IdentityMismatch);
                }
                if set_or_equal_digest(
                    &mut ledger.entries[checkpoint_index],
                    completion.final_digest,
                )
                .is_err()
                {
                    retire_mismatch(entry, checkpoint_index, ledger, store, sink).await?;
                    return Err(ManifestV2DataError::DigestConflict);
                }
                if completion.final_size != entry.plaintext_size {
                    retire_mismatch(entry, checkpoint_index, ledger, store, sink).await?;
                    return Err(ManifestV2DataError::FinalMismatch);
                }
                ledger.entries[checkpoint_index].completion = Some(completion);
                progress.on_phase(ManifestV2ProgressPhase::Verifying);
                let result = receive_file_completion(
                    entry,
                    checkpoint_index,
                    ledger,
                    store,
                    sink,
                    completion.final_digest,
                )
                .await;
                progress.on_phase(ManifestV2ProgressPhase::Transferring);
                return result;
            }
            ManifestV2Frame::Cancel(cancel) if cancel.identity == identity => {
                return Err(ManifestV2DataError::UnexpectedFrame("peer canceled entry"));
            }
            ManifestV2Frame::Error(error) if error.identity == identity => {
                return Err(ManifestV2DataError::UnexpectedFrame(
                    "peer reported failure",
                ));
            }
            _ => {
                return Err(ManifestV2DataError::UnexpectedFrame(
                    "receiving entry payload",
                ));
            }
        }
    }
}

async fn receive_file_completion<S>(
    entry: &ManifestEntryV2,
    checkpoint_index: usize,
    ledger: &mut ReceiverDataPlaneLedgerV2,
    store: &ReceiverDataPlaneStoreV2,
    sink: &mut S,
    final_digest: ContentDigestV2,
) -> Result<(), ManifestV2DataError>
where
    S: ManifestV2PayloadSink,
{
    let completion = ledger.entries[checkpoint_index]
        .completion
        .ok_or_else(|| ManifestV2DataError::InvalidLedger("completion was not committed".into()))?;
    match completion.completion_choice {
        EntryCompletionChoiceV2::ReuseChosen => {
            if ledger.entries[checkpoint_index].arbiter != EntryArbiterV2::ReuseChosen {
                return Err(ManifestV2DataError::ReuseUnavailable);
            }
        }
        EntryCompletionChoiceV2::PayloadComplete => {
            if ledger.entries[checkpoint_index].arbiter != EntryArbiterV2::PayloadOpen
                || ledger.entries[checkpoint_index].plaintext_bytes != entry.plaintext_size
            {
                retire_mismatch(entry, checkpoint_index, ledger, store, sink).await?;
                return Err(ManifestV2DataError::FinalMismatch);
            }
            if let Err(error) = sink.verify_payload(entry, final_digest).await {
                retire_mismatch(entry, checkpoint_index, ledger, store, sink).await?;
                return Err(error);
            }
            ledger.entries[checkpoint_index].arbiter = EntryArbiterV2::PayloadCompleteChosen;
            store.save(ledger).await?;
        }
    }
    ledger.entries[checkpoint_index].completion = Some(completion);
    store.save(ledger).await?;
    Ok(())
}

async fn retire_mismatch<S>(
    entry: &ManifestEntryV2,
    checkpoint_index: usize,
    ledger: &mut ReceiverDataPlaneLedgerV2,
    store: &ReceiverDataPlaneStoreV2,
    sink: &mut S,
) -> Result<(), ManifestV2DataError>
where
    S: ManifestV2PayloadSink,
{
    let checkpoint = &mut ledger.entries[checkpoint_index];
    checkpoint.payload_retired = true;
    checkpoint.next_plaintext_block = 0;
    checkpoint.plaintext_bytes = 0;
    checkpoint.content_digest = None;
    checkpoint.completion = None;
    store.save(ledger).await?;
    sink.retire_payload(entry).await?;
    Ok(())
}

fn validate_accept(
    offer: &ManifestOfferV2,
    accept: &ManifestAcceptV2,
) -> Result<(), ManifestV2DataError> {
    if accept.identity.job_id != offer.manifest.job_id
        || accept.identity.generation != offer.manifest.generation
        || accept.manifest_digest != offer.structural_digest
        || accept.root_plans.len() != offer.manifest.roots.len()
        || accept.entry_plans.len() != offer.manifest.entries.len()
    {
        return Err(ManifestV2DataError::AcceptMismatch);
    }
    for (index, (root, plan)) in offer
        .manifest
        .roots
        .iter()
        .zip(&accept.root_plans)
        .enumerate()
    {
        if root.root_id != index as u32 || plan.root_id != root.root_id {
            return Err(ManifestV2DataError::AcceptMismatch);
        }
    }
    for (entry, plan) in offer.manifest.entries.iter().zip(&accept.entry_plans) {
        if plan.entry_id != entry.entry_id
            || entry.kind == ManifestEntryKindV2::Directory
                && (plan.disposition != EntryDispositionV2::ReceivePayload
                    || plan.next_plaintext_block != 0)
            || plan.disposition == EntryDispositionV2::ReuseExisting
                && (!matches!(entry.content_digest, EntryContentDigestV2::Known(_))
                    || plan.next_plaintext_block != 0)
        {
            return Err(ManifestV2DataError::AcceptMismatch);
        }
        let block_bytes = DEFAULT_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES as u64;
        let total_blocks = entry
            .plaintext_size
            .checked_add(block_bytes - 1)
            .ok_or(ManifestV2DataError::ArithmeticOverflow)?
            / block_bytes;
        if plan.next_plaintext_block > total_blocks {
            return Err(ManifestV2DataError::AcceptMismatch);
        }
    }
    Ok(())
}

fn validate_resume_status(
    manifest: &ManifestV2,
    record: &SenderDeliveryRecordV2,
    status: &ResumeStatusV2,
) -> Result<(), ManifestV2DataError> {
    let accept = record.accept().ok_or_else(|| {
        ManifestV2DataError::InvalidLedger("sender Accept is missing during resume".into())
    })?;
    if status.identity != record.identity()
        || Some(status.accept_body_digest) != record.accept_body_digest()
        || status.plan_revision != accept.plan_revision
        || status.entries.len() != manifest.entries.len()
    {
        return Err(ManifestV2DataError::AcceptMismatch);
    }
    for (entry, checkpoint) in manifest.entries.iter().zip(&status.entries) {
        let total_blocks = entry
            .plaintext_size
            .checked_add(DEFAULT_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES as u64 - 1)
            .ok_or(ManifestV2DataError::ArithmeticOverflow)?
            / DEFAULT_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES as u64;
        let result_matches_arbiter = checkpoint.entry_result.as_ref().is_none_or(|result| {
            matches!(
                (checkpoint.arbiter, result.result),
                (
                    EntryArbiterV2::ReuseChosen,
                    EntryResultKindV2::ReusedExisting
                ) | (
                    EntryArbiterV2::PayloadCompleteChosen,
                    EntryResultKindV2::Saved
                )
            )
        });
        let arbiter_shape_valid = match checkpoint.arbiter {
            EntryArbiterV2::PayloadOpen => checkpoint.entry_result.is_none(),
            EntryArbiterV2::ReuseChosen => {
                entry.kind == ManifestEntryKindV2::RegularFile
                    && checkpoint.next_plaintext_block == 0
                    && checkpoint.content_digest.is_some()
            }
            EntryArbiterV2::PayloadCompleteChosen => true,
        };
        if checkpoint.entry_id != entry.entry_id
            || checkpoint.next_plaintext_block > total_blocks
            || !arbiter_shape_valid
            || !result_matches_arbiter
            || checkpoint.entry_result.as_ref().is_some_and(|result| {
                result.identity != status.identity
                    || result.entry_id != entry.entry_id
                    || completion_from_result(entry, result).is_err()
            })
        {
            return Err(ManifestV2DataError::InvalidLedger(
                "receiver resume boundary is inconsistent".into(),
            ));
        }
    }
    if matches!(
        record.phase(),
        SenderTransferPhaseV2::WaitingForReceiverSave | SenderTransferPhaseV2::Delivered
    ) {
        let completed = record.completed_data_summary().ok_or_else(|| {
            ManifestV2DataError::InvalidLedger(
                "sender waiting-for-save record has no completed result set".into(),
            )
        })?;
        if status
            .entries
            .iter()
            .map(|entry| entry.entry_result.clone())
            .collect::<Option<Vec<_>>>()
            .as_deref()
            != Some(completed.entry_results.as_slice())
        {
            return Err(ManifestV2DataError::InvalidLedger(
                "receiver resume result set differs from the sender checkpoint".into(),
            ));
        }
    }
    Ok(())
}

fn resume_plaintext_bytes(
    manifest: &ManifestV2,
    status: &ResumeStatusV2,
) -> Result<u64, ManifestV2DataError> {
    manifest
        .entries
        .iter()
        .zip(&status.entries)
        .try_fold(0_u64, |total, (entry, checkpoint)| {
            total
                .checked_add(resume_entry_plaintext_bytes(entry, checkpoint)?)
                .ok_or(ManifestV2DataError::ArithmeticOverflow)
        })
}

fn resume_entry_plaintext_bytes(
    entry: &ManifestEntryV2,
    checkpoint: &ResumeEntryV2,
) -> Result<u64, ManifestV2DataError> {
    if checkpoint.entry_id != entry.entry_id {
        return Err(ManifestV2DataError::EntryOrder);
    }
    if checkpoint.entry_result.is_some()
        || matches!(
            checkpoint.arbiter,
            EntryArbiterV2::ReuseChosen | EntryArbiterV2::PayloadCompleteChosen
        )
    {
        return Ok(entry.plaintext_size);
    }
    checkpoint
        .next_plaintext_block
        .checked_mul(DEFAULT_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES as u64)
        .map(|bytes| bytes.min(entry.plaintext_size))
        .ok_or(ManifestV2DataError::ArithmeticOverflow)
}

fn completion_from_result(
    entry: &ManifestEntryV2,
    result: &EntryResultV2,
) -> Result<EntryCompleteV2, ManifestV2DataError> {
    if result.entry_id != entry.entry_id || result.final_size != entry.plaintext_size {
        return Err(ManifestV2DataError::FinalMismatch);
    }
    let final_digest = match entry.kind {
        ManifestEntryKindV2::RegularFile => result
            .final_digest
            .ok_or(ManifestV2DataError::FinalMismatch)?,
        ManifestEntryKindV2::Directory if result.final_digest.is_none() => empty_digest(),
        ManifestEntryKindV2::Directory => return Err(ManifestV2DataError::FinalMismatch),
    };
    Ok(EntryCompleteV2 {
        identity: result.identity,
        entry_id: result.entry_id,
        final_size: result.final_size,
        final_digest,
        completion_choice: match result.result {
            EntryResultKindV2::Saved => EntryCompletionChoiceV2::PayloadComplete,
            EntryResultKindV2::ReusedExisting => EntryCompletionChoiceV2::ReuseChosen,
        },
    })
}

fn completion_from_resume_checkpoint(
    identity: JobGenerationV2,
    entry: &ManifestEntryV2,
    checkpoint: &ResumeEntryV2,
) -> Result<EntryCompleteV2, ManifestV2DataError> {
    let (final_digest, completion_choice) = match checkpoint.arbiter {
        EntryArbiterV2::PayloadOpen => {
            return Err(ManifestV2DataError::InvalidLedger(
                "open payload cannot be treated as complete".into(),
            ));
        }
        EntryArbiterV2::ReuseChosen => (
            checkpoint.content_digest.ok_or_else(|| {
                ManifestV2DataError::InvalidLedger(
                    "reused entry is missing its content digest".into(),
                )
            })?,
            EntryCompletionChoiceV2::ReuseChosen,
        ),
        EntryArbiterV2::PayloadCompleteChosen => (
            if entry.kind == ManifestEntryKindV2::Directory {
                empty_digest()
            } else {
                checkpoint.content_digest.ok_or_else(|| {
                    ManifestV2DataError::InvalidLedger(
                        "completed file is missing its content digest".into(),
                    )
                })?
            },
            EntryCompletionChoiceV2::PayloadComplete,
        ),
    };
    Ok(EntryCompleteV2 {
        identity,
        entry_id: entry.entry_id,
        final_size: entry.plaintext_size,
        final_digest,
        completion_choice,
    })
}

fn sender_checkpoint_digest(
    manifest_digest: ContentDigestV2,
    accept_body_digest: ContentDigestV2,
    intent: SenderResumeIntentV2,
) -> ContentDigestV2 {
    let mut hasher = blake3::Hasher::new_derive_key("envoix/manifest/v2/sender-checkpoint");
    hasher.update(&manifest_digest.0);
    hasher.update(&accept_body_digest.0);
    hasher.update(&[match intent {
        SenderResumeIntentV2::ContinueData => 1,
        SenderResumeIntentV2::AwaitDelivery => 2,
    }]);
    ContentDigestV2(*hasher.finalize().as_bytes())
}

pub fn sender_resume_intent(
    manifest_digest: ContentDigestV2,
    accept_body_digest: ContentDigestV2,
    request: &ResumeRequestV2,
) -> Result<SenderResumeIntentV2, ManifestV2DataError> {
    for intent in [
        SenderResumeIntentV2::ContinueData,
        SenderResumeIntentV2::AwaitDelivery,
    ] {
        if request.sender_checkpoint_digest
            == sender_checkpoint_digest(manifest_digest, accept_body_digest, intent)
        {
            return Ok(intent);
        }
    }
    Err(ManifestV2DataError::InvalidLedger(
        "sender reconnect intent is invalid".into(),
    ))
}

fn validate_block(
    entry: &ManifestEntryV2,
    checkpoint: &ReceiverEntryCheckpointV2,
    block: &EntryBlockV2,
) -> Result<(), ManifestV2DataError> {
    let start = checkpoint
        .start
        .ok_or_else(|| ManifestV2DataError::InvalidLedger("entry start is missing".into()))?;
    let expected_offset = block
        .block_index
        .checked_mul(start.plaintext_block_bytes as u64)
        .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
    let end = block
        .plaintext_offset
        .checked_add(block.plaintext_length as u64)
        .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
    if block.identity != start.identity
        || block.entry_id != entry.entry_id
        || block.block_index != checkpoint.next_plaintext_block
        || block.plaintext_offset != expected_offset
        || end > entry.plaintext_size
        || block.plaintext_length == 0
        || block.plaintext_length > start.plaintext_block_bytes
        || block.encoded_bytes.is_empty()
        || block.encoded_bytes.len() > MAX_MANIFEST_V2_BLOCK_ENCODED_BYTES as usize
        || start.encoding == EntryEncodingV2::Identity
            && block.plaintext_length as usize != block.encoded_bytes.len()
        || end < entry.plaintext_size && block.plaintext_length != start.plaintext_block_bytes
    {
        return Err(ManifestV2DataError::BlockOrder);
    }
    Ok(())
}

#[cfg(test)]
#[path = "manifest_v2_engine_tests.rs"]
mod tests;

async fn select_entry_encoding(
    source: &PreparedFileSource,
    policy: CompressionPolicyV2,
) -> Result<EntryEncodingV2, ManifestV2DataError> {
    match policy {
        CompressionPolicyV2::Never => Ok(EntryEncodingV2::Identity),
        CompressionPolicyV2::Always => Ok(EntryEncodingV2::Zstd),
        CompressionPolicyV2::Smart => {
            let mut file = source.open().await?;
            let mut sample = vec![0_u8; SMART_COMPRESSION_SAMPLE_BYTES];
            let read = file.read(&mut sample).await?;
            sample.truncate(read);
            if sample.is_empty() {
                return Ok(EntryEncodingV2::Identity);
            }
            let compressed = zstd::bulk::compress(&sample, ZSTD_COMPRESSION_LEVEL)
                .map_err(|_| ManifestV2DataError::InvalidCompressedBlock)?;
            Ok(if compressed.len() < sample.len() {
                EntryEncodingV2::Zstd
            } else {
                EntryEncodingV2::Identity
            })
        }
    }
}

fn encode_block(
    encoding: EntryEncodingV2,
    plaintext: &[u8],
) -> Result<Vec<u8>, ManifestV2DataError> {
    let encoded = match encoding {
        EntryEncodingV2::Identity => Ok(plaintext.to_vec()),
        EntryEncodingV2::Zstd => zstd::bulk::compress(plaintext, ZSTD_COMPRESSION_LEVEL)
            .map_err(|_| ManifestV2DataError::InvalidCompressedBlock),
    }?;
    if encoded.is_empty() || encoded.len() > MAX_MANIFEST_V2_BLOCK_ENCODED_BYTES as usize {
        return Err(ManifestV2DataError::InvalidCompressedBlock);
    }
    Ok(encoded)
}

fn decode_block(
    encoding: EntryEncodingV2,
    block: &EntryBlockV2,
) -> Result<EntryBlockV2, ManifestV2DataError> {
    let expected = block.plaintext_length as usize;
    let plaintext = match encoding {
        EntryEncodingV2::Identity => block.encoded_bytes.clone(),
        EntryEncodingV2::Zstd => zstd::bulk::decompress(&block.encoded_bytes, expected)
            .map_err(|_| ManifestV2DataError::InvalidCompressedBlock)?,
    };
    if plaintext.len() != expected {
        return Err(ManifestV2DataError::InvalidCompressedBlock);
    }
    let mut decoded = block.clone();
    decoded.encoded_bytes = plaintext;
    Ok(decoded)
}

fn set_or_equal_digest(
    checkpoint: &mut ReceiverEntryCheckpointV2,
    digest: ContentDigestV2,
) -> Result<(), ManifestV2DataError> {
    match checkpoint.content_digest {
        Some(existing) if existing != digest => Err(ManifestV2DataError::DigestConflict),
        Some(_) => Ok(()),
        None => {
            checkpoint.content_digest = Some(digest);
            Ok(())
        }
    }
}

async fn recv_entry_result<C>(
    connection: &mut C,
    identity: JobGenerationV2,
    entry_id: u32,
) -> Result<EntryResultV2, ManifestV2DataError>
where
    C: ManifestV2FrameConnection,
{
    match connection.recv_manifest_v2_frame().await? {
        ManifestV2Frame::EntryResult(result)
            if result.identity == identity && result.entry_id == entry_id =>
        {
            Ok(result)
        }
        _ => Err(ManifestV2DataError::UnexpectedFrame(
            "waiting for EntryResult",
        )),
    }
}

fn validate_entry_result(
    entry: &ManifestEntryV2,
    identity: JobGenerationV2,
    completion: EntryCompleteV2,
    result: &EntryResultV2,
) -> Result<(), ManifestV2DataError> {
    let expected_result = match completion.completion_choice {
        EntryCompletionChoiceV2::PayloadComplete => EntryResultKindV2::Saved,
        EntryCompletionChoiceV2::ReuseChosen => EntryResultKindV2::ReusedExisting,
    };
    if result.identity != identity
        || result.entry_id != entry.entry_id
        || result.result != expected_result
        || result.final_size != entry.plaintext_size
        || entry.kind == ManifestEntryKindV2::Directory && result.final_digest.is_some()
        || entry.kind == ManifestEntryKindV2::RegularFile && result.final_digest.is_none()
    {
        return Err(ManifestV2DataError::FinalMismatch);
    }
    Ok(())
}

fn update_completion_set(
    hasher: &mut blake3::Hasher,
    completion: EntryCompleteV2,
) -> Result<(), ManifestV2DataError> {
    let bytes = encode_manifest_v2_frame(&ManifestV2Frame::EntryComplete(completion))?;
    hasher.update(&(bytes.len() as u32).to_be_bytes());
    hasher.update(&bytes);
    Ok(())
}

fn handle_sender_control(
    frame: ManifestV2Frame,
    identity: JobGenerationV2,
) -> Result<(), ManifestV2DataError> {
    match frame {
        ManifestV2Frame::Cancel(cancel) if cancel.identity == identity => {
            Err(ManifestV2DataError::UnexpectedFrame("peer canceled job"))
        }
        ManifestV2Frame::Error(error) if error.identity == identity => Err(
            ManifestV2DataError::UnexpectedFrame("peer reported failure"),
        ),
        _ => Err(ManifestV2DataError::UnexpectedFrame(
            "sending entry payload",
        )),
    }
}

fn empty_digest() -> ContentDigestV2 {
    ContentDigestV2(*blake3::hash(&[]).as_bytes())
}
