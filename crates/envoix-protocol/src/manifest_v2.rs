//! Frozen structural Manifest v2 contract.
//!
//! Goal 0 intentionally exposes only the bounded structural offer codec. The
//! transfer engine, persistence, destination effects, and native integration
//! are implemented by later goals against this contract.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MANIFEST_V2_ALPN: &[u8] = b"envoix/manifest/2";
pub const MANIFEST_V2_PROTOCOL_VERSION: u16 = 2;

pub const MAX_MANIFEST_V2_ENCODED_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_MANIFEST_V2_ENTRIES: usize = 10_000;
pub const MAX_MANIFEST_V2_ROOTS: usize = 1_024;
pub const MAX_MANIFEST_V2_COMPONENT_BYTES: usize = 255;
pub const MAX_MANIFEST_V2_PATH_BYTES: usize = 4_096;
pub const MAX_MANIFEST_V2_PATH_DEPTH: usize = 64;
pub const DEFAULT_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES: u32 = 4 * 1024 * 1024;
pub const MAX_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES: u32 = 16 * 1024 * 1024;
pub const MAX_MANIFEST_V2_BLOCK_ENCODED_BYTES: u32 =
    MAX_MANIFEST_V2_BLOCK_PLAINTEXT_BYTES + 64 * 1024;

const OFFER_MAGIC: &[u8; 4] = b"ENV2";
const OFFER_HEADER_BYTES: usize = 12;
const DIGEST_BYTES: usize = 32;
const JOB_ID_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct JobIdV2(pub [u8; JOB_ID_BYTES]);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ContentDigestV2(pub [u8; DIGEST_BYTES]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum ManifestV2FrameType {
    Offer = 1,
    Accept = 2,
    AcceptCommittedAck = 3,
    EntryStart = 4,
    EntryContentDigest = 5,
    EntryBlock = 6,
    EntryComplete = 7,
    EntryResult = 8,
    JobComplete = 9,
    DeliveryProof = 10,
    DeliveryProofAck = 11,
    ResumeRequest = 12,
    ResumeStatus = 13,
    ProofChallenge = 14,
    ProofResponse = 15,
    Cancel = 16,
    Error = 17,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u8)]
pub enum CompressionPolicyV2 {
    Never = 0,
    Always = 1,
    Smart = 2,
}

impl TryFrom<u8> for CompressionPolicyV2 {
    type Error = ManifestV2CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Never),
            1 => Ok(Self::Always),
            2 => Ok(Self::Smart),
            other => Err(ManifestV2CodecError::UnknownCompressionPolicy(other)),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u8)]
pub enum ManifestEntryKindV2 {
    RegularFile = 1,
    Directory = 2,
}

impl TryFrom<u8> for ManifestEntryKindV2 {
    type Error = ManifestV2CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::RegularFile),
            2 => Ok(Self::Directory),
            other => Err(ManifestV2CodecError::UnknownEntryKind(other)),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EntryContentDigestV2 {
    Deferred,
    Known(ContentDigestV2),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SourceCompletenessV2 {
    Complete,
    UserApprovedPartial { omitted_entry_count: u64 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestRootV2 {
    pub root_id: u32,
    pub root_entry_id: u32,
    pub requested_name: String,
    pub completeness: SourceCompletenessV2,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestEntryV2 {
    pub entry_id: u32,
    pub root_id: u32,
    pub parent_entry_id: Option<u32>,
    pub component: String,
    pub kind: ManifestEntryKindV2,
    pub plaintext_size: u64,
    pub content_digest: EntryContentDigestV2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestTotalsV2 {
    pub file_count: u32,
    pub directory_count: u32,
    pub total_plaintext_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestV2 {
    pub job_id: JobIdV2,
    pub generation: u32,
    pub selection_revision: u64,
    pub compression_policy: CompressionPolicyV2,
    pub roots: Vec<ManifestRootV2>,
    pub entries: Vec<ManifestEntryV2>,
    pub totals: ManifestTotalsV2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestOfferV2 {
    pub structural_digest: ContentDigestV2,
    pub manifest: ManifestV2,
}

impl ManifestV2 {
    pub fn validate(&self) -> Result<(), ManifestV2ValidationError> {
        if self.job_id.0 == [0; JOB_ID_BYTES] {
            return Err(ManifestV2ValidationError::ZeroJobId);
        }
        if self.generation == 0 {
            return Err(ManifestV2ValidationError::ZeroGeneration);
        }
        if self.selection_revision == 0 {
            return Err(ManifestV2ValidationError::ZeroSelectionRevision);
        }
        if self.roots.is_empty() {
            return Err(ManifestV2ValidationError::EmptyRoots);
        }
        if self.roots.len() > MAX_MANIFEST_V2_ROOTS {
            return Err(ManifestV2ValidationError::TooManyRoots {
                count: self.roots.len(),
                maximum: MAX_MANIFEST_V2_ROOTS,
            });
        }
        if self.entries.is_empty() {
            return Err(ManifestV2ValidationError::EmptyEntries);
        }
        if self.entries.len() > MAX_MANIFEST_V2_ENTRIES {
            return Err(ManifestV2ValidationError::TooManyEntries {
                count: self.entries.len(),
                maximum: MAX_MANIFEST_V2_ENTRIES,
            });
        }

        let mut root_entry_ids = HashSet::with_capacity(self.roots.len());
        for (index, root) in self.roots.iter().enumerate() {
            let expected = u32::try_from(index).expect("root limit fits in u32");
            if root.root_id != expected {
                return Err(ManifestV2ValidationError::NonCanonicalRootId {
                    expected,
                    actual: root.root_id,
                });
            }
            if usize::try_from(root.root_entry_id)
                .ok()
                .filter(|entry_id| *entry_id < self.entries.len())
                .is_none()
            {
                return Err(ManifestV2ValidationError::InvalidRootEntry {
                    root_id: root.root_id,
                    entry_id: root.root_entry_id,
                });
            }
            validate_component(&root.requested_name).map_err(|violation| {
                ManifestV2ValidationError::UnsafeRootName {
                    root_id: root.root_id,
                    violation,
                }
            })?;
            if matches!(
                root.completeness,
                SourceCompletenessV2::UserApprovedPartial {
                    omitted_entry_count: 0
                }
            ) {
                return Err(ManifestV2ValidationError::EmptyPartialFact {
                    root_id: root.root_id,
                });
            }
            root_entry_ids.insert(root.root_entry_id);
        }

        let mut sibling_components = HashSet::with_capacity(self.entries.len());
        let mut file_count = 0_u32;
        let mut directory_count = 0_u32;
        let mut total_plaintext_bytes = 0_u64;
        let mut depths: Vec<usize> = Vec::with_capacity(self.entries.len());
        let mut path_bytes: Vec<usize> = Vec::with_capacity(self.entries.len());
        let mut children = vec![Vec::new(); self.entries.len()];

        for (index, entry) in self.entries.iter().enumerate() {
            let expected = u32::try_from(index).expect("entry limit fits in u32");
            if entry.entry_id != expected {
                return Err(ManifestV2ValidationError::NonCanonicalEntryId {
                    expected,
                    actual: entry.entry_id,
                });
            }
            validate_component(&entry.component).map_err(|violation| {
                ManifestV2ValidationError::UnsafeComponent {
                    entry_id: entry.entry_id,
                    violation,
                }
            })?;

            let root_index = usize::try_from(entry.root_id).unwrap_or(usize::MAX);
            let Some(root) = self.roots.get(root_index) else {
                return Err(ManifestV2ValidationError::UnknownRoot {
                    entry_id: entry.entry_id,
                    root_id: entry.root_id,
                });
            };

            let (depth, bytes) = match entry.parent_entry_id {
                None => {
                    if root.root_entry_id != entry.entry_id {
                        return Err(ManifestV2ValidationError::UnexpectedRootEntry {
                            entry_id: entry.entry_id,
                        });
                    }
                    if root.requested_name != entry.component {
                        return Err(ManifestV2ValidationError::RootNameMismatch {
                            root_id: root.root_id,
                            entry_id: entry.entry_id,
                        });
                    }
                    (0_usize, 0_usize)
                }
                Some(parent_id) => {
                    let parent_index = usize::try_from(parent_id).unwrap_or(usize::MAX);
                    if parent_index >= index {
                        return Err(ManifestV2ValidationError::NonCanonicalParent {
                            entry_id: entry.entry_id,
                            parent_entry_id: parent_id,
                        });
                    }
                    let parent = &self.entries[parent_index];
                    if parent.root_id != entry.root_id {
                        return Err(ManifestV2ValidationError::CrossRootParent {
                            entry_id: entry.entry_id,
                            parent_entry_id: parent_id,
                        });
                    }
                    if parent.kind != ManifestEntryKindV2::Directory {
                        return Err(ManifestV2ValidationError::FileParent {
                            entry_id: entry.entry_id,
                            parent_entry_id: parent_id,
                        });
                    }
                    children[parent_index].push(index);
                    (
                        depths[parent_index] + 1,
                        path_bytes[parent_index]
                            .checked_add(entry.component.len())
                            .and_then(|value| value.checked_add(1))
                            .ok_or(ManifestV2ValidationError::PathSizeOverflow {
                                entry_id: entry.entry_id,
                            })?,
                    )
                }
            };

            if depth > MAX_MANIFEST_V2_PATH_DEPTH {
                return Err(ManifestV2ValidationError::PathTooDeep {
                    entry_id: entry.entry_id,
                    depth,
                    maximum: MAX_MANIFEST_V2_PATH_DEPTH,
                });
            }
            if bytes > MAX_MANIFEST_V2_PATH_BYTES {
                return Err(ManifestV2ValidationError::PathTooLong {
                    entry_id: entry.entry_id,
                    bytes,
                    maximum: MAX_MANIFEST_V2_PATH_BYTES,
                });
            }
            depths.push(depth);
            path_bytes.push(bytes);

            if !sibling_components.insert((entry.root_id, entry.parent_entry_id, &entry.component))
            {
                return Err(ManifestV2ValidationError::DuplicateSiblingComponent {
                    entry_id: entry.entry_id,
                });
            }

            match entry.kind {
                ManifestEntryKindV2::RegularFile => {
                    file_count = file_count
                        .checked_add(1)
                        .ok_or(ManifestV2ValidationError::AggregateOverflow)?;
                    total_plaintext_bytes = total_plaintext_bytes
                        .checked_add(entry.plaintext_size)
                        .ok_or(ManifestV2ValidationError::AggregateOverflow)?;
                }
                ManifestEntryKindV2::Directory => {
                    if entry.plaintext_size != 0
                        || entry.content_digest != EntryContentDigestV2::Deferred
                    {
                        return Err(ManifestV2ValidationError::InvalidDirectoryMetadata {
                            entry_id: entry.entry_id,
                        });
                    }
                    directory_count = directory_count
                        .checked_add(1)
                        .ok_or(ManifestV2ValidationError::AggregateOverflow)?;
                }
            }
        }

        if root_entry_ids.len() != self.roots.len() {
            return Err(ManifestV2ValidationError::DuplicateRootEntry);
        }

        let mut canonical_order = Vec::with_capacity(self.entries.len());
        for root in &self.roots {
            let mut stack = vec![root.root_entry_id as usize];
            while let Some(entry_index) = stack.pop() {
                canonical_order.push(entry_index);
                children[entry_index].sort_unstable_by(|left, right| {
                    self.entries[*left]
                        .component
                        .cmp(&self.entries[*right].component)
                });
                stack.extend(children[entry_index].iter().rev().copied());
            }
        }
        for (expected, actual) in canonical_order.into_iter().enumerate() {
            if expected != actual {
                return Err(ManifestV2ValidationError::NonCanonicalEntryOrder {
                    expected: expected as u32,
                    actual: actual as u32,
                });
            }
        }

        let actual = ManifestTotalsV2 {
            file_count,
            directory_count,
            total_plaintext_bytes,
        };
        if self.totals != actual {
            return Err(ManifestV2ValidationError::TotalsMismatch {
                declared: self.totals,
                actual,
            });
        }
        Ok(())
    }
}

pub fn encode_manifest_offer_v2(manifest: &ManifestV2) -> Result<Vec<u8>, ManifestV2CodecError> {
    let offer = build_manifest_offer_v2(manifest.clone())?;
    let mut body = Vec::new();
    encode_manifest_body(&offer.manifest, &mut body);
    let payload_len = DIGEST_BYTES
        .checked_add(body.len())
        .ok_or(ManifestV2CodecError::EncodedOfferTooLarge)?;
    let encoded_len = OFFER_HEADER_BYTES
        .checked_add(payload_len)
        .ok_or(ManifestV2CodecError::EncodedOfferTooLarge)?;
    if encoded_len > MAX_MANIFEST_V2_ENCODED_BYTES {
        return Err(ManifestV2CodecError::EncodedOfferTooLarge);
    }

    let mut encoded = Vec::with_capacity(encoded_len);
    encoded.extend_from_slice(OFFER_MAGIC);
    push_u16(&mut encoded, MANIFEST_V2_PROTOCOL_VERSION);
    push_u16(&mut encoded, ManifestV2FrameType::Offer as u16);
    push_u32(
        &mut encoded,
        u32::try_from(payload_len).map_err(|_| ManifestV2CodecError::EncodedOfferTooLarge)?,
    );
    encoded.extend_from_slice(&offer.structural_digest.0);
    encoded.extend_from_slice(&body);
    Ok(encoded)
}

/// Validates and seals one structural Manifest without performing network I/O.
pub fn build_manifest_offer_v2(
    manifest: ManifestV2,
) -> Result<ManifestOfferV2, ManifestV2CodecError> {
    manifest.validate()?;
    let mut body = Vec::new();
    encode_manifest_body(&manifest, &mut body);
    let structural_digest = ContentDigestV2(*blake3::hash(&body).as_bytes());
    Ok(ManifestOfferV2 {
        structural_digest,
        manifest,
    })
}

pub fn decode_manifest_offer_v2(encoded: &[u8]) -> Result<ManifestOfferV2, ManifestV2CodecError> {
    if encoded.len() > MAX_MANIFEST_V2_ENCODED_BYTES {
        return Err(ManifestV2CodecError::EncodedOfferTooLarge);
    }
    let mut reader = Reader::new(encoded);
    if reader.read_exact(OFFER_MAGIC.len())? != OFFER_MAGIC {
        return Err(ManifestV2CodecError::BadMagic);
    }
    let version = reader.read_u16()?;
    if version != MANIFEST_V2_PROTOCOL_VERSION {
        return Err(ManifestV2CodecError::UnsupportedVersion(version));
    }
    let frame_type = reader.read_u16()?;
    if frame_type != ManifestV2FrameType::Offer as u16 {
        return Err(ManifestV2CodecError::UnexpectedFrameType(frame_type));
    }
    let payload_len = usize::try_from(reader.read_u32()?).expect("u32 fits usize");
    if payload_len != reader.remaining() {
        return Err(ManifestV2CodecError::LengthMismatch);
    }

    let claimed_digest = reader.read_array::<DIGEST_BYTES>()?;
    let body = reader.remaining_slice();
    let actual_digest = blake3::hash(body);
    if claimed_digest != *actual_digest.as_bytes() {
        return Err(ManifestV2CodecError::StructuralDigestMismatch);
    }

    let manifest = decode_manifest_body(&mut reader)?;
    if reader.remaining() != 0 {
        return Err(ManifestV2CodecError::TrailingBytes);
    }
    manifest.validate()?;
    Ok(ManifestOfferV2 {
        structural_digest: ContentDigestV2(claimed_digest),
        manifest,
    })
}

fn encode_manifest_body(manifest: &ManifestV2, output: &mut Vec<u8>) {
    output.extend_from_slice(&manifest.job_id.0);
    push_u32(output, manifest.generation);
    push_u64(output, manifest.selection_revision);
    output.push(manifest.compression_policy as u8);
    push_u32(output, manifest.roots.len() as u32);
    for root in &manifest.roots {
        push_u32(output, root.root_id);
        push_u32(output, root.root_entry_id);
        push_bytes(output, root.requested_name.as_bytes());
        match root.completeness {
            SourceCompletenessV2::Complete => output.push(0),
            SourceCompletenessV2::UserApprovedPartial {
                omitted_entry_count,
            } => {
                output.push(1);
                push_u64(output, omitted_entry_count);
            }
        }
    }
    push_u32(output, manifest.entries.len() as u32);
    let mut wire_paths: Vec<Vec<&str>> = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        push_u32(output, entry.entry_id);
        push_u32(output, entry.root_id);
        let path = match entry.parent_entry_id {
            None => Vec::new(),
            Some(parent_entry_id) => {
                let mut path = wire_paths[parent_entry_id as usize].clone();
                path.push(&entry.component);
                path
            }
        };
        push_u32(output, path.len() as u32);
        for component in &path {
            push_bytes(output, component.as_bytes());
        }
        wire_paths.push(path);
        output.push(entry.kind as u8);
        push_u64(output, entry.plaintext_size);
        match entry.content_digest {
            EntryContentDigestV2::Deferred => output.push(0),
            EntryContentDigestV2::Known(digest) => {
                output.push(1);
                output.extend_from_slice(&digest.0);
            }
        }
    }
    push_u32(output, manifest.totals.file_count);
    push_u32(output, manifest.totals.directory_count);
    push_u64(output, manifest.totals.total_plaintext_bytes);
}

fn decode_manifest_body(reader: &mut Reader<'_>) -> Result<ManifestV2, ManifestV2CodecError> {
    let job_id = JobIdV2(reader.read_array::<JOB_ID_BYTES>()?);
    let generation = reader.read_u32()?;
    let selection_revision = reader.read_u64()?;
    let compression_policy = CompressionPolicyV2::try_from(reader.read_u8()?)?;

    let root_count = usize::try_from(reader.read_u32()?).expect("u32 fits usize");
    if root_count > MAX_MANIFEST_V2_ROOTS {
        return Err(ManifestV2CodecError::RootCountTooLarge(root_count));
    }
    let mut roots = Vec::with_capacity(root_count);
    for _ in 0..root_count {
        let root_id = reader.read_u32()?;
        let root_entry_id = reader.read_u32()?;
        let requested_name = reader.read_component()?;
        let completeness = match reader.read_u8()? {
            0 => SourceCompletenessV2::Complete,
            1 => SourceCompletenessV2::UserApprovedPartial {
                omitted_entry_count: reader.read_u64()?,
            },
            other => return Err(ManifestV2CodecError::UnknownCompleteness(other)),
        };
        roots.push(ManifestRootV2 {
            root_id,
            root_entry_id,
            requested_name,
            completeness,
        });
    }

    let entry_count = usize::try_from(reader.read_u32()?).expect("u32 fits usize");
    if entry_count > MAX_MANIFEST_V2_ENTRIES {
        return Err(ManifestV2CodecError::EntryCountTooLarge(entry_count));
    }
    let mut entries = Vec::with_capacity(entry_count);
    let mut path_to_entry = HashMap::with_capacity(entry_count);
    for _ in 0..entry_count {
        let entry_id = reader.read_u32()?;
        let root_id = reader.read_u32()?;
        let path_count = usize::try_from(reader.read_u32()?).expect("u32 fits usize");
        if path_count > MAX_MANIFEST_V2_PATH_DEPTH {
            return Err(ManifestV2CodecError::PathDepthTooLarge(path_count));
        }
        let mut path = Vec::with_capacity(path_count);
        let mut path_bytes = 0_usize;
        for index in 0..path_count {
            let component = reader.read_component()?;
            path_bytes = path_bytes
                .checked_add(component.len())
                .and_then(|value| value.checked_add(usize::from(index > 0)))
                .ok_or(ManifestV2CodecError::PathBytesTooLarge(usize::MAX))?;
            if path_bytes > MAX_MANIFEST_V2_PATH_BYTES {
                return Err(ManifestV2CodecError::PathBytesTooLarge(path_bytes));
            }
            path.push(component);
        }
        let root_index = usize::try_from(root_id).unwrap_or(usize::MAX);
        let root = roots
            .get(root_index)
            .ok_or(ManifestV2CodecError::UnknownRootInWire(root_id))?;
        let (parent_entry_id, component) = if let Some(component) = path.last().cloned() {
            let parent_path = path[..path.len() - 1].to_vec();
            let parent_entry_id = path_to_entry
                .get(&(root_id, parent_path))
                .copied()
                .ok_or(ManifestV2CodecError::MissingWireParent(entry_id))?;
            (Some(parent_entry_id), component)
        } else {
            (None, root.requested_name.clone())
        };
        let kind = ManifestEntryKindV2::try_from(reader.read_u8()?)?;
        let plaintext_size = reader.read_u64()?;
        let content_digest = match reader.read_u8()? {
            0 => EntryContentDigestV2::Deferred,
            1 => EntryContentDigestV2::Known(ContentDigestV2(reader.read_array::<DIGEST_BYTES>()?)),
            other => return Err(ManifestV2CodecError::UnknownDigestTag(other)),
        };
        let entry = ManifestEntryV2 {
            entry_id,
            root_id,
            parent_entry_id,
            component,
            kind,
            plaintext_size,
            content_digest,
        };
        if path_to_entry.insert((root_id, path), entry_id).is_some() {
            return Err(ManifestV2CodecError::DuplicateWirePath(entry_id));
        }
        entries.push(entry);
    }

    Ok(ManifestV2 {
        job_id,
        generation,
        selection_revision,
        compression_policy,
        roots,
        entries,
        totals: ManifestTotalsV2 {
            file_count: reader.read_u32()?,
            directory_count: reader.read_u32()?,
            total_plaintext_bytes: reader.read_u64()?,
        },
    })
}

fn validate_component(component: &str) -> Result<(), ManifestV2ComponentViolation> {
    if component.is_empty() {
        return Err(ManifestV2ComponentViolation::Empty);
    }
    if component == "." {
        return Err(ManifestV2ComponentViolation::CurrentDirectory);
    }
    if component == ".." {
        return Err(ManifestV2ComponentViolation::ParentDirectory);
    }
    if component.len() > MAX_MANIFEST_V2_COMPONENT_BYTES {
        return Err(ManifestV2ComponentViolation::TooLong {
            bytes: component.len(),
            maximum: MAX_MANIFEST_V2_COMPONENT_BYTES,
        });
    }
    if component
        .chars()
        .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(ManifestV2ComponentViolation::ControlOrSeparator);
    }
    Ok(())
}

fn push_bytes(output: &mut Vec<u8>, bytes: &[u8]) {
    push_u32(output, bytes.len() as u32);
    output.extend_from_slice(bytes);
}

fn push_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn push_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn remaining_slice(&self) -> &'a [u8] {
        &self.bytes[self.offset..]
    }

    fn read_exact(&mut self, length: usize) -> Result<&'a [u8], ManifestV2CodecError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(ManifestV2CodecError::Truncated)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(ManifestV2CodecError::Truncated)?;
        self.offset = end;
        Ok(bytes)
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], ManifestV2CodecError> {
        self.read_exact(N)?
            .try_into()
            .map_err(|_| ManifestV2CodecError::Truncated)
    }

    fn read_u8(&mut self) -> Result<u8, ManifestV2CodecError> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, ManifestV2CodecError> {
        Ok(u16::from_be_bytes(self.read_array()?))
    }

    fn read_u32(&mut self) -> Result<u32, ManifestV2CodecError> {
        Ok(u32::from_be_bytes(self.read_array()?))
    }

    fn read_u64(&mut self) -> Result<u64, ManifestV2CodecError> {
        Ok(u64::from_be_bytes(self.read_array()?))
    }

    fn read_component(&mut self) -> Result<String, ManifestV2CodecError> {
        let length = usize::try_from(self.read_u32()?).expect("u32 fits usize");
        if length > MAX_MANIFEST_V2_COMPONENT_BYTES {
            return Err(ManifestV2CodecError::ComponentTooLarge(length));
        }
        let bytes = self.read_exact(length)?;
        let value = std::str::from_utf8(bytes).map_err(|_| ManifestV2CodecError::InvalidUtf8)?;
        Ok(value.to_owned())
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ManifestV2ComponentViolation {
    #[error("component is empty")]
    Empty,
    #[error("component is current-directory marker")]
    CurrentDirectory,
    #[error("component is parent-directory marker")]
    ParentDirectory,
    #[error("component contains a control character or path separator")]
    ControlOrSeparator,
    #[error("component is {bytes} bytes; maximum is {maximum}")]
    TooLong { bytes: usize, maximum: usize },
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ManifestV2ValidationError {
    #[error("job id is all zeroes")]
    ZeroJobId,
    #[error("generation zero is reserved")]
    ZeroGeneration,
    #[error("selection revision zero is reserved")]
    ZeroSelectionRevision,
    #[error("manifest has no roots")]
    EmptyRoots,
    #[error("manifest has {count} roots; maximum is {maximum}")]
    TooManyRoots { count: usize, maximum: usize },
    #[error("manifest has no entries")]
    EmptyEntries,
    #[error("manifest has {count} entries; maximum is {maximum}")]
    TooManyEntries { count: usize, maximum: usize },
    #[error("root id {actual} is noncanonical; expected {expected}")]
    NonCanonicalRootId { expected: u32, actual: u32 },
    #[error("root {root_id} has unsafe requested name: {violation}")]
    UnsafeRootName {
        root_id: u32,
        violation: ManifestV2ComponentViolation,
    },
    #[error("root {root_id} references invalid entry {entry_id}")]
    InvalidRootEntry { root_id: u32, entry_id: u32 },
    #[error("root {root_id} declares a partial source without omitted entries")]
    EmptyPartialFact { root_id: u32 },
    #[error("multiple roots reference the same root entry")]
    DuplicateRootEntry,
    #[error("entry id {actual} is noncanonical; expected {expected}")]
    NonCanonicalEntryId { expected: u32, actual: u32 },
    #[error("entry {actual} is out of canonical preorder; expected entry {expected}")]
    NonCanonicalEntryOrder { expected: u32, actual: u32 },
    #[error("entry {entry_id} has unsafe component: {violation}")]
    UnsafeComponent {
        entry_id: u32,
        violation: ManifestV2ComponentViolation,
    },
    #[error("entry {entry_id} references unknown root {root_id}")]
    UnknownRoot { entry_id: u32, root_id: u32 },
    #[error("entry {entry_id} is not its root's declared root entry")]
    UnexpectedRootEntry { entry_id: u32 },
    #[error("root {root_id} and entry {entry_id} have different requested names")]
    RootNameMismatch { root_id: u32, entry_id: u32 },
    #[error("entry {entry_id} has noncanonical parent {parent_entry_id}")]
    NonCanonicalParent { entry_id: u32, parent_entry_id: u32 },
    #[error("entry {entry_id} has parent {parent_entry_id} in another root")]
    CrossRootParent { entry_id: u32, parent_entry_id: u32 },
    #[error("entry {entry_id} has regular-file parent {parent_entry_id}")]
    FileParent { entry_id: u32, parent_entry_id: u32 },
    #[error("entry {entry_id} path size overflowed")]
    PathSizeOverflow { entry_id: u32 },
    #[error("entry {entry_id} path depth {depth} exceeds {maximum}")]
    PathTooDeep {
        entry_id: u32,
        depth: usize,
        maximum: usize,
    },
    #[error("entry {entry_id} path length {bytes} exceeds {maximum}")]
    PathTooLong {
        entry_id: u32,
        bytes: usize,
        maximum: usize,
    },
    #[error("entry {entry_id} duplicates a sibling component")]
    DuplicateSiblingComponent { entry_id: u32 },
    #[error("directory entry {entry_id} carries file metadata")]
    InvalidDirectoryMetadata { entry_id: u32 },
    #[error("manifest aggregate overflowed")]
    AggregateOverflow,
    #[error("declared totals {declared:?} do not match {actual:?}")]
    TotalsMismatch {
        declared: ManifestTotalsV2,
        actual: ManifestTotalsV2,
    },
}

#[derive(Debug, Error, PartialEq)]
pub enum ManifestV2CodecError {
    #[error("manifest validation failed: {0}")]
    Validation(#[from] ManifestV2ValidationError),
    #[error("encoded offer exceeds its fixed budget")]
    EncodedOfferTooLarge,
    #[error("encoded offer is truncated")]
    Truncated,
    #[error("offer magic is invalid")]
    BadMagic,
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("unexpected frame type {0}")]
    UnexpectedFrameType(u16),
    #[error("declared payload length does not match the frame")]
    LengthMismatch,
    #[error("structural digest does not match canonical body")]
    StructuralDigestMismatch,
    #[error("unknown compression policy {0}")]
    UnknownCompressionPolicy(u8),
    #[error("unknown entry kind {0}")]
    UnknownEntryKind(u8),
    #[error("unknown source completeness tag {0}")]
    UnknownCompleteness(u8),
    #[error("unknown content digest tag {0}")]
    UnknownDigestTag(u8),
    #[error("root count {0} exceeds the fixed limit")]
    RootCountTooLarge(usize),
    #[error("entry count {0} exceeds the fixed limit")]
    EntryCountTooLarge(usize),
    #[error("component length {0} exceeds the fixed limit")]
    ComponentTooLarge(usize),
    #[error("wire path depth {0} exceeds the fixed limit")]
    PathDepthTooLarge(usize),
    #[error("wire path length {0} exceeds the fixed limit")]
    PathBytesTooLarge(usize),
    #[error("wire entry references unknown root {0}")]
    UnknownRootInWire(u32),
    #[error("wire entry {0} appears before its parent")]
    MissingWireParent(u32),
    #[error("wire entry {0} duplicates a canonical path")]
    DuplicateWirePath(u32),
    #[error("component is not valid UTF-8")]
    InvalidUtf8,
    #[error("offer has trailing bytes")]
    TrailingBytes,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> ManifestV2 {
        ManifestV2 {
            job_id: JobIdV2(*b"job-v2-fixture!!"),
            generation: 7,
            selection_revision: 11,
            compression_policy: CompressionPolicyV2::Smart,
            roots: vec![
                ManifestRootV2 {
                    root_id: 0,
                    root_entry_id: 0,
                    requested_name: "Photos".into(),
                    completeness: SourceCompletenessV2::UserApprovedPartial {
                        omitted_entry_count: 2,
                    },
                },
                ManifestRootV2 {
                    root_id: 1,
                    root_entry_id: 3,
                    requested_name: "notes.txt".into(),
                    completeness: SourceCompletenessV2::Complete,
                },
            ],
            entries: vec![
                ManifestEntryV2 {
                    entry_id: 0,
                    root_id: 0,
                    parent_entry_id: None,
                    component: "Photos".into(),
                    kind: ManifestEntryKindV2::Directory,
                    plaintext_size: 0,
                    content_digest: EntryContentDigestV2::Deferred,
                },
                ManifestEntryV2 {
                    entry_id: 1,
                    root_id: 0,
                    parent_entry_id: Some(0),
                    component: "Empty".into(),
                    kind: ManifestEntryKindV2::Directory,
                    plaintext_size: 0,
                    content_digest: EntryContentDigestV2::Deferred,
                },
                ManifestEntryV2 {
                    entry_id: 2,
                    root_id: 0,
                    parent_entry_id: Some(0),
                    component: "a.jpg".into(),
                    kind: ManifestEntryKindV2::RegularFile,
                    plaintext_size: 3,
                    content_digest: EntryContentDigestV2::Known(ContentDigestV2([0x11; 32])),
                },
                ManifestEntryV2 {
                    entry_id: 3,
                    root_id: 1,
                    parent_entry_id: None,
                    component: "notes.txt".into(),
                    kind: ManifestEntryKindV2::RegularFile,
                    plaintext_size: 5,
                    content_digest: EntryContentDigestV2::Deferred,
                },
            ],
            totals: ManifestTotalsV2 {
                file_count: 2,
                directory_count: 2,
                total_plaintext_bytes: 8,
            },
        }
    }

    fn golden_offer_bytes() -> Vec<u8> {
        let compact: String = include_str!("../tests/fixtures/manifest_v2_offer.golden.hex")
            .lines()
            .map(|line| line.split('#').next().unwrap_or_default())
            .flat_map(str::chars)
            .filter(|character| !character.is_whitespace())
            .collect();
        assert_eq!(compact.len() % 2, 0, "golden hex must contain byte pairs");
        (0..compact.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&compact[offset..offset + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn offer_codec_matches_frozen_golden_vector() {
        let manifest = sample_manifest();
        let encoded = encode_manifest_offer_v2(&manifest).unwrap();
        let actual_hex: String = encoded.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            encoded,
            golden_offer_bytes(),
            "actual canonical hex: {actual_hex}"
        );
        assert_eq!(
            decode_manifest_offer_v2(&encoded).unwrap().manifest,
            manifest
        );
    }

    #[test]
    fn frame_type_ids_are_frozen() {
        let actual = [
            ManifestV2FrameType::Offer as u16,
            ManifestV2FrameType::Accept as u16,
            ManifestV2FrameType::AcceptCommittedAck as u16,
            ManifestV2FrameType::EntryStart as u16,
            ManifestV2FrameType::EntryContentDigest as u16,
            ManifestV2FrameType::EntryBlock as u16,
            ManifestV2FrameType::EntryComplete as u16,
            ManifestV2FrameType::EntryResult as u16,
            ManifestV2FrameType::JobComplete as u16,
            ManifestV2FrameType::DeliveryProof as u16,
            ManifestV2FrameType::DeliveryProofAck as u16,
            ManifestV2FrameType::ResumeRequest as u16,
            ManifestV2FrameType::ResumeStatus as u16,
            ManifestV2FrameType::ProofChallenge as u16,
            ManifestV2FrameType::ProofResponse as u16,
            ManifestV2FrameType::Cancel as u16,
            ManifestV2FrameType::Error as u16,
        ];
        assert_eq!(
            actual,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17]
        );
    }

    #[test]
    fn offer_codec_rejects_envelope_and_digest_tampering() {
        let encoded = encode_manifest_offer_v2(&sample_manifest()).unwrap();

        let mut bad_magic = encoded.clone();
        bad_magic[0] ^= 1;
        assert_eq!(
            decode_manifest_offer_v2(&bad_magic),
            Err(ManifestV2CodecError::BadMagic)
        );

        let mut bad_version = encoded.clone();
        bad_version[5] = 3;
        assert_eq!(
            decode_manifest_offer_v2(&bad_version),
            Err(ManifestV2CodecError::UnsupportedVersion(3))
        );

        let mut bad_frame_type = encoded.clone();
        bad_frame_type[7] = 2;
        assert_eq!(
            decode_manifest_offer_v2(&bad_frame_type),
            Err(ManifestV2CodecError::UnexpectedFrameType(2))
        );

        let mut bad_digest = encoded.clone();
        bad_digest[OFFER_HEADER_BYTES] ^= 1;
        assert_eq!(
            decode_manifest_offer_v2(&bad_digest),
            Err(ManifestV2CodecError::StructuralDigestMismatch)
        );

        assert_eq!(
            decode_manifest_offer_v2(&encoded[..encoded.len() - 1]),
            Err(ManifestV2CodecError::LengthMismatch)
        );
    }

    #[test]
    fn offer_codec_rejects_unknown_noncanonical_and_trailing_body_data() {
        fn resign_body(encoded: &mut [u8]) {
            let body_offset = OFFER_HEADER_BYTES + DIGEST_BYTES;
            let digest = blake3::hash(&encoded[body_offset..]);
            encoded[OFFER_HEADER_BYTES..body_offset].copy_from_slice(digest.as_bytes());
        }

        let encoded = encode_manifest_offer_v2(&sample_manifest()).unwrap();
        let body_offset = OFFER_HEADER_BYTES + DIGEST_BYTES;
        let compression_policy_offset = body_offset + JOB_ID_BYTES + 4 + 8;
        let mut unknown_policy = encoded.clone();
        unknown_policy[compression_policy_offset] = u8::MAX;
        resign_body(&mut unknown_policy);
        assert_eq!(
            decode_manifest_offer_v2(&unknown_policy),
            Err(ManifestV2CodecError::UnknownCompressionPolicy(u8::MAX))
        );

        let first_root_name_offset = compression_policy_offset + 1 + 4 + 4 + 4 + 4;
        let mut invalid_utf8 = encoded.clone();
        invalid_utf8[first_root_name_offset] = u8::MAX;
        resign_body(&mut invalid_utf8);
        assert_eq!(
            decode_manifest_offer_v2(&invalid_utf8),
            Err(ManifestV2CodecError::InvalidUtf8)
        );

        let mut trailing = encoded.clone();
        trailing.push(0);
        let payload_length = u32::try_from(trailing.len() - OFFER_HEADER_BYTES).unwrap();
        trailing[8..12].copy_from_slice(&payload_length.to_be_bytes());
        resign_body(&mut trailing);
        assert_eq!(
            decode_manifest_offer_v2(&trailing),
            Err(ManifestV2CodecError::TrailingBytes)
        );
    }

    #[test]
    fn forest_validation_rejects_unsafe_and_noncanonical_entries() {
        let mut unsafe_component = sample_manifest();
        unsafe_component.entries[2].component = "../a.jpg".into();
        assert!(matches!(
            unsafe_component.validate(),
            Err(ManifestV2ValidationError::UnsafeComponent { .. })
        ));

        let mut duplicate = sample_manifest();
        duplicate.entries[1].component = "a.jpg".into();
        assert_eq!(
            duplicate.validate(),
            Err(ManifestV2ValidationError::DuplicateSiblingComponent { entry_id: 2 })
        );

        let mut forward_parent = sample_manifest();
        forward_parent.entries[1].parent_entry_id = Some(2);
        assert_eq!(
            forward_parent.validate(),
            Err(ManifestV2ValidationError::NonCanonicalParent {
                entry_id: 1,
                parent_entry_id: 2,
            })
        );

        let mut noncanonical_order = sample_manifest();
        noncanonical_order.entries.swap(1, 2);
        noncanonical_order.entries[1].entry_id = 1;
        noncanonical_order.entries[2].entry_id = 2;
        assert_eq!(
            noncanonical_order.validate(),
            Err(ManifestV2ValidationError::NonCanonicalEntryOrder {
                expected: 1,
                actual: 2,
            })
        );
    }

    #[test]
    fn totals_and_directory_metadata_are_checked() {
        let mut zero_generation = sample_manifest();
        zero_generation.generation = 0;
        assert_eq!(
            zero_generation.validate(),
            Err(ManifestV2ValidationError::ZeroGeneration)
        );

        let mut wrong_totals = sample_manifest();
        wrong_totals.totals.total_plaintext_bytes = 9;
        assert!(matches!(
            wrong_totals.validate(),
            Err(ManifestV2ValidationError::TotalsMismatch { .. })
        ));

        let mut directory_with_size = sample_manifest();
        directory_with_size.entries[0].plaintext_size = 1;
        assert_eq!(
            directory_with_size.validate(),
            Err(ManifestV2ValidationError::InvalidDirectoryMetadata { entry_id: 0 })
        );

        let mut overflow = sample_manifest();
        overflow.entries[2].plaintext_size = u64::MAX;
        overflow.entries[3].plaintext_size = 1;
        assert_eq!(
            overflow.validate(),
            Err(ManifestV2ValidationError::AggregateOverflow)
        );
    }

    #[test]
    fn decoder_rejects_counts_before_allocating_entries() {
        let encoded = encode_manifest_offer_v2(&sample_manifest()).unwrap();
        let body_offset = OFFER_HEADER_BYTES + DIGEST_BYTES;
        let root_count_offset = body_offset + JOB_ID_BYTES + 4 + 8 + 1;
        let mut oversized = encoded.clone();
        oversized[root_count_offset..root_count_offset + 4]
            .copy_from_slice(&u32::MAX.to_be_bytes());
        let body = &oversized[body_offset..];
        let digest = blake3::hash(body);
        oversized[OFFER_HEADER_BYTES..body_offset].copy_from_slice(digest.as_bytes());
        assert_eq!(
            decode_manifest_offer_v2(&oversized),
            Err(ManifestV2CodecError::RootCountTooLarge(u32::MAX as usize))
        );
    }

    #[test]
    fn maximum_entry_count_with_maximum_components_fits_offer_budget() {
        let mut entries = Vec::with_capacity(MAX_MANIFEST_V2_ENTRIES);
        entries.push(ManifestEntryV2 {
            entry_id: 0,
            root_id: 0,
            parent_entry_id: None,
            component: "root".into(),
            kind: ManifestEntryKindV2::Directory,
            plaintext_size: 0,
            content_digest: EntryContentDigestV2::Deferred,
        });
        for index in 1..MAX_MANIFEST_V2_ENTRIES {
            let prefix = format!("f{index:05}");
            let component = format!(
                "{prefix}{}",
                "a".repeat(MAX_MANIFEST_V2_COMPONENT_BYTES - prefix.len())
            );
            entries.push(ManifestEntryV2 {
                entry_id: index as u32,
                root_id: 0,
                parent_entry_id: Some(0),
                component,
                kind: ManifestEntryKindV2::RegularFile,
                plaintext_size: 1,
                content_digest: EntryContentDigestV2::Known(ContentDigestV2([0x22; 32])),
            });
        }
        let manifest = ManifestV2 {
            job_id: JobIdV2([0x33; JOB_ID_BYTES]),
            generation: 1,
            selection_revision: 1,
            compression_policy: CompressionPolicyV2::Never,
            roots: vec![ManifestRootV2 {
                root_id: 0,
                root_entry_id: 0,
                requested_name: "root".into(),
                completeness: SourceCompletenessV2::Complete,
            }],
            entries,
            totals: ManifestTotalsV2 {
                file_count: (MAX_MANIFEST_V2_ENTRIES - 1) as u32,
                directory_count: 1,
                total_plaintext_bytes: (MAX_MANIFEST_V2_ENTRIES - 1) as u64,
            },
        };

        let encoded = encode_manifest_offer_v2(&manifest).unwrap();
        const EXPECTED_MAX_SHAPE_OFFER_BYTES: usize = 3_129_823;
        assert_eq!(encoded.len(), EXPECTED_MAX_SHAPE_OFFER_BYTES);
        assert!(encoded.len() <= MAX_MANIFEST_V2_ENCODED_BYTES);
        assert_eq!(
            decode_manifest_offer_v2(&encoded).unwrap().manifest,
            manifest
        );
    }
}
