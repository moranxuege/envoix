//! Canonical bounded data-plane frames for Manifest v2.

use std::fmt;

use async_trait::async_trait;
use envoix_error::CoreError;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::ProtocolError;
use crate::manifest_v2::{
    ContentDigestV2, JobIdV2, MANIFEST_V2_PROTOCOL_VERSION, MAX_MANIFEST_V2_BLOCK_ENCODED_BYTES,
    MAX_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES, MAX_MANIFEST_V2_COMPONENT_BYTES,
    MAX_MANIFEST_V2_ENCODED_BYTES, MAX_MANIFEST_V2_ENTRIES, MAX_MANIFEST_V2_ROOTS, ManifestOfferV2,
    ManifestV2FrameType, decode_manifest_offer_v2, encode_manifest_offer_v2,
};

const MAGIC: &[u8; 4] = b"ENV2";
const HEADER_BYTES: usize = 12;
const COMMON_PREFIX_BYTES: usize = 20;
const DIGEST_BYTES: usize = 32;
const NONCE_BYTES: usize = 32;
const MAX_CONTROL_FRAME_BYTES: usize = MAX_MANIFEST_V2_ENCODED_BYTES;
const ENTRY_BLOCK_FIXED_PAYLOAD_BYTES: usize = COMMON_PREFIX_BYTES + 4 + 8 + 8 + 4 + 4;
const RESUME_REQUEST_FIXED_PAYLOAD_BYTES: usize =
    COMMON_PREFIX_BYTES + 4 + DIGEST_BYTES * 2 + NONCE_BYTES;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JobGenerationV2 {
    pub job_id: JobIdV2,
    pub generation: u32,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProofCapabilityV2(pub [u8; DIGEST_BYTES]);

impl fmt::Debug for ProofCapabilityV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProofCapabilityV2(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u8)]
pub enum EntryDispositionV2 {
    ReceivePayload = 0,
    ReuseExisting = 1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u8)]
pub enum EntryEncodingV2 {
    Identity = 0,
    Zstd = 1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u8)]
pub enum EntryDigestDecisionV2 {
    Proposed = 0,
    ContinuePayload = 1,
    ReuseExisting = 2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u8)]
pub enum EntryCompletionChoiceV2 {
    PayloadComplete = 0,
    ReuseChosen = 1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u8)]
pub enum EntryResultKindV2 {
    Saved = 0,
    ReusedExisting = 1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u8)]
pub enum EntryArbiterV2 {
    PayloadOpen = 0,
    ReuseChosen = 1,
    PayloadCompleteChosen = 2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u8)]
pub enum CancelScopeV2 {
    Job = 0,
    Entry = 1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u8)]
pub enum ManifestFailurePhaseV2 {
    Offer = 0,
    Destination = 1,
    Payload = 2,
    Verify = 3,
    Save = 4,
    Proof = 5,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct RootPlanV2 {
    pub root_id: u32,
    pub planned_name: String,
}

impl fmt::Debug for RootPlanV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RootPlanV2")
            .field("root_id", &self.root_id)
            .field("planned_name_bytes", &self.planned_name.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntryPlanV2 {
    pub entry_id: u32,
    pub disposition: EntryDispositionV2,
    pub next_plaintext_block: u64,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestAcceptV2 {
    pub identity: JobGenerationV2,
    pub manifest_digest: ContentDigestV2,
    pub accept_nonce: [u8; NONCE_BYTES],
    pub proof_capability: ProofCapabilityV2,
    pub plan_revision: u32,
    pub root_plans: Vec<RootPlanV2>,
    pub entry_plans: Vec<EntryPlanV2>,
}

impl fmt::Debug for ManifestAcceptV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManifestAcceptV2")
            .field("identity", &self.identity)
            .field("manifest_digest", &self.manifest_digest)
            .field("accept_nonce", &"<redacted>")
            .field("proof_capability", &self.proof_capability)
            .field("plan_revision", &self.plan_revision)
            .field("root_plan_count", &self.root_plans.len())
            .field("entry_plan_count", &self.entry_plans.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntryStartV2 {
    pub identity: JobGenerationV2,
    pub entry_id: u32,
    pub encoding: EntryEncodingV2,
    pub plaintext_block_bytes: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntryContentDigestFrameV2 {
    pub identity: JobGenerationV2,
    pub entry_id: u32,
    pub digest: ContentDigestV2,
    pub decision: EntryDigestDecisionV2,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntryBlockV2 {
    pub identity: JobGenerationV2,
    pub entry_id: u32,
    pub block_index: u64,
    pub plaintext_offset: u64,
    pub plaintext_length: u32,
    pub encoded_bytes: Vec<u8>,
}

impl fmt::Debug for EntryBlockV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EntryBlockV2")
            .field("identity", &self.identity)
            .field("entry_id", &self.entry_id)
            .field("block_index", &self.block_index)
            .field("plaintext_offset", &self.plaintext_offset)
            .field("plaintext_length", &self.plaintext_length)
            .field("encoded_length", &self.encoded_bytes.len())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntryCompleteV2 {
    pub identity: JobGenerationV2,
    pub entry_id: u32,
    pub final_size: u64,
    pub final_digest: ContentDigestV2,
    pub completion_choice: EntryCompletionChoiceV2,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntryResultV2 {
    pub identity: JobGenerationV2,
    pub entry_id: u32,
    pub result: EntryResultKindV2,
    pub final_size: u64,
    pub final_digest: Option<ContentDigestV2>,
    pub final_component_override: Option<String>,
}

impl fmt::Debug for EntryResultV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EntryResultV2")
            .field("identity", &self.identity)
            .field("entry_id", &self.entry_id)
            .field("result", &self.result)
            .field("final_size", &self.final_size)
            .field("final_digest", &self.final_digest)
            .field(
                "final_component_override_bytes",
                &self.final_component_override.as_ref().map(String::len),
            )
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JobCompleteV2 {
    pub identity: JobGenerationV2,
    pub sender_completion_set_digest: ContentDigestV2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeliveryProofV2 {
    pub identity: JobGenerationV2,
    pub manifest_digest: ContentDigestV2,
    pub result_set_digest: ContentDigestV2,
    pub proof_nonce: [u8; NONCE_BYTES],
    pub proof_mac: [u8; DIGEST_BYTES],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResumeRequestV2 {
    pub identity: JobGenerationV2,
    pub offer: ManifestOfferV2,
    pub accept_body_digest: ContentDigestV2,
    pub sender_checkpoint_digest: ContentDigestV2,
    pub challenge_nonce: [u8; NONCE_BYTES],
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResumeEntryV2 {
    pub entry_id: u32,
    pub arbiter: EntryArbiterV2,
    pub next_plaintext_block: u64,
    pub content_digest: Option<ContentDigestV2>,
    pub entry_result: Option<EntryResultV2>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResumeStatusV2 {
    pub identity: JobGenerationV2,
    pub accept_body_digest: ContentDigestV2,
    pub plan_revision: u32,
    pub entries: Vec<ResumeEntryV2>,
    pub challenge_nonce: [u8; NONCE_BYTES],
    pub challenge_mac: [u8; DIGEST_BYTES],
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CancelV2 {
    pub identity: JobGenerationV2,
    pub scope: CancelScopeV2,
    pub entry_id: Option<u32>,
    pub failure_code: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestErrorV2 {
    pub identity: JobGenerationV2,
    pub failure_code: u32,
    pub phase: ManifestFailurePhaseV2,
    pub entry_id: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifestV2Frame {
    Offer(ManifestOfferV2),
    Accept(ManifestAcceptV2),
    EntryStart(EntryStartV2),
    EntryContentDigest(EntryContentDigestFrameV2),
    EntryBlock(EntryBlockV2),
    EntryComplete(EntryCompleteV2),
    EntryResult(EntryResultV2),
    JobComplete(JobCompleteV2),
    DeliveryProof(DeliveryProofV2),
    ResumeRequest(ResumeRequestV2),
    ResumeStatus(ResumeStatusV2),
    Cancel(CancelV2),
    Error(ManifestErrorV2),
}

#[derive(Debug, Error)]
pub enum ManifestV2FrameCodecError {
    #[error("Manifest v2 frame is truncated")]
    Truncated,
    #[error("Manifest v2 frame magic is invalid")]
    BadMagic,
    #[error("unsupported Manifest v2 frame version {0}")]
    UnsupportedVersion(u16),
    #[error("unknown Manifest v2 frame type {0}")]
    UnknownFrameType(u16),
    #[error("Manifest v2 frame length {actual} does not match declared {declared}")]
    LengthMismatch { declared: usize, actual: usize },
    #[error("Manifest v2 frame exceeds its bounded allocation")]
    FrameTooLarge,
    #[error("Manifest v2 frame has trailing bytes")]
    TrailingBytes,
    #[error("unknown {name} tag {value}")]
    UnknownTag { name: &'static str, value: u8 },
    #[error("Manifest v2 array count exceeds its contract")]
    CountTooLarge,
    #[error("Manifest v2 string is invalid UTF-8")]
    InvalidUtf8,
    #[error("Manifest v2 component is unsafe")]
    UnsafeComponent,
    #[error("Manifest v2 identity is invalid")]
    InvalidIdentity,
    #[error("Manifest v2 resume challenge is invalid")]
    InvalidChallenge,
    #[error("Manifest v2 entry id is outside the offered entry set")]
    InvalidEntryId,
    #[error("Manifest v2 root id or ordering is invalid")]
    InvalidRootId,
    #[error("Manifest v2 block shape is invalid")]
    InvalidBlock,
    #[error("Manifest v2 optional field tag is invalid")]
    InvalidOptional,
    #[error("Manifest v2 Cancel scope and entry disagree")]
    InvalidCancelScope,
    #[error("Manifest Offer codec rejected the frame: {0}")]
    Offer(String),
    #[error("Offer must use the structural Manifest codec")]
    UnexpectedOfferPayload,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[async_trait]
pub trait ManifestV2FrameConnection: Send {
    async fn send_manifest_v2_frame(&mut self, frame: ManifestV2Frame)
    -> Result<(), ProtocolError>;

    async fn recv_manifest_v2_frame(&mut self) -> Result<ManifestV2Frame, ProtocolError>;

    async fn send_entry_block_or_recv_frame(
        &mut self,
        block: EntryBlockV2,
    ) -> Result<Option<ManifestV2Frame>, ProtocolError> {
        self.send_manifest_v2_frame(ManifestV2Frame::EntryBlock(block))
            .await?;
        Ok(None)
    }

    fn export_keying_material(
        &self,
        label: &[u8],
        context: &[u8],
    ) -> Result<[u8; 32], ProtocolError>;

    async fn close(&mut self) -> Result<(), ProtocolError>;
}

pub fn encode_manifest_v2_frame(
    frame: &ManifestV2Frame,
) -> Result<Vec<u8>, ManifestV2FrameCodecError> {
    if let ManifestV2Frame::Offer(offer) = frame {
        let rebuilt = crate::manifest_v2::build_manifest_offer_v2(offer.manifest.clone())
            .map_err(|error| ManifestV2FrameCodecError::Offer(error.to_string()))?;
        if rebuilt.structural_digest != offer.structural_digest {
            return Err(ManifestV2FrameCodecError::Offer(
                "structural digest does not match the canonical Manifest body".into(),
            ));
        }
        return encode_manifest_offer_v2(&offer.manifest)
            .map_err(|error| ManifestV2FrameCodecError::Offer(error.to_string()));
    }

    let mut payload = Vec::new();
    let frame_type = encode_payload(frame, &mut payload)?;
    let maximum = maximum_payload_bytes(frame_type)?;
    if payload.len() > maximum {
        return Err(ManifestV2FrameCodecError::FrameTooLarge);
    }
    let payload_length =
        u32::try_from(payload.len()).map_err(|_| ManifestV2FrameCodecError::FrameTooLarge)?;
    let mut encoded = Vec::with_capacity(HEADER_BYTES + payload.len());
    encoded.extend_from_slice(MAGIC);
    encoded.extend_from_slice(&MANIFEST_V2_PROTOCOL_VERSION.to_be_bytes());
    encoded.extend_from_slice(&(frame_type as u16).to_be_bytes());
    encoded.extend_from_slice(&payload_length.to_be_bytes());
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

pub fn decode_manifest_v2_frame(
    encoded: &[u8],
) -> Result<ManifestV2Frame, ManifestV2FrameCodecError> {
    let header = parse_header(encoded)?;
    if encoded.len() != HEADER_BYTES + header.payload_length {
        return Err(ManifestV2FrameCodecError::LengthMismatch {
            declared: header.payload_length,
            actual: encoded.len().saturating_sub(HEADER_BYTES),
        });
    }
    if header.frame_type == ManifestV2FrameType::Offer {
        return decode_manifest_offer_v2(encoded)
            .map(ManifestV2Frame::Offer)
            .map_err(|error| ManifestV2FrameCodecError::Offer(error.to_string()));
    }
    let maximum = maximum_payload_bytes(header.frame_type)?;
    if header.payload_length > maximum {
        return Err(ManifestV2FrameCodecError::FrameTooLarge);
    }
    let mut reader = Reader::new(&encoded[HEADER_BYTES..]);
    let frame = decode_payload(header.frame_type, &mut reader)?;
    reader.finish()?;
    Ok(frame)
}

pub fn canonical_manifest_v2_frame_body_digest(
    frame: &ManifestV2Frame,
) -> Result<ContentDigestV2, ManifestV2FrameCodecError> {
    let encoded = encode_manifest_v2_frame(frame)?;
    let body = encoded
        .get(HEADER_BYTES..)
        .ok_or(ManifestV2FrameCodecError::Truncated)?;
    Ok(ContentDigestV2(*blake3::hash(body).as_bytes()))
}

pub async fn read_manifest_v2_frame<R>(
    reader: &mut R,
) -> Result<ManifestV2Frame, ManifestV2FrameCodecError>
where
    R: AsyncRead + Unpin,
{
    let mut header_bytes = [0_u8; HEADER_BYTES];
    reader.read_exact(&mut header_bytes).await?;
    let header = parse_header(&header_bytes)?;
    let maximum = maximum_payload_bytes(header.frame_type)?;
    if header.payload_length > maximum {
        return Err(ManifestV2FrameCodecError::FrameTooLarge);
    }
    let mut encoded = Vec::with_capacity(HEADER_BYTES + header.payload_length);
    encoded.extend_from_slice(&header_bytes);
    encoded.resize(HEADER_BYTES + header.payload_length, 0);
    reader.read_exact(&mut encoded[HEADER_BYTES..]).await?;
    decode_manifest_v2_frame(&encoded)
}

pub async fn write_manifest_v2_frame<W>(
    writer: &mut W,
    frame: &ManifestV2Frame,
) -> Result<(), ManifestV2FrameCodecError>
where
    W: AsyncWrite + Unpin,
{
    let encoded = encode_manifest_v2_frame(frame)?;
    writer.write_all(&encoded).await?;
    writer.flush().await?;
    Ok(())
}

#[derive(Clone, Copy)]
struct Header {
    frame_type: ManifestV2FrameType,
    payload_length: usize,
}

fn maximum_payload_bytes(
    frame_type: ManifestV2FrameType,
) -> Result<usize, ManifestV2FrameCodecError> {
    match frame_type {
        ManifestV2FrameType::EntryBlock => ENTRY_BLOCK_FIXED_PAYLOAD_BYTES
            .checked_add(MAX_MANIFEST_V2_BLOCK_ENCODED_BYTES as usize)
            .ok_or(ManifestV2FrameCodecError::FrameTooLarge),
        ManifestV2FrameType::ResumeRequest => RESUME_REQUEST_FIXED_PAYLOAD_BYTES
            .checked_add(MAX_MANIFEST_V2_ENCODED_BYTES)
            .ok_or(ManifestV2FrameCodecError::FrameTooLarge),
        _ => Ok(MAX_CONTROL_FRAME_BYTES - HEADER_BYTES),
    }
}

fn parse_header(encoded: &[u8]) -> Result<Header, ManifestV2FrameCodecError> {
    let header = encoded
        .get(..HEADER_BYTES)
        .ok_or(ManifestV2FrameCodecError::Truncated)?;
    if &header[..4] != MAGIC {
        return Err(ManifestV2FrameCodecError::BadMagic);
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    if version != MANIFEST_V2_PROTOCOL_VERSION {
        return Err(ManifestV2FrameCodecError::UnsupportedVersion(version));
    }
    let raw_type = u16::from_be_bytes([header[6], header[7]]);
    let frame_type = frame_type(raw_type)?;
    let payload_length =
        u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
    Ok(Header {
        frame_type,
        payload_length,
    })
}

fn frame_type(value: u16) -> Result<ManifestV2FrameType, ManifestV2FrameCodecError> {
    match value {
        1 => Ok(ManifestV2FrameType::Offer),
        2 => Ok(ManifestV2FrameType::Accept),
        3 => Ok(ManifestV2FrameType::EntryStart),
        4 => Ok(ManifestV2FrameType::EntryContentDigest),
        5 => Ok(ManifestV2FrameType::EntryBlock),
        6 => Ok(ManifestV2FrameType::EntryComplete),
        7 => Ok(ManifestV2FrameType::EntryResult),
        8 => Ok(ManifestV2FrameType::JobComplete),
        9 => Ok(ManifestV2FrameType::DeliveryProof),
        10 => Ok(ManifestV2FrameType::ResumeRequest),
        11 => Ok(ManifestV2FrameType::ResumeStatus),
        12 => Ok(ManifestV2FrameType::Cancel),
        13 => Ok(ManifestV2FrameType::Error),
        other => Err(ManifestV2FrameCodecError::UnknownFrameType(other)),
    }
}

fn encode_payload(
    frame: &ManifestV2Frame,
    output: &mut Vec<u8>,
) -> Result<ManifestV2FrameType, ManifestV2FrameCodecError> {
    match frame {
        ManifestV2Frame::Offer(_) => Err(ManifestV2FrameCodecError::UnexpectedOfferPayload),
        ManifestV2Frame::Accept(value) => {
            identity(output, value.identity)?;
            if value.proof_capability.0 == [0; DIGEST_BYTES] || value.plan_revision == 0 {
                return Err(ManifestV2FrameCodecError::InvalidIdentity);
            }
            digest(output, value.manifest_digest);
            output.extend_from_slice(&value.accept_nonce);
            output.extend_from_slice(&value.proof_capability.0);
            u32_value(output, value.plan_revision);
            count(output, value.root_plans.len(), MAX_MANIFEST_V2_ROOTS)?;
            for (index, root) in value.root_plans.iter().enumerate() {
                if root.root_id != index as u32 {
                    return Err(ManifestV2FrameCodecError::InvalidRootId);
                }
                u32_value(output, root.root_id);
                component(output, &root.planned_name)?;
            }
            count(output, value.entry_plans.len(), MAX_MANIFEST_V2_ENTRIES)?;
            for (index, entry) in value.entry_plans.iter().enumerate() {
                if entry.entry_id != index as u32 {
                    return Err(ManifestV2FrameCodecError::InvalidEntryId);
                }
                u32_value(output, entry.entry_id);
                output.push(entry.disposition as u8);
                u64_value(output, entry.next_plaintext_block);
            }
            Ok(ManifestV2FrameType::Accept)
        }
        ManifestV2Frame::EntryStart(value) => {
            identity(output, value.identity)?;
            checked_entry_id(value.entry_id)?;
            if value.plaintext_block_bytes == 0
                || value.plaintext_block_bytes > MAX_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES
            {
                return Err(ManifestV2FrameCodecError::InvalidBlock);
            }
            u32_value(output, value.entry_id);
            output.push(value.encoding as u8);
            u32_value(output, value.plaintext_block_bytes);
            Ok(ManifestV2FrameType::EntryStart)
        }
        ManifestV2Frame::EntryContentDigest(value) => {
            identity(output, value.identity)?;
            checked_entry_id(value.entry_id)?;
            u32_value(output, value.entry_id);
            digest(output, value.digest);
            output.push(value.decision as u8);
            Ok(ManifestV2FrameType::EntryContentDigest)
        }
        ManifestV2Frame::EntryBlock(value) => {
            identity(output, value.identity)?;
            checked_entry_id(value.entry_id)?;
            if value.plaintext_length == 0
                || value.plaintext_length > MAX_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES
                || value.encoded_bytes.is_empty()
                || value.encoded_bytes.len() > MAX_MANIFEST_V2_BLOCK_ENCODED_BYTES as usize
            {
                return Err(ManifestV2FrameCodecError::InvalidBlock);
            }
            u32_value(output, value.entry_id);
            u64_value(output, value.block_index);
            u64_value(output, value.plaintext_offset);
            u32_value(output, value.plaintext_length);
            u32_value(
                output,
                u32::try_from(value.encoded_bytes.len())
                    .map_err(|_| ManifestV2FrameCodecError::InvalidBlock)?,
            );
            output.extend_from_slice(&value.encoded_bytes);
            Ok(ManifestV2FrameType::EntryBlock)
        }
        ManifestV2Frame::EntryComplete(value) => {
            identity(output, value.identity)?;
            checked_entry_id(value.entry_id)?;
            u32_value(output, value.entry_id);
            u64_value(output, value.final_size);
            digest(output, value.final_digest);
            output.push(value.completion_choice as u8);
            Ok(ManifestV2FrameType::EntryComplete)
        }
        ManifestV2Frame::EntryResult(value) => {
            encode_entry_result_body(output, value)?;
            Ok(ManifestV2FrameType::EntryResult)
        }
        ManifestV2Frame::JobComplete(value) => {
            identity(output, value.identity)?;
            digest(output, value.sender_completion_set_digest);
            Ok(ManifestV2FrameType::JobComplete)
        }
        ManifestV2Frame::DeliveryProof(value) => {
            identity(output, value.identity)?;
            digest(output, value.manifest_digest);
            digest(output, value.result_set_digest);
            output.extend_from_slice(&value.proof_nonce);
            output.extend_from_slice(&value.proof_mac);
            Ok(ManifestV2FrameType::DeliveryProof)
        }
        ManifestV2Frame::ResumeRequest(value) => {
            identity(output, value.identity)?;
            if value.challenge_nonce == [0; NONCE_BYTES] {
                return Err(ManifestV2FrameCodecError::InvalidChallenge);
            }
            if value.identity.job_id != value.offer.manifest.job_id
                || value.identity.generation != value.offer.manifest.generation
            {
                return Err(ManifestV2FrameCodecError::InvalidIdentity);
            }
            let rebuilt_offer =
                crate::manifest_v2::build_manifest_offer_v2(value.offer.manifest.clone())
                    .map_err(|error| ManifestV2FrameCodecError::Offer(error.to_string()))?;
            if rebuilt_offer.structural_digest != value.offer.structural_digest {
                return Err(ManifestV2FrameCodecError::Offer(
                    "structural digest does not match the canonical Manifest body".into(),
                ));
            }
            let encoded_offer = encode_manifest_offer_v2(&value.offer.manifest)
                .map_err(|error| ManifestV2FrameCodecError::Offer(error.to_string()))?;
            u32_value(
                output,
                u32::try_from(encoded_offer.len())
                    .map_err(|_| ManifestV2FrameCodecError::FrameTooLarge)?,
            );
            output.extend_from_slice(&encoded_offer);
            digest(output, value.accept_body_digest);
            digest(output, value.sender_checkpoint_digest);
            output.extend_from_slice(&value.challenge_nonce);
            Ok(ManifestV2FrameType::ResumeRequest)
        }
        ManifestV2Frame::ResumeStatus(value) => {
            identity(output, value.identity)?;
            if value.challenge_nonce == [0; NONCE_BYTES] || value.challenge_mac == [0; DIGEST_BYTES]
            {
                return Err(ManifestV2FrameCodecError::InvalidChallenge);
            }
            digest(output, value.accept_body_digest);
            u32_value(output, value.plan_revision);
            count(output, value.entries.len(), MAX_MANIFEST_V2_ENTRIES)?;
            for (index, entry) in value.entries.iter().enumerate() {
                if entry.entry_id != index as u32 {
                    return Err(ManifestV2FrameCodecError::InvalidEntryId);
                }
                u32_value(output, entry.entry_id);
                output.push(entry.arbiter as u8);
                u64_value(output, entry.next_plaintext_block);
                optional_digest(output, entry.content_digest);
                match &entry.entry_result {
                    Some(result) => {
                        output.push(1);
                        encode_entry_result_body(output, result)?;
                    }
                    None => output.push(0),
                }
            }
            output.extend_from_slice(&value.challenge_nonce);
            output.extend_from_slice(&value.challenge_mac);
            Ok(ManifestV2FrameType::ResumeStatus)
        }
        ManifestV2Frame::Cancel(value) => {
            identity(output, value.identity)?;
            if matches!(value.scope, CancelScopeV2::Job) != value.entry_id.is_none() {
                return Err(ManifestV2FrameCodecError::InvalidCancelScope);
            }
            output.push(value.scope as u8);
            optional_u32(output, value.entry_id);
            u32_value(output, value.failure_code);
            Ok(ManifestV2FrameType::Cancel)
        }
        ManifestV2Frame::Error(value) => {
            identity(output, value.identity)?;
            u32_value(output, value.failure_code);
            output.push(value.phase as u8);
            optional_u32(output, value.entry_id);
            Ok(ManifestV2FrameType::Error)
        }
    }
}

fn decode_payload(
    frame_type: ManifestV2FrameType,
    reader: &mut Reader<'_>,
) -> Result<ManifestV2Frame, ManifestV2FrameCodecError> {
    let frame = match frame_type {
        ManifestV2FrameType::Offer => {
            return Err(ManifestV2FrameCodecError::UnexpectedOfferPayload);
        }
        ManifestV2FrameType::Accept => {
            let identity = reader.identity()?;
            let manifest_digest = reader.digest()?;
            let accept_nonce = reader.array()?;
            let proof_capability = ProofCapabilityV2(reader.array()?);
            let plan_revision = reader.u32()?;
            if proof_capability.0 == [0; DIGEST_BYTES] || plan_revision == 0 {
                return Err(ManifestV2FrameCodecError::InvalidIdentity);
            }
            let root_count = reader.count(MAX_MANIFEST_V2_ROOTS)?;
            let mut root_plans = Vec::with_capacity(root_count);
            for index in 0..root_count {
                let root_id = reader.u32()?;
                if root_id != index as u32 {
                    return Err(ManifestV2FrameCodecError::InvalidRootId);
                }
                root_plans.push(RootPlanV2 {
                    root_id,
                    planned_name: reader.component()?,
                });
            }
            let entry_count = reader.count(MAX_MANIFEST_V2_ENTRIES)?;
            let mut entry_plans = Vec::with_capacity(entry_count);
            for index in 0..entry_count {
                let entry_id = reader.u32()?;
                if entry_id != index as u32 {
                    return Err(ManifestV2FrameCodecError::InvalidEntryId);
                }
                entry_plans.push(EntryPlanV2 {
                    entry_id,
                    disposition: entry_disposition(reader.u8()?)?,
                    next_plaintext_block: reader.u64()?,
                });
            }
            ManifestV2Frame::Accept(ManifestAcceptV2 {
                identity,
                manifest_digest,
                accept_nonce,
                proof_capability,
                plan_revision,
                root_plans,
                entry_plans,
            })
        }
        ManifestV2FrameType::EntryStart => {
            let value = EntryStartV2 {
                identity: reader.identity()?,
                entry_id: reader.entry_id()?,
                encoding: entry_encoding(reader.u8()?)?,
                plaintext_block_bytes: reader.u32()?,
            };
            if value.plaintext_block_bytes == 0
                || value.plaintext_block_bytes > MAX_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES
            {
                return Err(ManifestV2FrameCodecError::InvalidBlock);
            }
            ManifestV2Frame::EntryStart(value)
        }
        ManifestV2FrameType::EntryContentDigest => {
            ManifestV2Frame::EntryContentDigest(EntryContentDigestFrameV2 {
                identity: reader.identity()?,
                entry_id: reader.entry_id()?,
                digest: reader.digest()?,
                decision: entry_digest_decision(reader.u8()?)?,
            })
        }
        ManifestV2FrameType::EntryBlock => {
            let value = EntryBlockV2 {
                identity: reader.identity()?,
                entry_id: reader.entry_id()?,
                block_index: reader.u64()?,
                plaintext_offset: reader.u64()?,
                plaintext_length: reader.u32()?,
                encoded_bytes: reader.bytes(MAX_MANIFEST_V2_BLOCK_ENCODED_BYTES as usize)?,
            };
            if value.plaintext_length == 0
                || value.plaintext_length > MAX_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES
                || value.encoded_bytes.is_empty()
            {
                return Err(ManifestV2FrameCodecError::InvalidBlock);
            }
            ManifestV2Frame::EntryBlock(value)
        }
        ManifestV2FrameType::EntryComplete => ManifestV2Frame::EntryComplete(EntryCompleteV2 {
            identity: reader.identity()?,
            entry_id: reader.entry_id()?,
            final_size: reader.u64()?,
            final_digest: reader.digest()?,
            completion_choice: entry_completion(reader.u8()?)?,
        }),
        ManifestV2FrameType::EntryResult => ManifestV2Frame::EntryResult(reader.entry_result()?),
        ManifestV2FrameType::JobComplete => ManifestV2Frame::JobComplete(JobCompleteV2 {
            identity: reader.identity()?,
            sender_completion_set_digest: reader.digest()?,
        }),
        ManifestV2FrameType::DeliveryProof => ManifestV2Frame::DeliveryProof(DeliveryProofV2 {
            identity: reader.identity()?,
            manifest_digest: reader.digest()?,
            result_set_digest: reader.digest()?,
            proof_nonce: reader.array()?,
            proof_mac: reader.array()?,
        }),
        ManifestV2FrameType::ResumeRequest => {
            let identity = reader.identity()?;
            let encoded_offer = reader.bytes(MAX_MANIFEST_V2_ENCODED_BYTES)?;
            let offer = decode_manifest_offer_v2(&encoded_offer)
                .map_err(|error| ManifestV2FrameCodecError::Offer(error.to_string()))?;
            if identity.job_id != offer.manifest.job_id
                || identity.generation != offer.manifest.generation
            {
                return Err(ManifestV2FrameCodecError::InvalidIdentity);
            }
            let request = ResumeRequestV2 {
                identity,
                offer,
                accept_body_digest: reader.digest()?,
                sender_checkpoint_digest: reader.digest()?,
                challenge_nonce: reader.array()?,
            };
            if request.challenge_nonce == [0; NONCE_BYTES] {
                return Err(ManifestV2FrameCodecError::InvalidChallenge);
            }
            ManifestV2Frame::ResumeRequest(request)
        }
        ManifestV2FrameType::ResumeStatus => {
            let identity = reader.identity()?;
            let accept_body_digest = reader.digest()?;
            let plan_revision = reader.u32()?;
            let count = reader.count(MAX_MANIFEST_V2_ENTRIES)?;
            let mut entries = Vec::with_capacity(count);
            for index in 0..count {
                let entry_id = reader.entry_id()?;
                if entry_id != index as u32 {
                    return Err(ManifestV2FrameCodecError::InvalidEntryId);
                }
                let arbiter = entry_arbiter(reader.u8()?)?;
                let next_plaintext_block = reader.u64()?;
                let content_digest = reader.optional_digest()?;
                let entry_result = match reader.u8()? {
                    0 => None,
                    1 => Some(reader.entry_result()?),
                    _ => return Err(ManifestV2FrameCodecError::InvalidOptional),
                };
                if entry_result.as_ref().is_some_and(|result| {
                    result.identity != identity || result.entry_id != entry_id
                }) {
                    return Err(ManifestV2FrameCodecError::InvalidIdentity);
                }
                entries.push(ResumeEntryV2 {
                    entry_id,
                    arbiter,
                    next_plaintext_block,
                    content_digest,
                    entry_result,
                });
            }
            let status = ResumeStatusV2 {
                identity,
                accept_body_digest,
                plan_revision,
                entries,
                challenge_nonce: reader.array()?,
                challenge_mac: reader.array()?,
            };
            if status.challenge_nonce == [0; NONCE_BYTES]
                || status.challenge_mac == [0; DIGEST_BYTES]
            {
                return Err(ManifestV2FrameCodecError::InvalidChallenge);
            }
            ManifestV2Frame::ResumeStatus(status)
        }
        ManifestV2FrameType::Cancel => {
            let identity = reader.identity()?;
            let scope = cancel_scope(reader.u8()?)?;
            let entry_id = reader.optional_u32()?;
            if matches!(scope, CancelScopeV2::Job) != entry_id.is_none() {
                return Err(ManifestV2FrameCodecError::InvalidCancelScope);
            }
            ManifestV2Frame::Cancel(CancelV2 {
                identity,
                scope,
                entry_id,
                failure_code: reader.u32()?,
            })
        }
        ManifestV2FrameType::Error => ManifestV2Frame::Error(ManifestErrorV2 {
            identity: reader.identity()?,
            failure_code: reader.u32()?,
            phase: failure_phase(reader.u8()?)?,
            entry_id: reader.optional_u32()?,
        }),
    };
    Ok(frame)
}

fn encode_entry_result_body(
    output: &mut Vec<u8>,
    value: &EntryResultV2,
) -> Result<(), ManifestV2FrameCodecError> {
    identity(output, value.identity)?;
    checked_entry_id(value.entry_id)?;
    u32_value(output, value.entry_id);
    output.push(value.result as u8);
    u64_value(output, value.final_size);
    optional_digest(output, value.final_digest);
    match &value.final_component_override {
        Some(component_value) => {
            output.push(1);
            component(output, component_value)?;
        }
        None => output.push(0),
    }
    Ok(())
}

fn identity(
    output: &mut Vec<u8>,
    identity: JobGenerationV2,
) -> Result<(), ManifestV2FrameCodecError> {
    if identity.job_id.0 == [0; 16] || identity.generation == 0 {
        return Err(ManifestV2FrameCodecError::InvalidIdentity);
    }
    output.extend_from_slice(&identity.job_id.0);
    u32_value(output, identity.generation);
    Ok(())
}

fn checked_entry_id(entry_id: u32) -> Result<(), ManifestV2FrameCodecError> {
    if entry_id as usize >= MAX_MANIFEST_V2_ENTRIES {
        return Err(ManifestV2FrameCodecError::InvalidEntryId);
    }
    Ok(())
}

fn digest(output: &mut Vec<u8>, value: ContentDigestV2) {
    output.extend_from_slice(&value.0);
}

fn optional_digest(output: &mut Vec<u8>, value: Option<ContentDigestV2>) {
    match value {
        Some(value) => {
            output.push(1);
            digest(output, value);
        }
        None => output.push(0),
    }
}

fn optional_u32(output: &mut Vec<u8>, value: Option<u32>) {
    match value {
        Some(value) => {
            output.push(1);
            u32_value(output, value);
        }
        None => output.push(0),
    }
}

fn count(
    output: &mut Vec<u8>,
    value: usize,
    maximum: usize,
) -> Result<(), ManifestV2FrameCodecError> {
    if value > maximum {
        return Err(ManifestV2FrameCodecError::CountTooLarge);
    }
    u32_value(
        output,
        u32::try_from(value).map_err(|_| ManifestV2FrameCodecError::CountTooLarge)?,
    );
    Ok(())
}

fn component(output: &mut Vec<u8>, value: &str) -> Result<(), ManifestV2FrameCodecError> {
    validate_component(value)?;
    u32_value(
        output,
        u32::try_from(value.len()).map_err(|_| ManifestV2FrameCodecError::UnsafeComponent)?,
    );
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

fn validate_component(value: &str) -> Result<(), ManifestV2FrameCodecError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.len() > MAX_MANIFEST_V2_COMPONENT_BYTES
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(ManifestV2FrameCodecError::UnsafeComponent);
    }
    Ok(())
}

fn u32_value(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn u64_value(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

macro_rules! tag_decoder {
    ($name:ident, $label:literal, $type:ty, {$($value:literal => $variant:path),+ $(,)?}) => {
        fn $name(value: u8) -> Result<$type, ManifestV2FrameCodecError> {
            match value {
                $($value => Ok($variant),)+
                other => Err(ManifestV2FrameCodecError::UnknownTag {
                    name: $label,
                    value: other,
                }),
            }
        }
    };
}

tag_decoder!(entry_disposition, "entry disposition", EntryDispositionV2, {
    0 => EntryDispositionV2::ReceivePayload,
    1 => EntryDispositionV2::ReuseExisting,
});
tag_decoder!(entry_encoding, "entry encoding", EntryEncodingV2, {
    0 => EntryEncodingV2::Identity,
    1 => EntryEncodingV2::Zstd,
});
tag_decoder!(entry_digest_decision, "entry digest decision", EntryDigestDecisionV2, {
    0 => EntryDigestDecisionV2::Proposed,
    1 => EntryDigestDecisionV2::ContinuePayload,
    2 => EntryDigestDecisionV2::ReuseExisting,
});
tag_decoder!(entry_completion, "entry completion", EntryCompletionChoiceV2, {
    0 => EntryCompletionChoiceV2::PayloadComplete,
    1 => EntryCompletionChoiceV2::ReuseChosen,
});
tag_decoder!(entry_result_kind, "entry result", EntryResultKindV2, {
    0 => EntryResultKindV2::Saved,
    1 => EntryResultKindV2::ReusedExisting,
});
tag_decoder!(entry_arbiter, "entry arbiter", EntryArbiterV2, {
    0 => EntryArbiterV2::PayloadOpen,
    1 => EntryArbiterV2::ReuseChosen,
    2 => EntryArbiterV2::PayloadCompleteChosen,
});
tag_decoder!(cancel_scope, "cancel scope", CancelScopeV2, {
    0 => CancelScopeV2::Job,
    1 => CancelScopeV2::Entry,
});
tag_decoder!(failure_phase, "failure phase", ManifestFailurePhaseV2, {
    0 => ManifestFailurePhaseV2::Offer,
    1 => ManifestFailurePhaseV2::Destination,
    2 => ManifestFailurePhaseV2::Payload,
    3 => ManifestFailurePhaseV2::Verify,
    4 => ManifestFailurePhaseV2::Save,
    5 => ManifestFailurePhaseV2::Proof,
});

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn exact(&mut self, length: usize) -> Result<&'a [u8], ManifestV2FrameCodecError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(ManifestV2FrameCodecError::Truncated)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(ManifestV2FrameCodecError::Truncated)?;
        self.offset = end;
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ManifestV2FrameCodecError> {
        self.exact(N)?
            .try_into()
            .map_err(|_| ManifestV2FrameCodecError::Truncated)
    }

    fn u8(&mut self) -> Result<u8, ManifestV2FrameCodecError> {
        Ok(self.exact(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, ManifestV2FrameCodecError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, ManifestV2FrameCodecError> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    fn identity(&mut self) -> Result<JobGenerationV2, ManifestV2FrameCodecError> {
        let identity = JobGenerationV2 {
            job_id: JobIdV2(self.array()?),
            generation: self.u32()?,
        };
        if identity.job_id.0 == [0; 16] || identity.generation == 0 {
            return Err(ManifestV2FrameCodecError::InvalidIdentity);
        }
        Ok(identity)
    }

    fn digest(&mut self) -> Result<ContentDigestV2, ManifestV2FrameCodecError> {
        Ok(ContentDigestV2(self.array()?))
    }

    fn entry_id(&mut self) -> Result<u32, ManifestV2FrameCodecError> {
        let entry_id = self.u32()?;
        checked_entry_id(entry_id)?;
        Ok(entry_id)
    }

    fn count(&mut self, maximum: usize) -> Result<usize, ManifestV2FrameCodecError> {
        let count = self.u32()? as usize;
        if count > maximum {
            return Err(ManifestV2FrameCodecError::CountTooLarge);
        }
        Ok(count)
    }

    fn bytes(&mut self, maximum: usize) -> Result<Vec<u8>, ManifestV2FrameCodecError> {
        let length = self.u32()? as usize;
        if length > maximum {
            return Err(ManifestV2FrameCodecError::FrameTooLarge);
        }
        Ok(self.exact(length)?.to_vec())
    }

    fn component(&mut self) -> Result<String, ManifestV2FrameCodecError> {
        let bytes = self.bytes(MAX_MANIFEST_V2_COMPONENT_BYTES)?;
        let value = std::str::from_utf8(&bytes)
            .map_err(|_| ManifestV2FrameCodecError::InvalidUtf8)?
            .to_owned();
        validate_component(&value)?;
        Ok(value)
    }

    fn optional_u32(&mut self) -> Result<Option<u32>, ManifestV2FrameCodecError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.entry_id()?)),
            _ => Err(ManifestV2FrameCodecError::InvalidOptional),
        }
    }

    fn optional_digest(&mut self) -> Result<Option<ContentDigestV2>, ManifestV2FrameCodecError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.digest()?)),
            _ => Err(ManifestV2FrameCodecError::InvalidOptional),
        }
    }

    fn entry_result(&mut self) -> Result<EntryResultV2, ManifestV2FrameCodecError> {
        let identity = self.identity()?;
        let entry_id = self.entry_id()?;
        let result = entry_result_kind(self.u8()?)?;
        let final_size = self.u64()?;
        let final_digest = self.optional_digest()?;
        let final_component_override = match self.u8()? {
            0 => None,
            1 => Some(self.component()?),
            _ => return Err(ManifestV2FrameCodecError::InvalidOptional),
        };
        Ok(EntryResultV2 {
            identity,
            entry_id,
            result,
            final_size,
            final_digest,
            final_component_override,
        })
    }

    fn finish(&self) -> Result<(), ManifestV2FrameCodecError> {
        if self.offset != self.bytes.len() {
            return Err(ManifestV2FrameCodecError::TrailingBytes);
        }
        Ok(())
    }
}

impl From<ManifestV2FrameCodecError> for CoreError {
    fn from(error: ManifestV2FrameCodecError) -> Self {
        match error {
            ManifestV2FrameCodecError::Io(error) => CoreError::Io(error.to_string()),
            other => CoreError::Protocol(other.to_string()),
        }
    }
}

#[cfg(test)]
#[path = "manifest_v2_frames_tests.rs"]
mod tests;
