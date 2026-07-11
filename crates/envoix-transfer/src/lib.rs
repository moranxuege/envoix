//! File-transfer state machine.

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
mod chunk_size_validation_tests {
    use super::{MAX_CHUNK_SIZE, MIN_CHUNK_SIZE, validate_chunk_size};

    #[test]
    fn accepts_in_range_powers_of_two() {
        assert!(validate_chunk_size(MIN_CHUNK_SIZE).is_ok());
        assert!(validate_chunk_size(64 * 1024).is_ok());
        assert!(validate_chunk_size(MAX_CHUNK_SIZE).is_ok());
    }

    #[test]
    fn rejects_out_of_range_or_non_power_of_two() {
        assert!(validate_chunk_size(MIN_CHUNK_SIZE - 1).is_err());
        assert!(validate_chunk_size(MAX_CHUNK_SIZE * 2).is_err());
        assert!(validate_chunk_size(24 * 1024).is_err()); // in range, not a power of two
    }
}

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
            file_hash,
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
                    if let Err(error) = finalize_received_file(
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
                        notify_error(connection, &error).await;
                        return Err(error);
                    }
                    // The file is finalized - a durable fact. The ack is
                    // best-effort from here: if the path died, suppressing
                    // Completed would also suppress the mailbox receipt post,
                    // which exists precisely for the lost-ack case.
                    if let Err(error) = send_complete_ack(connection, &header.transfer_id).await {
                        tracing::warn!(
                            %error,
                            "complete ack undeliverable; sender will learn via mailbox"
                        );
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
) -> Result<(), TransferError> {
    if expected_offset != header.file_size {
        return Err(CoreError::Transfer(format!(
            "transfer complete but expected offset {expected_offset} does not match file size {}",
            header.file_size
        )));
    }
    file.flush().await?;
    // The landing name: differs from header.file_name for a fresh re-receive
    // beside an existing same-name final.
    let target_name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&header.file_name);
    let actual_hash = hasher.finalize().to_hex().to_string();
    write_resume_state_for_offset(
        output_dir,
        header,
        target_name,
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
    LocalFileStorage::finalize_temp_file(temp_path, final_path).await?;
    LocalFileStorage::delete_resume_state(output_dir, &header.file_name, &header.transfer_id)
        .await?;
    // Durable completion receipt: survives the final file being moved/published
    // away, so a re-offer of this file can be re-confirmed (a lost CompleteAck
    // re-delivered) without the file and without re-transfer.
    LocalFileStorage::write_receipt(
        output_dir,
        &TransferReceipt {
            file_name: target_name.to_owned(),
            file_size: header.file_size,
            file_hash: actual_hash,
        },
    )
    .await?;
    Ok(())
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
    if let Err(error) = send_complete_ack(connection, &header.transfer_id).await {
        tracing::warn!(%error, "complete ack undeliverable; sender will learn via mailbox");
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

    if let Err(error) = send_complete_ack(connection, &header.transfer_id).await {
        tracing::warn!(%error, "complete ack undeliverable; sender will learn via mailbox");
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
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::io;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn read_full_chunk_accumulates_short_reads() {
        let mut reader = ShortRead {
            bytes: b"abcdef",
            position: 0,
            max_read: 2,
        };
        let mut buffer = [0_u8; 5];

        let bytes_read = read_full_chunk(&mut reader, &mut buffer).await.unwrap();

        assert_eq!(bytes_read, 5);
        assert_eq!(&buffer, b"abcde");

        let mut buffer = [0_u8; 5];
        let bytes_read = read_full_chunk(&mut reader, &mut buffer).await.unwrap();

        assert_eq!(bytes_read, 1);
        assert_eq!(&buffer[..bytes_read], b"f");
    }

    #[tokio::test]
    async fn transfers_file_over_frame_connection() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("hello.txt");
        tokio::fs::write(&source_path, b"hello over frames")
            .await
            .unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(4)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
                    .unwrap()
            }
        });

        let send_summary = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
            .await
            .unwrap();
        let receive_summary = receiver.await.unwrap();

        assert_eq!(send_summary.bytes_transferred, 17);
        assert_eq!(receive_summary.bytes_transferred, 17);
        assert_eq!(
            tokio::fs::read(output_dir.join("hello.txt")).await.unwrap(),
            b"hello over frames"
        );
    }

    /// Complete a transfer of `contents` as `file_name` into `output_dir`.
    async fn complete_transfer_once(source_path: &Path, output_dir: &Path) {
        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.to_path_buf();
            async move {
                TransferEngine::new(4)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
            }
        });
        TransferEngine::new(4)
            .send_file(
                &mut sender_connection,
                source_path.to_path_buf(),
                true,
                &NoopEventSink,
            )
            .await
            .unwrap();
        receiver.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn finalize_writes_completion_receipt() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("receipt.txt");
        tokio::fs::write(&source_path, b"receipt me").await.unwrap();

        complete_transfer_once(&source_path, &output_dir).await;

        let receipt = LocalFileStorage::read_receipt(&output_dir, "receipt.txt")
            .await
            .unwrap()
            .expect("finalize writes a receipt");
        assert_eq!(receipt.file_name, "receipt.txt");
        assert_eq!(receipt.file_size, 10);
        assert_eq!(
            receipt.file_hash,
            blake3::hash(b"receipt me").to_hex().to_string()
        );
    }

    #[tokio::test]
    async fn reoffer_with_receipt_completes_without_resend() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("moved.bin");
        tokio::fs::write(&source_path, b"published and moved away")
            .await
            .unwrap();

        complete_transfer_once(&source_path, &output_dir).await;
        // The app publishes the file elsewhere and deletes the output copy
        // (what Android does); only the receipt remains.
        tokio::fs::remove_file(output_dir.join("moved.bin"))
            .await
            .unwrap();

        // Re-offer: both sides complete, re-delivering the CompleteAck...
        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(4)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
            }
        });
        let send_summary = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
            .await
            .expect("re-offer against a receipt completes");
        let receive_summary = receiver
            .await
            .unwrap()
            .expect("receipted receive completes");
        assert_eq!(send_summary.bytes_transferred, 24);
        assert_eq!(receive_summary.bytes_transferred, 24);

        // ...without recreating the file or writing any temp — zero bytes moved.
        let mut entries = tokio::fs::read_dir(&output_dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                name.starts_with(".envoix-receipt."),
                "unexpected file recreated by receipted re-offer: {name}"
            );
        }
    }

    /// Field bug: a receipt from an EARLIER completed transfer must not
    /// pre-empt the resume of a NEW in-flight partial of the same file.
    #[tokio::test]
    async fn receipt_does_not_preempt_an_in_flight_partial() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("again.bin");
        let content = b"resume beats receipt";
        tokio::fs::write(&source_path, content).await.unwrap();

        // Transfer once: receipt written; then the file is published away.
        complete_transfer_once(&source_path, &output_dir).await;
        tokio::fs::remove_file(output_dir.join("again.bin"))
            .await
            .unwrap();

        // A NEW transfer of the same file is mid-flight: plant its partial
        // (first 8 bytes) + resume state, as a pause would leave them.
        let tid = TransferId::new("transfer-partial");
        let state = TransferResumeState {
            transfer_id: tid.clone(),
            file_name: "again.bin".into(),
            file_size: content.len() as u64,
            chunk_size: 4,
            bytes_received: 8,
            next_chunk_index: 2,
            hash_bytes: 0,
            hash_checkpoint: None,
            target_file_name: None,
        };
        LocalFileStorage::write_resume_state(&output_dir, &state)
            .await
            .unwrap();
        let temp = LocalFileStorage::resumable_temp_path(&output_dir, "again.bin", &tid).unwrap();
        tokio::fs::write(&temp, &content[..8]).await.unwrap();

        // Resume: the partial must continue (producing the real file), not the
        // receipt path (which produces nothing).
        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(4)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
            }
        });
        let summary = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
            .await
            .expect("resume completes");
        receiver.await.unwrap().expect("receive completes");
        assert_eq!(summary.bytes_transferred, content.len() as u64);
        assert_eq!(
            tokio::fs::read(output_dir.join("again.bin")).await.unwrap(),
            content,
            "the partial path must produce the real file - the receipt path produces none"
        );
    }

    #[tokio::test]
    async fn reoffer_with_different_content_is_refused() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("swap.bin");
        tokio::fs::write(&source_path, b"original content")
            .await
            .unwrap();

        complete_transfer_once(&source_path, &output_dir).await;
        tokio::fs::remove_file(output_dir.join("swap.bin"))
            .await
            .unwrap();
        // Same name, same size, different bytes.
        tokio::fs::write(&source_path, b"altered contents")
            .await
            .unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(4)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
            }
        });
        let send_result = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
            .await;
        let receive_result = receiver.await.unwrap();

        assert!(send_result.is_err(), "sender must not report success");
        let error = receive_result.unwrap_err();
        assert!(
            matches!(&error, CoreError::Storage(m) if m.contains("different content")),
            "unexpected receiver error: {error:?}"
        );
    }

    #[tokio::test]
    async fn fresh_send_ignores_receipt_and_retransfers() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("fresh.bin");
        tokio::fs::write(&source_path, b"fresh again")
            .await
            .unwrap();

        complete_transfer_once(&source_path, &output_dir).await;
        tokio::fs::remove_file(output_dir.join("fresh.bin"))
            .await
            .unwrap();

        // resume=false (--fresh): the receipt is ignored, the file re-transfers.
        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(4)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
            }
        });
        TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
            .await
            .unwrap();
        receiver.await.unwrap().unwrap();
        assert_eq!(
            tokio::fs::read(output_dir.join("fresh.bin")).await.unwrap(),
            b"fresh again"
        );
    }

    #[tokio::test]
    async fn rejects_sender_receiver_chunk_size_mismatch() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("mismatch.txt");
        tokio::fs::write(&source_path, b"chunk size mismatch")
            .await
            .unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(8)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
            }
        });

        let send_error = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
            .await
            .unwrap_err();
        let receive_error = receiver.await.unwrap().unwrap_err();

        assert!(matches!(
            send_error,
            CoreError::Transport(_) | CoreError::Transfer(_)
        ));
        assert!(matches!(receive_error, CoreError::Transfer(_)));
        assert!(
            !fs::try_exists(output_dir.join("mismatch.txt"))
                .await
                .unwrap()
        );
        assert_no_sidecars(&output_dir).await;
    }

    #[tokio::test]
    async fn resumes_after_receiver_stops_mid_transfer() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("resume.txt");
        tokio::fs::write(&source_path, b"resume over two connections")
            .await
            .unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let stopped = std::sync::Arc::new(AtomicBool::new(false));
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            let stopped = stopped.clone();
            async move {
                TransferEngine::new(4)
                    .receive_file(
                        &mut receiver_connection,
                        output_dir,
                        &StopAfterBytesSink { bytes: 8, stopped },
                    )
                    .await
            }
        });

        let send_error = TransferEngine::new(4)
            .send_file(
                &mut sender_connection,
                source_path.clone(),
                false,
                &NoopEventSink,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            send_error,
            CoreError::Transport(_) | CoreError::Transfer(_)
        ));
        match receiver.await {
            Ok(result) => assert!(result.is_err() || stopped.load(Ordering::SeqCst)),
            Err(_) => assert!(stopped.load(Ordering::SeqCst)),
        }

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(4)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
                    .unwrap()
            }
        });

        let send_summary = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
            .await
            .unwrap();
        let receive_summary = receiver.await.unwrap();

        assert_eq!(send_summary.bytes_transferred, 27);
        assert_eq!(receive_summary.bytes_transferred, 27);
        assert_eq!(
            tokio::fs::read(output_dir.join("resume.txt"))
                .await
                .unwrap(),
            b"resume over two connections"
        );
    }

    #[tokio::test]
    async fn sender_fails_when_receiver_drops_before_ack() {
        // The CompleteAck is the receiver's proof that it finalized, so the sender
        // requires it. If the receiver reads Complete but drops without acking
        // (a crash / network death before finalizing), the sender must report
        // failure - never a false success. In the healthy path this cannot happen:
        // the receiver holds the connection open until the sender closes, so the
        // ack is delivered; the rare true-network-death case is recovered by resume
        // on a retry.
        let root = unique_test_dir();
        let source_dir = root.join("source");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("race.txt");
        tokio::fs::write(&source_path, b"receiver vanishes before acking")
            .await
            .unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn(async move {
            let transfer_id = receive_header_and_resume(&mut receiver_connection).await;
            // Drain chunks, then drop the connection on Complete WITHOUT acking.
            loop {
                match receiver_connection.recv_frame().await.unwrap() {
                    Frame::Chunk(_) => {}
                    Frame::Complete(complete) => {
                        assert_eq!(complete.transfer_id, transfer_id);
                        break;
                    }
                    other => panic!("unexpected frame while draining: {other:?}"),
                }
            }
            // receiver_connection is dropped here without a CompleteAck.
        });

        let result = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
            .await;
        assert!(
            result.is_err(),
            "sender must fail when the receiver never sends CompleteAck"
        );
        receiver.await.unwrap();
    }

    #[tokio::test]
    async fn sender_times_out_when_receiver_never_acks_but_keeps_connection_open() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("stalled-ack.txt");
        tokio::fs::write(&source_path, b"receiver stalls before acking")
            .await
            .unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn(async move {
            let transfer_id = receive_header_and_resume(&mut receiver_connection).await;
            loop {
                match receiver_connection.recv_frame().await.unwrap() {
                    Frame::Chunk(_) => {}
                    Frame::Complete(complete) => {
                        assert_eq!(complete.transfer_id, transfer_id);
                        break;
                    }
                    other => panic!("unexpected frame while draining: {other:?}"),
                }
            }
            let _hold_connection = receiver_connection;
            std::future::pending::<()>().await;
        });

        let error = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            CoreError::Transfer(message) if message.contains("did not confirm completion")
        ));
        receiver.abort();
        let _ = receiver.await;
    }

    #[tokio::test]
    async fn sender_fails_when_receiver_reports_error_after_complete() {
        // A genuine post-Complete failure on the receiver (hash mismatch, finalize
        // error, ...) is signaled with an Error frame; the sender must surface it
        // as a failure - not swallow it like the benign ack-lost close race.
        let root = unique_test_dir();
        let source_dir = root.join("source");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("rejected.txt");
        tokio::fs::write(&source_path, b"payload the receiver will reject")
            .await
            .unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn(async move {
            let transfer_id = receive_header_and_resume(&mut receiver_connection).await;
            loop {
                match receiver_connection.recv_frame().await.unwrap() {
                    Frame::Chunk(_) => {}
                    Frame::Complete(complete) => {
                        assert_eq!(complete.transfer_id, transfer_id);
                        break;
                    }
                    other => panic!("unexpected frame while draining: {other:?}"),
                }
            }
            // Simulate a receiver-side finalize/verify failure after Complete.
            receiver_connection
                .send_frame(Frame::Error(ErrorFrame {
                    message: "completed file hash mismatch".into(),
                }))
                .await
                .unwrap();
        });

        let result = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
            .await;
        assert!(
            result.is_err(),
            "sender must fail when the receiver reports a post-Complete error"
        );
        receiver.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_notifies_peer_with_error_frame() {
        let root = unique_test_dir();
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let cancel = TransferCancelToken::new();
        let receiver_cancel = cancel.clone();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(4)
                    .receive_file_with_cancel(
                        &mut receiver_connection,
                        output_dir,
                        &NoopEventSink,
                        &receiver_cancel,
                    )
                    .await
            }
        });

        let transfer_id = TransferId::new("cancel-transfer");
        sender_connection
            .send_frame(Frame::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                role: PeerRole::Sender,
            }))
            .await
            .unwrap();
        expect_ready(sender_connection.recv_frame().await.unwrap()).unwrap();
        sender_connection
            .send_frame(Frame::FileHeader(FileHeader {
                transfer_id: transfer_id.clone(),
                file_name: "cancel.txt".into(),
                file_size: 8,
                chunk_size: 4,
                resume_requested: false,
            }))
            .await
            .unwrap();
        expect_resume_status(
            sender_connection.recv_frame().await.unwrap(),
            &transfer_id,
            4,
        )
        .unwrap();

        cancel.cancel();

        let frame = sender_connection.recv_frame().await.unwrap();
        assert!(matches!(
            frame,
            Frame::Error(ErrorFrame { message }) if message == USER_INTERRUPT_MESSAGE
        ));
        let error = receiver.await.unwrap().unwrap_err();
        assert!(matches!(
            error,
            CoreError::Transfer(message) if message == USER_INTERRUPT_MESSAGE
        ));
    }

    #[tokio::test]
    async fn pause_notifies_peer_with_pause_message() {
        let root = unique_test_dir();
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let cancel = TransferCancelToken::new();
        let receiver_cancel = cancel.clone();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(4)
                    .receive_file_with_cancel(
                        &mut receiver_connection,
                        output_dir,
                        &NoopEventSink,
                        &receiver_cancel,
                    )
                    .await
            }
        });

        let transfer_id = TransferId::new("pause-transfer");
        sender_connection
            .send_frame(Frame::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                role: PeerRole::Sender,
            }))
            .await
            .unwrap();
        expect_ready(sender_connection.recv_frame().await.unwrap()).unwrap();
        sender_connection
            .send_frame(Frame::FileHeader(FileHeader {
                transfer_id: transfer_id.clone(),
                file_name: "pause.txt".into(),
                file_size: 8,
                chunk_size: 4,
                resume_requested: false,
            }))
            .await
            .unwrap();
        expect_resume_status(
            sender_connection.recv_frame().await.unwrap(),
            &transfer_id,
            4,
        )
        .unwrap();

        cancel.pause();

        // The peer learns it was a pause, not a cancel (best-effort frame).
        let frame = sender_connection.recv_frame().await.unwrap();
        assert!(matches!(
            frame,
            Frame::Error(ErrorFrame { message }) if message == USER_PAUSE_MESSAGE
        ));
        // Locally the stop is reported as a pause too.
        let error = receiver.await.unwrap().unwrap_err();
        assert!(matches!(
            error,
            CoreError::Transfer(message) if message == USER_PAUSE_MESSAGE
        ));
    }

    #[tokio::test]
    async fn peer_pause_message_maps_to_paused_by_peer() {
        // peer_error turns the peer's pause frame into the canonical local text.
        let error = peer_error(ErrorFrame {
            message: USER_PAUSE_MESSAGE.into(),
        });
        assert!(matches!(
            error,
            CoreError::Transfer(message) if message == PEER_PAUSE_MESSAGE
        ));
    }

    #[tokio::test]
    async fn sender_reports_explicit_peer_interrupt() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("peer-interrupt.txt");
        tokio::fs::write(&source_path, b"peer interrupt")
            .await
            .unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn(async move {
            let transfer_id = receive_header_and_resume(&mut receiver_connection).await;
            loop {
                match receiver_connection.recv_frame().await.unwrap() {
                    Frame::Chunk(_) => {}
                    Frame::Complete(_) => {
                        receiver_connection
                            .send_frame(Frame::Error(ErrorFrame {
                                message: USER_INTERRUPT_MESSAGE.into(),
                            }))
                            .await
                            .unwrap();
                        break transfer_id;
                    }
                    frame => panic!("unexpected frame: {frame:?}"),
                }
            }
        });

        let error = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
            .await
            .unwrap_err();

        receiver.await.unwrap();
        assert!(matches!(
            error,
            CoreError::Transfer(message) if message == "transfer interrupted by peer"
        ));
    }

    #[tokio::test]
    async fn sender_reports_peer_close_during_send() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        let source_path = source_dir.join("peer-close.txt");
        tokio::fs::write(&source_path, b"peer closed while sending")
            .await
            .unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn(async move {
            receive_header_and_resume(&mut receiver_connection).await;
        });

        let error = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
            .await
            .unwrap_err();

        receiver.await.unwrap();
        assert!(matches!(
            error,
            CoreError::Transfer(message) if message == "connection closed by peer"
        ));
    }

    #[tokio::test]
    async fn corrupted_temp_prefix_restarts_from_zero() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        let source_path = source_dir.join("corrupt.txt");
        let source_bytes = b"abcdefghij";
        tokio::fs::write(&source_path, source_bytes).await.unwrap();

        let transfer_id = TransferId::new("old-transfer");
        let state = TransferResumeState {
            transfer_id: transfer_id.clone(),
            file_name: "corrupt.txt".into(),
            file_size: source_bytes.len() as u64,
            chunk_size: 5,
            bytes_received: 5,
            next_chunk_index: 1,
            hash_bytes: 5,
            hash_checkpoint: Some(blake3::hash(b"abcde").to_hex().to_string()),
            target_file_name: None,
        };
        LocalFileStorage::write_resume_state(&output_dir, &state)
            .await
            .unwrap();
        let temp_path =
            LocalFileStorage::resumable_temp_path(&output_dir, "corrupt.txt", &transfer_id)
                .unwrap();
        tokio::fs::write(&temp_path, b"xxxxx").await.unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(5)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
                    .unwrap()
            }
        });

        let send_summary = TransferEngine::new(5)
            .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
            .await
            .unwrap();
        let receive_summary = receiver.await.unwrap();

        assert_eq!(send_summary.bytes_transferred, source_bytes.len() as u64);
        assert_eq!(receive_summary.bytes_transferred, source_bytes.len() as u64);
        assert_eq!(
            tokio::fs::read(output_dir.join("corrupt.txt"))
                .await
                .unwrap(),
            source_bytes
        );
        assert!(!fs::try_exists(temp_path).await.unwrap());
    }

    #[tokio::test]
    async fn resumes_from_temp_file_when_sidecar_offset_is_stale() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        let source_path = source_dir.join("stale-sidecar.txt");
        let source_bytes = b"abcdefghij";
        tokio::fs::write(&source_path, source_bytes).await.unwrap();

        let transfer_id = TransferId::new("old-transfer");
        let state = TransferResumeState {
            transfer_id: transfer_id.clone(),
            file_name: "stale-sidecar.txt".into(),
            file_size: source_bytes.len() as u64,
            chunk_size: 5,
            bytes_received: 0,
            next_chunk_index: 0,
            hash_bytes: 0,
            hash_checkpoint: None,
            target_file_name: None,
        };
        LocalFileStorage::write_resume_state(&output_dir, &state)
            .await
            .unwrap();
        let temp_path =
            LocalFileStorage::resumable_temp_path(&output_dir, "stale-sidecar.txt", &transfer_id)
                .unwrap();
        tokio::fs::write(&temp_path, b"abcde").await.unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(5)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
                    .unwrap()
            }
        });

        let send_summary = TransferEngine::new(5)
            .send_file(&mut sender_connection, source_path, true, &NoopEventSink)
            .await
            .unwrap();
        let receive_summary = receiver.await.unwrap();

        assert_eq!(send_summary.bytes_transferred, 10);
        assert_eq!(receive_summary.bytes_transferred, 10);
        assert_eq!(
            tokio::fs::read(output_dir.join("stale-sidecar.txt"))
                .await
                .unwrap(),
            source_bytes
        );
    }

    #[tokio::test]
    async fn no_resume_ignores_compatible_sidecar() {
        let root = unique_test_dir();
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        let source_bytes = b"abcdefghij";
        let old_transfer_id = TransferId::new("old-transfer");
        let state = TransferResumeState {
            transfer_id: old_transfer_id.clone(),
            file_name: "fresh.txt".into(),
            file_size: source_bytes.len() as u64,
            chunk_size: 5,
            bytes_received: 5,
            next_chunk_index: 1,
            hash_bytes: 5,
            hash_checkpoint: Some(blake3::hash(b"abcde").to_hex().to_string()),
            target_file_name: None,
        };
        LocalFileStorage::write_resume_state(&output_dir, &state)
            .await
            .unwrap();
        let temp_path =
            LocalFileStorage::resumable_temp_path(&output_dir, "fresh.txt", &old_transfer_id)
                .unwrap();
        tokio::fs::write(&temp_path, b"abcde").await.unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(5)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
                    .unwrap()
            }
        });

        manual_send(
            &mut sender_connection,
            ManualSend {
                file_name: "fresh.txt",
                source_bytes,
                chunk_size: 5,
                resume_requested: false,
                bytes_to_send: source_bytes,
                complete_hash: blake3::hash(source_bytes).to_hex().to_string(),
                expected_resume_bytes: 0,
            },
        )
        .await;
        let receive_summary = receiver.await.unwrap();

        assert_eq!(receive_summary.bytes_transferred, source_bytes.len() as u64);
        assert_eq!(
            tokio::fs::read(output_dir.join("fresh.txt")).await.unwrap(),
            source_bytes
        );
    }

    #[tokio::test]
    async fn temp_shorter_than_state_rejects_resume_candidate() {
        let root = unique_test_dir();
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        let source_bytes = b"abcdefghij";
        let old_transfer_id = TransferId::new("old-transfer");
        let state = TransferResumeState {
            transfer_id: old_transfer_id.clone(),
            file_name: "short-temp.txt".into(),
            file_size: source_bytes.len() as u64,
            chunk_size: 5,
            bytes_received: 5,
            next_chunk_index: 1,
            hash_bytes: 5,
            hash_checkpoint: Some(blake3::hash(b"abcde").to_hex().to_string()),
            target_file_name: None,
        };
        LocalFileStorage::write_resume_state(&output_dir, &state)
            .await
            .unwrap();
        let temp_path =
            LocalFileStorage::resumable_temp_path(&output_dir, "short-temp.txt", &old_transfer_id)
                .unwrap();
        tokio::fs::write(&temp_path, b"abc").await.unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(5)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
                    .unwrap()
            }
        });

        manual_send(
            &mut sender_connection,
            ManualSend {
                file_name: "short-temp.txt",
                source_bytes,
                chunk_size: 5,
                resume_requested: true,
                bytes_to_send: source_bytes,
                complete_hash: blake3::hash(source_bytes).to_hex().to_string(),
                expected_resume_bytes: 0,
            },
        )
        .await;
        let receive_summary = receiver.await.unwrap();

        assert_eq!(receive_summary.bytes_transferred, source_bytes.len() as u64);
        assert_eq!(
            tokio::fs::read(output_dir.join("short-temp.txt"))
                .await
                .unwrap(),
            source_bytes
        );
        assert!(
            LocalFileStorage::read_resume_state(&output_dir, "short-temp.txt", &old_transfer_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(!fs::try_exists(temp_path).await.unwrap());
    }

    #[tokio::test]
    async fn inconsistent_resume_index_fails_explicitly() {
        let root = unique_test_dir();
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        let source_bytes = b"abcdefghij";
        let old_transfer_id = TransferId::new("old-transfer");
        let state = TransferResumeState {
            transfer_id: old_transfer_id.clone(),
            file_name: "bad-state.txt".into(),
            file_size: source_bytes.len() as u64,
            chunk_size: 5,
            bytes_received: 5,
            next_chunk_index: 7,
            hash_bytes: 5,
            hash_checkpoint: Some(blake3::hash(b"abcde").to_hex().to_string()),
            target_file_name: None,
        };
        LocalFileStorage::write_resume_state(&output_dir, &state)
            .await
            .unwrap();
        let temp_path =
            LocalFileStorage::resumable_temp_path(&output_dir, "bad-state.txt", &old_transfer_id)
                .unwrap();
        tokio::fs::write(&temp_path, b"abcde").await.unwrap();

        let header = FileHeader {
            transfer_id: TransferId::new("new-transfer"),
            file_name: "bad-state.txt".into(),
            file_size: source_bytes.len() as u64,
            chunk_size: 5,
            resume_requested: true,
        };
        let error = prepare_existing_resume_state(&output_dir, &header, state)
            .await
            .unwrap_err();

        assert!(
            matches!(error, CoreError::Transfer(message) if message.contains("inconsistent chunk index"))
        );
        assert!(
            LocalFileStorage::read_resume_state(&output_dir, "bad-state.txt", &old_transfer_id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(fs::try_exists(temp_path).await.unwrap());
    }

    #[tokio::test]
    async fn final_hash_mismatch_does_not_finalize_file() {
        let root = unique_test_dir();
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        let source_bytes = b"abcdefghij";

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(5)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
            }
        });

        manual_send(
            &mut sender_connection,
            ManualSend {
                file_name: "bad-hash.txt",
                source_bytes,
                chunk_size: 5,
                resume_requested: false,
                bytes_to_send: source_bytes,
                complete_hash: "not-the-right-hash".into(),
                expected_resume_bytes: 0,
            },
        )
        .await;
        let receive_error = receiver.await.unwrap().unwrap_err();

        assert!(matches!(receive_error, CoreError::Transfer(_)));
        assert!(
            !fs::try_exists(output_dir.join("bad-hash.txt"))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn already_complete_matching_file_returns_success() {
        let root = unique_test_dir();
        let source_dir = root.join("source");
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        let source_path = source_dir.join("done.txt");
        tokio::fs::write(&source_path, b"already done")
            .await
            .unwrap();
        tokio::fs::write(output_dir.join("done.txt"), b"already done")
            .await
            .unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(4)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
                    .unwrap()
            }
        });

        let send_summary = TransferEngine::new(4)
            .send_file(&mut sender_connection, source_path, false, &NoopEventSink)
            .await
            .unwrap();
        let receive_summary = receiver.await.unwrap();

        assert_eq!(send_summary.bytes_transferred, 12);
        assert_eq!(receive_summary.bytes_transferred, 12);
    }

    /// Lost-ack window: the sender vanishes right after Complete, before the
    /// ack can be delivered. The file is finalized (a durable fact), so the
    /// receive must still complete - suppressing Completed here would also
    /// suppress the mailbox receipt post that exists for exactly this case.
    #[tokio::test]
    async fn receiver_completes_when_the_ack_cannot_be_delivered() {
        let root = unique_test_dir();
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        let source_bytes = b"abcdefghij";

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(5)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
            }
        });

        let transfer_id = TransferId::new("manual-transfer");
        sender_connection
            .send_frame(Frame::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                role: PeerRole::Sender,
            }))
            .await
            .unwrap();
        expect_ready(sender_connection.recv_frame().await.unwrap()).unwrap();
        sender_connection
            .send_frame(Frame::FileHeader(FileHeader {
                transfer_id: transfer_id.clone(),
                file_name: "data.bin".into(),
                file_size: source_bytes.len() as u64,
                chunk_size: 5,
                resume_requested: false,
            }))
            .await
            .unwrap();
        expect_resume_status(
            sender_connection.recv_frame().await.unwrap(),
            &transfer_id,
            5,
        )
        .unwrap();
        for (index, chunk) in source_bytes.chunks(5).enumerate() {
            sender_connection
                .send_frame(Frame::Chunk(Chunk {
                    transfer_id: transfer_id.clone(),
                    index: index as u64,
                    offset: index as u64 * 5,
                    bytes: chunk.to_vec(),
                }))
                .await
                .unwrap();
        }
        sender_connection
            .send_frame(Frame::Complete(Complete {
                transfer_id: transfer_id.clone(),
                file_hash: blake3::hash(source_bytes).to_hex().to_string(),
            }))
            .await
            .unwrap();
        // The sender dies without reading the ack.
        drop(sender_connection);

        let summary = receiver
            .await
            .unwrap()
            .expect("finalized receive must complete");
        assert_eq!(summary.bytes_transferred, source_bytes.len() as u64);
        assert_eq!(
            tokio::fs::read(output_dir.join("data.bin")).await.unwrap(),
            source_bytes
        );
    }

    /// Crash-repair: finalize commits the file before the receipt, so a death
    /// in between leaves possession without proof. The existing-final recovery
    /// path must recreate the receipt - PostReceipt seals from it, and without
    /// it the confirmation duty is undischargeable forever.
    #[tokio::test]
    async fn existing_final_recovery_repairs_a_missing_receipt() {
        let root = unique_test_dir();
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        let source_bytes = b"abcdefghij";
        // The crash left the final file but no receipt sidecar.
        tokio::fs::write(output_dir.join("data.bin"), source_bytes)
            .await
            .unwrap();
        assert!(
            LocalFileStorage::read_receipt(&output_dir, "data.bin")
                .await
                .unwrap()
                .is_none()
        );

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(5)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
                    .unwrap()
            }
        });

        manual_send(
            &mut sender_connection,
            ManualSend {
                file_name: "data.bin",
                source_bytes,
                chunk_size: 5,
                resume_requested: true,
                bytes_to_send: source_bytes,
                complete_hash: blake3::hash(source_bytes).to_hex().to_string(),
                expected_resume_bytes: source_bytes.len() as u64,
            },
        )
        .await;
        receiver.await.unwrap();

        let receipt = LocalFileStorage::read_receipt(&output_dir, "data.bin")
            .await
            .unwrap()
            .expect("recovery repaired the receipt");
        assert_eq!(
            receipt.file_hash,
            blake3::hash(source_bytes).to_hex().to_string()
        );
    }

    /// Field bug (rooms 223606/135499): a fresh send of an already-present
    /// file "completed" in 308ms off the existing final - the fresh request
    /// was silently ignored. A fresh offer must move real bytes and land
    /// beside the original under a free name.
    #[tokio::test]
    async fn fresh_offer_beside_existing_final_lands_real_copy_under_new_name() {
        let root = unique_test_dir();
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        let source_bytes = b"abcdefghij";
        tokio::fs::write(output_dir.join("data.bin"), source_bytes)
            .await
            .unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(5)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
                    .unwrap()
            }
        });

        manual_send(
            &mut sender_connection,
            ManualSend {
                file_name: "data.bin",
                source_bytes,
                chunk_size: 5,
                resume_requested: false,
                bytes_to_send: source_bytes,
                complete_hash: blake3::hash(source_bytes).to_hex().to_string(),
                expected_resume_bytes: 0,
            },
        )
        .await;
        let receive_summary = receiver.await.unwrap();

        assert_eq!(
            receive_summary.bytes_transferred,
            source_bytes.len() as u64,
            "fresh offer must transfer real bytes, not answer from the final"
        );
        assert_eq!(
            tokio::fs::read(output_dir.join("data (1).bin"))
                .await
                .unwrap(),
            source_bytes,
            "fresh copy lands beside the original under a uniquified name"
        );
        assert_eq!(
            tokio::fs::read(output_dir.join("data.bin")).await.unwrap(),
            source_bytes,
            "the original file is untouched"
        );
        let receipt = LocalFileStorage::read_receipt(&output_dir, "data (1).bin")
            .await
            .unwrap()
            .expect("receipt is keyed by the file that actually exists");
        assert_eq!(receipt.file_size, source_bytes.len() as u64);
    }

    /// The pause+resume half of the same bug: an interrupted fresh re-receive
    /// records its landing name, and the resumed attempt (resume_requested is
    /// true on attempt 2+) must continue into that name - not be instantly
    /// answered by the same-name final that made it uniquify in the first place.
    #[tokio::test]
    async fn interrupted_fresh_offer_resumes_into_recorded_target() {
        let root = unique_test_dir();
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        let source_bytes = b"abcdefghij";
        tokio::fs::write(output_dir.join("data.bin"), source_bytes)
            .await
            .unwrap();
        // The state an interrupted fresh attempt leaves behind: 5 of 10 bytes
        // in the temp, landing name recorded.
        let old_transfer_id = TransferId::new("old-transfer");
        let state = TransferResumeState {
            transfer_id: old_transfer_id.clone(),
            file_name: "data.bin".into(),
            file_size: source_bytes.len() as u64,
            chunk_size: 5,
            bytes_received: 5,
            next_chunk_index: 1,
            hash_bytes: 5,
            hash_checkpoint: Some(blake3::hash(b"abcde").to_hex().to_string()),
            target_file_name: Some("data (1).bin".into()),
        };
        LocalFileStorage::write_resume_state(&output_dir, &state)
            .await
            .unwrap();
        let temp_path =
            LocalFileStorage::resumable_temp_path(&output_dir, "data.bin", &old_transfer_id)
                .unwrap();
        tokio::fs::write(&temp_path, b"abcde").await.unwrap();

        let (mut sender_connection, mut receiver_connection) = memory_connection_pair();
        let receiver = tokio::spawn({
            let output_dir = output_dir.clone();
            async move {
                TransferEngine::new(5)
                    .receive_file(&mut receiver_connection, output_dir, &NoopEventSink)
                    .await
                    .unwrap()
            }
        });

        manual_send(
            &mut sender_connection,
            ManualSend {
                file_name: "data.bin",
                source_bytes,
                chunk_size: 5,
                resume_requested: true,
                bytes_to_send: source_bytes,
                complete_hash: blake3::hash(source_bytes).to_hex().to_string(),
                expected_resume_bytes: 5,
            },
        )
        .await;
        let receive_summary = receiver.await.unwrap();

        assert_eq!(receive_summary.bytes_transferred, source_bytes.len() as u64);
        assert_eq!(
            tokio::fs::read(output_dir.join("data (1).bin"))
                .await
                .unwrap(),
            source_bytes,
            "resume continues into the recorded landing name"
        );
        assert_eq!(
            tokio::fs::read(output_dir.join("data.bin")).await.unwrap(),
            source_bytes,
            "the original file is untouched"
        );
    }

    #[tokio::test]
    async fn unique_final_name_skips_taken_names() {
        let root = unique_test_dir();
        let output_dir = root.join("output");
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        tokio::fs::write(output_dir.join("a.txt"), b"x")
            .await
            .unwrap();
        tokio::fs::write(output_dir.join("a (1).txt"), b"x")
            .await
            .unwrap();
        assert_eq!(
            unique_final_name(&output_dir, "a.txt").await.unwrap(),
            "a (2).txt"
        );
        assert_eq!(
            unique_final_name(&output_dir, "noext").await.unwrap(),
            "noext (1)"
        );
        assert_eq!(
            unique_final_name(&output_dir, ".dotfile").await.unwrap(),
            ".dotfile (1)"
        );
    }

    struct MemoryFrameConnection {
        tx: mpsc::Sender<Frame>,
        rx: mpsc::Receiver<Frame>,
    }

    fn memory_connection_pair() -> (MemoryFrameConnection, MemoryFrameConnection) {
        let (sender_tx, receiver_rx) = mpsc::channel(16);
        let (receiver_tx, sender_rx) = mpsc::channel(16);

        (
            MemoryFrameConnection {
                tx: sender_tx,
                rx: sender_rx,
            },
            MemoryFrameConnection {
                tx: receiver_tx,
                rx: receiver_rx,
            },
        )
    }

    struct ManualSend<'a> {
        file_name: &'a str,
        source_bytes: &'a [u8],
        chunk_size: u64,
        resume_requested: bool,
        bytes_to_send: &'a [u8],
        complete_hash: String,
        expected_resume_bytes: u64,
    }

    async fn manual_send(connection: &mut MemoryFrameConnection, request: ManualSend<'_>) {
        let transfer_id = TransferId::new("manual-transfer");
        connection
            .send_frame(Frame::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                role: PeerRole::Sender,
            }))
            .await
            .unwrap();
        expect_ready(connection.recv_frame().await.unwrap()).unwrap();
        connection
            .send_frame(Frame::FileHeader(FileHeader {
                transfer_id: transfer_id.clone(),
                file_name: request.file_name.into(),
                file_size: request.source_bytes.len() as u64,
                chunk_size: request.chunk_size,
                resume_requested: request.resume_requested,
            }))
            .await
            .unwrap();
        let resume_status = expect_resume_status(
            connection.recv_frame().await.unwrap(),
            &transfer_id,
            request.chunk_size as usize,
        )
        .unwrap();
        assert_eq!(resume_status.bytes_received, request.expected_resume_bytes);

        let mut offset = resume_status.bytes_received;
        for (index, chunk) in (resume_status.next_chunk_index..).zip(
            request.bytes_to_send[resume_status.bytes_received as usize..]
                .chunks(request.chunk_size as usize),
        ) {
            connection
                .send_frame(Frame::Chunk(Chunk {
                    transfer_id: transfer_id.clone(),
                    index,
                    offset,
                    bytes: chunk.to_vec(),
                }))
                .await
                .unwrap();
            offset += chunk.len() as u64;
        }
        connection
            .send_frame(Frame::Complete(Complete {
                transfer_id: transfer_id.clone(),
                file_hash: request.complete_hash.clone(),
            }))
            .await
            .unwrap();
        if request.complete_hash == blake3::hash(request.source_bytes).to_hex().as_str() {
            expect_complete_ack(connection.recv_frame().await.unwrap(), &transfer_id).unwrap();
        }
    }

    async fn receive_header_and_resume(connection: &mut MemoryFrameConnection) -> TransferId {
        expect_sender_hello(connection.recv_frame().await.unwrap()).unwrap();
        connection.send_frame(Frame::Ready(Ready)).await.unwrap();
        let header = expect_file_header(connection.recv_frame().await.unwrap()).unwrap();
        send_resume_status(connection, &header.transfer_id, 0, 0, String::new())
            .await
            .unwrap();
        header.transfer_id
    }

    async fn assert_no_sidecars(output_dir: &Path) {
        if !fs::try_exists(output_dir).await.unwrap() {
            return;
        }

        let mut entries = fs::read_dir(output_dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(
                !(name.ends_with(".json") || name.ends_with(".part")),
                "unexpected sidecar: {name}"
            );
        }
    }

    #[async_trait]
    impl FrameConnection for MemoryFrameConnection {
        async fn send_frame(&mut self, frame: Frame) -> Result<(), CoreError> {
            self.tx
                .send(frame)
                .await
                .map_err(|error| CoreError::Transport(error.to_string()))
        }

        async fn send_chunk(
            &mut self,
            transfer_id: &TransferId,
            index: u64,
            offset: u64,
            bytes: &[u8],
        ) -> Result<(), CoreError> {
            self.send_frame(Frame::Chunk(Chunk {
                transfer_id: transfer_id.clone(),
                index,
                offset,
                bytes: bytes.to_vec(),
            }))
            .await
        }

        async fn recv_frame(&mut self) -> Result<Frame, CoreError> {
            self.rx
                .recv()
                .await
                .ok_or_else(|| CoreError::Transport("memory connection closed".into()))
        }

        async fn close(&mut self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct StopAfterBytesSink {
        bytes: u64,
        stopped: std::sync::Arc<AtomicBool>,
    }

    impl EventSink for StopAfterBytesSink {
        fn on_event(&self, event: TransferEvent) {
            if let TransferEvent::Progress {
                bytes_transferred, ..
            } = event
                && bytes_transferred >= self.bytes
                && bytes_transferred > 0
            {
                self.stopped.store(true, Ordering::SeqCst);
                panic!("simulated receiver stop after {bytes_transferred} bytes");
            }
        }
    }

    struct ShortRead<'a> {
        bytes: &'a [u8],
        position: usize,
        max_read: usize,
    }

    impl AsyncRead for ShortRead<'_> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.position >= self.bytes.len() || buffer.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
            let bytes_to_read = self
                .max_read
                .min(self.bytes.len() - self.position)
                .min(buffer.remaining());
            let end = self.position + bytes_to_read;
            buffer.put_slice(&self.bytes[self.position..end]);
            self.position = end;
            Poll::Ready(Ok(()))
        }
    }

    struct TestDir(tempfile::TempDir);

    impl std::ops::Deref for TestDir {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            self.0.path()
        }
    }

    impl AsRef<Path> for TestDir {
        fn as_ref(&self) -> &Path {
            self.0.path()
        }
    }

    fn unique_test_dir() -> TestDir {
        TestDir(
            tempfile::Builder::new()
                .prefix("envoix-transfer-test-")
                .tempdir()
                .unwrap(),
        )
    }
}
