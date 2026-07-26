use std::fmt;

use envoix_attempt_api::RetirementIntent;

use crate::{ProductState, Quiescence, TransferRecord, WorkerKind};

pub mod identifiers;

pub use identifiers::PRODUCT_RECORD_SCHEMA_ID;

/// The version written by this build. Version 2 added `command_ledger` (BN2),
/// version 3 added `pairing` (F2b), and version 4 added the create request
/// identity. The body codec is self-describing, so every version back to
/// [`OLDEST_READABLE_RECORD_VERSION`] decodes with absent fields defaulted. An
/// older reader seeing a newer version takes the honest
/// [`RecordDecode::UnsupportedFuture`] quarantine, never the corrupt path.
pub const PRODUCT_RECORD_VERSION: u32 = 4;
/// The oldest record version this build still decodes.
pub const OLDEST_READABLE_RECORD_VERSION: u32 = 1;
const MAX_RECORD_BODY_BYTES: usize = 1024 * 1024;
const SCHEMA_LENGTH_BYTES: usize = 2;
const VERSION_BYTES: usize = 4;
const BODY_LENGTH_BYTES: usize = 4;

const _: () = assert!(PRODUCT_RECORD_SCHEMA_ID.len() <= u16::MAX as usize);
const _: () = assert!(MAX_RECORD_BODY_BYTES <= u32::MAX as usize);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordDecode {
    /// Boxed so the rare future-version answer does not carry the full record's
    /// stack footprint merely because it shares this result vocabulary.
    Loaded(Box<TransferRecord>),
    UnsupportedFuture {
        version: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordCodecError {
    Truncated,
    InvalidSchema,
    UnsupportedVersion { actual: u32 },
    BodyTooLarge { actual: usize, maximum: usize },
    LengthMismatch,
    MalformedBody,
    InvalidRecord(RecordInvariant),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordInvariant {
    ZeroGeneration,
    ProgressExceedsTotal,
    ReadySourceIsPreparing,
}

impl fmt::Display for RecordCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => formatter.write_str("product record envelope is truncated"),
            Self::InvalidSchema => formatter.write_str("product record schema is invalid"),
            Self::UnsupportedVersion { actual } => {
                write!(formatter, "product record version {actual} is unsupported")
            }
            Self::BodyTooLarge { actual, maximum } => write!(
                formatter,
                "product record body length {actual} exceeds maximum {maximum}"
            ),
            Self::LengthMismatch => {
                formatter.write_str("product record envelope length does not match its header")
            }
            Self::MalformedBody => formatter.write_str("product record body is malformed"),
            Self::InvalidRecord(invariant) => write!(
                formatter,
                "product record violates the {invariant:?} invariant"
            ),
        }
    }
}

impl std::error::Error for RecordCodecError {}

pub fn encode_record(record: &TransferRecord) -> Result<Vec<u8>, RecordCodecError> {
    validate_record(record)?;
    let body = serde_json::to_vec(record).map_err(|_| RecordCodecError::MalformedBody)?;
    if body.len() > MAX_RECORD_BODY_BYTES {
        return Err(RecordCodecError::BodyTooLarge {
            actual: body.len(),
            maximum: MAX_RECORD_BODY_BYTES,
        });
    }
    let schema = PRODUCT_RECORD_SCHEMA_ID.as_bytes();
    let mut encoded = Vec::with_capacity(
        SCHEMA_LENGTH_BYTES + schema.len() + VERSION_BYTES + BODY_LENGTH_BYTES + body.len(),
    );
    encoded.extend_from_slice(&(schema.len() as u16).to_be_bytes());
    encoded.extend_from_slice(schema);
    encoded.extend_from_slice(&PRODUCT_RECORD_VERSION.to_be_bytes());
    encoded.extend_from_slice(&(body.len() as u32).to_be_bytes());
    encoded.extend_from_slice(&body);
    Ok(encoded)
}

pub fn decode_record(encoded: &[u8]) -> Result<RecordDecode, RecordCodecError> {
    let schema_length_bytes = encoded
        .get(..SCHEMA_LENGTH_BYTES)
        .ok_or(RecordCodecError::Truncated)?;
    let schema_length =
        u16::from_be_bytes([schema_length_bytes[0], schema_length_bytes[1]]) as usize;
    if schema_length == 0 {
        return Err(RecordCodecError::InvalidSchema);
    }
    let schema_start = SCHEMA_LENGTH_BYTES;
    let schema_end = schema_start
        .checked_add(schema_length)
        .ok_or(RecordCodecError::LengthMismatch)?;
    if encoded.get(schema_start..schema_end) != Some(PRODUCT_RECORD_SCHEMA_ID.as_bytes()) {
        return Err(RecordCodecError::InvalidSchema);
    }

    let version_end = schema_end
        .checked_add(VERSION_BYTES)
        .ok_or(RecordCodecError::LengthMismatch)?;
    let version_bytes = encoded
        .get(schema_end..version_end)
        .ok_or(RecordCodecError::Truncated)?;
    let version = u32::from_be_bytes(version_bytes.try_into().expect("four-byte version"));
    if version > PRODUCT_RECORD_VERSION {
        return Ok(RecordDecode::UnsupportedFuture { version });
    }
    if version < OLDEST_READABLE_RECORD_VERSION {
        return Err(RecordCodecError::UnsupportedVersion { actual: version });
    }

    let body_length_end = version_end
        .checked_add(BODY_LENGTH_BYTES)
        .ok_or(RecordCodecError::LengthMismatch)?;
    let body_length_bytes = encoded
        .get(version_end..body_length_end)
        .ok_or(RecordCodecError::Truncated)?;
    let body_length =
        u32::from_be_bytes(body_length_bytes.try_into().expect("four-byte body length")) as usize;
    if body_length > MAX_RECORD_BODY_BYTES {
        return Err(RecordCodecError::BodyTooLarge {
            actual: body_length,
            maximum: MAX_RECORD_BODY_BYTES,
        });
    }
    let expected_length = body_length_end
        .checked_add(body_length)
        .ok_or(RecordCodecError::LengthMismatch)?;
    if encoded.len() != expected_length {
        return Err(RecordCodecError::LengthMismatch);
    }
    let record: TransferRecord = serde_json::from_slice(&encoded[body_length_end..])
        .map_err(|_| RecordCodecError::MalformedBody)?;
    validate_record(&record)?;
    Ok(RecordDecode::Loaded(Box::new(record)))
}

fn validate_record(record: &TransferRecord) -> Result<(), RecordCodecError> {
    if record.generation.get() == 0 {
        return Err(RecordCodecError::InvalidRecord(
            RecordInvariant::ZeroGeneration,
        ));
    }
    if record.total.get() != 0 && record.bytes.get() > record.total.get() {
        return Err(RecordCodecError::InvalidRecord(
            RecordInvariant::ProgressExceedsTotal,
        ));
    }
    // A `Preparing` card with a ready source is normally invalid — EXCEPT the
    // staging-retirement handoff window: `StageComplete` sets `source_ready` and
    // moves the worker to `Retiring(Staging, Finalize)` but stays `Preparing`
    // until `StagingRetired` launches the first attempt. That durable intermediate
    // state must round-trip; any OTHER `Preparing + source_ready` is still invalid.
    if record.facts.source_ready
        && record.state == ProductState::Preparing
        && !matches!(
            record.quiescence,
            Quiescence::Retiring {
                worker: WorkerKind::Staging,
                intent: RetirementIntent::Finalize,
            }
        )
    {
        return Err(RecordCodecError::InvalidRecord(
            RecordInvariant::ReadySourceIsPreparing,
        ));
    }
    Ok(())
}
