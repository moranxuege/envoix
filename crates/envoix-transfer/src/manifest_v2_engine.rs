//! Sequential Manifest v2 identity data plane.

use std::path::PathBuf;

use async_trait::async_trait;
use envoix_error::CoreError;
use envoix_protocol::manifest_v2::{
    ContentDigestV2, DEFAULT_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES, EntryContentDigestV2,
    ManifestEntryKindV2, ManifestEntryV2, ManifestOfferV2, ManifestV2, build_manifest_offer_v2,
};
use envoix_protocol::manifest_v2_frames::{
    AcceptCommittedAckV2, EntryArbiterV2, EntryBlockV2, EntryCompleteV2, EntryCompletionChoiceV2,
    EntryContentDigestFrameV2, EntryDispositionV2, EntryEncodingV2, EntryResultKindV2,
    EntryResultV2, EntryStartV2, JobCompleteV2, JobGenerationV2, ManifestAcceptV2, ManifestV2Frame,
    ManifestV2FrameCodecError, ManifestV2FrameConnection, ResumeEntryV2, ResumeStatusV2,
    canonical_manifest_v2_frame_body_digest, encode_manifest_v2_frame,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{CanonicalTransferJob, PreparedFileSource, TransferJobError};

const RECEIVER_DATA_PLANE_SCHEMA_VERSION: u16 = 1;
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
    #[error("protocol transport failed: {0}")]
    Transport(String),
    #[error("destination provider failed: {0}")]
    Destination(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<CoreError> for ManifestV2DataError {
    fn from(error: CoreError) -> Self {
        Self::Transport(error.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedEntryV2 {
    pub result: EntryResultKindV2,
    pub final_component_override: Option<String>,
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

    async fn commit_verified(
        &mut self,
        entry: &ManifestEntryV2,
        final_digest: ContentDigestV2,
    ) -> Result<SavedEntryV2, ManifestV2DataError>;

    async fn commit_reuse(
        &mut self,
        entry: &ManifestEntryV2,
        final_digest: ContentDigestV2,
    ) -> Result<SavedEntryV2, ManifestV2DataError>;

    async fn commit_directory(
        &mut self,
        entry: &ManifestEntryV2,
    ) -> Result<SavedEntryV2, ManifestV2DataError>;

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

    fn resume_status(&self) -> ResumeStatusV2 {
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
        }
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
        fs::rename(temporary_path, final_path).await?;
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

#[derive(Clone, Copy, Debug, Default)]
pub struct ManifestV2DataPlane;

impl ManifestV2DataPlane {
    pub async fn send<C>(
        job: &CanonicalTransferJob,
        connection: &mut C,
    ) -> Result<SenderDataPlaneSummaryV2, ManifestV2DataError>
    where
        C: ManifestV2FrameConnection,
    {
        let manifest = job.manifest().ok_or(ManifestV2DataError::JobNotSealed)?;
        let offer = build_manifest_offer_v2(manifest.clone()).map_err(|error| {
            ManifestV2DataError::Codec(ManifestV2FrameCodecError::Offer(error.to_string()))
        })?;
        connection
            .send_manifest_v2_frame(ManifestV2Frame::Offer(offer.clone()))
            .await?;
        let accept = match connection.recv_manifest_v2_frame().await? {
            ManifestV2Frame::Accept(accept) => accept,
            _ => return Err(ManifestV2DataError::UnexpectedFrame("waiting for Accept")),
        };
        validate_accept(&offer, &accept)?;
        let accept_body_digest =
            canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::Accept(accept.clone()))?;
        let identity = accept.identity;
        connection
            .send_manifest_v2_frame(ManifestV2Frame::AcceptCommittedAck(AcceptCommittedAckV2 {
                identity,
                accept_body_digest,
            }))
            .await?;

        let mut completion_hasher = blake3::Hasher::new();
        let mut entry_results = Vec::with_capacity(manifest.entries.len());
        for entry in &manifest.entries {
            let plan = accept.entry_plans[entry.entry_id as usize];
            let (completion, result) = if entry.kind == ManifestEntryKindV2::Directory {
                send_directory(connection, identity, entry).await?
            } else {
                let source = job.source_for_sealed_entry(entry.entry_id)?;
                send_file(
                    connection,
                    identity,
                    entry,
                    plan,
                    source,
                    accept_body_digest,
                )
                .await?
            };
            update_completion_set(&mut completion_hasher, completion)?;
            validate_entry_result(entry, identity, completion, &result)?;
            entry_results.push(result);
        }
        let sender_completion_set_digest =
            ContentDigestV2(*completion_hasher.finalize().as_bytes());
        connection
            .send_manifest_v2_frame(ManifestV2Frame::JobComplete(JobCompleteV2 {
                identity,
                sender_completion_set_digest,
            }))
            .await?;
        Ok(SenderDataPlaneSummaryV2 {
            identity,
            accept_body_digest,
            sender_completion_set_digest,
            entry_results,
        })
    }

    pub async fn receive<C, S>(
        offer: &ManifestOfferV2,
        ledger: &mut ReceiverDataPlaneLedgerV2,
        store: &ReceiverDataPlaneStoreV2,
        sink: &mut S,
        connection: &mut C,
    ) -> Result<ReceiverDataPlaneSummaryV2, ManifestV2DataError>
    where
        C: ManifestV2FrameConnection,
        S: ManifestV2PayloadSink,
    {
        ledger.validate(&offer.manifest)?;
        let identity = ledger.identity;
        let mut completion_hasher = blake3::Hasher::new();
        let mut entry_results = Vec::with_capacity(offer.manifest.entries.len());
        for entry in &offer.manifest.entries {
            let checkpoint_index = entry.entry_id as usize;
            let plan = ledger.accept.entry_plans[checkpoint_index];
            let start = recv_entry_start(connection, identity, entry.entry_id).await?;
            if start.encoding != EntryEncodingV2::Identity {
                return Err(ManifestV2DataError::UnsupportedEncoding);
            }
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
            let result = if entry.kind == ManifestEntryKindV2::Directory {
                receive_directory(
                    entry,
                    identity,
                    checkpoint_index,
                    ledger,
                    store,
                    sink,
                    connection,
                )
                .await?
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
                )
                .await?
            };
            let completion = ledger.entries[checkpoint_index].completion.ok_or_else(|| {
                ManifestV2DataError::InvalidLedger("entry completion missing".into())
            })?;
            update_completion_set(&mut completion_hasher, completion)?;
            entry_results.push(result);
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
        ledger.sender_completion_set_digest = Some(sender_digest);
        store.save(ledger).await?;
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
) -> Result<(EntryCompleteV2, EntryResultV2), ManifestV2DataError>
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
    let result = recv_entry_result(connection, identity, entry.entry_id).await?;
    Ok((completion, result))
}

async fn send_file<C>(
    connection: &mut C,
    identity: JobGenerationV2,
    entry: &ManifestEntryV2,
    plan: envoix_protocol::manifest_v2_frames::EntryPlanV2,
    source: PreparedFileSource,
    accept_body_digest: ContentDigestV2,
) -> Result<(EntryCompleteV2, EntryResultV2), ManifestV2DataError>
where
    C: ManifestV2FrameConnection,
{
    let block_bytes = DEFAULT_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES;
    connection
        .send_manifest_v2_frame(ManifestV2Frame::EntryStart(EntryStartV2 {
            identity,
            entry_id: entry.entry_id,
            encoding: EntryEncodingV2::Identity,
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
        return Ok((
            completion,
            recv_entry_result(connection, identity, entry.entry_id).await?,
        ));
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
            reuse_chosen = send_late_digest_and_receive_status(
                connection,
                identity,
                entry.entry_id,
                digest,
                accept_body_digest,
            )
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
                    encoded_bytes: bytes,
                })
                .await?;
            if let Some(frame) = response {
                handle_sender_control(frame, identity)?;
            }
        }
        block_index = block_index
            .checked_add(1)
            .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
        plaintext_offset = plaintext_offset
            .checked_add(length as u64)
            .ok_or(ManifestV2DataError::ArithmeticOverflow)?;
    }

    let final_digest = if reuse_chosen {
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
                        accept_body_digest,
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
    let result = recv_entry_result(connection, identity, entry.entry_id).await?;
    Ok((completion, result))
}

async fn send_late_digest_and_receive_status<C>(
    connection: &mut C,
    identity: JobGenerationV2,
    entry_id: u32,
    digest: ContentDigestV2,
    accept_body_digest: ContentDigestV2,
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
            },
        ))
        .await?;
    let status = match connection.recv_manifest_v2_frame().await? {
        ManifestV2Frame::ResumeStatus(status) => status,
        frame => {
            handle_sender_control(frame, identity)?;
            return Err(ManifestV2DataError::UnexpectedFrame(
                "waiting for late-digest status",
            ));
        }
    };
    if status.identity != identity || status.accept_body_digest != accept_body_digest {
        return Err(ManifestV2DataError::IdentityMismatch);
    }
    let current = status
        .entries
        .get(entry_id as usize)
        .ok_or(ManifestV2DataError::EntryOrder)?;
    if current.entry_id != entry_id || current.content_digest != Some(digest) {
        return Err(ManifestV2DataError::DigestConflict);
    }
    Ok(current.arbiter == EntryArbiterV2::ReuseChosen)
}

async fn await_hash_task(
    task: Option<tokio::task::JoinHandle<Result<ContentDigestV2, TransferJobError>>>,
) -> Result<ContentDigestV2, ManifestV2DataError> {
    task.ok_or(ManifestV2DataError::DigestConflict)?
        .await
        .map_err(|error| ManifestV2DataError::Transport(error.to_string()))?
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
) -> Result<EntryResultV2, ManifestV2DataError>
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
    let saved = sink.commit_directory(entry).await?;
    if saved.result != EntryResultKindV2::Saved {
        return Err(ManifestV2DataError::FinalMismatch);
    }
    let result = EntryResultV2 {
        identity,
        entry_id: entry.entry_id,
        result: saved.result,
        final_size: 0,
        final_digest: None,
        final_component_override: saved.final_component_override,
    };
    let checkpoint = &mut ledger.entries[checkpoint_index];
    checkpoint.arbiter = EntryArbiterV2::PayloadCompleteChosen;
    checkpoint.completion = Some(completion);
    checkpoint.result = Some(result.clone());
    store.save(ledger).await?;
    connection
        .send_manifest_v2_frame(ManifestV2Frame::EntryResult(result.clone()))
        .await?;
    Ok(result)
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
) -> Result<EntryResultV2, ManifestV2DataError>
where
    C: ManifestV2FrameConnection,
    S: ManifestV2PayloadSink,
{
    let identity = ledger.identity;
    if plan.disposition == EntryDispositionV2::ReuseExisting {
        ledger.entries[checkpoint_index]
            .content_digest
            .ok_or(ManifestV2DataError::ReuseUnavailable)?;
        ledger.entries[checkpoint_index].arbiter = EntryArbiterV2::ReuseChosen;
        store.save(ledger).await?;
    }

    loop {
        match connection.recv_manifest_v2_frame().await? {
            ManifestV2Frame::EntryContentDigest(digest_frame) => {
                if digest_frame.identity != identity || digest_frame.entry_id != entry.entry_id {
                    return Err(ManifestV2DataError::IdentityMismatch);
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
                    ledger.entries[checkpoint_index].arbiter = EntryArbiterV2::ReuseChosen;
                    ledger.entries[checkpoint_index].next_plaintext_block = 0;
                    ledger.entries[checkpoint_index].plaintext_bytes = 0;
                }
                store.save(ledger).await?;
                connection
                    .send_manifest_v2_frame(ManifestV2Frame::ResumeStatus(ledger.resume_status()))
                    .await?;
            }
            ManifestV2Frame::EntryBlock(block) => {
                validate_block(entry, &ledger.entries[checkpoint_index], &block)?;
                if ledger.entries[checkpoint_index].arbiter != EntryArbiterV2::PayloadOpen {
                    return Err(ManifestV2DataError::BlockOrder);
                }
                sink.write_block(entry, &block).await?;
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
                return receive_file_completion(
                    entry,
                    checkpoint_index,
                    ledger,
                    store,
                    sink,
                    connection,
                    completion.final_digest,
                )
                .await;
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

#[allow(clippy::too_many_arguments)]
async fn receive_file_completion<C, S>(
    entry: &ManifestEntryV2,
    checkpoint_index: usize,
    ledger: &mut ReceiverDataPlaneLedgerV2,
    store: &ReceiverDataPlaneStoreV2,
    sink: &mut S,
    connection: &mut C,
    final_digest: ContentDigestV2,
) -> Result<EntryResultV2, ManifestV2DataError>
where
    C: ManifestV2FrameConnection,
    S: ManifestV2PayloadSink,
{
    let completion = ledger.entries[checkpoint_index]
        .completion
        .ok_or_else(|| ManifestV2DataError::InvalidLedger("completion was not committed".into()))?;
    let saved = match completion.completion_choice {
        EntryCompletionChoiceV2::ReuseChosen => {
            if ledger.entries[checkpoint_index].arbiter != EntryArbiterV2::ReuseChosen {
                return Err(ManifestV2DataError::ReuseUnavailable);
            }
            sink.commit_reuse(entry, final_digest).await?
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
            sink.commit_verified(entry, final_digest).await?
        }
    };
    let expected_result = match completion.completion_choice {
        EntryCompletionChoiceV2::PayloadComplete => EntryResultKindV2::Saved,
        EntryCompletionChoiceV2::ReuseChosen => EntryResultKindV2::ReusedExisting,
    };
    if saved.result != expected_result {
        return Err(ManifestV2DataError::FinalMismatch);
    }
    let result = EntryResultV2 {
        identity: ledger.identity,
        entry_id: entry.entry_id,
        result: saved.result,
        final_size: entry.plaintext_size,
        final_digest: Some(final_digest),
        final_component_override: saved.final_component_override,
    };
    ledger.entries[checkpoint_index].result = Some(result.clone());
    store.save(ledger).await?;
    connection
        .send_manifest_v2_frame(ManifestV2Frame::EntryResult(result.clone()))
        .await?;
    Ok(result)
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
        || block.plaintext_length as usize != block.encoded_bytes.len()
        || block.plaintext_length == 0
        || block.plaintext_length > start.plaintext_block_bytes
        || end < entry.plaintext_size && block.plaintext_length != start.plaintext_block_bytes
    {
        return Err(ManifestV2DataError::BlockOrder);
    }
    Ok(())
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
