//! The versioned frame around one card's stored state.
//!
//! EH-01: `StoreImage` is written by a positional codec — field order and enum
//! ordinal ARE the wire format, and no field name is stored. Before this
//! module, `load_image` deserialized the entire current Rust shape and only
//! then compared the `schema` string it found inside. So the version lived
//! inside the bytes it was meant to version: a reordered field or an inserted
//! enum arm could reinterpret an old image under the current meaning, or fail
//! as `CorruptState`, before the schema check could say anything at all.
//!
//! This frame is read first and answers three questions without touching the
//! payload: is this ours, which schema, and which version. An unknown version
//! is a typed answer rather than a corruption.
//!
//! It sits INSIDE the C5 `OperationEnvelope` body. That envelope versions the
//! generic storage artifact and deliberately treats this body as opaque;
//! bumping it would touch every user of it and still say nothing about this
//! layout.

use crate::identifiers::OPERATION_STORE_STATE_SCHEMA_ID;

/// Marks a body as an enveloped state image. A legacy positional image begins
/// with the u32 length of its schema string, never this, so the two are
/// unambiguous on sight.
const MAGIC: [u8; 4] = *b"EVOS";

/// The layout version this build writes and is the only one it reads.
pub const STATE_FORMAT_VERSION: u32 = 2;

/// Why a stored body is not state this build can read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StateEnvelopeError {
    /// Not an enveloped image at all — including every pre-envelope body.
    NotEnveloped,
    /// The frame is ours and its header does not hold together. Deliberately
    /// never a fallback to some other decoder: a damaged frame is damaged.
    Malformed,
    /// Well-formed, and from a build this one cannot speak for. Answered BEFORE
    /// the payload is parsed, which is the entire point of the frame.
    UnsupportedVersion { schema: String, version: u32 },
}

/// Wraps `payload` for storage.
pub fn wrap(payload: &[u8]) -> Vec<u8> {
    let schema = OPERATION_STORE_STATE_SCHEMA_ID.as_bytes();
    let mut out = Vec::with_capacity(MAGIC.len() + 10 + schema.len() + payload.len());
    out.extend_from_slice(&MAGIC);
    // The schema id is a compile-time constant far below u16::MAX; the cast is
    // checked here anyway so a future rename cannot truncate it silently.
    let schema_len = u16::try_from(schema.len()).expect("the state schema id is short");
    out.extend_from_slice(&schema_len.to_be_bytes());
    out.extend_from_slice(schema);
    out.extend_from_slice(&STATE_FORMAT_VERSION.to_be_bytes());
    out.extend_from_slice(
        &u32::try_from(payload.len())
            .expect("payload fits u32")
            .to_be_bytes(),
    );
    out.extend_from_slice(payload);
    out
}

/// Reads the frame and returns its payload, deciding everything about identity
/// and version first.
pub fn unwrap(body: &[u8]) -> Result<&[u8], StateEnvelopeError> {
    let rest = body
        .strip_prefix(&MAGIC)
        .ok_or(StateEnvelopeError::NotEnveloped)?;

    let (schema_len, rest) = take(rest, 2)?;
    let schema_len = usize::from(u16::from_be_bytes([schema_len[0], schema_len[1]]));
    let (schema, rest) = take(rest, schema_len)?;
    let schema = core::str::from_utf8(schema).map_err(|_| StateEnvelopeError::Malformed)?;

    let (version, rest) = take(rest, 4)?;
    let version = u32::from_be_bytes([version[0], version[1], version[2], version[3]]);

    // Before the payload. A build that added a field would otherwise have
    // interpreted these bytes under its own shape and reported corruption.
    if schema != OPERATION_STORE_STATE_SCHEMA_ID || version != STATE_FORMAT_VERSION {
        return Err(StateEnvelopeError::UnsupportedVersion {
            schema: schema.to_owned(),
            version,
        });
    }

    let (payload_len, payload) = take(rest, 4)?;
    let payload_len = usize::try_from(u32::from_be_bytes([
        payload_len[0],
        payload_len[1],
        payload_len[2],
        payload_len[3],
    ]))
    .map_err(|_| StateEnvelopeError::Malformed)?;
    // Exactly, not at least: trailing bytes mean the writer and this reader
    // disagree about where the image ends, and guessing which is right is how a
    // truncation becomes a successful load of the wrong thing.
    if payload.len() != payload_len {
        return Err(StateEnvelopeError::Malformed);
    }
    Ok(payload)
}

/// Splits `count` bytes off the front, refusing to read past the end. Nothing
/// is allocated from a declared length before it has been checked against what
/// is actually there.
fn take(input: &[u8], count: usize) -> Result<(&[u8], &[u8]), StateEnvelopeError> {
    if input.len() < count {
        return Err(StateEnvelopeError::Malformed);
    }
    Ok(input.split_at(count))
}
