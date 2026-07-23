use std::fmt;

use crate::QuarantineReason;
use crate::identifiers::OPERATION_ENVELOPE_SCHEMA_ID;

pub const CURRENT_ENVELOPE_VERSION: u32 = 1;
pub const MAX_ENVELOPE_BODY_BYTES: usize = 1024 * 1024;

const SCHEMA_LENGTH_BYTES: usize = 2;
const VERSION_BYTES: usize = 4;
const BODY_LENGTH_BYTES: usize = 4;
const MAX_SCHEMA_ID_BYTES: usize = 256;

const _: () = assert!(OPERATION_ENVELOPE_SCHEMA_ID.len() <= u16::MAX as usize);
const _: () = assert!(MAX_ENVELOPE_BODY_BYTES <= u32::MAX as usize);

#[derive(Clone, Eq, PartialEq)]
pub struct OpaqueBody(Vec<u8>);

impl OpaqueBody {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl fmt::Debug for OpaqueBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpaqueBody")
            .field("length", &self.0.len())
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct OperationEnvelope {
    body: OpaqueBody,
}

impl OperationEnvelope {
    pub fn new(body: impl Into<Vec<u8>>) -> Result<Self, EnvelopeError> {
        let body = body.into();
        if body.len() > MAX_ENVELOPE_BODY_BYTES {
            return Err(EnvelopeError::BodyTooLarge {
                actual: body.len(),
                maximum: MAX_ENVELOPE_BODY_BYTES,
            });
        }
        Ok(Self {
            body: OpaqueBody(body),
        })
    }

    pub const fn schema_id(&self) -> &'static str {
        OPERATION_ENVELOPE_SCHEMA_ID
    }

    pub const fn version(&self) -> u32 {
        CURRENT_ENVELOPE_VERSION
    }

    pub const fn body(&self) -> &OpaqueBody {
        &self.body
    }

    pub fn into_body(self) -> OpaqueBody {
        self.body
    }

    /// Encodes `schema-len:u16 | schema | version:u32 | body-len:u32 | body`,
    /// with every integer in network byte order.
    pub fn encode(&self) -> Vec<u8> {
        let schema = OPERATION_ENVELOPE_SCHEMA_ID.as_bytes();
        let mut encoded = Vec::with_capacity(
            SCHEMA_LENGTH_BYTES
                + schema.len()
                + VERSION_BYTES
                + BODY_LENGTH_BYTES
                + self.body.0.len(),
        );
        encoded.extend_from_slice(&(schema.len() as u16).to_be_bytes());
        encoded.extend_from_slice(schema);
        encoded.extend_from_slice(&CURRENT_ENVELOPE_VERSION.to_be_bytes());
        encoded.extend_from_slice(&(self.body.0.len() as u32).to_be_bytes());
        encoded.extend_from_slice(&self.body.0);
        encoded
    }

    /// Classifies untrusted persisted bytes by envelope metadata only. The
    /// returned body remains opaque for the product-owned codec.
    pub fn decode(encoded: &[u8]) -> EnvelopeDecode {
        decode_for_load(encoded)
    }
}

impl fmt::Debug for OperationEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperationEnvelope")
            .field("schema_id", &OPERATION_ENVELOPE_SCHEMA_ID)
            .field("version", &CURRENT_ENVELOPE_VERSION)
            .field("body", &self.body)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvelopeError {
    BodyTooLarge { actual: usize, maximum: usize },
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge { actual, maximum } => write!(
                formatter,
                "operation envelope body length {actual} exceeds maximum {maximum}"
            ),
        }
    }
}

impl std::error::Error for EnvelopeError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvelopeDecode {
    Loaded(OperationEnvelope),
    Quarantined { reason: QuarantineReason },
}

fn decode_for_load(encoded: &[u8]) -> EnvelopeDecode {
    let Some(schema_length_bytes) = encoded.get(..SCHEMA_LENGTH_BYTES) else {
        return EnvelopeDecode::Quarantined {
            reason: QuarantineReason::Corrupt,
        };
    };
    let schema_length =
        u16::from_be_bytes([schema_length_bytes[0], schema_length_bytes[1]]) as usize;
    if schema_length == 0 || schema_length > MAX_SCHEMA_ID_BYTES {
        return EnvelopeDecode::Quarantined {
            reason: QuarantineReason::Corrupt,
        };
    }

    let schema_start = SCHEMA_LENGTH_BYTES;
    let schema_end = schema_start + schema_length;
    let version_end = schema_end + VERSION_BYTES;
    let Some(version_bytes) = encoded.get(schema_end..version_end) else {
        return EnvelopeDecode::Quarantined {
            reason: QuarantineReason::Corrupt,
        };
    };
    let version = u32::from_be_bytes([
        version_bytes[0],
        version_bytes[1],
        version_bytes[2],
        version_bytes[3],
    ]);
    if version > CURRENT_ENVELOPE_VERSION {
        return EnvelopeDecode::Quarantined {
            reason: QuarantineReason::UnsupportedFuture,
        };
    }
    if version != CURRENT_ENVELOPE_VERSION
        || encoded.get(schema_start..schema_end) != Some(OPERATION_ENVELOPE_SCHEMA_ID.as_bytes())
    {
        return EnvelopeDecode::Quarantined {
            reason: QuarantineReason::Corrupt,
        };
    }

    let body_length_end = version_end + BODY_LENGTH_BYTES;
    let Some(body_length_bytes) = encoded.get(version_end..body_length_end) else {
        return EnvelopeDecode::Quarantined {
            reason: QuarantineReason::Corrupt,
        };
    };
    let body_length = u32::from_be_bytes([
        body_length_bytes[0],
        body_length_bytes[1],
        body_length_bytes[2],
        body_length_bytes[3],
    ]) as usize;
    if body_length > MAX_ENVELOPE_BODY_BYTES
        || encoded.len() != body_length_end.saturating_add(body_length)
    {
        return EnvelopeDecode::Quarantined {
            reason: QuarantineReason::Corrupt,
        };
    }

    match OperationEnvelope::new(encoded[body_length_end..].to_vec()) {
        Ok(envelope) => EnvelopeDecode::Loaded(envelope),
        Err(_) => EnvelopeDecode::Quarantined {
            reason: QuarantineReason::Corrupt,
        },
    }
}
