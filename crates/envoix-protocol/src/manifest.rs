//! Versioned manifest types and validation for multi-item transfers.

use std::collections::{HashMap, HashSet};
use std::fmt;

use envoix_error::CoreError;
use envoix_types::{PeerRole, TransferId};
use num_enum::TryFromPrimitive;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use super::{
    MAX_FRAME_SIZE, PayloadReader, ProtocolError, read_frame_payload, write_bool, write_bytes,
    write_frame_header, write_peer_role, write_string, write_u8, write_u32, write_u64,
};

/// Existing single-file protocol ALPN retained during the manifest migration.
pub const SINGLE_FILE_V1_ALPN: &[u8] = b"envoix/1";

/// Additive manifest protocol ALPN. A peer must not advertise it until the
/// manifest frame family and transfer engine are available.
pub const MANIFEST_V1_ALPN: &[u8] = b"envoix/manifest/1";

/// Protocol version carried by Manifest v1 hello frames.
pub const MANIFEST_V1_PROTOCOL_VERSION: u32 = 1;

/// Maximum encoded size of one v1 manifest.
pub const MAX_MANIFEST_V1_ENCODED_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of file and directory entries in one v1 manifest.
pub const MAX_MANIFEST_V1_ENTRIES: usize = 10_000;

/// Maximum UTF-8 byte length of one manifest relative path.
pub const MAX_MANIFEST_V1_PATH_BYTES: usize = 4_096;

/// Maximum UTF-8 byte length of one path component.
pub const MAX_MANIFEST_V1_COMPONENT_BYTES: usize = 255;

/// Maximum number of components in one manifest relative path.
pub const MAX_MANIFEST_V1_PATH_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u8)]
enum ManifestFrameType {
    Hello = 16,
    Offer = 17,
    Accept = 18,
    EntryStart = 19,
    ResumeStatus = 20,
    Chunk = 21,
    EntryComplete = 22,
    EntryCompleteAck = 23,
    Complete = 24,
    CompleteAck = 25,
    Error = 26,
}

/// Wire protocol selected for a transfer shape.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferProtocol {
    /// Compatible single regular-file transfer over `envoix/1`.
    SingleFileV1,
    /// Multi-file or directory transfer over `envoix/manifest/1`.
    ManifestV1,
    /// Canonical single/multi-root transfer over `envoix/manifest/2`.
    ManifestV2,
}

impl TransferProtocol {
    /// Returns the exact ALPN for this protocol.
    pub const fn alpn(self) -> &'static [u8] {
        match self {
            Self::SingleFileV1 => SINGLE_FILE_V1_ALPN,
            Self::ManifestV1 => MANIFEST_V1_ALPN,
            Self::ManifestV2 => crate::manifest_v2::MANIFEST_V2_ALPN,
        }
    }

    /// Parses a negotiated ALPN without treating unknown protocols as a
    /// compatible fallback.
    pub fn from_alpn(alpn: &[u8]) -> Option<Self> {
        match alpn {
            SINGLE_FILE_V1_ALPN => Some(Self::SingleFileV1),
            MANIFEST_V1_ALPN => Some(Self::ManifestV1),
            crate::manifest_v2::MANIFEST_V2_ALPN => Some(Self::ManifestV2),
            _ => None,
        }
    }

    /// Selects the required protocol from the offered transfer shape.
    ///
    /// An empty transfer set is invalid. Exactly one regular file retains the
    /// compatibility path; every other non-empty shape requires Manifest v1.
    pub const fn required_for_shape(file_count: u32, directory_count: u32) -> Option<Self> {
        if file_count == 0 && directory_count == 0 {
            None
        } else if file_count == 1 && directory_count == 0 {
            Some(Self::SingleFileV1)
        } else {
            Some(Self::ManifestV1)
        }
    }
}

/// Stable identifier for one manifest transfer set.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ManifestId(pub String);

impl ManifestId {
    /// Creates a manifest identifier from its durable string representation.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for ManifestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Integrity algorithm fixed by the Manifest v1 contract.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestHashAlgorithm {
    /// 32-byte BLAKE3 digest.
    Blake3_256,
}

/// Type of one entry in a Manifest v1 transfer set.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestEntryKind {
    /// A regular file with a size and content hash.
    RegularFile,
    /// An explicit directory, including an empty directory.
    Directory,
}

/// One file or directory offered by a Manifest v1 sender.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestEntryV1 {
    /// Zero-based stable identifier matching this entry's manifest position.
    pub entry_id: u32,
    /// Portable `/`-separated path relative to the selected receive root.
    pub relative_path: String,
    /// Entry type.
    pub kind: ManifestEntryKind,
    /// File length, or exactly zero for a directory.
    pub size: u64,
    /// File BLAKE3 digest, or absent for a directory.
    pub hash: Option<[u8; 32]>,
    /// Optional informational modification timestamp.
    pub modified_at_unix_ms: Option<u64>,
}

/// Protocol-level description of one multi-item transfer set.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestV1 {
    /// Stable transfer-set identifier.
    pub manifest_id: ManifestId,
    /// Entries in parent-before-child order.
    pub entries: Vec<ManifestEntryV1>,
    /// Declared count of regular files.
    pub file_count: u32,
    /// Declared count of directories.
    pub directory_count: u32,
    /// Declared count of top-level roots.
    pub root_count: u32,
    /// Checked sum of all regular-file sizes.
    pub total_bytes: u64,
    /// Integrity algorithm; v1 accepts only BLAKE3-256.
    pub hash_algorithm: ManifestHashAlgorithm,
}

/// One control or payload message on the `envoix/manifest/1` ALPN.
///
/// This family is intentionally separate from [`crate::Frame`], preserving
/// exhaustive matches and implementations for the existing single-file API.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ManifestFrame {
    /// Opens an authenticated manifest conversation.
    Hello(ManifestHelloV1),
    /// Offers the complete transfer set before receiver writes.
    Offer(ManifestOfferV1),
    /// Returns receiver-owned entry dispositions and safe target mappings.
    Accept(ManifestAcceptV1),
    /// Starts one sequential file entry.
    EntryStart(ManifestEntryStartV1),
    /// Reports the verified resumable prefix for the active entry.
    ResumeStatus(ManifestResumeStatusV1),
    /// Carries sequential bytes for the active entry.
    Chunk(ManifestChunkV1),
    /// Ends bytes for one entry and repeats its offered hash.
    EntryComplete(ManifestEntryCompleteV1),
    /// Confirms that one entry was verified and committed.
    EntryCompleteAck(ManifestEntryCompleteAckV1),
    /// Marks the sender's end of the whole transfer set.
    Complete(ManifestCompleteV1),
    /// Returns the final per-entry results.
    CompleteAck(ManifestCompleteAckV1),
    /// Carries a typed manifest-level or entry-level failure.
    Error(ManifestErrorV1),
}

/// Initial manifest conversation marker after authentication.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestHelloV1 {
    /// Manifest protocol version. Exactly one in this frame family.
    pub protocol_version: u32,
    /// Sender or receiver role for this data-plane conversation.
    pub role: PeerRole,
}

/// Complete transfer-set offer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestOfferV1 {
    /// Validated transfer-set description.
    pub manifest: ManifestV1,
    /// Sequential payload chunk size requested by the sender.
    pub chunk_size: u64,
    /// Whether compatible per-entry resume state may be used.
    pub resume_requested: bool,
}

/// Receiver response to a valid manifest offer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestAcceptV1 {
    /// Transfer set these dispositions apply to.
    pub manifest_id: ManifestId,
    /// One receiver-owned decision for every offered entry.
    pub entries: Vec<ManifestEntryDispositionV1>,
}

/// Receiver action planned for one manifest entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestEntryDispositionV1 {
    /// Offered entry identifier.
    pub entry_id: u32,
    /// Payload or directory action selected by the receiver.
    pub disposition: ManifestEntryDispositionKind,
    /// Safe final relative path selected by conflict planning.
    pub final_relative_path: String,
}

/// Receiver action for one offered entry.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestEntryDispositionKind {
    /// Transfer and verify a regular-file payload.
    Transfer,
    /// Create or retain an explicit directory without payload.
    CreateDirectory,
    /// Reuse an existing regular file with identical content.
    SkipIdentical,
}

/// Starts one regular-file payload in the sequential v1 schedule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestEntryStartV1 {
    /// Transfer set containing the entry.
    pub manifest_id: ManifestId,
    /// Entry being started.
    pub entry_id: u32,
    /// Stable identifier reused by prefix resume and receipts.
    pub transfer_id: TransferId,
}

/// Receiver resume position for the active manifest entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestResumeStatusV1 {
    /// Transfer set containing the entry.
    pub manifest_id: ManifestId,
    /// Entry this status applies to.
    pub entry_id: u32,
    /// Stable per-entry transfer identifier.
    pub transfer_id: TransferId,
    /// Next sequential chunk index expected by the receiver.
    pub next_chunk_index: u64,
    /// Plaintext bytes already stored in staging.
    pub bytes_received: u64,
    /// BLAKE3 digest of the stored prefix.
    pub prefix_hash: [u8; 32],
}

/// Sequential payload bytes for one manifest entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestChunkV1 {
    /// Transfer set containing the entry.
    pub manifest_id: ManifestId,
    /// Entry receiving these bytes.
    pub entry_id: u32,
    /// Stable per-entry transfer identifier.
    pub transfer_id: TransferId,
    /// Zero-based sequential chunk index.
    pub index: u64,
    /// Plaintext offset of the first payload byte.
    pub offset: u64,
    /// Plaintext payload bytes; QUIC provides transport encryption.
    pub bytes: Vec<u8>,
}

/// Sender completion marker for one file entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestEntryCompleteV1 {
    /// Transfer set containing the entry.
    pub manifest_id: ManifestId,
    /// Entry being completed.
    pub entry_id: u32,
    /// Stable per-entry transfer identifier.
    pub transfer_id: TransferId,
    /// BLAKE3 digest revalidated while streaming.
    pub file_hash: [u8; 32],
}

/// Receiver acknowledgement after one file entry is committed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestEntryCompleteAckV1 {
    /// Transfer set containing the entry.
    pub manifest_id: ManifestId,
    /// Committed entry.
    pub entry_id: u32,
    /// Stable per-entry transfer identifier.
    pub transfer_id: TransferId,
}

/// Sender marker indicating that no more entries will start.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestCompleteV1 {
    /// Transfer set being completed.
    pub manifest_id: ManifestId,
}

/// Receiver acknowledgement containing the final partial-or-complete result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestCompleteAckV1 {
    /// Transfer set being completed.
    pub manifest_id: ManifestId,
    /// One final result for every offered entry.
    pub entries: Vec<ManifestEntryResultV1>,
}

/// Final outcome for one manifest entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestEntryResultV1 {
    /// Offered entry identifier.
    pub entry_id: u32,
    /// Terminal outcome.
    pub status: ManifestEntryResultStatus,
    /// Original offered path retained for conflict reporting.
    pub offered_relative_path: String,
    /// Final path for completed, skipped, or renamed entries.
    pub final_relative_path: Option<String>,
    /// Structured failure code for a failed entry.
    pub failure_code: Option<String>,
}

/// Terminal result states frozen by Manifest v1.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestEntryResultStatus {
    /// Payload or directory committed at the offered path.
    Completed,
    /// Existing identical content was retained.
    SkippedIdentical,
    /// Incoming content committed under a different safe path.
    Renamed,
    /// The entry failed with a structured code.
    Failed,
    /// Cancellation won before this entry committed.
    Cancelled,
}

/// Typed failure on a manifest conversation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestErrorV1 {
    /// Transfer set when known.
    pub manifest_id: Option<ManifestId>,
    /// Entry when the failure is entry-specific.
    pub entry_id: Option<u32>,
    /// Stable machine-readable failure code.
    pub code: String,
    /// Diagnostic message for logs and user-facing mapping fallback.
    pub message: String,
}

impl ManifestV1 {
    /// Validates encoded-size and structural invariants before receiver writes.
    ///
    /// `encoded_manifest_bytes` is supplied by the manifest codec so this
    /// model does not prematurely freeze a serialization format.
    pub fn validate(&self, encoded_manifest_bytes: usize) -> Result<(), ManifestValidationError> {
        if encoded_manifest_bytes > MAX_MANIFEST_V1_ENCODED_BYTES {
            return Err(ManifestValidationError::EncodedManifestTooLarge {
                actual: encoded_manifest_bytes,
                maximum: MAX_MANIFEST_V1_ENCODED_BYTES,
            });
        }
        self.validate_structure()
    }

    /// Validates all format-independent Manifest v1 invariants.
    pub fn validate_structure(&self) -> Result<(), ManifestValidationError> {
        if self.manifest_id.0.trim().is_empty() {
            return Err(ManifestValidationError::EmptyManifestId);
        }
        if self.entries.is_empty() {
            return Err(ManifestValidationError::EmptyManifest);
        }
        if self.entries.len() > MAX_MANIFEST_V1_ENTRIES {
            return Err(ManifestValidationError::TooManyEntries {
                actual: self.entries.len(),
                maximum: MAX_MANIFEST_V1_ENTRIES,
            });
        }

        let mut paths = HashSet::with_capacity(self.entries.len());
        let mut prior_entries = HashMap::with_capacity(self.entries.len());
        let mut file_count = 0_u32;
        let mut directory_count = 0_u32;
        let mut root_count = 0_u32;
        let mut total_bytes = 0_u64;

        for (index, entry) in self.entries.iter().enumerate() {
            let expected_id = index as u32;
            if entry.entry_id != expected_id {
                return Err(ManifestValidationError::EntryIdMismatch {
                    expected: expected_id,
                    actual: entry.entry_id,
                });
            }
            validate_manifest_relative_path(&entry.relative_path).map_err(|violation| {
                ManifestValidationError::UnsafePath {
                    entry_id: entry.entry_id,
                    path: entry.relative_path.clone(),
                    violation,
                }
            })?;
            if !paths.insert(entry.relative_path.as_str()) {
                return Err(ManifestValidationError::DuplicatePath {
                    path: entry.relative_path.clone(),
                });
            }

            if let Some((parent, _)) = entry.relative_path.rsplit_once('/') {
                match prior_entries.get(parent) {
                    Some(ManifestEntryKind::Directory) => {}
                    Some(ManifestEntryKind::RegularFile) => {
                        return Err(ManifestValidationError::ParentIsRegularFile {
                            path: entry.relative_path.clone(),
                            parent: parent.to_owned(),
                        });
                    }
                    None => {
                        return Err(ManifestValidationError::MissingParentDirectory {
                            path: entry.relative_path.clone(),
                            parent: parent.to_owned(),
                        });
                    }
                }
            } else {
                root_count += 1;
            }

            match entry.kind {
                ManifestEntryKind::RegularFile => {
                    if entry.hash.is_none() {
                        return Err(ManifestValidationError::FileHashMissing {
                            entry_id: entry.entry_id,
                        });
                    }
                    file_count += 1;
                    total_bytes = total_bytes
                        .checked_add(entry.size)
                        .ok_or(ManifestValidationError::TotalBytesOverflow)?;
                }
                ManifestEntryKind::Directory => {
                    if entry.size != 0 || entry.hash.is_some() {
                        return Err(ManifestValidationError::InvalidDirectoryMetadata {
                            entry_id: entry.entry_id,
                        });
                    }
                    directory_count += 1;
                }
            }
            prior_entries.insert(entry.relative_path.as_str(), entry.kind);
        }

        validate_declared_count("file_count", self.file_count, file_count)?;
        validate_declared_count("directory_count", self.directory_count, directory_count)?;
        validate_declared_count("root_count", self.root_count, root_count)?;
        if self.total_bytes != total_bytes {
            return Err(ManifestValidationError::TotalBytesMismatch {
                declared: self.total_bytes,
                actual: total_bytes,
            });
        }
        Ok(())
    }
}

fn validate_declared_count(
    field: &'static str,
    declared: u32,
    actual: u32,
) -> Result<(), ManifestValidationError> {
    if declared == actual {
        Ok(())
    } else {
        Err(ManifestValidationError::CountMismatch {
            field,
            declared,
            actual,
        })
    }
}

/// Validates one portable manifest path without joining it to a host path.
pub fn validate_manifest_relative_path(path: &str) -> Result<(), ManifestPathViolation> {
    if path.is_empty() {
        return Err(ManifestPathViolation::Empty);
    }
    if path.starts_with('/') {
        return Err(ManifestPathViolation::Absolute);
    }
    if path.ends_with('/') {
        return Err(ManifestPathViolation::LeadingOrTrailingSeparator);
    }
    if path.len() > MAX_MANIFEST_V1_PATH_BYTES {
        return Err(ManifestPathViolation::PathTooLong {
            actual: path.len(),
            maximum: MAX_MANIFEST_V1_PATH_BYTES,
        });
    }

    let components = path.split('/').collect::<Vec<_>>();
    if components.len() > MAX_MANIFEST_V1_PATH_DEPTH {
        return Err(ManifestPathViolation::TooDeep {
            actual: components.len(),
            maximum: MAX_MANIFEST_V1_PATH_DEPTH,
        });
    }
    for component in components {
        if component.is_empty() {
            return Err(ManifestPathViolation::EmptyComponent);
        }
        if component == "." {
            return Err(ManifestPathViolation::CurrentDirectoryComponent);
        }
        if component == ".." {
            return Err(ManifestPathViolation::ParentDirectoryComponent);
        }
        if component.len() > MAX_MANIFEST_V1_COMPONENT_BYTES {
            return Err(ManifestPathViolation::ComponentTooLong {
                actual: component.len(),
                maximum: MAX_MANIFEST_V1_COMPONENT_BYTES,
            });
        }
        if component.contains('\\') {
            return Err(ManifestPathViolation::Backslash);
        }
        if component.contains('\0') {
            return Err(ManifestPathViolation::Null);
        }
        if component.chars().any(char::is_control) {
            return Err(ManifestPathViolation::ControlCharacter);
        }
    }
    Ok(())
}

/// Portable path rule violated by a manifest entry.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ManifestPathViolation {
    /// The path contains no components.
    #[error("path is empty")]
    Empty,
    /// The path starts at a filesystem root.
    #[error("path is absolute")]
    Absolute,
    /// The path begins or ends with `/`.
    #[error("path has a leading or trailing separator")]
    LeadingOrTrailingSeparator,
    /// Two separators create an empty component.
    #[error("path contains an empty component")]
    EmptyComponent,
    /// A component is `.`.
    #[error("path contains a current-directory component")]
    CurrentDirectoryComponent,
    /// A component is `..`.
    #[error("path contains a parent-directory component")]
    ParentDirectoryComponent,
    /// The path uses a platform-specific reverse separator.
    #[error("path contains a backslash")]
    Backslash,
    /// The path contains NUL.
    #[error("path contains NUL")]
    Null,
    /// The path contains another control character.
    #[error("path contains a control character")]
    ControlCharacter,
    /// The full UTF-8 path exceeds the v1 byte limit.
    #[error("path uses {actual} bytes; maximum is {maximum}")]
    PathTooLong {
        /// Observed UTF-8 byte length.
        actual: usize,
        /// V1 maximum UTF-8 byte length.
        maximum: usize,
    },
    /// One UTF-8 component exceeds the v1 byte limit.
    #[error("path component uses {actual} bytes; maximum is {maximum}")]
    ComponentTooLong {
        /// Observed UTF-8 byte length.
        actual: usize,
        /// V1 maximum UTF-8 byte length.
        maximum: usize,
    },
    /// The component count exceeds the v1 depth limit.
    #[error("path depth is {actual}; maximum is {maximum}")]
    TooDeep {
        /// Observed component count.
        actual: usize,
        /// V1 maximum component count.
        maximum: usize,
    },
}

/// Structural reason that a complete Manifest v1 offer is invalid.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ManifestValidationError {
    /// The stable transfer-set identifier is empty.
    #[error("manifest id is empty")]
    EmptyManifestId,
    /// A transfer set contains no entries.
    #[error("manifest contains no entries")]
    EmptyManifest,
    /// The codec produced more than the v1 maximum encoded bytes.
    #[error("encoded manifest uses {actual} bytes; maximum is {maximum}")]
    EncodedManifestTooLarge {
        /// Observed encoded byte length.
        actual: usize,
        /// V1 maximum encoded byte length.
        maximum: usize,
    },
    /// The manifest contains too many entries.
    #[error("manifest contains {actual} entries; maximum is {maximum}")]
    TooManyEntries {
        /// Observed entry count.
        actual: usize,
        /// V1 maximum entry count.
        maximum: usize,
    },
    /// The entry identifier does not match its stable zero-based position.
    #[error("entry id is {actual}; expected {expected}")]
    EntryIdMismatch {
        /// Expected identifier.
        expected: u32,
        /// Offered identifier.
        actual: u32,
    },
    /// One relative path violates the portable v1 rules.
    #[error("entry {entry_id} has unsafe path {path:?}: {violation}")]
    UnsafePath {
        /// Offending entry identifier.
        entry_id: u32,
        /// Offending manifest path.
        path: String,
        /// Exact portable path violation.
        violation: ManifestPathViolation,
    },
    /// Two entries offer the same path.
    #[error("manifest path {path:?} is duplicated")]
    DuplicatePath {
        /// Duplicated relative path.
        path: String,
    },
    /// A nested entry does not have an explicit earlier directory entry.
    #[error("path {path:?} is missing parent directory entry {parent:?}")]
    MissingParentDirectory {
        /// Nested relative path.
        path: String,
        /// Missing immediate parent path.
        parent: String,
    },
    /// A nested entry attempts to descend through a regular file.
    #[error("path {path:?} descends through regular file {parent:?}")]
    ParentIsRegularFile {
        /// Nested relative path.
        path: String,
        /// Parent path declared as a file.
        parent: String,
    },
    /// A regular file has no required BLAKE3 digest.
    #[error("regular-file entry {entry_id} has no hash")]
    FileHashMissing {
        /// Offending entry identifier.
        entry_id: u32,
    },
    /// A directory has non-zero size or a file hash.
    #[error("directory entry {entry_id} has file metadata")]
    InvalidDirectoryMetadata {
        /// Offending entry identifier.
        entry_id: u32,
    },
    /// A declared aggregate count differs from the entries.
    #[error("{field} is {declared}; actual count is {actual}")]
    CountMismatch {
        /// Manifest field name.
        field: &'static str,
        /// Declared count.
        declared: u32,
        /// Count derived from entries.
        actual: u32,
    },
    /// Regular-file sizes overflow the v1 `u64` total.
    #[error("regular-file sizes overflow total_bytes")]
    TotalBytesOverflow,
    /// Declared aggregate bytes differ from the checked file-size sum.
    #[error("total_bytes is {declared}; actual total is {actual}")]
    TotalBytesMismatch {
        /// Declared byte total.
        declared: u64,
        /// Checked byte total.
        actual: u64,
    },
}

/// Reads one frame from an `envoix/manifest/1` stream.
pub async fn read_manifest_frame<R>(reader: &mut R) -> Result<ManifestFrame, ProtocolError>
where
    R: AsyncRead + Unpin,
{
    let (raw_frame_type, payload) = read_frame_payload(reader).await?;
    let frame_type = ManifestFrameType::try_from(raw_frame_type)
        .map_err(|error| CoreError::Protocol(error.to_string()))?;
    decode_manifest_frame(frame_type, &payload)
}

/// Writes one frame to an `envoix/manifest/1` stream.
pub async fn write_manifest_frame<W>(
    writer: &mut W,
    frame: &ManifestFrame,
) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    let (frame_type, payload) = encode_manifest_frame(frame)?;
    if payload.len() > MAX_FRAME_SIZE {
        return Err(CoreError::Protocol(format!(
            "frame length {} exceeds maximum {MAX_FRAME_SIZE}",
            payload.len()
        )));
    }
    write_frame_header(writer, frame_type as u8, payload.len()).await?;
    writer.write_all(&payload).await?;
    Ok(())
}

/// Writes a manifest chunk directly from borrowed payload bytes.
pub async fn write_manifest_chunk_frame<W>(
    writer: &mut W,
    manifest_id: &ManifestId,
    entry_id: u32,
    transfer_id: &TransferId,
    index: u64,
    offset: u64,
    bytes: &[u8],
) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    validate_identifier("manifest id", &manifest_id.0)?;
    validate_identifier("transfer id", &transfer_id.0)?;
    let manifest_id_len = encoded_field_length(manifest_id.0.len())?;
    let transfer_id_len = encoded_field_length(transfer_id.0.len())?;
    let bytes_len = encoded_field_length(bytes.len())?;
    let payload_len = 4_usize
        .checked_add(manifest_id.0.len())
        .and_then(|length| length.checked_add(4))
        .and_then(|length| length.checked_add(4))
        .and_then(|length| length.checked_add(transfer_id.0.len()))
        .and_then(|length| length.checked_add(8))
        .and_then(|length| length.checked_add(8))
        .and_then(|length| length.checked_add(4))
        .and_then(|length| length.checked_add(bytes.len()))
        .ok_or_else(|| CoreError::Protocol("frame length overflow".into()))?;

    write_frame_header(writer, ManifestFrameType::Chunk as u8, payload_len).await?;
    writer.write_all(&manifest_id_len.to_be_bytes()).await?;
    writer.write_all(manifest_id.0.as_bytes()).await?;
    writer.write_all(&entry_id.to_be_bytes()).await?;
    writer.write_all(&transfer_id_len.to_be_bytes()).await?;
    writer.write_all(transfer_id.0.as_bytes()).await?;
    writer.write_all(&index.to_be_bytes()).await?;
    writer.write_all(&offset.to_be_bytes()).await?;
    writer.write_all(&bytes_len.to_be_bytes()).await?;
    writer.write_all(bytes).await?;
    Ok(())
}

fn encode_manifest_frame(
    frame: &ManifestFrame,
) -> Result<(ManifestFrameType, Vec<u8>), ProtocolError> {
    let mut payload = Vec::new();
    let frame_type = match frame {
        ManifestFrame::Hello(hello) => {
            if hello.protocol_version != MANIFEST_V1_PROTOCOL_VERSION {
                return Err(CoreError::Protocol(format!(
                    "unsupported manifest version {}",
                    hello.protocol_version
                )));
            }
            write_u32(&mut payload, hello.protocol_version);
            write_peer_role(&mut payload, hello.role);
            ManifestFrameType::Hello
        }
        ManifestFrame::Offer(offer) => {
            write_u64(&mut payload, offer.chunk_size);
            write_bool(&mut payload, offer.resume_requested);
            let manifest = encode_manifest(&offer.manifest)?;
            write_bytes(&mut payload, &manifest)?;
            ManifestFrameType::Offer
        }
        ManifestFrame::Accept(accept) => {
            write_manifest_id(&mut payload, &accept.manifest_id)?;
            validate_entry_list_length(accept.entries.len())?;
            write_u32(&mut payload, accept.entries.len() as u32);
            let mut entry_ids = HashSet::with_capacity(accept.entries.len());
            for entry in &accept.entries {
                if !entry_ids.insert(entry.entry_id) {
                    return Err(CoreError::Protocol(format!(
                        "duplicate disposition for entry {}",
                        entry.entry_id
                    )));
                }
                validate_manifest_relative_path(&entry.final_relative_path)
                    .map_err(manifest_protocol_error)?;
                write_u32(&mut payload, entry.entry_id);
                write_disposition(&mut payload, entry.disposition);
                write_string(&mut payload, &entry.final_relative_path)?;
            }
            ManifestFrameType::Accept
        }
        ManifestFrame::EntryStart(start) => {
            write_entry_identity(
                &mut payload,
                &start.manifest_id,
                start.entry_id,
                &start.transfer_id,
            )?;
            ManifestFrameType::EntryStart
        }
        ManifestFrame::ResumeStatus(status) => {
            write_entry_identity(
                &mut payload,
                &status.manifest_id,
                status.entry_id,
                &status.transfer_id,
            )?;
            write_u64(&mut payload, status.next_chunk_index);
            write_u64(&mut payload, status.bytes_received);
            payload.extend_from_slice(&status.prefix_hash);
            ManifestFrameType::ResumeStatus
        }
        ManifestFrame::Chunk(chunk) => {
            write_entry_identity(
                &mut payload,
                &chunk.manifest_id,
                chunk.entry_id,
                &chunk.transfer_id,
            )?;
            write_u64(&mut payload, chunk.index);
            write_u64(&mut payload, chunk.offset);
            write_bytes(&mut payload, &chunk.bytes)?;
            ManifestFrameType::Chunk
        }
        ManifestFrame::EntryComplete(complete) => {
            write_entry_identity(
                &mut payload,
                &complete.manifest_id,
                complete.entry_id,
                &complete.transfer_id,
            )?;
            payload.extend_from_slice(&complete.file_hash);
            ManifestFrameType::EntryComplete
        }
        ManifestFrame::EntryCompleteAck(ack) => {
            write_entry_identity(
                &mut payload,
                &ack.manifest_id,
                ack.entry_id,
                &ack.transfer_id,
            )?;
            ManifestFrameType::EntryCompleteAck
        }
        ManifestFrame::Complete(complete) => {
            write_manifest_id(&mut payload, &complete.manifest_id)?;
            ManifestFrameType::Complete
        }
        ManifestFrame::CompleteAck(ack) => {
            write_manifest_id(&mut payload, &ack.manifest_id)?;
            validate_entry_list_length(ack.entries.len())?;
            write_u32(&mut payload, ack.entries.len() as u32);
            let mut entry_ids = HashSet::with_capacity(ack.entries.len());
            for entry in &ack.entries {
                validate_manifest_result(entry)?;
                if !entry_ids.insert(entry.entry_id) {
                    return Err(CoreError::Protocol(format!(
                        "duplicate result for entry {}",
                        entry.entry_id
                    )));
                }
                write_u32(&mut payload, entry.entry_id);
                write_result_status(&mut payload, entry.status);
                write_string(&mut payload, &entry.offered_relative_path)?;
                write_optional_string(&mut payload, entry.final_relative_path.as_deref())?;
                write_optional_string(&mut payload, entry.failure_code.as_deref())?;
            }
            ManifestFrameType::CompleteAck
        }
        ManifestFrame::Error(error) => {
            write_optional_manifest_id(&mut payload, error.manifest_id.as_ref())?;
            write_optional_u32(&mut payload, error.entry_id);
            if error.code.trim().is_empty() {
                return Err(CoreError::Protocol("manifest error code is empty".into()));
            }
            write_string(&mut payload, &error.code)?;
            write_string(&mut payload, &error.message)?;
            ManifestFrameType::Error
        }
    };
    Ok((frame_type, payload))
}

fn decode_manifest_frame(
    frame_type: ManifestFrameType,
    payload: &[u8],
) -> Result<ManifestFrame, ProtocolError> {
    let mut reader = PayloadReader::new(payload);
    let frame = match frame_type {
        ManifestFrameType::Hello => {
            let protocol_version = reader.read_u32()?;
            if protocol_version != MANIFEST_V1_PROTOCOL_VERSION {
                return Err(CoreError::Protocol(format!(
                    "unsupported manifest version {protocol_version}"
                )));
            }
            ManifestFrame::Hello(ManifestHelloV1 {
                protocol_version,
                role: reader.read_peer_role()?,
            })
        }
        ManifestFrameType::Offer => ManifestFrame::Offer(ManifestOfferV1 {
            chunk_size: reader.read_u64()?,
            resume_requested: reader.read_bool()?,
            manifest: decode_manifest(&reader.read_bytes()?)?,
        }),
        ManifestFrameType::Accept => {
            let manifest_id = read_manifest_id(&mut reader)?;
            let count = read_entry_list_length(&mut reader)?;
            let mut entries = Vec::with_capacity(count);
            let mut entry_ids = HashSet::with_capacity(count);
            for _ in 0..count {
                let entry_id = reader.read_u32()?;
                if !entry_ids.insert(entry_id) {
                    return Err(CoreError::Protocol(format!(
                        "duplicate disposition for entry {entry_id}"
                    )));
                }
                let disposition = read_disposition(&mut reader)?;
                let final_relative_path = reader.read_string()?;
                validate_manifest_relative_path(&final_relative_path)
                    .map_err(manifest_protocol_error)?;
                entries.push(ManifestEntryDispositionV1 {
                    entry_id,
                    disposition,
                    final_relative_path,
                });
            }
            ManifestFrame::Accept(ManifestAcceptV1 {
                manifest_id,
                entries,
            })
        }
        ManifestFrameType::EntryStart => {
            let (manifest_id, entry_id, transfer_id) = read_entry_identity(&mut reader)?;
            ManifestFrame::EntryStart(ManifestEntryStartV1 {
                manifest_id,
                entry_id,
                transfer_id,
            })
        }
        ManifestFrameType::ResumeStatus => {
            let (manifest_id, entry_id, transfer_id) = read_entry_identity(&mut reader)?;
            ManifestFrame::ResumeStatus(ManifestResumeStatusV1 {
                manifest_id,
                entry_id,
                transfer_id,
                next_chunk_index: reader.read_u64()?,
                bytes_received: reader.read_u64()?,
                prefix_hash: read_hash(&mut reader)?,
            })
        }
        ManifestFrameType::Chunk => {
            let (manifest_id, entry_id, transfer_id) = read_entry_identity(&mut reader)?;
            ManifestFrame::Chunk(ManifestChunkV1 {
                manifest_id,
                entry_id,
                transfer_id,
                index: reader.read_u64()?,
                offset: reader.read_u64()?,
                bytes: reader.read_bytes()?,
            })
        }
        ManifestFrameType::EntryComplete => {
            let (manifest_id, entry_id, transfer_id) = read_entry_identity(&mut reader)?;
            ManifestFrame::EntryComplete(ManifestEntryCompleteV1 {
                manifest_id,
                entry_id,
                transfer_id,
                file_hash: read_hash(&mut reader)?,
            })
        }
        ManifestFrameType::EntryCompleteAck => {
            let (manifest_id, entry_id, transfer_id) = read_entry_identity(&mut reader)?;
            ManifestFrame::EntryCompleteAck(ManifestEntryCompleteAckV1 {
                manifest_id,
                entry_id,
                transfer_id,
            })
        }
        ManifestFrameType::Complete => ManifestFrame::Complete(ManifestCompleteV1 {
            manifest_id: read_manifest_id(&mut reader)?,
        }),
        ManifestFrameType::CompleteAck => {
            let manifest_id = read_manifest_id(&mut reader)?;
            let count = read_entry_list_length(&mut reader)?;
            let mut entries = Vec::with_capacity(count);
            let mut entry_ids = HashSet::with_capacity(count);
            for _ in 0..count {
                let entry = ManifestEntryResultV1 {
                    entry_id: reader.read_u32()?,
                    status: read_result_status(&mut reader)?,
                    offered_relative_path: reader.read_string()?,
                    final_relative_path: read_optional_string(&mut reader)?,
                    failure_code: read_optional_string(&mut reader)?,
                };
                validate_manifest_result(&entry)?;
                if !entry_ids.insert(entry.entry_id) {
                    return Err(CoreError::Protocol(format!(
                        "duplicate result for entry {}",
                        entry.entry_id
                    )));
                }
                entries.push(entry);
            }
            ManifestFrame::CompleteAck(ManifestCompleteAckV1 {
                manifest_id,
                entries,
            })
        }
        ManifestFrameType::Error => {
            let error = ManifestErrorV1 {
                manifest_id: read_optional_manifest_id(&mut reader)?,
                entry_id: read_optional_u32(&mut reader)?,
                code: reader.read_string()?,
                message: reader.read_string()?,
            };
            if error.code.trim().is_empty() {
                return Err(CoreError::Protocol("manifest error code is empty".into()));
            }
            ManifestFrame::Error(error)
        }
    };
    reader.finish()?;
    Ok(frame)
}

fn encode_manifest(manifest: &ManifestV1) -> Result<Vec<u8>, ProtocolError> {
    manifest
        .validate_structure()
        .map_err(manifest_protocol_error)?;
    let mut output = Vec::new();
    write_manifest_id(&mut output, &manifest.manifest_id)?;
    write_u8(
        &mut output,
        match manifest.hash_algorithm {
            ManifestHashAlgorithm::Blake3_256 => 1,
        },
    );
    write_u32(&mut output, manifest.file_count);
    write_u32(&mut output, manifest.directory_count);
    write_u32(&mut output, manifest.root_count);
    write_u64(&mut output, manifest.total_bytes);
    write_u32(&mut output, manifest.entries.len() as u32);
    for entry in &manifest.entries {
        write_u32(&mut output, entry.entry_id);
        write_string(&mut output, &entry.relative_path)?;
        write_u8(
            &mut output,
            match entry.kind {
                ManifestEntryKind::RegularFile => 1,
                ManifestEntryKind::Directory => 2,
            },
        );
        write_u64(&mut output, entry.size);
        match entry.hash {
            Some(hash) => {
                write_bool(&mut output, true);
                output.extend_from_slice(&hash);
            }
            None => write_bool(&mut output, false),
        }
        match entry.modified_at_unix_ms {
            Some(modified_at) => {
                write_bool(&mut output, true);
                write_u64(&mut output, modified_at);
            }
            None => write_bool(&mut output, false),
        }
    }
    manifest
        .validate(output.len())
        .map_err(manifest_protocol_error)?;
    Ok(output)
}

fn decode_manifest(encoded: &[u8]) -> Result<ManifestV1, ProtocolError> {
    if encoded.len() > MAX_MANIFEST_V1_ENCODED_BYTES {
        return Err(manifest_protocol_error(
            ManifestValidationError::EncodedManifestTooLarge {
                actual: encoded.len(),
                maximum: MAX_MANIFEST_V1_ENCODED_BYTES,
            },
        ));
    }
    let mut reader = PayloadReader::new(encoded);
    let manifest_id = read_manifest_id(&mut reader)?;
    let hash_algorithm = match reader.read_u8()? {
        1 => ManifestHashAlgorithm::Blake3_256,
        value => {
            return Err(CoreError::Protocol(format!(
                "unknown manifest hash algorithm {value}"
            )));
        }
    };
    let file_count = reader.read_u32()?;
    let directory_count = reader.read_u32()?;
    let root_count = reader.read_u32()?;
    let total_bytes = reader.read_u64()?;
    let entry_count = read_entry_list_length(&mut reader)?;
    let mut entries = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let entry_id = reader.read_u32()?;
        let relative_path = reader.read_string()?;
        let kind = match reader.read_u8()? {
            1 => ManifestEntryKind::RegularFile,
            2 => ManifestEntryKind::Directory,
            value => {
                return Err(CoreError::Protocol(format!(
                    "unknown manifest entry kind {value}"
                )));
            }
        };
        let size = reader.read_u64()?;
        let hash = if reader.read_bool()? {
            Some(read_hash(&mut reader)?)
        } else {
            None
        };
        let modified_at_unix_ms = if reader.read_bool()? {
            Some(reader.read_u64()?)
        } else {
            None
        };
        entries.push(ManifestEntryV1 {
            entry_id,
            relative_path,
            kind,
            size,
            hash,
            modified_at_unix_ms,
        });
    }
    reader.finish()?;
    let manifest = ManifestV1 {
        manifest_id,
        entries,
        file_count,
        directory_count,
        root_count,
        total_bytes,
        hash_algorithm,
    };
    manifest
        .validate(encoded.len())
        .map_err(manifest_protocol_error)?;
    Ok(manifest)
}

fn write_manifest_id(output: &mut Vec<u8>, manifest_id: &ManifestId) -> Result<(), ProtocolError> {
    validate_identifier("manifest id", &manifest_id.0)?;
    write_string(output, &manifest_id.0)
}

fn read_manifest_id(reader: &mut PayloadReader<'_>) -> Result<ManifestId, ProtocolError> {
    let value = reader.read_string()?;
    validate_identifier("manifest id", &value)?;
    Ok(ManifestId::new(value))
}

fn write_transfer_id(output: &mut Vec<u8>, transfer_id: &TransferId) -> Result<(), ProtocolError> {
    validate_identifier("transfer id", &transfer_id.0)?;
    write_string(output, &transfer_id.0)
}

fn read_transfer_id(reader: &mut PayloadReader<'_>) -> Result<TransferId, ProtocolError> {
    let value = reader.read_string()?;
    validate_identifier("transfer id", &value)?;
    Ok(TransferId::new(value))
}

fn write_entry_identity(
    output: &mut Vec<u8>,
    manifest_id: &ManifestId,
    entry_id: u32,
    transfer_id: &TransferId,
) -> Result<(), ProtocolError> {
    write_manifest_id(output, manifest_id)?;
    write_u32(output, entry_id);
    write_transfer_id(output, transfer_id)
}

fn read_entry_identity(
    reader: &mut PayloadReader<'_>,
) -> Result<(ManifestId, u32, TransferId), ProtocolError> {
    Ok((
        read_manifest_id(reader)?,
        reader.read_u32()?,
        read_transfer_id(reader)?,
    ))
}

fn write_disposition(output: &mut Vec<u8>, disposition: ManifestEntryDispositionKind) {
    write_u8(
        output,
        match disposition {
            ManifestEntryDispositionKind::Transfer => 1,
            ManifestEntryDispositionKind::CreateDirectory => 2,
            ManifestEntryDispositionKind::SkipIdentical => 3,
        },
    );
}

fn read_disposition(
    reader: &mut PayloadReader<'_>,
) -> Result<ManifestEntryDispositionKind, ProtocolError> {
    match reader.read_u8()? {
        1 => Ok(ManifestEntryDispositionKind::Transfer),
        2 => Ok(ManifestEntryDispositionKind::CreateDirectory),
        3 => Ok(ManifestEntryDispositionKind::SkipIdentical),
        value => Err(CoreError::Protocol(format!(
            "unknown manifest entry disposition {value}"
        ))),
    }
}

fn write_result_status(output: &mut Vec<u8>, status: ManifestEntryResultStatus) {
    write_u8(
        output,
        match status {
            ManifestEntryResultStatus::Completed => 1,
            ManifestEntryResultStatus::SkippedIdentical => 2,
            ManifestEntryResultStatus::Renamed => 3,
            ManifestEntryResultStatus::Failed => 4,
            ManifestEntryResultStatus::Cancelled => 5,
        },
    );
}

fn read_result_status(
    reader: &mut PayloadReader<'_>,
) -> Result<ManifestEntryResultStatus, ProtocolError> {
    match reader.read_u8()? {
        1 => Ok(ManifestEntryResultStatus::Completed),
        2 => Ok(ManifestEntryResultStatus::SkippedIdentical),
        3 => Ok(ManifestEntryResultStatus::Renamed),
        4 => Ok(ManifestEntryResultStatus::Failed),
        5 => Ok(ManifestEntryResultStatus::Cancelled),
        value => Err(CoreError::Protocol(format!(
            "unknown manifest entry result {value}"
        ))),
    }
}

fn validate_manifest_result(result: &ManifestEntryResultV1) -> Result<(), ProtocolError> {
    validate_manifest_relative_path(&result.offered_relative_path)
        .map_err(manifest_protocol_error)?;
    if let Some(path) = &result.final_relative_path {
        validate_manifest_relative_path(path).map_err(manifest_protocol_error)?;
    }
    match result.status {
        ManifestEntryResultStatus::Completed
        | ManifestEntryResultStatus::SkippedIdentical
        | ManifestEntryResultStatus::Renamed
            if result.final_relative_path.is_none() =>
        {
            Err(CoreError::Protocol(format!(
                "entry {} result has no final path",
                result.entry_id
            )))
        }
        ManifestEntryResultStatus::Failed
            if result
                .failure_code
                .as_deref()
                .is_none_or(|code| code.trim().is_empty()) =>
        {
            Err(CoreError::Protocol(format!(
                "failed entry {} has no failure code",
                result.entry_id
            )))
        }
        _ => Ok(()),
    }
}

fn write_optional_manifest_id(
    output: &mut Vec<u8>,
    manifest_id: Option<&ManifestId>,
) -> Result<(), ProtocolError> {
    write_bool(output, manifest_id.is_some());
    if let Some(manifest_id) = manifest_id {
        write_manifest_id(output, manifest_id)?;
    }
    Ok(())
}

fn read_optional_manifest_id(
    reader: &mut PayloadReader<'_>,
) -> Result<Option<ManifestId>, ProtocolError> {
    if reader.read_bool()? {
        Ok(Some(read_manifest_id(reader)?))
    } else {
        Ok(None)
    }
}

fn write_optional_u32(output: &mut Vec<u8>, value: Option<u32>) {
    write_bool(output, value.is_some());
    if let Some(value) = value {
        write_u32(output, value);
    }
}

fn read_optional_u32(reader: &mut PayloadReader<'_>) -> Result<Option<u32>, ProtocolError> {
    if reader.read_bool()? {
        Ok(Some(reader.read_u32()?))
    } else {
        Ok(None)
    }
}

fn write_optional_string(output: &mut Vec<u8>, value: Option<&str>) -> Result<(), ProtocolError> {
    write_bool(output, value.is_some());
    if let Some(value) = value {
        write_string(output, value)?;
    }
    Ok(())
}

fn read_optional_string(reader: &mut PayloadReader<'_>) -> Result<Option<String>, ProtocolError> {
    if reader.read_bool()? {
        Ok(Some(reader.read_string()?))
    } else {
        Ok(None)
    }
}

fn read_hash(reader: &mut PayloadReader<'_>) -> Result<[u8; 32], ProtocolError> {
    Ok(reader
        .take(32)?
        .try_into()
        .expect("slice length was checked"))
}

fn validate_entry_list_length(length: usize) -> Result<(), ProtocolError> {
    if length > MAX_MANIFEST_V1_ENTRIES {
        Err(CoreError::Protocol(format!(
            "manifest entry list contains {length} entries; maximum is {MAX_MANIFEST_V1_ENTRIES}"
        )))
    } else {
        Ok(())
    }
}

fn read_entry_list_length(reader: &mut PayloadReader<'_>) -> Result<usize, ProtocolError> {
    let length = reader.read_u32()? as usize;
    validate_entry_list_length(length)?;
    Ok(length)
}

fn validate_identifier(field: &str, value: &str) -> Result<(), ProtocolError> {
    if value.trim().is_empty() {
        Err(CoreError::Protocol(format!("{field} is empty")))
    } else {
        Ok(())
    }
}

fn encoded_field_length(length: usize) -> Result<u32, ProtocolError> {
    u32::try_from(length).map_err(|_| CoreError::Protocol("field length exceeds u32".into()))
}

fn manifest_protocol_error(error: impl fmt::Display) -> ProtocolError {
    CoreError::Protocol(format!("invalid manifest: {error}"))
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
