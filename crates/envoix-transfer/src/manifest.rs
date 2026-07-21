//! Sequential Manifest v1 transfer engine.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::time::Duration;

use envoix_error::CoreError;
use envoix_protocol::{
    MANIFEST_V1_PROTOCOL_VERSION, MAX_MANIFEST_V1_ENTRIES, ManifestAcceptV1, ManifestChunkV1,
    ManifestCompleteAckV1, ManifestCompleteV1, ManifestEntryCompleteAckV1, ManifestEntryCompleteV1,
    ManifestEntryDispositionKind, ManifestEntryDispositionV1, ManifestEntryKind,
    ManifestEntryResultStatus, ManifestEntryResultV1, ManifestEntryStartV1, ManifestEntryV1,
    ManifestErrorV1, ManifestFrame, ManifestFrameConnection, ManifestHashAlgorithm,
    ManifestHelloV1, ManifestId, ManifestOfferV1, ManifestResumeStatusV1, ManifestV1,
    validate_manifest_relative_path,
};
use envoix_storage::{LocalFileStorage, ResumeLease, TransferReceipt, TransferResumeState};
use envoix_types::{PeerRole, TransferDirection, TransferId};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::{
    DEFAULT_CHUNK_SIZE, PEER_INTERRUPT_MESSAGE, PEER_PAUSE_MESSAGE, TransferCancelToken,
    USER_INTERRUPT_MESSAGE, USER_PAUSE_MESSAGE, validate_chunk_size,
};

const STATE_ROOT_NAME: &str = ".envoix-manifest-state";
const RECEIVE_PLAN_NAME: &str = "receive-plan.json";
const RESUME_STATE_WRITE_INTERVAL: u64 = 8 * 1024 * 1024;
const MAX_DIRECTORY_PLAN_ATTEMPTS: usize = 100;
#[cfg(not(test))]
const COMPLETE_ACK_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(test)]
const COMPLETE_ACK_TIMEOUT: Duration = Duration::from_millis(500);

const ERROR_CANCELLED: &str = "manifest.cancelled";
const ERROR_PAUSED: &str = "manifest.paused";
const ERROR_SOURCE_CHANGED: &str = "manifest.source_changed";
const ERROR_RECEIVE_FAILED: &str = "manifest.receive_failed";

/// Error type returned by the Manifest transfer engine.
pub type ManifestTransferError = CoreError;

/// Validated mapping from Manifest file entries to local source paths.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ManifestSendRequest {
    /// Offered transfer-set description.
    pub manifest: ManifestV1,
    #[serde(with = "source_paths_serde")]
    source_paths: BTreeMap<u32, PathBuf>,
}

mod source_paths_serde {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use serde::ser::SerializeMap;

    pub fn serialize<S>(paths: &BTreeMap<u32, PathBuf>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(Some(paths.len()))?;
        for (entry_id, path) in paths {
            map.serialize_entry(&entry_id.to_string(), path)?;
        }
        map.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<BTreeMap<u32, PathBuf>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encoded = <BTreeMap<String, PathBuf> as serde::Deserialize>::deserialize(deserializer)?;
        encoded
            .into_iter()
            .map(|(entry_id, path)| {
                entry_id
                    .parse::<u32>()
                    .map(|entry_id| (entry_id, path))
                    .map_err(serde::de::Error::custom)
            })
            .collect()
    }
}

impl ManifestSendRequest {
    /// Creates a request with exactly one source path for every regular-file
    /// entry and no path for directory entries.
    pub fn new(
        manifest: ManifestV1,
        source_paths: impl IntoIterator<Item = (u32, PathBuf)>,
    ) -> Result<Self, ManifestTransferError> {
        manifest
            .validate_structure()
            .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
        let source_paths = source_paths.into_iter().collect::<BTreeMap<_, _>>();
        let file_ids = manifest
            .entries
            .iter()
            .filter(|entry| entry.kind == ManifestEntryKind::RegularFile)
            .map(|entry| entry.entry_id)
            .collect::<HashSet<_>>();
        let supplied_ids = source_paths.keys().copied().collect::<HashSet<_>>();
        if supplied_ids != file_ids {
            let mut missing = file_ids
                .difference(&supplied_ids)
                .copied()
                .collect::<Vec<_>>();
            let mut unexpected = supplied_ids
                .difference(&file_ids)
                .copied()
                .collect::<Vec<_>>();
            missing.sort_unstable();
            unexpected.sort_unstable();
            return Err(CoreError::InvalidInput(format!(
                "manifest source mapping mismatch: missing={missing:?}, unexpected={unexpected:?}"
            )));
        }
        Ok(Self {
            manifest,
            source_paths,
        })
    }

    /// Builds a validated Manifest from user-selected file and directory roots.
    ///
    /// Entries are deterministic (root selection order, then lexical children),
    /// directories precede their descendants, and symbolic links or special
    /// files are rejected rather than followed.
    pub async fn from_paths(
        manifest_id: ManifestId,
        roots: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, ManifestTransferError> {
        let roots = roots.into_iter().collect::<Vec<_>>();
        if roots.is_empty() {
            return Err(CoreError::InvalidInput(
                "a Manifest send needs at least one selected path".into(),
            ));
        }
        let mut root_names = HashSet::with_capacity(roots.len());
        let mut pending = Vec::with_capacity(roots.len());
        for root in roots.into_iter().rev() {
            let name = portable_file_name(&root)?;
            if !root_names.insert(name.clone()) {
                return Err(CoreError::InvalidInput(format!(
                    "selected Manifest roots have the same name: {name}"
                )));
            }
            pending.push((root, name));
        }

        let cancel = TransferCancelToken::new();
        let mut entries = Vec::new();
        let mut source_paths = BTreeMap::new();
        while let Some((source, relative_path)) = pending.pop() {
            if entries.len() >= MAX_MANIFEST_V1_ENTRIES {
                return Err(CoreError::InvalidInput(format!(
                    "manifest entry count exceeds {MAX_MANIFEST_V1_ENTRIES}"
                )));
            }
            validate_manifest_relative_path(&relative_path)
                .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
            let metadata = fs::symlink_metadata(&source).await?;
            if metadata.file_type().is_symlink() {
                return Err(CoreError::InvalidInput(format!(
                    "symbolic links are not supported in a Manifest: {}",
                    source.display()
                )));
            }
            let entry_id = entries.len() as u32;
            let modified_at_unix_ms = modified_at_unix_ms(&metadata);
            if metadata.is_dir() {
                entries.push(ManifestEntryV1 {
                    entry_id,
                    relative_path: relative_path.clone(),
                    kind: ManifestEntryKind::Directory,
                    size: 0,
                    hash: None,
                    modified_at_unix_ms,
                });
                let mut children = Vec::new();
                let mut directory = fs::read_dir(&source).await?;
                while let Some(child) = directory.next_entry().await? {
                    let name = child.file_name().into_string().map_err(|_| {
                        CoreError::InvalidInput(format!(
                            "Manifest path is not valid UTF-8: {}",
                            child.path().display()
                        ))
                    })?;
                    let child_relative = format!("{relative_path}/{name}");
                    validate_manifest_relative_path(&child_relative)
                        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
                    children.push((child.path(), child_relative));
                }
                children.sort_by(|left, right| left.1.cmp(&right.1));
                pending.extend(children.into_iter().rev());
            } else if metadata.is_file() {
                let (size, hash) = hash_path(&source, DEFAULT_CHUNK_SIZE, &cancel).await?;
                entries.push(ManifestEntryV1 {
                    entry_id,
                    relative_path,
                    kind: ManifestEntryKind::RegularFile,
                    size,
                    hash: Some(hash),
                    modified_at_unix_ms,
                });
                source_paths.insert(entry_id, source);
            } else {
                return Err(CoreError::InvalidInput(format!(
                    "Manifest source is not a regular file or directory: {}",
                    source.display()
                )));
            }
        }

        let file_count = source_paths.len() as u32;
        let directory_count = entries.len() as u32 - file_count;
        let total_bytes = entries
            .iter()
            .try_fold(0_u64, |total, entry| total.checked_add(entry.size))
            .ok_or_else(|| CoreError::InvalidInput("Manifest total size overflow".into()))?;
        Self::new(
            ManifestV1 {
                manifest_id,
                entries,
                file_count,
                directory_count,
                root_count: root_names.len() as u32,
                total_bytes,
                hash_algorithm: ManifestHashAlgorithm::Blake3_256,
            },
            source_paths,
        )
    }

    /// Revalidates a deserialized durable request without changing it.
    pub fn validate(&self) -> Result<(), ManifestTransferError> {
        Self::new(self.manifest.clone(), self.source_paths.clone()).map(|_| ())
    }

    /// Local source for one regular-file entry.
    pub fn source_path(&self, entry_id: u32) -> Result<&Path, ManifestTransferError> {
        self.source_paths
            .get(&entry_id)
            .map(PathBuf::as_path)
            .ok_or_else(|| {
                CoreError::InvalidInput(format!("no source path for manifest entry {entry_id}"))
            })
    }
}

fn portable_file_name(path: &Path) -> Result<String, ManifestTransferError> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            CoreError::InvalidInput(format!(
                "selected Manifest path has no portable file name: {}",
                path.display()
            ))
        })?
        .to_owned();
    validate_manifest_relative_path(&name)
        .map_err(|error| CoreError::InvalidInput(error.to_string()))?;
    Ok(name)
}

fn modified_at_unix_ms(metadata: &std::fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

/// Observer for aggregate and per-entry Manifest transfer lifecycle events.
pub trait ManifestEventSink: Send + Sync {
    /// Announces the complete validated plan once both peers have accepted it.
    ///
    /// The default keeps existing sink implementations source-compatible.
    fn on_manifest_plan(&self, _direction: TransferDirection, _manifest: &ManifestV1) {}

    /// Handles one Manifest transfer event.
    fn on_manifest_event(&self, event: ManifestTransferEvent);
}

/// Event sink that ignores Manifest transfer events.
#[derive(Clone, Copy, Debug, Default)]
pub struct ManifestNoopEventSink;

impl ManifestEventSink for ManifestNoopEventSink {
    fn on_manifest_event(&self, _event: ManifestTransferEvent) {}
}

/// User-visible Manifest transfer lifecycle event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifestTransferEvent {
    /// Sender-side preflight is hashing and validating one source file.
    PreparingEntry {
        manifest_id: ManifestId,
        entry_id: u32,
        relative_path: String,
        size: u64,
    },
    /// The complete transfer set has been accepted.
    Started {
        manifest_id: ManifestId,
        direction: TransferDirection,
        file_count: u32,
        directory_count: u32,
        total_bytes: u64,
    },
    /// One file entry has entered its sequential payload phase.
    EntryStarted {
        manifest_id: ManifestId,
        entry_id: u32,
        transfer_id: TransferId,
        relative_path: String,
        total_bytes: u64,
        bytes_resumed: u64,
    },
    /// Aggregate logical progress and the active entry's persisted bytes.
    Progress {
        manifest_id: ManifestId,
        entry_id: u32,
        entry_bytes: u64,
        entry_total_bytes: u64,
        completed_bytes: u64,
        total_bytes: u64,
    },
    /// One file or directory reached a terminal result.
    EntryCompleted {
        manifest_id: ManifestId,
        result: ManifestEntryResultV1,
    },
    /// Every offered entry has a successful terminal result.
    Completed { summary: ManifestTransferSummary },
}

/// Successful aggregate result returned by a Manifest transfer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestTransferSummary {
    pub manifest_id: ManifestId,
    pub file_count: u32,
    pub directory_count: u32,
    pub total_bytes: u64,
    pub entries: Vec<ManifestEntryResultV1>,
}

/// Sequential multi-file and directory transfer engine.
#[derive(Clone, Debug)]
pub struct ManifestTransferEngine {
    chunk_size: usize,
}

impl ManifestTransferEngine {
    /// Creates an engine using the same chunk-size constraints as the existing
    /// single-file engine.
    pub fn new(chunk_size: usize) -> Self {
        Self { chunk_size }
    }

    /// Sends one validated Manifest request.
    pub async fn send_manifest(
        &self,
        connection: &mut dyn ManifestFrameConnection,
        request: ManifestSendRequest,
        resume: bool,
        events: &dyn ManifestEventSink,
    ) -> Result<ManifestTransferSummary, ManifestTransferError> {
        let cancel = TransferCancelToken::new();
        self.send_manifest_with_cancel(connection, request, resume, events, &cancel)
            .await
    }

    /// Sends one Manifest request and notifies the peer if cancelled.
    pub async fn send_manifest_with_cancel(
        &self,
        connection: &mut dyn ManifestFrameConnection,
        request: ManifestSendRequest,
        resume: bool,
        events: &dyn ManifestEventSink,
        cancel: &TransferCancelToken,
    ) -> Result<ManifestTransferSummary, ManifestTransferError> {
        validate_chunk_size(self.chunk_size)?;
        check_cancelled(
            connection,
            cancel,
            Some(&request.manifest.manifest_id),
            None,
        )
        .await?;

        connection
            .send_manifest_frame(ManifestFrame::Hello(ManifestHelloV1 {
                protocol_version: MANIFEST_V1_PROTOCOL_VERSION,
                role: PeerRole::Sender,
            }))
            .await?;
        expect_manifest_hello(
            recv_manifest_or_cancel(
                connection,
                cancel,
                Some(&request.manifest.manifest_id),
                None,
            )
            .await?,
            PeerRole::Receiver,
        )?;
        preflight_sources(&request, events, cancel).await?;
        connection
            .send_manifest_frame(ManifestFrame::Offer(ManifestOfferV1 {
                manifest: request.manifest.clone(),
                chunk_size: self.chunk_size as u64,
                resume_requested: resume,
            }))
            .await?;
        let accept = expect_manifest_accept(
            recv_manifest_or_cancel(
                connection,
                cancel,
                Some(&request.manifest.manifest_id),
                None,
            )
            .await?,
            &request.manifest,
        )?;

        events.on_manifest_plan(TransferDirection::Send, &request.manifest);

        events.on_manifest_event(ManifestTransferEvent::Started {
            manifest_id: request.manifest.manifest_id.clone(),
            direction: TransferDirection::Send,
            file_count: request.manifest.file_count,
            directory_count: request.manifest.directory_count,
            total_bytes: request.manifest.total_bytes,
        });

        let dispositions = accept
            .entries
            .iter()
            .map(|entry| (entry.entry_id, entry))
            .collect::<HashMap<_, _>>();
        let mut completed_bytes = request
            .manifest
            .entries
            .iter()
            .filter(|entry| {
                dispositions
                    .get(&entry.entry_id)
                    .is_some_and(|disposition| {
                        disposition.disposition == ManifestEntryDispositionKind::SkipIdentical
                    })
            })
            .map(|entry| entry.size)
            .sum::<u64>();

        for entry in &request.manifest.entries {
            let disposition = dispositions
                .get(&entry.entry_id)
                .expect("accept validation covers every entry");
            if entry.kind == ManifestEntryKind::Directory
                || disposition.disposition == ManifestEntryDispositionKind::SkipIdentical
            {
                continue;
            }
            self.send_entry(
                connection,
                &request,
                entry,
                &mut completed_bytes,
                events,
                cancel,
            )
            .await?;
        }

        connection
            .send_manifest_frame(ManifestFrame::Complete(ManifestCompleteV1 {
                manifest_id: request.manifest.manifest_id.clone(),
            }))
            .await?;
        let final_frame = recv_manifest_or_cancel_with_timeout(
            connection,
            cancel,
            Some(&request.manifest.manifest_id),
            None,
            COMPLETE_ACK_TIMEOUT,
        )
        .await?;
        let ack = expect_manifest_complete_ack(final_frame, &request.manifest, &accept)?;
        let summary = ManifestTransferSummary {
            manifest_id: request.manifest.manifest_id.clone(),
            file_count: request.manifest.file_count,
            directory_count: request.manifest.directory_count,
            total_bytes: request.manifest.total_bytes,
            entries: ack.entries,
        };
        events.on_manifest_event(ManifestTransferEvent::Completed {
            summary: summary.clone(),
        });
        Ok(summary)
    }

    async fn send_entry(
        &self,
        connection: &mut dyn ManifestFrameConnection,
        request: &ManifestSendRequest,
        entry: &ManifestEntryV1,
        completed_bytes: &mut u64,
        events: &dyn ManifestEventSink,
        cancel: &TransferCancelToken,
    ) -> Result<(), ManifestTransferError> {
        let transfer_id = manifest_entry_transfer_id(&request.manifest.manifest_id, entry.entry_id);
        connection
            .send_manifest_frame(ManifestFrame::EntryStart(ManifestEntryStartV1 {
                manifest_id: request.manifest.manifest_id.clone(),
                entry_id: entry.entry_id,
                transfer_id: transfer_id.clone(),
            }))
            .await?;
        let resume_status = expect_manifest_resume_status(
            recv_manifest_or_cancel(
                connection,
                cancel,
                Some(&request.manifest.manifest_id),
                Some(entry.entry_id),
            )
            .await?,
            &request.manifest.manifest_id,
            entry,
            &transfer_id,
            self.chunk_size,
        )?;

        let source_path = request.source_path(entry.entry_id)?;
        let mut file = fs::File::open(source_path).await?;
        let mut hasher = blake3::Hasher::new();
        let mut start_offset = 0_u64;
        let mut start_index = 0_u64;
        if resume_status.bytes_received > 0 {
            hash_open_file_prefix(
                &mut file,
                &mut hasher,
                resume_status.bytes_received,
                self.chunk_size,
                cancel,
            )
            .await?;
            if hasher.finalize().as_bytes() == &resume_status.prefix_hash {
                start_offset = resume_status.bytes_received;
                start_index = resume_status.next_chunk_index;
            } else {
                hasher = blake3::Hasher::new();
            }
        }

        events.on_manifest_event(ManifestTransferEvent::EntryStarted {
            manifest_id: request.manifest.manifest_id.clone(),
            entry_id: entry.entry_id,
            transfer_id: transfer_id.clone(),
            relative_path: entry.relative_path.clone(),
            total_bytes: entry.size,
            bytes_resumed: start_offset,
        });
        file.seek(SeekFrom::Start(start_offset)).await?;
        let mut buffer = vec![0_u8; self.chunk_size];
        let mut index = start_index;
        let mut offset = start_offset;
        loop {
            check_cancelled(
                connection,
                cancel,
                Some(&request.manifest.manifest_id),
                Some(entry.entry_id),
            )
            .await?;
            let bytes_read = read_full_chunk(&mut file, &mut buffer).await?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
            if let Some(frame) = connection
                .send_manifest_chunk_or_recv_frame(
                    &request.manifest.manifest_id,
                    entry.entry_id,
                    &transfer_id,
                    index,
                    offset,
                    &buffer[..bytes_read],
                )
                .await?
            {
                return Err(unexpected_send_control(frame));
            }
            offset += bytes_read as u64;
            index += 1;
            events.on_manifest_event(ManifestTransferEvent::Progress {
                manifest_id: request.manifest.manifest_id.clone(),
                entry_id: entry.entry_id,
                entry_bytes: offset,
                entry_total_bytes: entry.size,
                completed_bytes: completed_bytes.saturating_add(offset),
                total_bytes: request.manifest.total_bytes,
            });
        }
        if offset != entry.size {
            let error = CoreError::Transfer(format!(
                "manifest source {} ended at {offset} bytes; offered {}",
                entry.relative_path, entry.size
            ));
            notify_manifest_error(
                connection,
                Some(&request.manifest.manifest_id),
                Some(entry.entry_id),
                ERROR_SOURCE_CHANGED,
                &error.to_string(),
            )
            .await;
            return Err(error);
        }
        let actual_hash = *hasher.finalize().as_bytes();
        if entry.hash != Some(actual_hash) {
            let error = CoreError::Transfer(format!(
                "manifest source {} changed after preflight",
                entry.relative_path
            ));
            notify_manifest_error(
                connection,
                Some(&request.manifest.manifest_id),
                Some(entry.entry_id),
                ERROR_SOURCE_CHANGED,
                &error.to_string(),
            )
            .await;
            return Err(error);
        }
        connection
            .send_manifest_frame(ManifestFrame::EntryComplete(ManifestEntryCompleteV1 {
                manifest_id: request.manifest.manifest_id.clone(),
                entry_id: entry.entry_id,
                transfer_id: transfer_id.clone(),
                file_hash: actual_hash,
            }))
            .await?;
        expect_manifest_entry_complete_ack(
            recv_manifest_or_cancel_with_timeout(
                connection,
                cancel,
                Some(&request.manifest.manifest_id),
                Some(entry.entry_id),
                COMPLETE_ACK_TIMEOUT,
            )
            .await?,
            &request.manifest.manifest_id,
            entry.entry_id,
            &transfer_id,
        )?;
        *completed_bytes = completed_bytes.saturating_add(entry.size);
        Ok(())
    }
}

impl ManifestTransferEngine {
    /// Receives one Manifest transfer into `output_dir`.
    pub async fn receive_manifest(
        &self,
        connection: &mut dyn ManifestFrameConnection,
        output_dir: PathBuf,
        events: &dyn ManifestEventSink,
    ) -> Result<ManifestTransferSummary, ManifestTransferError> {
        let cancel = TransferCancelToken::new();
        self.receive_manifest_with_cancel(connection, output_dir, events, &cancel)
            .await
    }

    /// Receives one Manifest transfer and preserves resumable staging if
    /// cancellation interrupts the active entry.
    pub async fn receive_manifest_with_cancel(
        &self,
        connection: &mut dyn ManifestFrameConnection,
        output_dir: PathBuf,
        events: &dyn ManifestEventSink,
        cancel: &TransferCancelToken,
    ) -> Result<ManifestTransferSummary, ManifestTransferError> {
        validate_chunk_size(self.chunk_size)?;
        expect_manifest_hello(
            recv_manifest_or_cancel(connection, cancel, None, None).await?,
            PeerRole::Sender,
        )?;
        connection
            .send_manifest_frame(ManifestFrame::Hello(ManifestHelloV1 {
                protocol_version: MANIFEST_V1_PROTOCOL_VERSION,
                role: PeerRole::Receiver,
            }))
            .await?;
        let offer = expect_manifest_offer(
            recv_manifest_or_cancel(connection, cancel, None, None).await?,
            self.chunk_size,
        )?;
        let manifest_id = offer.manifest.manifest_id.clone();
        let plan = match plan_and_materialize_directories(&output_dir, &offer.manifest).await {
            Ok(plan) => plan,
            Err(error) => {
                notify_manifest_error(
                    connection,
                    Some(&manifest_id),
                    None,
                    ERROR_RECEIVE_FAILED,
                    &error.to_string(),
                )
                .await;
                return Err(error);
            }
        };
        connection
            .send_manifest_frame(ManifestFrame::Accept(plan.accept.clone()))
            .await?;
        events.on_manifest_plan(TransferDirection::Receive, &offer.manifest);
        events.on_manifest_event(ManifestTransferEvent::Started {
            manifest_id: manifest_id.clone(),
            direction: TransferDirection::Receive,
            file_count: offer.manifest.file_count,
            directory_count: offer.manifest.directory_count,
            total_bytes: offer.manifest.total_bytes,
        });

        let dispositions = plan
            .accept
            .entries
            .iter()
            .map(|entry| (entry.entry_id, entry))
            .collect::<HashMap<_, _>>();
        let mut results = Vec::with_capacity(offer.manifest.entries.len());
        let mut completed_bytes = 0_u64;
        for entry in &offer.manifest.entries {
            let disposition = dispositions
                .get(&entry.entry_id)
                .expect("receive plan covers every entry");
            match disposition.disposition {
                ManifestEntryDispositionKind::CreateDirectory => {
                    let result =
                        successful_entry_result(entry, &disposition.final_relative_path, false);
                    events.on_manifest_event(ManifestTransferEvent::EntryCompleted {
                        manifest_id: manifest_id.clone(),
                        result: result.clone(),
                    });
                    results.push(result);
                }
                ManifestEntryDispositionKind::SkipIdentical => {
                    completed_bytes = completed_bytes.saturating_add(entry.size);
                    let result = ManifestEntryResultV1 {
                        entry_id: entry.entry_id,
                        status: ManifestEntryResultStatus::SkippedIdentical,
                        offered_relative_path: entry.relative_path.clone(),
                        final_relative_path: Some(disposition.final_relative_path.clone()),
                        failure_code: None,
                    };
                    events.on_manifest_event(ManifestTransferEvent::EntryCompleted {
                        manifest_id: manifest_id.clone(),
                        result: result.clone(),
                    });
                    results.push(result);
                }
                ManifestEntryDispositionKind::Transfer => {
                    let result = match self
                        .receive_entry(
                            connection,
                            &offer,
                            entry,
                            &disposition.final_relative_path,
                            &output_dir,
                            completed_bytes,
                            events,
                            cancel,
                        )
                        .await
                    {
                        Ok(result) => result,
                        Err(error) => {
                            notify_manifest_error(
                                connection,
                                Some(&manifest_id),
                                Some(entry.entry_id),
                                ERROR_RECEIVE_FAILED,
                                &error.to_string(),
                            )
                            .await;
                            return Err(error);
                        }
                    };
                    completed_bytes = completed_bytes.saturating_add(entry.size);
                    events.on_manifest_event(ManifestTransferEvent::EntryCompleted {
                        manifest_id: manifest_id.clone(),
                        result: result.clone(),
                    });
                    results.push(result);
                }
            }
        }

        expect_manifest_complete(
            recv_manifest_or_cancel(connection, cancel, Some(&manifest_id), None).await?,
            &manifest_id,
        )?;
        connection
            .send_manifest_frame(ManifestFrame::CompleteAck(ManifestCompleteAckV1 {
                manifest_id: manifest_id.clone(),
                entries: results.clone(),
            }))
            .await?;
        let summary = ManifestTransferSummary {
            manifest_id,
            file_count: offer.manifest.file_count,
            directory_count: offer.manifest.directory_count,
            total_bytes: offer.manifest.total_bytes,
            entries: results,
        };
        events.on_manifest_event(ManifestTransferEvent::Completed {
            summary: summary.clone(),
        });
        Ok(summary)
    }

    #[allow(clippy::too_many_arguments)]
    async fn receive_entry(
        &self,
        connection: &mut dyn ManifestFrameConnection,
        offer: &ManifestOfferV1,
        entry: &ManifestEntryV1,
        planned_relative_path: &str,
        output_dir: &Path,
        completed_before_entry: u64,
        events: &dyn ManifestEventSink,
        cancel: &TransferCancelToken,
    ) -> Result<ManifestEntryResultV1, ManifestTransferError> {
        let transfer_id = manifest_entry_transfer_id(&offer.manifest.manifest_id, entry.entry_id);
        expect_manifest_entry_start(
            recv_manifest_or_cancel(
                connection,
                cancel,
                Some(&offer.manifest.manifest_id),
                Some(entry.entry_id),
            )
            .await?,
            &offer.manifest.manifest_id,
            entry.entry_id,
            &transfer_id,
        )?;
        let state_dir = manifest_state_directory(output_dir, &offer.manifest.manifest_id).await?;
        let storage_name = manifest_entry_storage_name(entry.entry_id);
        let mut prepared = prepare_manifest_receive_state(
            &state_dir,
            &storage_name,
            &transfer_id,
            entry,
            offer.chunk_size,
            offer.resume_requested,
            self.chunk_size,
            cancel,
        )
        .await?;
        connection
            .send_manifest_frame(ManifestFrame::ResumeStatus(ManifestResumeStatusV1 {
                manifest_id: offer.manifest.manifest_id.clone(),
                entry_id: entry.entry_id,
                transfer_id: transfer_id.clone(),
                next_chunk_index: prepared.state.next_chunk_index,
                bytes_received: prepared.state.bytes_received,
                prefix_hash: prepared.prefix_hash,
            }))
            .await?;
        events.on_manifest_event(ManifestTransferEvent::EntryStarted {
            manifest_id: offer.manifest.manifest_id.clone(),
            entry_id: entry.entry_id,
            transfer_id: transfer_id.clone(),
            relative_path: entry.relative_path.clone(),
            total_bytes: entry.size,
            bytes_resumed: prepared.state.bytes_received,
        });
        events.on_manifest_event(ManifestTransferEvent::Progress {
            manifest_id: offer.manifest.manifest_id.clone(),
            entry_id: entry.entry_id,
            entry_bytes: prepared.state.bytes_received,
            entry_total_bytes: entry.size,
            completed_bytes: completed_before_entry.saturating_add(prepared.state.bytes_received),
            total_bytes: offer.manifest.total_bytes,
        });

        let mut expected_index = prepared.state.next_chunk_index;
        let mut expected_offset = prepared.state.bytes_received;
        let mut last_state_bytes = expected_offset;
        loop {
            let frame = match recv_manifest_or_cancel(
                connection,
                cancel,
                Some(&offer.manifest.manifest_id),
                Some(entry.entry_id),
            )
            .await
            {
                Ok(frame) => frame,
                Err(error) => {
                    persist_manifest_resume_state(
                        &state_dir,
                        &storage_name,
                        &transfer_id,
                        entry,
                        offer.chunk_size,
                        expected_offset,
                        expected_index,
                        &prepared.hasher,
                    )
                    .await?;
                    return Err(error);
                }
            };
            match frame {
                ManifestFrame::Chunk(chunk) => {
                    if expected_offset > 0 && chunk.index == 0 && chunk.offset == 0 {
                        prepared.file.set_len(0).await?;
                        prepared.file.flush().await?;
                        prepared.hasher = blake3::Hasher::new();
                        expected_index = 0;
                        expected_offset = 0;
                        last_state_bytes = 0;
                    }
                    validate_manifest_chunk(
                        &chunk,
                        &offer.manifest.manifest_id,
                        entry,
                        &transfer_id,
                        expected_index,
                        expected_offset,
                    )?;
                    let next_offset = expected_offset
                        .checked_add(chunk.bytes.len() as u64)
                        .ok_or_else(|| {
                            CoreError::Transfer("manifest chunk offset overflow".into())
                        })?;
                    if next_offset > entry.size {
                        return Err(CoreError::Transfer(format!(
                            "manifest chunk exceeds offered size for {}",
                            entry.relative_path
                        )));
                    }
                    prepared.file.write_all(&chunk.bytes).await?;
                    prepared.hasher.update(&chunk.bytes);
                    expected_offset = next_offset;
                    expected_index += 1;
                    if expected_offset.saturating_sub(last_state_bytes)
                        >= RESUME_STATE_WRITE_INTERVAL
                    {
                        prepared.file.flush().await?;
                        persist_manifest_resume_state(
                            &state_dir,
                            &storage_name,
                            &transfer_id,
                            entry,
                            offer.chunk_size,
                            expected_offset,
                            expected_index,
                            &prepared.hasher,
                        )
                        .await?;
                        last_state_bytes = expected_offset;
                    }
                    events.on_manifest_event(ManifestTransferEvent::Progress {
                        manifest_id: offer.manifest.manifest_id.clone(),
                        entry_id: entry.entry_id,
                        entry_bytes: expected_offset,
                        entry_total_bytes: entry.size,
                        completed_bytes: completed_before_entry.saturating_add(expected_offset),
                        total_bytes: offer.manifest.total_bytes,
                    });
                }
                ManifestFrame::EntryComplete(complete) => {
                    validate_manifest_entry_complete(
                        &complete,
                        &offer.manifest.manifest_id,
                        entry,
                        &transfer_id,
                    )?;
                    let final_relative_path = finalize_manifest_entry(
                        output_dir,
                        &state_dir,
                        &storage_name,
                        planned_relative_path,
                        entry,
                        &transfer_id,
                        prepared,
                        expected_offset,
                        &complete,
                    )
                    .await?;
                    connection
                        .send_manifest_frame(ManifestFrame::EntryCompleteAck(
                            ManifestEntryCompleteAckV1 {
                                manifest_id: offer.manifest.manifest_id.clone(),
                                entry_id: entry.entry_id,
                                transfer_id,
                            },
                        ))
                        .await?;
                    return Ok(successful_entry_result(entry, &final_relative_path, false));
                }
                ManifestFrame::Error(error) => return Err(peer_manifest_error(error)),
                frame => {
                    return Err(CoreError::Transfer(format!(
                        "unexpected Manifest frame while receiving entry {}: {frame:?}",
                        entry.entry_id
                    )));
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct ReceivePlan {
    accept: ManifestAcceptV1,
}

struct PreparedManifestReceive {
    state: TransferResumeState,
    file: fs::File,
    hasher: blake3::Hasher,
    prefix_hash: [u8; 32],
    _lease: ResumeLease,
}

async fn preflight_sources(
    request: &ManifestSendRequest,
    events: &dyn ManifestEventSink,
    cancel: &TransferCancelToken,
) -> Result<(), ManifestTransferError> {
    for entry in request
        .manifest
        .entries
        .iter()
        .filter(|entry| entry.kind == ManifestEntryKind::RegularFile)
    {
        if cancel.is_cancelled() {
            return Err(interrupted_error(cancel));
        }
        events.on_manifest_event(ManifestTransferEvent::PreparingEntry {
            manifest_id: request.manifest.manifest_id.clone(),
            entry_id: entry.entry_id,
            relative_path: entry.relative_path.clone(),
            size: entry.size,
        });
        let path = request.source_path(entry.entry_id)?;
        let metadata = fs::symlink_metadata(path).await?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CoreError::InvalidInput(format!(
                "manifest source is not a regular file: {}",
                path.display()
            )));
        }
        if metadata.len() != entry.size {
            return Err(CoreError::InvalidInput(format!(
                "manifest source size changed for {}: expected {}, got {}",
                entry.relative_path,
                entry.size,
                metadata.len()
            )));
        }
        if entry.modified_at_unix_ms.is_some()
            && modified_at_unix_ms(&metadata) != entry.modified_at_unix_ms
        {
            return Err(CoreError::InvalidInput(format!(
                "manifest source modification time changed for {}",
                entry.relative_path
            )));
        }
    }
    Ok(())
}

async fn plan_and_materialize_directories(
    output_dir: &Path,
    manifest: &ManifestV1,
) -> Result<ReceivePlan, ManifestTransferError> {
    fs::create_dir_all(output_dir).await?;
    let root_metadata = fs::metadata(output_dir).await?;
    if !root_metadata.is_dir() {
        return Err(CoreError::Storage(format!(
            "manifest output root is not a directory: {}",
            output_dir.display()
        )));
    }

    let state_dir = manifest_state_directory(output_dir, &manifest.manifest_id).await?;
    if let Some(mut plan) = read_receive_plan(&state_dir).await? {
        plan.accept = refresh_persisted_plan(output_dir, manifest, plan.accept).await?;
        materialize_planned_directories(output_dir, manifest, &plan)
            .await
            .map_err(|error| match error {
                PlanMaterializeError::Collision => CoreError::Storage(
                    "persisted Manifest directory mapping now collides with an unsafe object"
                        .into(),
                ),
                PlanMaterializeError::Fatal(error) => error,
            })?;
        write_receive_plan(&state_dir, &plan).await?;
        return Ok(plan);
    }

    for _ in 0..MAX_DIRECTORY_PLAN_ATTEMPTS {
        let plan = plan_receive_conflicts(output_dir, manifest).await?;
        match materialize_planned_directories(output_dir, manifest, &plan).await {
            Ok(()) => {
                write_receive_plan(&state_dir, &plan).await?;
                return Ok(plan);
            }
            Err(PlanMaterializeError::Collision) => continue,
            Err(PlanMaterializeError::Fatal(error)) => return Err(error),
        }
    }
    Err(CoreError::Storage(format!(
        "could not claim manifest directory names after {MAX_DIRECTORY_PLAN_ATTEMPTS} attempts"
    )))
}

async fn read_receive_plan(state_dir: &Path) -> Result<Option<ReceivePlan>, ManifestTransferError> {
    let path = state_dir.join(RECEIVE_PLAN_NAME);
    let bytes = match fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let accept = serde_json::from_slice(&bytes).map_err(|error| {
        CoreError::Storage(format!(
            "invalid persisted Manifest receive plan {}: {error}",
            path.display()
        ))
    })?;
    Ok(Some(ReceivePlan { accept }))
}

async fn write_receive_plan(
    state_dir: &Path,
    plan: &ReceivePlan,
) -> Result<(), ManifestTransferError> {
    let path = state_dir.join(RECEIVE_PLAN_NAME);
    let temp_path = state_dir.join(format!("{RECEIVE_PLAN_NAME}.tmp"));
    let bytes = serde_json::to_vec_pretty(&plan.accept)
        .map_err(|error| CoreError::Storage(error.to_string()))?;
    let mut file = fs::File::create(&temp_path).await?;
    file.write_all(&bytes).await?;
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    fs::rename(temp_path, path).await?;
    Ok(())
}

async fn refresh_persisted_plan(
    output_dir: &Path,
    manifest: &ManifestV1,
    mut accept: ManifestAcceptV1,
) -> Result<ManifestAcceptV1, ManifestTransferError> {
    expect_manifest_accept(ManifestFrame::Accept(accept.clone()), manifest)?;
    let entries = manifest
        .entries
        .iter()
        .map(|entry| (entry.entry_id, entry))
        .collect::<HashMap<_, _>>();
    let mut reserved_paths = accept
        .entries
        .iter()
        .map(|entry| entry.final_relative_path.clone())
        .collect::<HashSet<_>>();
    for disposition in &mut accept.entries {
        let entry = entries
            .get(&disposition.entry_id)
            .expect("persisted plan validation covers every entry");
        if entry.kind == ManifestEntryKind::Directory {
            continue;
        }
        let path = output_dir.join(Path::new(&disposition.final_relative_path));
        if path_exists_without_following(&path).await?
            && existing_file_matches(
                &path,
                entry.size,
                entry.hash.expect("validated file entries have hashes"),
            )
            .await?
        {
            disposition.disposition = ManifestEntryDispositionKind::SkipIdentical;
            continue;
        }
        disposition.disposition = ManifestEntryDispositionKind::Transfer;
        if path_exists_without_following(&path).await? {
            reserved_paths.remove(&disposition.final_relative_path);
            disposition.final_relative_path = unique_relative_file_path(
                output_dir,
                &disposition.final_relative_path,
                &entry.relative_path,
                &reserved_paths,
            )
            .await?;
            reserved_paths.insert(disposition.final_relative_path.clone());
        }
    }
    expect_manifest_accept(ManifestFrame::Accept(accept.clone()), manifest)?;
    Ok(accept)
}

async fn unique_relative_file_path(
    output_dir: &Path,
    planned_relative_path: &str,
    offered_relative_path: &str,
    reserved_paths: &HashSet<String>,
) -> Result<String, ManifestTransferError> {
    for index in 1_u64.. {
        let candidate =
            collision_relative_path(planned_relative_path, offered_relative_path, index)?;
        if reserved_paths.contains(&candidate) {
            continue;
        }
        if !path_exists_without_following(&output_dir.join(Path::new(&candidate))).await? {
            return Ok(candidate);
        }
    }
    unreachable!("u64 name suffix space is effectively unbounded")
}

async fn plan_receive_conflicts(
    output_dir: &Path,
    manifest: &ManifestV1,
) -> Result<ReceivePlan, ManifestTransferError> {
    let root_entries = manifest
        .entries
        .iter()
        .filter(|entry| !entry.relative_path.contains('/'))
        .collect::<Vec<_>>();
    let offered_root_names = root_entries
        .iter()
        .map(|entry| entry.relative_path.clone())
        .collect::<HashSet<_>>();
    let mut used_root_names = HashSet::new();
    let mut mapped_roots = HashMap::new();
    let mut root_actions = HashMap::new();

    for entry in root_entries {
        let existing_path = output_dir.join(&entry.relative_path);
        let exists = path_exists_without_following(&existing_path).await?;
        let (mapped_name, action) = match entry.kind {
            ManifestEntryKind::Directory => {
                let name = if !exists && !used_root_names.contains(&entry.relative_path) {
                    entry.relative_path.clone()
                } else {
                    unique_top_level_name(
                        output_dir,
                        &entry.relative_path,
                        true,
                        &offered_root_names,
                        &used_root_names,
                    )
                    .await?
                };
                (name, ManifestEntryDispositionKind::CreateDirectory)
            }
            ManifestEntryKind::RegularFile => {
                if exists
                    && existing_file_matches(
                        &existing_path,
                        entry.size,
                        entry.hash.expect("validated file entries have hashes"),
                    )
                    .await?
                {
                    (
                        entry.relative_path.clone(),
                        ManifestEntryDispositionKind::SkipIdentical,
                    )
                } else if exists || used_root_names.contains(&entry.relative_path) {
                    (
                        unique_top_level_name(
                            output_dir,
                            &entry.relative_path,
                            false,
                            &offered_root_names,
                            &used_root_names,
                        )
                        .await?,
                        ManifestEntryDispositionKind::Transfer,
                    )
                } else {
                    (
                        entry.relative_path.clone(),
                        ManifestEntryDispositionKind::Transfer,
                    )
                }
            }
        };
        used_root_names.insert(mapped_name.clone());
        mapped_roots.insert(entry.relative_path.clone(), mapped_name);
        root_actions.insert(entry.entry_id, action);
    }

    let mut entries = Vec::with_capacity(manifest.entries.len());
    let mut final_paths = HashSet::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        let (root, suffix) = entry
            .relative_path
            .split_once('/')
            .map_or((entry.relative_path.as_str(), None), |(root, suffix)| {
                (root, Some(suffix))
            });
        let mapped_root = mapped_roots.get(root).ok_or_else(|| {
            CoreError::Protocol(format!("manifest root {root:?} was not planned"))
        })?;
        let final_relative_path = suffix.map_or_else(
            || mapped_root.clone(),
            |suffix| format!("{mapped_root}/{suffix}"),
        );
        if !final_paths.insert(final_relative_path.clone()) {
            return Err(CoreError::Storage(format!(
                "manifest conflict plan maps multiple entries to {final_relative_path:?}"
            )));
        }
        let disposition = if suffix.is_none() {
            *root_actions
                .get(&entry.entry_id)
                .expect("every root has a conflict action")
        } else {
            match entry.kind {
                ManifestEntryKind::Directory => ManifestEntryDispositionKind::CreateDirectory,
                ManifestEntryKind::RegularFile => ManifestEntryDispositionKind::Transfer,
            }
        };
        entries.push(ManifestEntryDispositionV1 {
            entry_id: entry.entry_id,
            disposition,
            final_relative_path,
        });
    }

    Ok(ReceivePlan {
        accept: ManifestAcceptV1 {
            manifest_id: manifest.manifest_id.clone(),
            entries,
        },
    })
}

enum PlanMaterializeError {
    Collision,
    Fatal(CoreError),
}

async fn materialize_planned_directories(
    output_dir: &Path,
    manifest: &ManifestV1,
    plan: &ReceivePlan,
) -> Result<(), PlanMaterializeError> {
    let final_paths = plan
        .accept
        .entries
        .iter()
        .map(|entry| (entry.entry_id, entry.final_relative_path.as_str()))
        .collect::<HashMap<_, _>>();
    let mut created = Vec::new();
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.kind == ManifestEntryKind::Directory)
    {
        let relative_path = final_paths
            .get(&entry.entry_id)
            .expect("receive plan covers every directory");
        if let Err(error) = create_directory_chain(output_dir, relative_path, &mut created).await {
            rollback_created_directories(&created).await;
            return Err(error);
        }
    }
    Ok(())
}

async fn create_directory_chain(
    output_dir: &Path,
    relative_path: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(), PlanMaterializeError> {
    let mut current = output_dir.to_path_buf();
    for component in relative_path.split('/') {
        current.push(component);
        match fs::symlink_metadata(&current).await {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(PlanMaterializeError::Collision);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current).await {
                    Ok(()) => created.push(current.clone()),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        return Err(PlanMaterializeError::Collision);
                    }
                    Err(error) => {
                        return Err(PlanMaterializeError::Fatal(error.into()));
                    }
                }
            }
            Err(error) => return Err(PlanMaterializeError::Fatal(error.into())),
        }
    }
    Ok(())
}

async fn rollback_created_directories(created: &[PathBuf]) {
    for path in created.iter().rev() {
        let _ = fs::remove_dir(path).await;
    }
}

async fn unique_top_level_name(
    output_dir: &Path,
    original_name: &str,
    directory: bool,
    offered_names: &HashSet<String>,
    used_names: &HashSet<String>,
) -> Result<String, ManifestTransferError> {
    for index in 1_u64.. {
        let candidate = suffixed_name(original_name, index, directory);
        if offered_names.contains(&candidate) || used_names.contains(&candidate) {
            continue;
        }
        if !path_exists_without_following(&output_dir.join(&candidate)).await? {
            return Ok(candidate);
        }
    }
    unreachable!("u64 name suffix space is effectively unbounded")
}

fn suffixed_name(original_name: &str, index: u64, directory: bool) -> String {
    if directory {
        return format!("{original_name} ({index})");
    }
    let path = Path::new(original_name);
    let stem = path.file_stem().and_then(|value| value.to_str());
    let extension = path.extension().and_then(|value| value.to_str());
    match (stem, extension) {
        (Some(stem), Some(extension)) if !stem.is_empty() => {
            format!("{stem} ({index}).{extension}")
        }
        _ => format!("{original_name} ({index})"),
    }
}

async fn path_exists_without_following(path: &Path) -> Result<bool, ManifestTransferError> {
    match fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

async fn existing_file_matches(
    path: &Path,
    expected_size: u64,
    expected_hash: [u8; 32],
) -> Result<bool, ManifestTransferError> {
    let metadata = fs::symlink_metadata(path).await?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_size {
        return Ok(false);
    }
    let cancel = TransferCancelToken::new();
    let (actual_size, actual_hash) = hash_path(path, 64 * 1024, &cancel).await?;
    Ok(actual_size == expected_size && actual_hash == expected_hash)
}

async fn manifest_state_directory(
    output_dir: &Path,
    manifest_id: &ManifestId,
) -> Result<PathBuf, ManifestTransferError> {
    let state_root = ensure_private_directory(output_dir, STATE_ROOT_NAME).await?;
    let digest = blake3::hash(manifest_id.0.as_bytes()).to_hex().to_string();
    ensure_private_directory(&state_root, &digest).await
}

/// Removes only the private resume state for one Manifest receive.
///
/// Completed destination entries are deliberately preserved. The function
/// refuses symlinked state paths so an explicit Remove action cannot escape
/// the receiver-owned state namespace.
pub async fn discard_manifest_resume_state(
    output_dir: &Path,
    manifest_id: &ManifestId,
) -> Result<(), ManifestTransferError> {
    let state_root = output_dir.join(STATE_ROOT_NAME);
    let digest = blake3::hash(manifest_id.0.as_bytes()).to_hex().to_string();
    let state_dir = state_root.join(digest);
    for path in [&state_root, &state_dir] {
        match fs::symlink_metadata(path).await {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(CoreError::Storage(format!(
                    "manifest state path is not a safe directory: {}",
                    path.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    fs::remove_dir_all(&state_dir).await?;
    match fs::remove_dir(&state_root).await {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::NotFound
            ) => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

async fn ensure_private_directory(
    parent: &Path,
    name: &str,
) -> Result<PathBuf, ManifestTransferError> {
    let path = parent.join(name);
    match fs::symlink_metadata(&path).await {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(CoreError::Storage(format!(
                    "manifest state path is not a safe directory: {}",
                    path.display()
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::create_dir(&path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = fs::symlink_metadata(&path).await?;
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        return Err(CoreError::Storage(format!(
                            "manifest state path is not a safe directory: {}",
                            path.display()
                        )));
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(path)
}

#[allow(clippy::too_many_arguments)]
async fn prepare_manifest_receive_state(
    state_dir: &Path,
    storage_name: &str,
    transfer_id: &TransferId,
    entry: &ManifestEntryV1,
    chunk_size: u64,
    resume_requested: bool,
    buffer_size: usize,
    cancel: &TransferCancelToken,
) -> Result<PreparedManifestReceive, ManifestTransferError> {
    let lease = LocalFileStorage::try_acquire_resume_lease(state_dir, storage_name, transfer_id)?
        .ok_or_else(|| {
        CoreError::Storage(format!(
            "manifest entry {} resume state is already in use",
            entry.entry_id
        ))
    })?;
    if !resume_requested {
        reset_manifest_resume_state(state_dir, storage_name, transfer_id).await?;
    }

    let mut state = if resume_requested {
        match LocalFileStorage::read_resume_state(state_dir, storage_name, transfer_id).await {
            Ok(Some(state)) if manifest_resume_state_is_compatible(&state, entry, chunk_size) => {
                state
            }
            Ok(Some(_)) | Err(_) => {
                reset_manifest_resume_state(state_dir, storage_name, transfer_id).await?;
                fresh_manifest_resume_state(storage_name, transfer_id, entry, chunk_size)
            }
            Ok(None) => fresh_manifest_resume_state(storage_name, transfer_id, entry, chunk_size),
        }
    } else {
        fresh_manifest_resume_state(storage_name, transfer_id, entry, chunk_size)
    };
    let temp_path = LocalFileStorage::resumable_temp_path(state_dir, storage_name, transfer_id)?;
    let temp_len = match fs::metadata(&temp_path).await {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    if temp_len < state.bytes_received {
        reset_manifest_resume_state(state_dir, storage_name, transfer_id).await?;
        state = fresh_manifest_resume_state(storage_name, transfer_id, entry, chunk_size);
    } else if temp_len > state.bytes_received {
        let file = fs::OpenOptions::new().write(true).open(&temp_path).await?;
        file.set_len(state.bytes_received).await?;
    }

    let mut hasher = blake3::Hasher::new();
    if state.bytes_received > 0 {
        hash_file_prefix_from_path(
            &temp_path,
            &mut hasher,
            state.bytes_received,
            buffer_size,
            cancel,
        )
        .await?;
    }
    let prefix_hash = *hasher.finalize().as_bytes();
    persist_manifest_resume_state(
        state_dir,
        storage_name,
        transfer_id,
        entry,
        chunk_size,
        state.bytes_received,
        state.next_chunk_index,
        &hasher,
    )
    .await?;
    let (_, file) = LocalFileStorage::open_resumable_destination(state_dir, &state).await?;
    Ok(PreparedManifestReceive {
        state,
        file,
        hasher,
        prefix_hash,
        _lease: lease,
    })
}

fn manifest_resume_state_is_compatible(
    state: &TransferResumeState,
    entry: &ManifestEntryV1,
    chunk_size: u64,
) -> bool {
    state.file_size == entry.size
        && state.chunk_size == chunk_size
        && state.bytes_received <= entry.size
        && state.next_chunk_index == next_chunk_index(state.bytes_received, chunk_size)
}

fn fresh_manifest_resume_state(
    storage_name: &str,
    transfer_id: &TransferId,
    entry: &ManifestEntryV1,
    chunk_size: u64,
) -> TransferResumeState {
    TransferResumeState {
        transfer_id: transfer_id.clone(),
        file_name: storage_name.to_owned(),
        file_size: entry.size,
        chunk_size,
        bytes_received: 0,
        next_chunk_index: 0,
        hash_bytes: 0,
        hash_checkpoint: None,
        target_file_name: None,
    }
}

async fn reset_manifest_resume_state(
    state_dir: &Path,
    storage_name: &str,
    transfer_id: &TransferId,
) -> Result<(), ManifestTransferError> {
    LocalFileStorage::delete_resume_state(state_dir, storage_name, transfer_id).await?;
    LocalFileStorage::delete_resume_temp(state_dir, storage_name, transfer_id).await
}

#[allow(clippy::too_many_arguments)]
async fn persist_manifest_resume_state(
    state_dir: &Path,
    storage_name: &str,
    transfer_id: &TransferId,
    entry: &ManifestEntryV1,
    chunk_size: u64,
    bytes_received: u64,
    next_chunk_index: u64,
    hasher: &blake3::Hasher,
) -> Result<(), ManifestTransferError> {
    LocalFileStorage::write_resume_state(
        state_dir,
        &TransferResumeState {
            transfer_id: transfer_id.clone(),
            file_name: storage_name.to_owned(),
            file_size: entry.size,
            chunk_size,
            bytes_received,
            next_chunk_index,
            hash_bytes: bytes_received,
            hash_checkpoint: Some(hasher.finalize().to_hex().to_string()),
            target_file_name: None,
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn finalize_manifest_entry(
    output_dir: &Path,
    state_dir: &Path,
    storage_name: &str,
    planned_relative_path: &str,
    entry: &ManifestEntryV1,
    transfer_id: &TransferId,
    mut prepared: PreparedManifestReceive,
    expected_offset: u64,
    complete: &ManifestEntryCompleteV1,
) -> Result<String, ManifestTransferError> {
    if expected_offset != entry.size {
        return Err(CoreError::Transfer(format!(
            "manifest entry {} completed at {expected_offset} bytes; expected {}",
            entry.entry_id, entry.size
        )));
    }
    prepared.file.flush().await?;
    prepared.file.sync_all().await?;
    let actual_hash = *prepared.hasher.finalize().as_bytes();
    if entry.hash != Some(actual_hash) || complete.file_hash != actual_hash {
        return Err(CoreError::Transfer(format!(
            "manifest entry {} hash does not match offer",
            entry.entry_id
        )));
    }
    drop(prepared.file);
    let temp_path = LocalFileStorage::resumable_temp_path(state_dir, storage_name, transfer_id)?;
    let mut final_relative_path = planned_relative_path.to_owned();
    let mut collision_index = 1_u64;
    loop {
        validate_manifest_relative_path(&final_relative_path)
            .map_err(|error| CoreError::Storage(error.to_string()))?;
        ensure_safe_manifest_parent(output_dir, &final_relative_path).await?;
        let final_path = output_dir.join(Path::new(&final_relative_path));
        if LocalFileStorage::finalize_temp_file(&temp_path, &final_path).await? {
            break;
        }
        final_relative_path =
            collision_relative_path(planned_relative_path, &entry.relative_path, collision_index)?;
        collision_index += 1;
    }
    LocalFileStorage::delete_resume_state(state_dir, storage_name, transfer_id).await?;
    LocalFileStorage::write_receipt(
        state_dir,
        &TransferReceipt {
            transfer_id: transfer_id.clone(),
            file_name: storage_name.to_owned(),
            file_size: entry.size,
            file_hash: blake3::Hash::from_bytes(actual_hash).to_hex().to_string(),
        },
    )
    .await?;
    Ok(final_relative_path)
}

async fn ensure_safe_manifest_parent(
    output_dir: &Path,
    relative_path: &str,
) -> Result<(), ManifestTransferError> {
    let Some((parent, _)) = relative_path.rsplit_once('/') else {
        return Ok(());
    };
    let mut current = output_dir.to_path_buf();
    for component in parent.split('/') {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).await?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(CoreError::Storage(format!(
                "manifest destination ancestor is unsafe: {}",
                current.display()
            )));
        }
    }
    Ok(())
}

fn collision_relative_path(
    planned_relative_path: &str,
    offered_relative_path: &str,
    index: u64,
) -> Result<String, ManifestTransferError> {
    let planned_parent = planned_relative_path
        .rsplit_once('/')
        .map(|(parent, _)| parent);
    let offered_name = offered_relative_path
        .rsplit_once('/')
        .map_or(offered_relative_path, |(_, name)| name);
    let candidate = suffixed_name(offered_name, index, false);
    let relative_path =
        planned_parent.map_or(candidate.clone(), |parent| format!("{parent}/{candidate}"));
    validate_manifest_relative_path(&relative_path)
        .map_err(|error| CoreError::Storage(error.to_string()))?;
    Ok(relative_path)
}

fn successful_entry_result(
    entry: &ManifestEntryV1,
    final_relative_path: &str,
    skipped_identical: bool,
) -> ManifestEntryResultV1 {
    let status = if skipped_identical {
        ManifestEntryResultStatus::SkippedIdentical
    } else if final_relative_path == entry.relative_path {
        ManifestEntryResultStatus::Completed
    } else {
        ManifestEntryResultStatus::Renamed
    };
    ManifestEntryResultV1 {
        entry_id: entry.entry_id,
        status,
        offered_relative_path: entry.relative_path.clone(),
        final_relative_path: Some(final_relative_path.to_owned()),
        failure_code: None,
    }
}

fn manifest_entry_storage_name(entry_id: u32) -> String {
    format!("entry-{entry_id}")
}

fn manifest_entry_transfer_id(manifest_id: &ManifestId, entry_id: u32) -> TransferId {
    let digest = blake3::hash(manifest_id.0.as_bytes()).to_hex();
    TransferId::new(format!("manifest-{digest}-entry-{entry_id}"))
}

async fn hash_path(
    path: &Path,
    buffer_size: usize,
    cancel: &TransferCancelToken,
) -> Result<(u64, [u8; 32]), ManifestTransferError> {
    let mut file = fs::File::open(path).await?;
    let mut buffer = vec![0_u8; buffer_size];
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0_u64;
    loop {
        if cancel.is_cancelled() {
            return Err(interrupted_error(cancel));
        }
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| CoreError::Transfer("manifest source size overflow".into()))?;
        hasher.update(&buffer[..read]);
    }
    Ok((bytes, *hasher.finalize().as_bytes()))
}

async fn hash_file_prefix_from_path(
    path: &Path,
    hasher: &mut blake3::Hasher,
    bytes_to_hash: u64,
    buffer_size: usize,
    cancel: &TransferCancelToken,
) -> Result<(), ManifestTransferError> {
    let mut file = fs::File::open(path).await?;
    hash_open_file_prefix(&mut file, hasher, bytes_to_hash, buffer_size, cancel).await
}

async fn hash_open_file_prefix(
    file: &mut fs::File,
    hasher: &mut blake3::Hasher,
    bytes_to_hash: u64,
    buffer_size: usize,
    cancel: &TransferCancelToken,
) -> Result<(), ManifestTransferError> {
    file.seek(SeekFrom::Start(0)).await?;
    let mut remaining = bytes_to_hash;
    let mut buffer = vec![0_u8; buffer_size];
    while remaining > 0 {
        if cancel.is_cancelled() {
            return Err(interrupted_error(cancel));
        }
        let requested =
            usize::try_from(remaining.min(buffer_size as u64)).expect("bounded by buffer size");
        let read = file.read(&mut buffer[..requested]).await?;
        if read == 0 {
            return Err(CoreError::Transfer(format!(
                "manifest resume prefix ended {} bytes early",
                remaining
            )));
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(())
}

async fn read_full_chunk(
    file: &mut fs::File,
    buffer: &mut [u8],
) -> Result<usize, ManifestTransferError> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = file.read(&mut buffer[filled..]).await?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}

fn next_chunk_index(bytes_received: u64, chunk_size: u64) -> u64 {
    if bytes_received == 0 {
        0
    } else {
        bytes_received.div_ceil(chunk_size)
    }
}

fn expect_manifest_hello(
    frame: ManifestFrame,
    expected_role: PeerRole,
) -> Result<(), ManifestTransferError> {
    match frame {
        ManifestFrame::Hello(ManifestHelloV1 {
            protocol_version: MANIFEST_V1_PROTOCOL_VERSION,
            role,
        }) if role == expected_role => Ok(()),
        ManifestFrame::Error(error) => Err(peer_manifest_error(error)),
        frame => Err(CoreError::Protocol(format!(
            "expected Manifest hello from {expected_role:?}, got {frame:?}"
        ))),
    }
}

fn expect_manifest_offer(
    frame: ManifestFrame,
    expected_chunk_size: usize,
) -> Result<ManifestOfferV1, ManifestTransferError> {
    match frame {
        ManifestFrame::Offer(offer) if offer.chunk_size == expected_chunk_size as u64 => {
            offer
                .manifest
                .validate_structure()
                .map_err(|error| CoreError::Protocol(error.to_string()))?;
            Ok(offer)
        }
        ManifestFrame::Offer(offer) => Err(CoreError::Protocol(format!(
            "sender Manifest chunk size {} does not match receiver chunk size {expected_chunk_size}",
            offer.chunk_size
        ))),
        ManifestFrame::Error(error) => Err(peer_manifest_error(error)),
        frame => Err(CoreError::Protocol(format!(
            "expected Manifest offer, got {frame:?}"
        ))),
    }
}

fn expect_manifest_accept(
    frame: ManifestFrame,
    manifest: &ManifestV1,
) -> Result<ManifestAcceptV1, ManifestTransferError> {
    let accept = match frame {
        ManifestFrame::Accept(accept) => accept,
        ManifestFrame::Error(error) => return Err(peer_manifest_error(error)),
        frame => {
            return Err(CoreError::Protocol(format!(
                "expected Manifest accept, got {frame:?}"
            )));
        }
    };
    if accept.manifest_id != manifest.manifest_id || accept.entries.len() != manifest.entries.len()
    {
        return Err(CoreError::Protocol(
            "Manifest accept does not cover the offered transfer set".into(),
        ));
    }
    let by_id = accept
        .entries
        .iter()
        .map(|entry| (entry.entry_id, entry))
        .collect::<HashMap<_, _>>();
    if by_id.len() != manifest.entries.len() {
        return Err(CoreError::Protocol(
            "Manifest accept contains duplicate entry ids".into(),
        ));
    }
    let mut final_paths = HashSet::with_capacity(accept.entries.len());
    let mut accepted_parent_paths = HashMap::new();
    for offered in &manifest.entries {
        let disposition = by_id.get(&offered.entry_id).ok_or_else(|| {
            CoreError::Protocol(format!("Manifest accept omits entry {}", offered.entry_id))
        })?;
        validate_manifest_relative_path(&disposition.final_relative_path)
            .map_err(|error| CoreError::Protocol(error.to_string()))?;
        if !final_paths.insert(disposition.final_relative_path.as_str()) {
            return Err(CoreError::Protocol(format!(
                "Manifest accept reuses final path {:?}",
                disposition.final_relative_path
            )));
        }
        match (offered.kind, disposition.disposition) {
            (ManifestEntryKind::Directory, ManifestEntryDispositionKind::CreateDirectory)
            | (ManifestEntryKind::RegularFile, ManifestEntryDispositionKind::Transfer)
            | (ManifestEntryKind::RegularFile, ManifestEntryDispositionKind::SkipIdentical) => {}
            _ => {
                return Err(CoreError::Protocol(format!(
                    "invalid disposition {:?} for entry {} ({:?})",
                    disposition.disposition, offered.entry_id, offered.kind
                )));
            }
        }
        if let Some((offered_parent, _)) = offered.relative_path.rsplit_once('/') {
            let expected_parent = accepted_parent_paths.get(offered_parent).ok_or_else(|| {
                CoreError::Protocol(format!(
                    "Manifest accept has no mapped parent for {:?}",
                    offered.relative_path
                ))
            })?;
            let actual_parent = disposition
                .final_relative_path
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .ok_or_else(|| {
                    CoreError::Protocol(format!(
                        "Manifest accept flattens nested entry {:?}",
                        offered.relative_path
                    ))
                })?;
            if actual_parent != *expected_parent {
                return Err(CoreError::Protocol(format!(
                    "Manifest accept remaps entry {} outside its mapped parent",
                    offered.entry_id
                )));
            }
        }
        if offered.kind == ManifestEntryKind::Directory {
            accepted_parent_paths.insert(
                offered.relative_path.as_str(),
                disposition.final_relative_path.as_str(),
            );
        }
    }
    Ok(accept)
}

fn expect_manifest_resume_status(
    frame: ManifestFrame,
    manifest_id: &ManifestId,
    entry: &ManifestEntryV1,
    transfer_id: &TransferId,
    chunk_size: usize,
) -> Result<ManifestResumeStatusV1, ManifestTransferError> {
    match frame {
        ManifestFrame::ResumeStatus(status)
            if &status.manifest_id == manifest_id
                && status.entry_id == entry.entry_id
                && &status.transfer_id == transfer_id
                && status.bytes_received <= entry.size
                && status.next_chunk_index
                    == next_chunk_index(status.bytes_received, chunk_size as u64) =>
        {
            Ok(status)
        }
        ManifestFrame::Error(error) => Err(peer_manifest_error(error)),
        frame => Err(CoreError::Protocol(format!(
            "expected valid Manifest resume status for entry {}, got {frame:?}",
            entry.entry_id
        ))),
    }
}

fn expect_manifest_entry_start(
    frame: ManifestFrame,
    manifest_id: &ManifestId,
    entry_id: u32,
    transfer_id: &TransferId,
) -> Result<(), ManifestTransferError> {
    match frame {
        ManifestFrame::EntryStart(start)
            if &start.manifest_id == manifest_id
                && start.entry_id == entry_id
                && &start.transfer_id == transfer_id =>
        {
            Ok(())
        }
        ManifestFrame::Error(error) => Err(peer_manifest_error(error)),
        frame => Err(CoreError::Protocol(format!(
            "expected Manifest entry start for {entry_id}, got {frame:?}"
        ))),
    }
}

fn expect_manifest_entry_complete_ack(
    frame: ManifestFrame,
    manifest_id: &ManifestId,
    entry_id: u32,
    transfer_id: &TransferId,
) -> Result<(), ManifestTransferError> {
    match frame {
        ManifestFrame::EntryCompleteAck(ack)
            if &ack.manifest_id == manifest_id
                && ack.entry_id == entry_id
                && &ack.transfer_id == transfer_id =>
        {
            Ok(())
        }
        ManifestFrame::Error(error) => Err(peer_manifest_error(error)),
        frame => Err(CoreError::Protocol(format!(
            "expected Manifest entry completion ack for {entry_id}, got {frame:?}"
        ))),
    }
}

fn validate_manifest_entry_complete(
    complete: &ManifestEntryCompleteV1,
    manifest_id: &ManifestId,
    entry: &ManifestEntryV1,
    transfer_id: &TransferId,
) -> Result<(), ManifestTransferError> {
    if &complete.manifest_id != manifest_id
        || complete.entry_id != entry.entry_id
        || &complete.transfer_id != transfer_id
        || entry.hash != Some(complete.file_hash)
    {
        return Err(CoreError::Protocol(format!(
            "invalid Manifest completion for entry {}",
            entry.entry_id
        )));
    }
    Ok(())
}

fn expect_manifest_complete(
    frame: ManifestFrame,
    manifest_id: &ManifestId,
) -> Result<(), ManifestTransferError> {
    match frame {
        ManifestFrame::Complete(complete) if &complete.manifest_id == manifest_id => Ok(()),
        ManifestFrame::Error(error) => Err(peer_manifest_error(error)),
        frame => Err(CoreError::Protocol(format!(
            "expected Manifest completion, got {frame:?}"
        ))),
    }
}

fn expect_manifest_complete_ack(
    frame: ManifestFrame,
    manifest: &ManifestV1,
    accept: &ManifestAcceptV1,
) -> Result<ManifestCompleteAckV1, ManifestTransferError> {
    let ack = match frame {
        ManifestFrame::CompleteAck(ack) => ack,
        ManifestFrame::Error(error) => return Err(peer_manifest_error(error)),
        frame => {
            return Err(CoreError::Protocol(format!(
                "expected Manifest completion ack, got {frame:?}"
            )));
        }
    };
    if ack.manifest_id != manifest.manifest_id || ack.entries.len() != manifest.entries.len() {
        return Err(CoreError::Protocol(
            "Manifest completion ack does not cover the offered transfer set".into(),
        ));
    }
    let results = ack
        .entries
        .iter()
        .map(|result| (result.entry_id, result))
        .collect::<HashMap<_, _>>();
    if results.len() != manifest.entries.len() {
        return Err(CoreError::Protocol(
            "Manifest completion ack contains duplicate entry ids".into(),
        ));
    }
    let dispositions = accept
        .entries
        .iter()
        .map(|entry| (entry.entry_id, entry))
        .collect::<HashMap<_, _>>();
    let mut final_paths = HashSet::new();
    for entry in &manifest.entries {
        let result = results.get(&entry.entry_id).ok_or_else(|| {
            CoreError::Protocol(format!(
                "Manifest completion ack omits entry {}",
                entry.entry_id
            ))
        })?;
        if result.offered_relative_path != entry.relative_path
            || matches!(
                result.status,
                ManifestEntryResultStatus::Failed | ManifestEntryResultStatus::Cancelled
            )
        {
            return Err(CoreError::Protocol(format!(
                "Manifest entry {} did not complete successfully",
                entry.entry_id
            )));
        }
        let final_path = result.final_relative_path.as_deref().ok_or_else(|| {
            CoreError::Protocol(format!(
                "Manifest result {} has no final path",
                entry.entry_id
            ))
        })?;
        validate_manifest_relative_path(final_path)
            .map_err(|error| CoreError::Protocol(error.to_string()))?;
        if !final_paths.insert(final_path) {
            return Err(CoreError::Protocol(format!(
                "Manifest completion ack reuses final path {final_path:?}"
            )));
        }
        let disposition = dispositions
            .get(&entry.entry_id)
            .expect("accept validation covers every entry");
        match disposition.disposition {
            ManifestEntryDispositionKind::SkipIdentical
                if result.status != ManifestEntryResultStatus::SkippedIdentical =>
            {
                return Err(CoreError::Protocol(format!(
                    "Manifest entry {} was accepted as identical but result changed",
                    entry.entry_id
                )));
            }
            ManifestEntryDispositionKind::CreateDirectory
                if final_path != disposition.final_relative_path =>
            {
                return Err(CoreError::Protocol(format!(
                    "Manifest directory {} changed after its name was claimed",
                    entry.entry_id
                )));
            }
            _ => {}
        }
    }
    Ok(ack)
}

fn validate_manifest_chunk(
    chunk: &ManifestChunkV1,
    manifest_id: &ManifestId,
    entry: &ManifestEntryV1,
    transfer_id: &TransferId,
    expected_index: u64,
    expected_offset: u64,
) -> Result<(), ManifestTransferError> {
    if &chunk.manifest_id != manifest_id
        || chunk.entry_id != entry.entry_id
        || &chunk.transfer_id != transfer_id
        || chunk.index != expected_index
        || chunk.offset != expected_offset
    {
        return Err(CoreError::Protocol(format!(
            "invalid Manifest chunk sequence for entry {}: index={} offset={} expected_index={expected_index} expected_offset={expected_offset}",
            entry.entry_id, chunk.index, chunk.offset
        )));
    }
    Ok(())
}

async fn recv_manifest_or_cancel(
    connection: &mut dyn ManifestFrameConnection,
    cancel: &TransferCancelToken,
    manifest_id: Option<&ManifestId>,
    entry_id: Option<u32>,
) -> Result<ManifestFrame, ManifestTransferError> {
    tokio::select! {
        frame = connection.recv_manifest_frame() => frame,
        () = cancel.cancelled() => {
            notify_cancelled(connection, cancel, manifest_id, entry_id).await;
            Err(interrupted_error(cancel))
        }
    }
}

async fn recv_manifest_or_cancel_with_timeout(
    connection: &mut dyn ManifestFrameConnection,
    cancel: &TransferCancelToken,
    manifest_id: Option<&ManifestId>,
    entry_id: Option<u32>,
    timeout: Duration,
) -> Result<ManifestFrame, ManifestTransferError> {
    tokio::select! {
        frame = connection.recv_manifest_frame() => frame,
        () = cancel.cancelled() => {
            notify_cancelled(connection, cancel, manifest_id, entry_id).await;
            Err(interrupted_error(cancel))
        }
        () = tokio::time::sleep(timeout) => Err(CoreError::Transfer(format!(
            "peer did not confirm Manifest completion within {} seconds",
            timeout.as_secs_f64()
        ))),
    }
}

async fn check_cancelled(
    connection: &mut dyn ManifestFrameConnection,
    cancel: &TransferCancelToken,
    manifest_id: Option<&ManifestId>,
    entry_id: Option<u32>,
) -> Result<(), ManifestTransferError> {
    if cancel.is_cancelled() {
        notify_cancelled(connection, cancel, manifest_id, entry_id).await;
        return Err(interrupted_error(cancel));
    }
    Ok(())
}

async fn notify_cancelled(
    connection: &mut dyn ManifestFrameConnection,
    cancel: &TransferCancelToken,
    manifest_id: Option<&ManifestId>,
    entry_id: Option<u32>,
) {
    let (code, message) = if cancel.is_pause() {
        (ERROR_PAUSED, USER_PAUSE_MESSAGE)
    } else {
        (ERROR_CANCELLED, USER_INTERRUPT_MESSAGE)
    };
    notify_manifest_error(connection, manifest_id, entry_id, code, message).await;
}

async fn notify_manifest_error(
    connection: &mut dyn ManifestFrameConnection,
    manifest_id: Option<&ManifestId>,
    entry_id: Option<u32>,
    code: &str,
    message: &str,
) {
    let _ = connection
        .send_manifest_frame(ManifestFrame::Error(ManifestErrorV1 {
            manifest_id: manifest_id.cloned(),
            entry_id,
            code: code.to_owned(),
            message: message.to_owned(),
        }))
        .await;
}

fn unexpected_send_control(frame: ManifestFrame) -> ManifestTransferError {
    match frame {
        ManifestFrame::Error(error) => peer_manifest_error(error),
        frame => CoreError::Protocol(format!(
            "unexpected Manifest control frame while sending payload: {frame:?}"
        )),
    }
}

fn peer_manifest_error(error: ManifestErrorV1) -> ManifestTransferError {
    match error.code.as_str() {
        ERROR_CANCELLED => CoreError::Transfer(PEER_INTERRUPT_MESSAGE.into()),
        ERROR_PAUSED => CoreError::Transfer(PEER_PAUSE_MESSAGE.into()),
        _ => CoreError::Transfer(format!(
            "peer reported Manifest error {}: {}",
            error.code, error.message
        )),
    }
}

fn interrupted_error(cancel: &TransferCancelToken) -> ManifestTransferError {
    let message = if cancel.is_pause() {
        USER_PAUSE_MESSAGE
    } else {
        USER_INTERRUPT_MESSAGE
    };
    CoreError::Transfer(message.into())
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
