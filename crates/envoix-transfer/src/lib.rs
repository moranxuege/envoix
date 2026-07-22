//! File-transfer state machine.

mod delivery_v2;
mod destination_v2;
mod job;
mod manifest;
mod manifest_v2_engine;
mod persistence_v2;

pub use destination_v2::{
    AUTO_RECEIVE_PLAINTEXT_LIMIT_BYTES, DestinationDecisionV2, DestinationModeV2,
    DestinationPlanErrorV2, DestinationPlanStoreV2, DestinationRequestV2, DestinationWritePlanV2,
    LocalDestinationProviderV2, POST_SAVE_RESERVE_BYTES, StorageDomainIdentityV2,
    local_allocatable_bytes,
};

pub use delivery_v2::{
    DeliveryAuthorityErrorV2, ManifestV2DeliveryAuthority, ReceiverDeliveryRecordV2,
    ReceiverDeliveryStoreV2, SenderDeliveryRecordV2, SenderDeliveryStoreV2, SenderTransferPhaseV2,
};

pub use job::{
    AddSourceResult, CanonicalTransferJob, DEFAULT_INVENTORY_PAGE_SIZE, InventoryCursor,
    InventoryItem, InventoryPage, InventorySummary, JobLifecycle, LocalSourceOrigin,
    MAX_INVENTORY_PAGE_SIZE, PreparedFileSource, ProviderSourceIssue, SourceDecision, SourceIssue,
    SourceIssueKind, SourceItemId, SourceSelectionInfo, SourceSelectionState, TransferJobError,
    TransferJobStore,
};

pub use manifest::{
    ManifestEventSink, ManifestNoopEventSink, ManifestSendRequest, ManifestTransferEngine,
    ManifestTransferEvent, ManifestTransferSummary, discard_manifest_resume_state,
};

pub use manifest_v2_engine::{
    ManifestV2DataError, ManifestV2DataPlane, ManifestV2PayloadSink, ManifestV2ProgressPhase,
    ManifestV2ProgressSink, ManifestV2ResultGate, NoopManifestV2ResultGate,
    ReceiverDataPlaneLedgerV2, ReceiverDataPlaneStoreV2, ReceiverDataPlaneSummaryV2, SavedEntryV2,
    SenderDataPlaneSummaryV2, SenderResumeIntentV2, VerifiedEntryV2, sender_resume_intent,
};

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use envoix_error::CoreError;
use envoix_protocol::{
    Chunk, Complete, CompleteAck, ErrorFrame, FileHeader, Frame, FrameConnection, Hello, Ready,
    ResumeStatus,
};
use envoix_storage::{LocalFileStorage, TransferReceipt, TransferResumeState};
use envoix_types::{
    DataPath, PROTOCOL_VERSION, PairingStep, PeerRole, TransferDirection, TransferId,
};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Notify;

/// Default sequential chunk size used by clients that do not override it.
pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;
/// Minimum accepted transfer chunk size.
pub const MIN_CHUNK_SIZE: usize = 16 * 1024;
/// Maximum accepted transfer chunk size.
pub const MAX_CHUNK_SIZE: usize = 16 * 1024 * 1024;

// A max-size Chunk frame is MAX_CHUNK_SIZE of payload plus the frame header and
// the chunk's index/offset metadata; the codec rejects any frame longer than
// `envoix_protocol::MAX_FRAME_SIZE` (protocol/src/lib.rs). That crate sits below
// this one and hardcodes the literal, so the invariant is unenforced there —
// pin it here (where both consts are visible) so raising MAX_CHUNK_SIZE without
// raising MAX_FRAME_SIZE is a COMPILE error instead of silently rejecting valid
// max-size chunks at runtime. 1 KiB comfortably covers the per-frame overhead.
const _: () = assert!(
    envoix_protocol::MAX_FRAME_SIZE >= MAX_CHUNK_SIZE + 1024,
    "MAX_FRAME_SIZE must exceed MAX_CHUNK_SIZE by the chunk-frame overhead; \
     raising MAX_CHUNK_SIZE requires raising envoix_protocol::MAX_FRAME_SIZE too",
);

/// Validate a chunk size against the transfer engine's constraints: it must fall
/// within [`MIN_CHUNK_SIZE`]..=[`MAX_CHUNK_SIZE`] and be a power of two.
pub fn validate_chunk_size(chunk_size: usize) -> Result<(), CoreError> {
    if !(MIN_CHUNK_SIZE..=MAX_CHUNK_SIZE).contains(&chunk_size) {
        return Err(CoreError::InvalidInput(format!(
            "chunk size must be between {MIN_CHUNK_SIZE} and {MAX_CHUNK_SIZE} bytes"
        )));
    }
    if !chunk_size.is_power_of_two() {
        return Err(CoreError::InvalidInput(
            "chunk size must be a power of two".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod chunk_size_validation_tests;

/// Protocol error text sent when a local user interrupts a transfer.
pub const USER_INTERRUPT_MESSAGE: &str = "transfer interrupted by user";
/// Protocol error text sent when a local user pauses a transfer (same wire frame
/// as an interrupt — delivery is best-effort; a degraded path may drop it, so
/// receivers must not depend on it and fall back to connection-lost handling).
pub const USER_PAUSE_MESSAGE: &str = "transfer paused by user";
/// Local error text when the peer reported an interrupt.
pub const PEER_INTERRUPT_MESSAGE: &str = "transfer interrupted by peer";
/// Local error text when the peer reported a pause.
pub const PEER_PAUSE_MESSAGE: &str = "transfer paused by peer";
#[cfg(not(test))]
const COMPLETE_ACK_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(test)]
const COMPLETE_ACK_TIMEOUT: Duration = Duration::from_millis(250);
const RESUME_STATE_WRITE_INTERVAL: u64 = 8 * 1024 * 1024;

/// Error type returned by the transfer state machine.
pub type TransferError = CoreError;

/// Observer for transfer lifecycle and progress events.
pub trait EventSink: Send + Sync {
    /// Handles one transfer event.
    fn on_event(&self, event: TransferEvent);
}

/// Event sink that ignores all transfer events.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn on_event(&self, _event: TransferEvent) {}
}

/// Shared cancellation flag used for graceful user interrupts.
#[derive(Clone, Debug, Default)]
pub struct TransferCancelToken {
    inner: Arc<CancelInner>,
}

#[derive(Debug, Default)]
struct CancelInner {
    cancelled: AtomicBool,
    /// Set (before `cancelled`) when the interrupt is a pause, so the engine can
    /// tell the peer — and report locally — "paused" rather than "interrupted".
    paused: AtomicBool,
    notify: Notify,
}

impl TransferCancelToken {
    /// Creates a token in the non-cancelled state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation and wakes waiters.
    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
        }
    }

    /// Requests a pause: same interrupt mechanics as [`cancel`](Self::cancel),
    /// but flagged so the stop is reported as a pause (resumable intent) on both
    /// sides. Peer delivery of the reason is best-effort.
    pub fn pause(&self) {
        self.inner.paused.store(true, Ordering::SeqCst);
        self.cancel();
    }

    /// Returns whether the requested interrupt was a pause.
    pub fn is_pause(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst)
    }

    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Waits until cancellation is requested.
    pub async fn cancelled(&self) {
        loop {
            let notified = self.inner.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// User-visible transfer lifecycle event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransferEvent {
    /// Diagnostic-only status for logs and path reporting. It never changes the
    /// canonical transfer lifecycle.
    Diagnostic {
        /// Human-readable diagnostic detail.
        message: String,
    },
    /// A send or receive operation has started.
    Started {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// Direction of this local operation.
        direction: TransferDirection,
        /// File name being transferred.
        file_name: String,
        /// Total expected plaintext bytes.
        total_bytes: u64,
        /// Plaintext bytes already present before this attempt started.
        bytes_resumed: u64,
    },
    /// SEND only: every byte and the Complete frame have been sent; awaiting
    /// the receiver's CompleteAck (the final round trip - real, failure-prone,
    /// and previously invisible inside "100%%"). See the state-machine design.
    Confirming {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// BLAKE3 hash of the bytes actually sent (the `Complete` frame's
        /// hash) - the committed proof basis for receipt verification, so a
        /// receipt is never checked against the (mutable) source path later.
        file_hash: String,
    },
    /// More plaintext bytes have been sent or persisted.
    Progress {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// Plaintext bytes transferred so far.
        bytes_transferred: u64,
        /// Total expected plaintext bytes.
        total_bytes: u64,
    },
    /// Canonical Manifest-v2 lifecycle fact. Native UI must project this
    /// variant directly and must not infer state from diagnostic text.
    ManifestV2Phase {
        transfer_id: TransferId,
        direction: TransferDirection,
        phase: ManifestV2ProgressPhase,
    },
    /// A hash-only verification phase has started.
    HashStarted {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// Direction of this local operation.
        direction: TransferDirection,
        /// File name being verified.
        file_name: String,
        /// Number of plaintext bytes being hashed.
        bytes_to_hash: u64,
    },
    /// A hash-only verification phase completed.
    HashCompleted {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// Direction of this local operation.
        direction: TransferDirection,
        /// File name that was verified.
        file_name: String,
        /// Number of plaintext bytes hashed.
        bytes_hashed: u64,
    },
    /// Transfer completed and, on receive, the file was finalized.
    Completed {
        /// Transfer identifier for correlating events.
        transfer_id: TransferId,
        /// File name that completed.
        file_name: String,
        /// Final plaintext byte count.
        bytes_transferred: u64,
    },
    /// The current transfer attempt failed before completion.
    Failed {
        /// Direction of this local operation.
        direction: TransferDirection,
        /// Human-readable failure reason.
        reason: String,
    },
    /// Rendezvous-room pairing progress. Emitted by the session layer.
    Pairing {
        /// Which pairing step was reached.
        step: PairingStep,
    },
    /// Establishing the peer connection (dialing, or accepting after a room
    /// pairing). Emitted by the session layer.
    Connecting,
    /// A data path to the peer was selected for the first time.
    ///
    /// Emitted by the session layer (the engine does not know transports).
    Connected {
        /// The selected path.
        path: DataPath,
    },
    /// The selected data path changed (e.g. a relay -> direct upgrade after
    /// hole-punching succeeded).
    PathChanged {
        /// The newly selected path.
        path: DataPath,
    },
}

/// Summary returned after a successful send or receive operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferSummary {
    /// Transfer identifier for the completed transfer.
    pub transfer_id: TransferId,
    /// File name used for the transfer.
    pub file_name: String,
    /// Plaintext bytes transferred.
    pub bytes_transferred: u64,
    /// BLAKE3 hash verified by both sides before completion.
    pub file_hash: String,
}

/// Sequential single-file transfer engine.
#[derive(Clone, Debug)]
pub struct TransferEngine {
    chunk_size: usize,
}

impl TransferEngine {
    /// Creates a transfer engine using a fixed chunk size.
    pub fn new(chunk_size: usize) -> Self {
        Self { chunk_size }
    }

    /// Sends one file over an established frame connection.
    pub async fn send_file(
        &self,
        connection: &mut dyn FrameConnection,
        path: PathBuf,
        resume: bool,
        events: &dyn EventSink,
    ) -> Result<TransferSummary, TransferError> {
        let cancel = TransferCancelToken::new();
        self.send_file_with_cancel(connection, path, resume, events, &cancel)
            .await
    }

    /// Sends one file and notifies the peer if `cancel` is triggered.
    pub async fn send_file_with_cancel(
        &self,
        connection: &mut dyn FrameConnection,
        path: PathBuf,
        resume: bool,
        events: &dyn EventSink,
        cancel: &TransferCancelToken,
    ) -> Result<TransferSummary, TransferError> {
        if self.chunk_size == 0 {
            return Err(CoreError::InvalidInput(
                "chunk size must be positive".into(),
            ));
        }

        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| CoreError::InvalidInput("source path has no file name".into()))?
            .to_owned();
        let metadata = tokio::fs::metadata(&path).await?;
        if !metadata.is_file() {
            return Err(CoreError::InvalidInput(format!(
                "source is not a file: {}",
                path.display()
            )));
        }

        let total_bytes = metadata.len();
        let transfer_id = random_transfer_id()?;

        connection
            .send_frame(Frame::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                role: PeerRole::Sender,
            }))
            .await?;
        expect_ready(recv_frame_or_cancel(connection, cancel).await?)?;

        connection
            .send_frame(Frame::FileHeader(FileHeader {
                transfer_id: transfer_id.clone(),
                file_name: file_name.clone(),
                file_size: total_bytes,
                chunk_size: self.chunk_size as u64,
                resume_requested: resume,
            }))
            .await?;
        let resume_status = expect_resume_status(
            recv_frame_or_cancel(connection, cancel).await?,
            &transfer_id,
            self.chunk_size,
        )?;
        if resume_status.bytes_received > total_bytes {
            return Err(CoreError::Transfer(format!(
                "receiver resume offset {} exceeds file size {total_bytes}",
                resume_status.bytes_received
            )));
        }

        let mut hasher = blake3::Hasher::new();
        let mut file = LocalFileStorage::open_source(&path).await?;
        let mut start_offset = 0;
        let mut start_index = 0;

        if resume_status.bytes_received > 0 {
            events.on_event(TransferEvent::HashStarted {
                transfer_id: transfer_id.clone(),
                direction: TransferDirection::Send,
                file_name: file_name.clone(),
                bytes_to_hash: resume_status.bytes_received,
            });
            if let Err(error) = hash_file_prefix(
                &mut file,
                &mut hasher,
                resume_status.bytes_received,
                self.chunk_size,
                cancel,
            )
            .await
            {
                if cancel.is_cancelled() {
                    notify_interrupted(connection, cancel).await;
                }
                return Err(error);
            }
            let prefix_hash = hasher.finalize().to_hex().to_string();
            events.on_event(TransferEvent::HashCompleted {
                transfer_id: transfer_id.clone(),
                direction: TransferDirection::Send,
                file_name: file_name.clone(),
                bytes_hashed: resume_status.bytes_received,
            });
            if prefix_hash == resume_status.prefix_hash {
                start_offset = resume_status.bytes_received;
                start_index = resume_status.next_chunk_index;
            } else {
                hasher = blake3::Hasher::new();
            }
        }

        events.on_event(TransferEvent::Started {
            transfer_id: transfer_id.clone(),
            direction: TransferDirection::Send,
            file_name: file_name.clone(),
            total_bytes,
            bytes_resumed: start_offset,
        });

        file.seek(SeekFrom::Start(start_offset)).await?;
        let mut buffer = vec![0_u8; self.chunk_size];
        let mut index = start_index;
        let mut offset = start_offset;

        loop {
            check_cancelled(connection, cancel).await?;
            let bytes_read = read_full_chunk(&mut file, &mut buffer).await?;
            if bytes_read == 0 {
                break;
            }

            hasher.update(&buffer[..bytes_read]);
            if let Err(error) = connection
                .send_chunk(&transfer_id, index, offset, &buffer[..bytes_read])
                .await
            {
                return Err(peer_closed_error(error));
            }

            offset += bytes_read as u64;
            index += 1;
            events.on_event(TransferEvent::Progress {
                transfer_id: transfer_id.clone(),
                bytes_transferred: offset,
                total_bytes,
            });
        }

        if offset != total_bytes {
            return Err(CoreError::Transfer(format!(
                "unexpected end of file: expected to read {} bytes but only read {}",
                total_bytes, offset
            )));
        }

        let file_hash = hasher.finalize().to_hex().to_string();
        connection
            .send_frame(Frame::Complete(Complete {
                transfer_id: transfer_id.clone(),
                file_hash: file_hash.clone(),
            }))
            .await
            .map_err(peer_closed_error)?;
        events.on_event(TransferEvent::Confirming {
            transfer_id: transfer_id.clone(),
            file_hash: file_hash.clone(),
        });
        // The whole file plus the Complete frame (which carries the file hash the
        // receiver verifies before finalizing) have been sent. Require the
        // receiver's CompleteAck: it is the receiver's proof that it finalized.
        // The receiver holds the connection open until we close it (it does not
        // close first), so the ack is delivered reliably rather than racing a
        // close. A genuine failure surfaces as an Error frame here (or earlier,
        // during the chunk phase); only a true network death in this final round
        // trip fails an otherwise-complete send, which resume recovers on retry.
        let ack =
            recv_frame_or_cancel_with_timeout(connection, cancel, COMPLETE_ACK_TIMEOUT).await?;
        expect_complete_ack(ack, &transfer_id)?;
        events.on_event(TransferEvent::Completed {
            transfer_id: transfer_id.clone(),
            file_name: file_name.clone(),
            bytes_transferred: offset,
        });

        Ok(TransferSummary {
            transfer_id,
            file_name,
            bytes_transferred: offset,
            file_hash,
        })
    }

    /// Receives one file over an established frame connection.
    pub async fn receive_file(
        &self,
        connection: &mut dyn FrameConnection,
        output_dir: PathBuf,
        events: &dyn EventSink,
    ) -> Result<TransferSummary, TransferError> {
        let cancel = TransferCancelToken::new();
        self.receive_file_with_cancel(connection, output_dir, events, &cancel)
            .await
    }

    /// Receives one file and notifies the peer if `cancel` is triggered.
    pub async fn receive_file_with_cancel(
        &self,
        connection: &mut dyn FrameConnection,
        output_dir: PathBuf,
        events: &dyn EventSink,
        cancel: &TransferCancelToken,
    ) -> Result<TransferSummary, TransferError> {
        expect_sender_hello(recv_frame_or_cancel(connection, cancel).await?)?;
        connection.send_frame(Frame::Ready(Ready)).await?;

        let header = expect_file_header(recv_frame_or_cancel(connection, cancel).await?)?;
        validate_header(&header, self.chunk_size)?;
        // One scan serves the whole ladder below: the in-flight partial (and
        // the landing name it recorded), the existing-final answer, and the
        // receipt short-circuit.
        let resume_state = LocalFileStorage::find_resume_state(
            &output_dir,
            &header.file_name,
            header.file_size,
            header.chunk_size,
        )
        .await?;
        // A prior fresh attempt recorded where it is landing; honor that name
        // so resuming it continues beside the original file instead of being
        // instantly answered by it (field bug: fresh re-send of an already-
        // present file "completed" in 308ms off the existing final, with the
        // fresh request silently ignored).
        let mut target_name = resume_state
            .as_ref()
            .and_then(|state| state.target_file_name.clone())
            .unwrap_or_else(|| header.file_name.clone());
        if fs::try_exists(output_dir.join(&target_name)).await? {
            if header.resume_requested {
                let final_path = output_dir.join(&target_name);
                return receive_existing_final(connection, header, final_path, events).await;
            }
            // A fresh send must move real bytes: never answer it from an
            // existing same-name final - land beside it under a free name.
            target_name = unique_final_name(&output_dir, &header.file_name).await?;
        }
        let final_path = output_dir.join(&target_name);
        // The file itself may have been moved/published away after completion;
        // its receipt then re-confirms a re-offer without any bytes re-sent.
        // Gated on resume_requested so a --fresh send forces a real re-receive,
        // and PRE-EMPTED by an in-flight partial: a receipt re-confirms an
        // ALREADY-completed transfer - it must never override one that is
        // mid-flight (field bug: pause+resume of a fresh re-send of a
        // previously-completed file "completed" instantly off the old receipt,
        // orphaning the partial and delivering nothing).
        if header.resume_requested
            && resume_state.is_none()
            && let Some(receipt) =
                LocalFileStorage::read_receipt(&output_dir, &header.file_name).await?
            && receipt.file_size == header.file_size
        {
            return receive_from_receipt(connection, header, receipt, events).await;
        }

        let prepared = prepare_receive_state(
            &output_dir,
            &header,
            resume_state,
            &target_name,
            events,
            self.chunk_size,
        )
        .await?;
        let temp_path = prepared.temp_path;
        let mut file = prepared.file;
        let mut hasher = prepared.hasher;

        send_resume_status(
            connection,
            &header.transfer_id,
            prepared.state.next_chunk_index,
            prepared.state.bytes_received,
            prepared.prefix_hash,
        )
        .await?;

        events.on_event(TransferEvent::Started {
            transfer_id: header.transfer_id.clone(),
            direction: TransferDirection::Receive,
            file_name: header.file_name.clone(),
            total_bytes: header.file_size,
            bytes_resumed: prepared.state.bytes_received,
        });

        let mut expected_index = prepared.state.next_chunk_index;
        let mut expected_offset = prepared.state.bytes_received;
        let mut last_resume_state_bytes = prepared.state.bytes_received;
        events.on_event(TransferEvent::Progress {
            transfer_id: header.transfer_id.clone(),
            bytes_transferred: expected_offset,
            total_bytes: header.file_size,
        });

        loop {
            let frame = match recv_frame_or_cancel(connection, cancel).await {
                Ok(frame) => frame,
                Err(error) => {
                    file.flush().await?;
                    write_resume_state_for_offset(
                        &output_dir,
                        &header,
                        &target_name,
                        expected_offset,
                        expected_index,
                        Some(hasher.finalize().to_hex().to_string()),
                    )
                    .await?;
                    return Err(error);
                }
            };

            match frame {
                Frame::Chunk(chunk) => {
                    if expected_offset > 0 && chunk.index == 0 && chunk.offset == 0 {
                        file.set_len(0).await?;
                        file.flush().await?;
                        expected_index = 0;
                        expected_offset = 0;
                        last_resume_state_bytes = 0;
                        hasher = blake3::Hasher::new();
                        write_resume_state_for_offset(
                            &output_dir,
                            &header,
                            &target_name,
                            0,
                            0,
                            None,
                        )
                        .await?;
                    }
                    validate_chunk(&chunk, &header.transfer_id, expected_index, expected_offset)?;
                    if chunk.bytes.len() as u64 + expected_offset > header.file_size {
                        return Err(CoreError::Transfer(format!(
                            "chunk data exceeds expected file size: chunk offset {} + data length {} > expected file size {}",
                            chunk.offset,
                            chunk.bytes.len(),
                            header.file_size
                        )));
                    }
                    file.write_all(&chunk.bytes).await?;
                    hasher.update(&chunk.bytes);

                    expected_index += 1;
                    expected_offset += chunk.bytes.len() as u64;
                    if expected_offset.saturating_sub(last_resume_state_bytes)
                        >= RESUME_STATE_WRITE_INTERVAL
                    {
                        file.flush().await?;
                        write_resume_state_for_offset(
                            &output_dir,
                            &header,
                            &target_name,
                            expected_offset,
                            expected_index,
                            Some(hasher.finalize().to_hex().to_string()),
                        )
                        .await?;
                        last_resume_state_bytes = expected_offset;
                    }
                    events.on_event(TransferEvent::Progress {
                        transfer_id: header.transfer_id.clone(),
                        bytes_transferred: expected_offset,
                        total_bytes: header.file_size,
                    });
                }
                Frame::Complete(complete) if complete.transfer_id == header.transfer_id => {
                    // Verify + atomically finalize. On ANY failure the transfer did
                    // not succeed, so signal the sender explicitly with an Error
                    // frame before returning: otherwise the sender's close-race
                    // tolerance would take a real failure (size/hash mismatch,
                    // finalize/rename error) for the benign ack-lost-on-close race.
                    // The claim may land under a later name than the one
                    // selected at start (see finalize_received_file).
                    let target_name = match finalize_received_file(
                        &header,
                        &output_dir,
                        &temp_path,
                        &final_path,
                        file,
                        hasher,
                        &complete,
                        expected_offset,
                        expected_index,
                    )
                    .await
                    {
                        Ok(landed) => landed,
                        Err(error) => {
                            notify_error(connection, &error).await;
                            return Err(error);
                        }
                    };
                    // The file is finalized - a durable fact. The ack is
                    // best-effort from here: if the path died, suppressing
                    // Completed would also suppress the mailbox receipt post,
                    // which exists precisely for the lost-ack case.
                    match send_complete_ack(connection, &header.transfer_id).await {
                        Ok(()) => tracing::info!(
                            target: "envoix::timeline",
                            layer = "protocol",
                            event = "complete_ack",
                            outcome = "sent",
                        ),
                        Err(error) => {
                            tracing::info!(
                                target: "envoix::timeline",
                                layer = "protocol",
                                event = "complete_ack",
                                outcome = "failed",
                                cause = %error,
                            );
                            tracing::warn!(
                                %error,
                                "complete ack undeliverable; sender will learn via mailbox"
                            );
                        }
                    }
                    events.on_event(TransferEvent::Completed {
                        transfer_id: header.transfer_id.clone(),
                        file_name: target_name.clone(),
                        bytes_transferred: expected_offset,
                    });

                    return Ok(TransferSummary {
                        transfer_id: header.transfer_id,
                        file_name: target_name,
                        bytes_transferred: expected_offset,
                        file_hash: complete.file_hash,
                    });
                }
                Frame::Error(error) => return Err(peer_error(error)),
                frame => {
                    return Err(CoreError::Transfer(format!(
                        "unexpected frame while receiving chunks: {frame:?}"
                    )));
                }
            }
        }
    }
}

fn expect_ready(frame: Frame) -> Result<(), TransferError> {
    match frame {
        Frame::Ready(_) => Ok(()),
        Frame::Error(error) => Err(peer_error(error)),
        frame => Err(CoreError::Transfer(format!(
            "expected Ready, got {frame:?}"
        ))),
    }
}

fn expect_sender_hello(frame: Frame) -> Result<(), TransferError> {
    match frame {
        Frame::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            role: PeerRole::Sender,
        }) => Ok(()),
        Frame::Error(error) => Err(peer_error(error)),
        frame => Err(CoreError::Transfer(format!(
            "expected sender Hello, got {frame:?}"
        ))),
    }
}

fn expect_file_header(frame: Frame) -> Result<FileHeader, TransferError> {
    match frame {
        Frame::FileHeader(header) => Ok(header),
        Frame::Error(error) => Err(peer_error(error)),
        frame => Err(CoreError::Transfer(format!(
            "expected FileHeader, got {frame:?}"
        ))),
    }
}

fn expect_resume_status(
    frame: Frame,
    transfer_id: &TransferId,
    chunk_size: usize,
) -> Result<ResumeStatus, TransferError> {
    match frame {
        Frame::ResumeStatus(status)
            if &status.transfer_id == transfer_id
                && status.next_chunk_index
                    == next_chunk_index(status.bytes_received, chunk_size as u64) =>
        {
            Ok(status)
        }
        Frame::Error(error) => Err(peer_error(error)),
        frame => Err(CoreError::Transfer(format!(
            "expected valid ResumeStatus for {transfer_id}, got {frame:?}"
        ))),
    }
}

fn expect_complete_ack(frame: Frame, transfer_id: &TransferId) -> Result<(), TransferError> {
    match frame {
        Frame::CompleteAck(ack) if &ack.transfer_id == transfer_id => Ok(()),
        Frame::Error(error) => Err(peer_error(error)),
        frame => Err(CoreError::Transfer(format!(
            "expected CompleteAck for {transfer_id}, got {frame:?}"
        ))),
    }
}

async fn recv_frame_or_cancel(
    connection: &mut dyn FrameConnection,
    cancel: &TransferCancelToken,
) -> Result<Frame, TransferError> {
    tokio::select! {
        frame = connection.recv_frame() => frame,
        () = cancel.cancelled() => {
            notify_interrupted(connection, cancel).await;
            Err(interrupted_error(cancel))
        }
    }
}

async fn recv_frame_or_cancel_with_timeout(
    connection: &mut dyn FrameConnection,
    cancel: &TransferCancelToken,
    timeout: Duration,
) -> Result<Frame, TransferError> {
    tokio::select! {
        frame = connection.recv_frame() => frame,
        () = cancel.cancelled() => {
            notify_interrupted(connection, cancel).await;
            Err(interrupted_error(cancel))
        }
        () = tokio::time::sleep(timeout) => Err(CoreError::Transfer(format!(
            "receiver did not confirm completion within {} seconds; retry may resume the transfer",
            timeout.as_secs()
        ))),
    }
}

async fn check_cancelled(
    connection: &mut dyn FrameConnection,
    cancel: &TransferCancelToken,
) -> Result<(), TransferError> {
    if cancel.is_cancelled() {
        notify_interrupted(connection, cancel).await;
        return Err(interrupted_error(cancel));
    }

    Ok(())
}

/// Verify and atomically finalize a fully-received file: check the byte count and
/// blake3 hash, move the temp file into place, and clear resume state. Any error
/// here means the transfer did NOT succeed (size/hash mismatch, or a finalize /
/// rename / cleanup failure), which the caller signals back to the sender.
// Single call site; the arguments are the receive loop's finalization state and
// grouping them into a struct would only add indirection.
#[allow(clippy::too_many_arguments)]
async fn finalize_received_file(
    header: &FileHeader,
    output_dir: &Path,
    temp_path: &Path,
    final_path: &Path,
    mut file: fs::File,
    hasher: blake3::Hasher,
    complete: &Complete,
    expected_offset: u64,
    expected_index: u64,
) -> Result<String, TransferError> {
    if expected_offset != header.file_size {
        return Err(CoreError::Transfer(format!(
            "transfer complete but expected offset {expected_offset} does not match file size {}",
            header.file_size
        )));
    }
    file.flush().await?;
    // The landing name: differs from header.file_name for a fresh re-receive
    // beside an existing same-name final.
    let mut target_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&header.file_name)
        .to_owned();
    let actual_hash = hasher.finalize().to_hex().to_string();
    write_resume_state_for_offset(
        output_dir,
        header,
        &target_name,
        expected_offset,
        expected_index,
        Some(actual_hash.clone()),
    )
    .await?;
    drop(file);
    if actual_hash != complete.file_hash {
        return Err(CoreError::Transfer(format!(
            "completed file hash {actual_hash} does not match expected {}",
            complete.file_hash
        )));
    }
    // Claim the destination atomically; the name selected at receive start is
    // an observation, not ownership, and the namespace may have moved on
    // (another finalizer, the user, another app). A refused claim takes the
    // next free name instead of failing a completed transfer.
    while !LocalFileStorage::finalize_temp_file(temp_path, &output_dir.join(&target_name)).await? {
        target_name = unique_final_name(output_dir, &header.file_name).await?;
    }
    LocalFileStorage::delete_resume_state(output_dir, &header.file_name, &header.transfer_id)
        .await?;
    // Durable completion receipt: survives the final file being moved/published
    // away, so a re-offer of this file can be re-confirmed (a lost CompleteAck
    // re-delivered) without the file and without re-transfer.
    LocalFileStorage::write_receipt(
        output_dir,
        &TransferReceipt {
            transfer_id: header.transfer_id.clone(),
            file_name: target_name.clone(),
            file_size: header.file_size,
            file_hash: actual_hash,
        },
    )
    .await?;
    Ok(target_name)
}

/// Best-effort notify the peer of a terminal error, so the sender can tell a real
/// failure from a benign disconnect (it arrives as a `Frame::Error`, not a bare
/// connection close).
async fn notify_error(connection: &mut dyn FrameConnection, error: &TransferError) {
    let _ = connection
        .send_frame(Frame::Error(ErrorFrame {
            message: error.to_string(),
        }))
        .await;
}

async fn notify_interrupted(connection: &mut dyn FrameConnection, cancel: &TransferCancelToken) {
    let message = if cancel.is_pause() {
        USER_PAUSE_MESSAGE
    } else {
        USER_INTERRUPT_MESSAGE
    };
    let _ = connection
        .send_frame(Frame::Error(ErrorFrame {
            message: message.into(),
        }))
        .await;
}

fn interrupted_error(cancel: &TransferCancelToken) -> TransferError {
    let message = if cancel.is_pause() {
        USER_PAUSE_MESSAGE
    } else {
        USER_INTERRUPT_MESSAGE
    };
    CoreError::Transfer(message.into())
}

fn peer_error(error: ErrorFrame) -> TransferError {
    if error.message == USER_INTERRUPT_MESSAGE {
        return CoreError::Transfer(PEER_INTERRUPT_MESSAGE.into());
    }
    if error.message == USER_PAUSE_MESSAGE {
        return CoreError::Transfer(PEER_PAUSE_MESSAGE.into());
    }
    CoreError::Transfer(format!("peer reported error: {}", error.message))
}

fn peer_closed_error(error: TransferError) -> TransferError {
    match error {
        CoreError::Io(_) | CoreError::Transport(_) => {
            CoreError::Transfer("connection closed by peer".into())
        }
        error => error,
    }
}

fn validate_chunk(
    chunk: &Chunk,
    transfer_id: &TransferId,
    expected_index: u64,
    expected_offset: u64,
) -> Result<(), TransferError> {
    if &chunk.transfer_id != transfer_id {
        return Err(CoreError::Transfer(format!(
            "chunk transfer id {} does not match {transfer_id}",
            chunk.transfer_id
        )));
    }
    if chunk.index != expected_index {
        return Err(CoreError::Transfer(format!(
            "chunk index {} does not match expected {expected_index}",
            chunk.index
        )));
    }
    if chunk.offset != expected_offset {
        return Err(CoreError::Transfer(format!(
            "chunk offset {} does not match expected {expected_offset}",
            chunk.offset
        )));
    }
    Ok(())
}

async fn write_resume_state_for_offset(
    output_dir: &Path,
    header: &FileHeader,
    target_name: &str,
    bytes_received: u64,
    next_chunk_index: u64,
    hash_checkpoint: Option<String>,
) -> Result<(), TransferError> {
    LocalFileStorage::write_resume_state(
        output_dir,
        &TransferResumeState {
            transfer_id: header.transfer_id.clone(),
            file_name: header.file_name.clone(),
            file_size: header.file_size,
            chunk_size: header.chunk_size,
            bytes_received,
            next_chunk_index,
            hash_bytes: bytes_received,
            hash_checkpoint,
            target_file_name: (target_name != header.file_name).then(|| target_name.to_owned()),
        },
    )
    .await
}

struct PreparedReceive {
    state: TransferResumeState,
    temp_path: PathBuf,
    file: fs::File,
    hasher: blake3::Hasher,
    prefix_hash: String,
}

async fn prepare_receive_state(
    output_dir: &Path,
    header: &FileHeader,
    resume_state: Option<TransferResumeState>,
    target_name: &str,
    events: &dyn EventSink,
    buffer_size: usize,
) -> Result<PreparedReceive, TransferError> {
    if header.chunk_size == 0 {
        return Err(CoreError::Transfer("chunk size must be positive".into()));
    }

    let state = if header.resume_requested {
        match resume_state {
            Some(state) => match prepare_existing_resume_state(output_dir, header, state).await? {
                Some(state) => state,
                None => fresh_resume_state(output_dir, header).await?,
            },
            None => fresh_resume_state(output_dir, header).await?,
        }
    } else {
        fresh_resume_state(output_dir, header).await?
    };

    let temp_path =
        LocalFileStorage::resumable_temp_path(output_dir, &state.file_name, &state.transfer_id)?;
    let mut hasher = blake3::Hasher::new();
    if state.bytes_received > 0 {
        hash_receive_prefix_with_events(
            &temp_path,
            &mut hasher,
            events,
            header,
            state.bytes_received,
            buffer_size,
        )
        .await?;
    }
    let prefix_hash = hasher.finalize().to_hex().to_string();
    write_resume_state_for_offset(
        output_dir,
        header,
        target_name,
        state.bytes_received,
        state.next_chunk_index,
        Some(prefix_hash.clone()),
    )
    .await?;
    let (temp_path, file) =
        LocalFileStorage::open_resumable_destination(output_dir, &state).await?;

    Ok(PreparedReceive {
        state,
        temp_path,
        file,
        hasher,
        prefix_hash,
    })
}

async fn prepare_existing_resume_state(
    output_dir: &Path,
    header: &FileHeader,
    mut state: TransferResumeState,
) -> Result<Option<TransferResumeState>, TransferError> {
    if state.bytes_received > state.file_size {
        tracing::warn!(
            transfer_id = %state.transfer_id,
            file_name = state.file_name,
            bytes_received = state.bytes_received,
            file_size = state.file_size,
            "resume state records more bytes than file size; deleting it"
        );
        delete_resume_candidate(output_dir, &state).await?;
        return Ok(None);
    }
    let expected_next_chunk_index = next_chunk_index(state.bytes_received, state.chunk_size);
    if state.next_chunk_index != expected_next_chunk_index {
        let message = format!(
            "resume state has inconsistent chunk index: next_chunk_index={} expected_next_chunk_index={} bytes_received={} chunk_size={}",
            state.next_chunk_index,
            expected_next_chunk_index,
            state.bytes_received,
            state.chunk_size
        );
        return Err(CoreError::Transfer(message));
    }

    let old_transfer_id = state.transfer_id.clone();
    let old_temp_path =
        LocalFileStorage::resumable_temp_path(output_dir, &state.file_name, &old_transfer_id)?;
    let temp_len = match fs::metadata(&old_temp_path).await {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(CoreError::from(error)),
    };

    if temp_len < state.bytes_received {
        tracing::warn!(
            "resume temp length {temp_len} is shorter than recorded length {}; starting fresh",
            state.bytes_received
        );
        delete_resume_candidate(output_dir, &state).await?;
        return Ok(None);
    }
    if temp_len > state.bytes_received {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&old_temp_path)
            .await?;
        file.set_len(state.bytes_received).await?;
        file.sync_data().await?;
    }

    state.transfer_id = header.transfer_id.clone();
    LocalFileStorage::rebind_resume_temp(
        output_dir,
        &state.file_name,
        &old_transfer_id,
        &state.transfer_id,
    )
    .await?;
    if old_transfer_id != state.transfer_id {
        LocalFileStorage::delete_resume_state(output_dir, &state.file_name, &old_transfer_id)
            .await?;
    }
    state.hash_bytes = 0;
    state.hash_checkpoint = None;
    LocalFileStorage::write_resume_state(output_dir, &state).await?;

    Ok(Some(state))
}

async fn delete_resume_candidate(
    output_dir: &Path,
    state: &TransferResumeState,
) -> Result<(), TransferError> {
    LocalFileStorage::delete_resume_temp(output_dir, &state.file_name, &state.transfer_id).await?;
    LocalFileStorage::delete_resume_state(output_dir, &state.file_name, &state.transfer_id).await
}

/// First `name (n).ext` (n = 1, 2, ...) that does not exist in `output_dir`;
/// used when a fresh receive must land beside an existing same-name final.
async fn unique_final_name(output_dir: &Path, file_name: &str) -> Result<String, TransferError> {
    let (stem, extension) = match file_name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (file_name, None),
    };
    for n in 1..=9999u32 {
        let candidate = match extension {
            Some(extension) => format!("{stem} ({n}).{extension}"),
            None => format!("{stem} ({n})"),
        };
        if !fs::try_exists(output_dir.join(&candidate)).await? {
            return Ok(candidate);
        }
    }
    Err(CoreError::Storage(format!(
        "no free landing name for {file_name}"
    )))
}

async fn fresh_resume_state(
    output_dir: &Path,
    header: &FileHeader,
) -> Result<TransferResumeState, TransferError> {
    let state = TransferResumeState {
        transfer_id: header.transfer_id.clone(),
        file_name: header.file_name.clone(),
        file_size: header.file_size,
        chunk_size: header.chunk_size,
        bytes_received: 0,
        next_chunk_index: 0,
        hash_bytes: 0,
        hash_checkpoint: None,
        target_file_name: None,
    };
    LocalFileStorage::delete_resume_temp(output_dir, &state.file_name, &state.transfer_id).await?;
    LocalFileStorage::write_resume_state(output_dir, &state).await?;
    let temp_path =
        LocalFileStorage::resumable_temp_path(output_dir, &state.file_name, &state.transfer_id)?;
    let file = fs::File::create(temp_path).await?;
    file.sync_data().await?;

    Ok(state)
}

async fn receive_existing_final(
    connection: &mut dyn FrameConnection,
    header: FileHeader,
    final_path: PathBuf,
    events: &dyn EventSink,
) -> Result<TransferSummary, TransferError> {
    let metadata = fs::metadata(&final_path).await?;
    if metadata.len() != header.file_size {
        return Err(CoreError::Storage(format!(
            "destination already exists with different size: {}",
            final_path.display()
        )));
    }

    let final_hash = hash_receive_file_with_events(&final_path, events, &header).await?;

    send_resume_status(
        connection,
        &header.transfer_id,
        next_chunk_index(header.file_size, header.chunk_size),
        header.file_size,
        final_hash.clone(),
    )
    .await?;

    match connection.recv_frame().await? {
        Frame::Complete(complete) if complete.transfer_id == header.transfer_id => {
            if complete.file_hash != final_hash {
                return Err(CoreError::Storage(format!(
                    "destination already exists with different content: {}",
                    final_path.display()
                )));
            }
        }
        Frame::Chunk(chunk)
            if chunk.transfer_id == header.transfer_id && chunk.index == 0 && chunk.offset == 0 =>
        {
            return Err(CoreError::Storage(format!(
                "destination already exists with different content: {}",
                final_path.display()
            )));
        }
        frame => {
            return Err(CoreError::Transfer(format!(
                "unexpected frame for existing destination: {frame:?}"
            )));
        }
    }

    let file_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or(header.file_name.clone());
    // Crash repair BEFORE the ack: finalize commits the file before it writes
    // the receipt, so a death in between leaves possession without proof -
    // and PostReceipt seals from the on-disk receipt, so a missing one makes
    // the confirmation duty undischargeable forever. Recovery must repair,
    // not just read; the write is an idempotent overwrite when all is well.
    if let Some(output_dir) = final_path.parent() {
        LocalFileStorage::write_receipt(
            output_dir,
            &TransferReceipt {
                transfer_id: header.transfer_id.clone(),
                file_name: file_name.clone(),
                file_size: header.file_size,
                file_hash: final_hash.clone(),
            },
        )
        .await?;
    }

    // Possession is already durable; the ack is best-effort (see the
    // receive loop: suppressing Completed here would suppress the mailbox
    // receipt post the lost-ack design depends on).
    match send_complete_ack(connection, &header.transfer_id).await {
        Ok(()) => tracing::info!(
            target: "envoix::timeline",
            layer = "protocol",
            event = "complete_ack",
            outcome = "sent",
        ),
        Err(error) => {
            tracing::info!(
                target: "envoix::timeline",
                layer = "protocol",
                event = "complete_ack",
                outcome = "failed",
                cause = %error,
            );
            tracing::warn!(%error, "complete ack undeliverable; sender will learn via mailbox");
        }
    }

    events.on_event(TransferEvent::Completed {
        transfer_id: header.transfer_id.clone(),
        file_name: file_name.clone(),
        bytes_transferred: header.file_size,
    });

    Ok(TransferSummary {
        transfer_id: header.transfer_id,
        file_name,
        bytes_transferred: header.file_size,
        file_hash: final_hash,
    })
}

/// Serve a re-offer of a file we hold a completion receipt for (the file itself
/// may already be moved/published away): claim "received in full" with the
/// receipt's hash, expect the sender's `Complete`, verify it against the
/// receipt, and ack — re-delivering a lost CompleteAck with zero bytes re-sent.
/// A sender whose content does not match its own claimed prefix restarts from
/// chunk 0 instead of sending `Complete`; that means a DIFFERENT file under
/// this name, which we refuse (mirroring [`receive_existing_final`]).
async fn receive_from_receipt(
    connection: &mut dyn FrameConnection,
    header: FileHeader,
    receipt: TransferReceipt,
    events: &dyn EventSink,
) -> Result<TransferSummary, TransferError> {
    send_resume_status(
        connection,
        &header.transfer_id,
        next_chunk_index(header.file_size, header.chunk_size),
        header.file_size,
        receipt.file_hash.clone(),
    )
    .await?;

    match connection.recv_frame().await? {
        Frame::Complete(complete) if complete.transfer_id == header.transfer_id => {
            if complete.file_hash != receipt.file_hash {
                return Err(CoreError::Storage(format!(
                    "completed earlier with different content: {}",
                    header.file_name
                )));
            }
        }
        Frame::Chunk(chunk)
            if chunk.transfer_id == header.transfer_id && chunk.index == 0 && chunk.offset == 0 =>
        {
            return Err(CoreError::Storage(format!(
                "completed earlier with different content: {}",
                header.file_name
            )));
        }
        Frame::Error(error) => return Err(peer_error(error)),
        frame => {
            return Err(CoreError::Transfer(format!(
                "unexpected frame for receipted file: {frame:?}"
            )));
        }
    }

    match send_complete_ack(connection, &header.transfer_id).await {
        Ok(()) => tracing::info!(
            target: "envoix::timeline",
            layer = "protocol",
            event = "complete_ack",
            outcome = "sent",
        ),
        Err(error) => {
            tracing::info!(
                target: "envoix::timeline",
                layer = "protocol",
                event = "complete_ack",
                outcome = "failed",
                cause = %error,
            );
            tracing::warn!(%error, "complete ack undeliverable; sender will learn via mailbox");
        }
    }

    events.on_event(TransferEvent::Completed {
        transfer_id: header.transfer_id.clone(),
        file_name: receipt.file_name.clone(),
        bytes_transferred: header.file_size,
    });

    Ok(TransferSummary {
        transfer_id: header.transfer_id,
        file_name: receipt.file_name,
        bytes_transferred: header.file_size,
        file_hash: receipt.file_hash,
    })
}

async fn hash_receive_file_with_events(
    path: &Path,
    events: &dyn EventSink,
    header: &FileHeader,
) -> Result<String, TransferError> {
    emit_receive_hash_started(events, header, header.file_size);
    let hash = hash_file(path).await?;
    emit_receive_hash_completed(events, header, header.file_size);
    Ok(hash)
}

async fn hash_receive_prefix_with_events(
    path: &Path,
    hasher: &mut blake3::Hasher,
    events: &dyn EventSink,
    header: &FileHeader,
    bytes_to_hash: u64,
    buffer_size: usize,
) -> Result<(), TransferError> {
    emit_receive_hash_started(events, header, bytes_to_hash);
    let mut file = fs::File::open(path).await?;
    let cancel = TransferCancelToken::new();
    hash_file_prefix(&mut file, hasher, bytes_to_hash, buffer_size, &cancel).await?;
    emit_receive_hash_completed(events, header, bytes_to_hash);
    Ok(())
}

fn emit_receive_hash_started(events: &dyn EventSink, header: &FileHeader, bytes_to_hash: u64) {
    events.on_event(TransferEvent::HashStarted {
        transfer_id: header.transfer_id.clone(),
        direction: TransferDirection::Receive,
        file_name: header.file_name.clone(),
        bytes_to_hash,
    });
}

fn emit_receive_hash_completed(events: &dyn EventSink, header: &FileHeader, bytes_hashed: u64) {
    events.on_event(TransferEvent::HashCompleted {
        transfer_id: header.transfer_id.clone(),
        direction: TransferDirection::Receive,
        file_name: header.file_name.clone(),
        bytes_hashed,
    });
}

async fn send_resume_status(
    connection: &mut dyn FrameConnection,
    transfer_id: &TransferId,
    next_chunk_index: u64,
    bytes_received: u64,
    prefix_hash: String,
) -> Result<(), TransferError> {
    connection
        .send_frame(Frame::ResumeStatus(ResumeStatus {
            transfer_id: transfer_id.clone(),
            next_chunk_index,
            bytes_received,
            prefix_hash,
        }))
        .await
}

async fn send_complete_ack(
    connection: &mut dyn FrameConnection,
    transfer_id: &TransferId,
) -> Result<(), TransferError> {
    connection
        .send_frame(Frame::CompleteAck(CompleteAck {
            transfer_id: transfer_id.clone(),
        }))
        .await
}

async fn hash_file(path: &Path) -> Result<String, TransferError> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; DEFAULT_CHUNK_SIZE];

    loop {
        let bytes_read = file.read(&mut buffer).await?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hasher.finalize().to_hex().to_string())
}

async fn hash_file_prefix(
    file: &mut fs::File,
    hasher: &mut blake3::Hasher,
    bytes_to_hash: u64,
    buffer_size: usize,
    cancel: &TransferCancelToken,
) -> Result<(), TransferError> {
    file.seek(SeekFrom::Start(0)).await?;
    let mut remaining = bytes_to_hash;
    let mut buffer = vec![0_u8; buffer_size.max(1)];
    while remaining > 0 {
        if cancel.is_cancelled() {
            return Err(interrupted_error(cancel));
        }
        let limit = remaining.min(buffer.len() as u64) as usize;
        let bytes_read = file.read(&mut buffer[..limit]).await?;
        if bytes_read == 0 {
            return Err(CoreError::Transfer(format!(
                "unexpected end while hashing prefix: expected {bytes_to_hash} bytes"
            )));
        }
        hasher.update(&buffer[..bytes_read]);
        remaining -= bytes_read as u64;
    }

    Ok(())
}

async fn read_full_chunk<R>(reader: &mut R, buffer: &mut [u8]) -> Result<usize, TransferError>
where
    R: AsyncRead + Unpin,
{
    let mut filled = 0;
    while filled < buffer.len() {
        let bytes_read = reader.read(&mut buffer[filled..]).await?;
        if bytes_read == 0 {
            break;
        }
        filled += bytes_read;
    }
    Ok(filled)
}

fn validate_header(header: &FileHeader, receiver_chunk_size: usize) -> Result<(), TransferError> {
    if receiver_chunk_size == 0 {
        return Err(CoreError::Transfer("chunk size must be positive".into()));
    }
    if header.chunk_size == 0 {
        return Err(CoreError::Transfer("chunk size must be positive".into()));
    }
    if header.chunk_size != receiver_chunk_size as u64 {
        return Err(CoreError::Transfer(format!(
            "sender chunk size {} does not match receiver chunk size {receiver_chunk_size}",
            header.chunk_size
        )));
    }
    LocalFileStorage::resumable_temp_path(Path::new("."), &header.file_name, &header.transfer_id)?;
    Ok(())
}

fn random_transfer_id() -> Result<TransferId, TransferError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| CoreError::Transfer(format!("failed to generate transfer id: {error}")))?;
    Ok(TransferId::new(format!(
        "transfer-{}",
        blake3::hash(&bytes).to_hex()
    )))
}

fn next_chunk_index(bytes_received: u64, chunk_size: u64) -> u64 {
    if bytes_received == 0 {
        0
    } else {
        bytes_received.div_ceil(chunk_size)
    }
}

#[cfg(test)]
mod tests;
