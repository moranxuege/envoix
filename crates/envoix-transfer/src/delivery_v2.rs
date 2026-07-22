//! Receiver save proof and sender-authoritative terminal confirmation.

use std::fmt;
use std::path::PathBuf;

use envoix_protocol::manifest_v2::{ContentDigestV2, JobIdV2, ManifestOfferV2};
use envoix_protocol::manifest_v2_frames::{
    DeliveryProofAckV2, DeliveryProofV2, EntryResultV2, JobGenerationV2, ManifestAcceptV2,
    ManifestV2Frame, ManifestV2FrameConnection, ProofCapabilityV2, ProofChallengeV2,
    ProofResponseV2, canonical_manifest_v2_frame_body_digest, encode_manifest_v2_frame,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::{ManifestV2DataError, ReceiverDataPlaneSummaryV2, SenderDataPlaneSummaryV2};

const SENDER_DELIVERY_SCHEMA_VERSION: u16 = 1;
const RECEIVER_DELIVERY_SCHEMA_VERSION: u16 = 1;
const CHALLENGE_KEY_CONTEXT: &str = "envoix/manifest/v2/accept-challenge-key";
const DELIVERY_KEY_CONTEXT: &str = "envoix/manifest/v2/delivery-proof-key";
const CHALLENGE_TRANSCRIPT_CONTEXT: &[u8] = b"envoix/manifest/v2/accept-challenge";
const DELIVERY_TRANSCRIPT_CONTEXT: &[u8] = b"envoix/manifest/v2/delivery-proof";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SenderTransferPhaseV2 {
    Offering,
    Transferring,
    WaitingForReceiverSave,
    Delivered,
    Failed,
    Canceled,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SenderDeliveryRecordV2 {
    schema_version: u16,
    identity: JobGenerationV2,
    manifest_digest: ContentDigestV2,
    phase: SenderTransferPhaseV2,
    accept_body_digest: Option<ContentDigestV2>,
    proof_capability: Option<ProofCapabilityV2>,
    entry_results: Vec<EntryResultV2>,
    delivery_proof: Option<DeliveryProofV2>,
}

impl fmt::Debug for SenderDeliveryRecordV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SenderDeliveryRecordV2")
            .field("identity", &self.identity)
            .field("manifest_digest", &self.manifest_digest)
            .field("phase", &self.phase)
            .field("accept_committed", &self.accept_body_digest.is_some())
            .field("result_count", &self.entry_results.len())
            .field("proof_committed", &self.delivery_proof.is_some())
            .finish()
    }
}

impl SenderDeliveryRecordV2 {
    pub fn new(offer: &ManifestOfferV2) -> Self {
        Self {
            schema_version: SENDER_DELIVERY_SCHEMA_VERSION,
            identity: JobGenerationV2 {
                job_id: offer.manifest.job_id,
                generation: offer.manifest.generation,
            },
            manifest_digest: offer.structural_digest,
            phase: SenderTransferPhaseV2::Offering,
            accept_body_digest: None,
            proof_capability: None,
            entry_results: Vec::new(),
            delivery_proof: None,
        }
    }

    pub fn phase(&self) -> SenderTransferPhaseV2 {
        self.phase
    }

    pub fn identity(&self) -> JobGenerationV2 {
        self.identity
    }

    pub fn validate_offer(&self, offer: &ManifestOfferV2) -> Result<(), DeliveryAuthorityErrorV2> {
        let identity = JobGenerationV2 {
            job_id: offer.manifest.job_id,
            generation: offer.manifest.generation,
        };
        if self.schema_version != SENDER_DELIVERY_SCHEMA_VERSION
            || self.identity != identity
            || self.manifest_digest != offer.structural_digest
        {
            return Err(DeliveryAuthorityErrorV2::InvalidRecord);
        }
        self.validate()
    }

    pub fn commit_accept(
        &mut self,
        accept: &ManifestAcceptV2,
        accept_body_digest: ContentDigestV2,
    ) -> Result<(), DeliveryAuthorityErrorV2> {
        if matches!(
            self.phase,
            SenderTransferPhaseV2::Failed | SenderTransferPhaseV2::Canceled
        ) {
            return Err(DeliveryAuthorityErrorV2::InvalidRecord);
        }
        if accept.identity != self.identity || accept.manifest_digest != self.manifest_digest {
            return Err(DeliveryAuthorityErrorV2::IdentityMismatch);
        }
        match (self.accept_body_digest, self.proof_capability) {
            (Some(existing_digest), Some(existing_capability))
                if existing_digest == accept_body_digest
                    && existing_capability == accept.proof_capability => {}
            (None, None) => {
                self.accept_body_digest = Some(accept_body_digest);
                self.proof_capability = Some(accept.proof_capability);
            }
            _ => return Err(DeliveryAuthorityErrorV2::CapabilityMismatch),
        }
        if self.phase == SenderTransferPhaseV2::Offering {
            self.phase = SenderTransferPhaseV2::Transferring;
        }
        Ok(())
    }

    pub fn commit_results(
        &mut self,
        summary: &SenderDataPlaneSummaryV2,
    ) -> Result<(), DeliveryAuthorityErrorV2> {
        if summary.identity != self.identity
            || self.accept_body_digest != Some(summary.accept_body_digest)
        {
            return Err(DeliveryAuthorityErrorV2::IdentityMismatch);
        }
        if !matches!(
            self.phase,
            SenderTransferPhaseV2::Transferring
                | SenderTransferPhaseV2::WaitingForReceiverSave
                | SenderTransferPhaseV2::Delivered
        ) {
            return Err(DeliveryAuthorityErrorV2::InvalidRecord);
        }
        if self.entry_results.is_empty() {
            self.entry_results = summary.entry_results.clone();
        } else if self.entry_results != summary.entry_results {
            return Err(DeliveryAuthorityErrorV2::ResultMismatch);
        }
        if self.phase == SenderTransferPhaseV2::Transferring {
            self.phase = SenderTransferPhaseV2::WaitingForReceiverSave;
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), DeliveryAuthorityErrorV2> {
        let accept_committed = self.accept_body_digest.is_some() && self.proof_capability.is_some();
        if self.accept_body_digest.is_some() != self.proof_capability.is_some()
            || self
                .entry_results
                .iter()
                .enumerate()
                .any(|(index, result)| {
                    result.identity != self.identity || result.entry_id != index as u32
                })
            || self
                .delivery_proof
                .is_some_and(|proof| proof.identity != self.identity)
        {
            return Err(DeliveryAuthorityErrorV2::InvalidRecord);
        }
        if let Some(proof) = self.delivery_proof {
            let capability = self
                .proof_capability
                .ok_or(DeliveryAuthorityErrorV2::InvalidRecord)?;
            if proof.manifest_digest != self.manifest_digest
                || proof.result_set_digest != result_set_digest(&self.entry_results)?
                || proof.proof_mac
                    != delivery_mac(
                        capability,
                        proof.identity,
                        proof.manifest_digest,
                        proof.result_set_digest,
                        proof.proof_nonce,
                    )
            {
                return Err(DeliveryAuthorityErrorV2::InvalidRecord);
            }
        }
        let valid_phase = match self.phase {
            SenderTransferPhaseV2::Offering => {
                !accept_committed && self.entry_results.is_empty() && self.delivery_proof.is_none()
            }
            SenderTransferPhaseV2::Transferring => {
                accept_committed && self.entry_results.is_empty() && self.delivery_proof.is_none()
            }
            SenderTransferPhaseV2::WaitingForReceiverSave => {
                accept_committed && !self.entry_results.is_empty() && self.delivery_proof.is_none()
            }
            SenderTransferPhaseV2::Delivered => {
                accept_committed && !self.entry_results.is_empty() && self.delivery_proof.is_some()
            }
            SenderTransferPhaseV2::Failed | SenderTransferPhaseV2::Canceled => {
                self.delivery_proof.is_none()
            }
        };
        valid_phase
            .then_some(())
            .ok_or(DeliveryAuthorityErrorV2::InvalidRecord)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverDeliveryRecordV2 {
    schema_version: u16,
    identity: JobGenerationV2,
    manifest_digest: ContentDigestV2,
    result_set_digest: ContentDigestV2,
    entry_results: Vec<EntryResultV2>,
    proof_capability: ProofCapabilityV2,
    delivery_proof: Option<DeliveryProofV2>,
    proof_acknowledged: bool,
}

impl fmt::Debug for ReceiverDeliveryRecordV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReceiverDeliveryRecordV2")
            .field("identity", &self.identity)
            .field("manifest_digest", &self.manifest_digest)
            .field("result_set_digest", &self.result_set_digest)
            .field("result_count", &self.entry_results.len())
            .field("proof_committed", &self.delivery_proof.is_some())
            .field("proof_acknowledged", &self.proof_acknowledged)
            .finish()
    }
}

impl ReceiverDeliveryRecordV2 {
    pub fn new(
        offer: &ManifestOfferV2,
        accept: &ManifestAcceptV2,
        summary: &ReceiverDataPlaneSummaryV2,
    ) -> Result<Self, DeliveryAuthorityErrorV2> {
        if accept.identity != summary.identity
            || accept.identity.job_id != offer.manifest.job_id
            || accept.identity.generation != offer.manifest.generation
            || accept.manifest_digest != offer.structural_digest
            || summary.entry_results.len() != offer.manifest.entries.len()
        {
            return Err(DeliveryAuthorityErrorV2::IdentityMismatch);
        }
        let result_set_digest = result_set_digest(&summary.entry_results)?;
        Ok(Self {
            schema_version: RECEIVER_DELIVERY_SCHEMA_VERSION,
            identity: accept.identity,
            manifest_digest: offer.structural_digest,
            result_set_digest,
            entry_results: summary.entry_results.clone(),
            proof_capability: accept.proof_capability,
            delivery_proof: None,
            proof_acknowledged: false,
        }
        .validated()?)
    }

    pub fn delivery_proof(&self) -> Option<DeliveryProofV2> {
        self.delivery_proof
    }

    pub fn proof_acknowledged(&self) -> bool {
        self.proof_acknowledged
    }

    fn validate(&self) -> Result<(), DeliveryAuthorityErrorV2> {
        if self.schema_version != RECEIVER_DELIVERY_SCHEMA_VERSION
            || self
                .entry_results
                .iter()
                .enumerate()
                .any(|(index, result)| {
                    result.identity != self.identity || result.entry_id != index as u32
                })
            || result_set_digest(&self.entry_results)? != self.result_set_digest
            || self.proof_acknowledged && self.delivery_proof.is_none()
        {
            return Err(DeliveryAuthorityErrorV2::InvalidRecord);
        }
        if let Some(proof) = self.delivery_proof {
            if proof.identity != self.identity
                || proof.manifest_digest != self.manifest_digest
                || proof.result_set_digest != self.result_set_digest
                || proof.proof_mac
                    != delivery_mac(
                        self.proof_capability,
                        proof.identity,
                        proof.manifest_digest,
                        proof.result_set_digest,
                        proof.proof_nonce,
                    )
            {
                return Err(DeliveryAuthorityErrorV2::InvalidRecord);
            }
        }
        Ok(())
    }

    fn validated(self) -> Result<Self, DeliveryAuthorityErrorV2> {
        self.validate()?;
        Ok(self)
    }
}

#[derive(Debug, Error)]
pub enum DeliveryAuthorityErrorV2 {
    #[error("delivery authority belongs to another job or generation")]
    IdentityMismatch,
    #[error("receiver proof capability changed for an accepted job")]
    CapabilityMismatch,
    #[error("receiver result set changed after commit")]
    ResultMismatch,
    #[error("delivery proof MAC is invalid")]
    InvalidProof,
    #[error("delivery proof acknowledgement is invalid")]
    InvalidProofAck,
    #[error("delivery record schema is unsupported or inconsistent")]
    InvalidRecord,
    #[error("receiver entropy unavailable")]
    Entropy,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Codec(#[from] envoix_protocol::manifest_v2_frames::ManifestV2FrameCodecError),
    #[error("transport failed: {0}")]
    Transport(String),
}

impl From<DeliveryAuthorityErrorV2> for ManifestV2DataError {
    fn from(error: DeliveryAuthorityErrorV2) -> Self {
        ManifestV2DataError::Delivery(error.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct SenderDeliveryStoreV2 {
    directory: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ReceiverDeliveryStoreV2 {
    directory: PathBuf,
}

macro_rules! delivery_store {
    ($store:ty, $record:ty, $prefix:literal) => {
        impl $store {
            pub fn new(directory: impl Into<PathBuf>) -> Self {
                Self {
                    directory: directory.into(),
                }
            }

            pub async fn save(&self, record: &$record) -> Result<(), DeliveryAuthorityErrorV2> {
                record.validate()?;
                fs::create_dir_all(&self.directory).await?;
                let final_path = self.path(record.identity);
                let temporary_path = final_path.with_extension("tmp");
                let bytes = serde_json::to_vec(record)?;
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
            ) -> Result<Option<$record>, DeliveryAuthorityErrorV2> {
                let bytes = match fs::read(self.path(identity)).await {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => return Err(error.into()),
                };
                let record: $record = serde_json::from_slice(&bytes)?;
                if record.identity != identity || record.validate().is_err() {
                    return Err(DeliveryAuthorityErrorV2::InvalidRecord);
                }
                Ok(Some(record))
            }

            fn path(&self, identity: JobGenerationV2) -> PathBuf {
                self.directory.join(format!(
                    "{}-{}-{}.json",
                    $prefix,
                    encode_job_id(identity.job_id),
                    identity.generation
                ))
            }
        }
    };
}

delivery_store!(
    SenderDeliveryStoreV2,
    SenderDeliveryRecordV2,
    "sender-delivery"
);
delivery_store!(
    ReceiverDeliveryStoreV2,
    ReceiverDeliveryRecordV2,
    "receiver-delivery"
);

#[derive(Clone, Copy, Debug, Default)]
pub struct ManifestV2DeliveryAuthority;

impl ManifestV2DeliveryAuthority {
    pub async fn receiver_send_proof<C>(
        record: &mut ReceiverDeliveryRecordV2,
        store: &ReceiverDeliveryStoreV2,
        connection: &mut C,
    ) -> Result<DeliveryProofV2, DeliveryAuthorityErrorV2>
    where
        C: ManifestV2FrameConnection,
    {
        let proof = match record.delivery_proof {
            Some(proof) => proof,
            None => {
                let mut proof_nonce = [0_u8; 32];
                getrandom::fill(&mut proof_nonce).map_err(|_| DeliveryAuthorityErrorV2::Entropy)?;
                let proof = build_delivery_proof(
                    record.identity,
                    record.manifest_digest,
                    record.result_set_digest,
                    proof_nonce,
                    record.proof_capability,
                );
                record.delivery_proof = Some(proof);
                store.save(record).await?;
                proof
            }
        };
        connection
            .send_manifest_v2_frame(ManifestV2Frame::DeliveryProof(proof))
            .await
            .map_err(|error| DeliveryAuthorityErrorV2::Transport(error.to_string()))?;
        let ack = match connection
            .recv_manifest_v2_frame()
            .await
            .map_err(|error| DeliveryAuthorityErrorV2::Transport(error.to_string()))?
        {
            ManifestV2Frame::DeliveryProofAck(ack) => ack,
            _ => return Err(DeliveryAuthorityErrorV2::InvalidProofAck),
        };
        let proof_digest =
            canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::DeliveryProof(proof))?;
        if ack.identity != record.identity || ack.delivery_proof_digest != proof_digest {
            return Err(DeliveryAuthorityErrorV2::InvalidProofAck);
        }
        record.proof_acknowledged = true;
        store.save(record).await?;
        Ok(proof)
    }

    pub async fn sender_confirm_delivery<C>(
        record: &mut SenderDeliveryRecordV2,
        store: &SenderDeliveryStoreV2,
        connection: &mut C,
    ) -> Result<DeliveryProofV2, DeliveryAuthorityErrorV2>
    where
        C: ManifestV2FrameConnection,
    {
        if !matches!(
            record.phase,
            SenderTransferPhaseV2::WaitingForReceiverSave | SenderTransferPhaseV2::Delivered
        ) {
            return Err(DeliveryAuthorityErrorV2::InvalidRecord);
        }
        let proof = match connection
            .recv_manifest_v2_frame()
            .await
            .map_err(|error| DeliveryAuthorityErrorV2::Transport(error.to_string()))?
        {
            ManifestV2Frame::DeliveryProof(proof) => proof,
            _ => return Err(DeliveryAuthorityErrorV2::InvalidProof),
        };
        if record.phase == SenderTransferPhaseV2::Delivered {
            if record.delivery_proof != Some(proof) {
                return Err(DeliveryAuthorityErrorV2::InvalidProof);
            }
            send_delivery_proof_ack(record.identity, proof, connection).await?;
            return Ok(proof);
        }
        let capability = record
            .proof_capability
            .ok_or(DeliveryAuthorityErrorV2::InvalidRecord)?;
        let expected_result_digest = result_set_digest(&record.entry_results)?;
        if proof.identity != record.identity
            || proof.manifest_digest != record.manifest_digest
            || proof.result_set_digest != expected_result_digest
            || proof.proof_mac
                != delivery_mac(
                    capability,
                    proof.identity,
                    proof.manifest_digest,
                    proof.result_set_digest,
                    proof.proof_nonce,
                )
        {
            return Err(DeliveryAuthorityErrorV2::InvalidProof);
        }
        record.delivery_proof = Some(proof);
        record.phase = SenderTransferPhaseV2::Delivered;
        store.save(record).await?;
        send_delivery_proof_ack(record.identity, proof, connection).await?;
        Ok(proof)
    }

    pub fn challenge(
        identity: JobGenerationV2,
    ) -> Result<ProofChallengeV2, DeliveryAuthorityErrorV2> {
        let mut challenge_nonce = [0_u8; 32];
        getrandom::fill(&mut challenge_nonce).map_err(|_| DeliveryAuthorityErrorV2::Entropy)?;
        Ok(ProofChallengeV2 {
            identity,
            challenge_nonce,
        })
    }

    pub fn answer_challenge(
        challenge: ProofChallengeV2,
        capability: ProofCapabilityV2,
    ) -> ProofResponseV2 {
        ProofResponseV2 {
            identity: challenge.identity,
            challenge_nonce: challenge.challenge_nonce,
            challenge_mac: challenge_mac(capability, challenge.identity, challenge.challenge_nonce),
        }
    }

    pub fn verify_challenge(
        challenge: ProofChallengeV2,
        response: ProofResponseV2,
        capability: ProofCapabilityV2,
    ) -> Result<(), DeliveryAuthorityErrorV2> {
        if response.identity != challenge.identity
            || response.challenge_nonce != challenge.challenge_nonce
            || response.challenge_mac
                != challenge_mac(capability, challenge.identity, challenge.challenge_nonce)
        {
            return Err(DeliveryAuthorityErrorV2::InvalidProof);
        }
        Ok(())
    }
}

async fn send_delivery_proof_ack<C>(
    identity: JobGenerationV2,
    proof: DeliveryProofV2,
    connection: &mut C,
) -> Result<(), DeliveryAuthorityErrorV2>
where
    C: ManifestV2FrameConnection,
{
    let proof_digest =
        canonical_manifest_v2_frame_body_digest(&ManifestV2Frame::DeliveryProof(proof))?;
    connection
        .send_manifest_v2_frame(ManifestV2Frame::DeliveryProofAck(DeliveryProofAckV2 {
            identity,
            delivery_proof_digest: proof_digest,
        }))
        .await
        .map_err(|error| DeliveryAuthorityErrorV2::Transport(error.to_string()))
}

fn result_set_digest(
    results: &[EntryResultV2],
) -> Result<ContentDigestV2, DeliveryAuthorityErrorV2> {
    let mut hasher = blake3::Hasher::new_derive_key("envoix/manifest/v2/result-set");
    for (index, result) in results.iter().enumerate() {
        if result.entry_id != index as u32 {
            return Err(DeliveryAuthorityErrorV2::ResultMismatch);
        }
        let encoded = encode_manifest_v2_frame(&ManifestV2Frame::EntryResult(result.clone()))?;
        hasher.update(&(encoded.len() as u32).to_be_bytes());
        hasher.update(&encoded);
    }
    Ok(ContentDigestV2(*hasher.finalize().as_bytes()))
}

fn build_delivery_proof(
    identity: JobGenerationV2,
    manifest_digest: ContentDigestV2,
    result_set_digest: ContentDigestV2,
    proof_nonce: [u8; 32],
    capability: ProofCapabilityV2,
) -> DeliveryProofV2 {
    DeliveryProofV2 {
        identity,
        manifest_digest,
        result_set_digest,
        proof_nonce,
        proof_mac: delivery_mac(
            capability,
            identity,
            manifest_digest,
            result_set_digest,
            proof_nonce,
        ),
    }
}

fn delivery_mac(
    capability: ProofCapabilityV2,
    identity: JobGenerationV2,
    manifest_digest: ContentDigestV2,
    result_set_digest: ContentDigestV2,
    proof_nonce: [u8; 32],
) -> [u8; 32] {
    let key = blake3::derive_key(DELIVERY_KEY_CONTEXT, &capability.0);
    let mut transcript = Vec::with_capacity(160);
    transcript.extend_from_slice(DELIVERY_TRANSCRIPT_CONTEXT);
    transcript.extend_from_slice(&identity.job_id.0);
    transcript.extend_from_slice(&identity.generation.to_be_bytes());
    transcript.extend_from_slice(&manifest_digest.0);
    transcript.extend_from_slice(&result_set_digest.0);
    transcript.extend_from_slice(&proof_nonce);
    *blake3::keyed_hash(&key, &transcript).as_bytes()
}

fn challenge_mac(
    capability: ProofCapabilityV2,
    identity: JobGenerationV2,
    challenge_nonce: [u8; 32],
) -> [u8; 32] {
    let key = blake3::derive_key(CHALLENGE_KEY_CONTEXT, &capability.0);
    let mut transcript = Vec::with_capacity(96);
    transcript.extend_from_slice(CHALLENGE_TRANSCRIPT_CONTEXT);
    transcript.extend_from_slice(&identity.job_id.0);
    transcript.extend_from_slice(&identity.generation.to_be_bytes());
    transcript.extend_from_slice(&challenge_nonce);
    *blake3::keyed_hash(&key, &transcript).as_bytes()
}

fn encode_job_id(job_id: JobIdV2) -> String {
    job_id.0.iter().map(|byte| format!("{byte:02x}")).collect()
}
