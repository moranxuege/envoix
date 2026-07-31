use blake3::Hasher;
use envoix_protocol::{
    Abort, Chunk, Complete, CompleteAck, ContentHash, FileHeader, Frame, FrameKind, Hello,
    IngressState, MAX_CHUNK_SIZE, ProtocolReason, Ready, ResumeMode, ResumeStatus,
};
use envoix_types::{ByteCount, OfferedName, TransferId};

use crate::{
    DurablePrefix, MachineFailure, ProtocolViolation, SourceReader, StagingSink, StorageFault,
    TransferError,
};

pub const CHECKPOINT_INTERVAL: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MonotonicMillis(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Deadline(MonotonicMillis);

impl Deadline {
    pub const fn at(time: MonotonicMillis) -> Self {
        Self(time)
    }

    pub const fn instant(self) -> MonotonicMillis {
        self.0
    }

    fn elapsed(self, now: MonotonicMillis) -> bool {
        now >= self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderRequest {
    transfer_id: TransferId,
    offered_name: OfferedName,
    file_size: ByteCount,
    chunk_size: ByteCount,
    resume: ResumeMode,
    /// What the source must hash to — staging's own digest.
    ///
    /// Required, not optional. The sender hashes what it reads and declares that
    /// in `Complete`, so without an expectation to hold it against, a provider
    /// that swapped the document produced a `Complete` for the NEW bytes, the
    /// receiver verified against that, and both sides agreed on a file the
    /// authority never staged. Taking it here rather than comparing at a call
    /// site means a sender cannot exist without stating what it intends to send.
    content_hash: ContentHash,
}

impl SenderRequest {
    pub fn new(
        transfer_id: TransferId,
        offered_name: OfferedName,
        file_size: ByteCount,
        chunk_size: ByteCount,
        resume: ResumeMode,
        content_hash: ContentHash,
    ) -> Result<Self, TransferError> {
        validate_chunk_size(chunk_size.get()).map_err(TransferError::Protocol)?;
        Ok(Self {
            transfer_id,
            offered_name,
            file_size,
            chunk_size,
            resume,
            content_hash,
        })
    }

    pub const fn transfer_id(&self) -> TransferId {
        self.transfer_id
    }

    pub const fn file_size(&self) -> ByteCount {
        self.file_size
    }

    pub const fn chunk_size(&self) -> ByteCount {
        self.chunk_size
    }

    pub const fn content_hash(&self) -> ContentHash {
        self.content_hash
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClaimedComplete {
    pub file_size: ByteCount,
    pub file_hash: ContentHash,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SenderProgress {
    pub bytes_sent: ByteCount,
    pub file_size: ByteCount,
    pub resumed_bytes: ByteCount,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceiverProgress {
    pub bytes_staged: ByteCount,
    pub file_size: ByteCount,
    pub resumed_bytes: ByteCount,
}

struct Wait<S> {
    state: S,
    deadline: Deadline,
}

impl<S> Wait<S> {
    fn new(state: S, deadline: Deadline) -> Self {
        Self { state, deadline }
    }

    fn into_live(self, now: MonotonicMillis) -> Result<S, MachineFailure> {
        if self.deadline.elapsed(now) {
            Err(MachineFailure::terminal(TransferError::Timeout))
        } else {
            Ok(self.state)
        }
    }
}

trait WaitIdentity {
    fn transfer_id(&self) -> Option<TransferId>;
}

macro_rules! bounded_wait {
    ($name:ident, $state:ident) => {
        pub struct $name(Wait<$state>);

        impl $name {
            pub const fn deadline(&self) -> Deadline {
                self.0.deadline
            }

            pub fn deadline_exceeded(self, now: MonotonicMillis) -> Result<Self, MachineFailure> {
                if self.0.deadline.elapsed(now) {
                    Err(MachineFailure::terminal(TransferError::Timeout))
                } else {
                    Ok(self)
                }
            }

            pub fn peer_closed(self) -> MachineFailure {
                MachineFailure::terminal(TransferError::PeerClosed)
            }

            pub fn cancelled(self) -> MachineFailure {
                MachineFailure::for_local(
                    TransferError::Cancelled,
                    self.0.state.transfer_id(),
                    ProtocolReason::Cancelled,
                )
            }

            pub fn paused(self) -> MachineFailure {
                MachineFailure::for_local(
                    TransferError::Paused,
                    self.0.state.transfer_id(),
                    ProtocolReason::Paused,
                )
            }
        }
    };
}

struct SenderReadyState {
    request: SenderRequest,
}

impl WaitIdentity for SenderReadyState {
    fn transfer_id(&self) -> Option<TransferId> {
        Some(self.request.transfer_id)
    }
}

struct SenderResumeState {
    request: SenderRequest,
}

impl WaitIdentity for SenderResumeState {
    fn transfer_id(&self) -> Option<TransferId> {
        Some(self.request.transfer_id)
    }
}

struct SenderAckState {
    completed: SenderCompleted,
}

impl WaitIdentity for SenderAckState {
    fn transfer_id(&self) -> Option<TransferId> {
        Some(self.completed.transfer_id)
    }
}

struct ReceiverHelloState {
    chunk_size: ByteCount,
}

impl WaitIdentity for ReceiverHelloState {
    fn transfer_id(&self) -> Option<TransferId> {
        None
    }
}

struct ReceiverHeaderState {
    chunk_size: ByteCount,
}

impl WaitIdentity for ReceiverHeaderState {
    fn transfer_id(&self) -> Option<TransferId> {
        None
    }
}

bounded_wait!(SenderAwaitReady, SenderReadyState);
bounded_wait!(SenderAwaitResume, SenderResumeState);
bounded_wait!(SenderAwaitAck, SenderAckState);
bounded_wait!(ReceiverAwaitHello, ReceiverHelloState);
bounded_wait!(ReceiverAwaitHeader, ReceiverHeaderState);

pub fn sender_start(request: SenderRequest, ready_deadline: Deadline) -> (SenderAwaitReady, Frame) {
    (
        SenderAwaitReady(Wait::new(SenderReadyState { request }, ready_deadline)),
        Frame::Hello(Hello),
    )
}

pub fn receiver_start(
    chunk_size: ByteCount,
    hello_deadline: Deadline,
) -> Result<ReceiverAwaitHello, TransferError> {
    validate_chunk_size(chunk_size.get()).map_err(TransferError::Protocol)?;
    Ok(ReceiverAwaitHello(Wait::new(
        ReceiverHelloState { chunk_size },
        hello_deadline,
    )))
}

impl SenderAwaitReady {
    pub const fn ingress_state(&self) -> IngressState {
        IngressState::AwaitReady
    }

    pub fn receive_ready(
        self,
        frame: Frame,
        now: MonotonicMillis,
        resume_deadline: Deadline,
    ) -> Result<(SenderAwaitResume, Frame), MachineFailure> {
        let state = self.0.into_live(now)?;
        match frame {
            Frame::Ready(_) => {
                let header = FileHeader {
                    transfer_id: state.request.transfer_id,
                    offered_name: state.request.offered_name.clone(),
                    file_size: state.request.file_size,
                    chunk_size: state.request.chunk_size,
                    resume: state.request.resume,
                };
                Ok((
                    SenderAwaitResume(Wait::new(
                        SenderResumeState {
                            request: state.request,
                        },
                        resume_deadline,
                    )),
                    Frame::FileHeader(header),
                ))
            }
            Frame::Abort(abort) => Err(peer_abort(abort)),
            other => Err(protocol_failure(
                state.request.transfer_id,
                IngressState::AwaitReady,
                other.kind(),
            )),
        }
    }
}

impl SenderAwaitResume {
    pub const fn ingress_state(&self) -> IngressState {
        IngressState::AwaitResumeStatus
    }

    pub fn receive_resume(
        self,
        frame: Frame,
        now: MonotonicMillis,
        ack_deadline: Deadline,
        source: &mut impl SourceReader,
    ) -> Result<SenderSending, MachineFailure> {
        let state = self.0.into_live(now)?;
        let status = match frame {
            Frame::ResumeStatus(status) => status,
            Frame::Abort(abort) => return Err(peer_abort(abort)),
            other => {
                return Err(protocol_failure(
                    state.request.transfer_id,
                    IngressState::AwaitResumeStatus,
                    other.kind(),
                ));
            }
        };
        validate_resume_status(&state.request, &status)
            .map_err(|error| MachineFailure::from_engine_error(error, state.request.transfer_id))?;

        let mut hasher = Box::new(Hasher::new());
        let mut offset = 0;
        let mut index = 0;
        if status.bytes_received.get() > 0 {
            let expected_hash = status.prefix_hash.ok_or_else(|| {
                MachineFailure::from_engine_error(
                    TransferError::Protocol(ProtocolViolation::MissingPrefixHash),
                    state.request.transfer_id,
                )
            })?;
            hash_source_prefix(
                source,
                status.bytes_received.get(),
                state.request.chunk_size.get() as usize,
                &mut hasher,
            )
            .map_err(|error| MachineFailure::from_engine_error(error, state.request.transfer_id))?;
            if content_hash(&hasher) == expected_hash {
                offset = status.bytes_received.get();
                index = status.next_chunk_index;
            } else {
                hasher = Box::new(Hasher::new());
            }
        }

        Ok(SenderSending {
            request: state.request,
            offset,
            index,
            resumed_bytes: offset,
            hasher,
            ack_deadline,
        })
    }
}

pub struct SenderSending {
    request: SenderRequest,
    offset: u64,
    index: u64,
    resumed_bytes: u64,
    hasher: Box<Hasher>,
    ack_deadline: Deadline,
}

impl SenderSending {
    pub const fn progress(&self) -> SenderProgress {
        SenderProgress {
            bytes_sent: ByteCount::new(self.offset),
            file_size: self.request.file_size,
            resumed_bytes: ByteCount::new(self.resumed_bytes),
        }
    }

    pub fn next_frame(
        mut self,
        source: &mut impl SourceReader,
    ) -> Result<SenderStep, MachineFailure> {
        if self.offset == self.request.file_size.get() {
            let file_hash = content_hash(&self.hasher);
            // What was READ is what was staged, or this send does not complete.
            //
            // The bytes are already on the wire — the network is wasted either
            // way — but a `Complete` is a claim about which file was sent, and
            // declaring the hash of whatever turned up would make a swapped
            // document indistinguishable from the one the user chose. Refusing
            // here is what makes `Ready`'s digest mean something after the fact
            // rather than only at the moment staging ran.
            if file_hash != self.request.content_hash {
                return Err(MachineFailure::from_engine_error(
                    TransferError::SourceChanged,
                    self.request.transfer_id,
                ));
            }
            let completed = SenderCompleted {
                transfer_id: self.request.transfer_id,
                file_size: self.request.file_size,
                file_hash,
            };
            let frame = Frame::Complete(Complete {
                transfer_id: self.request.transfer_id,
                file_hash,
            });
            return Ok(SenderStep::Complete {
                state: SenderAwaitAck(Wait::new(SenderAckState { completed }, self.ack_deadline)),
                frame,
            });
        }

        let remaining = self.request.file_size.get() - self.offset;
        let chunk_len = remaining.min(self.request.chunk_size.get()) as usize;
        let mut bytes = vec![0_u8; chunk_len];
        read_source_exact(
            source,
            self.offset,
            &mut bytes,
            self.request.file_size.get(),
        )
        .map_err(|error| MachineFailure::from_engine_error(error, self.request.transfer_id))?;
        self.hasher.update(&bytes);
        let frame = Frame::Chunk(Chunk {
            transfer_id: self.request.transfer_id,
            index: self.index,
            offset: ByteCount::new(self.offset),
            bytes,
        });
        self.offset += chunk_len as u64;
        self.index += 1;
        let progress = self.progress();
        Ok(SenderStep::Chunk {
            state: self,
            frame,
            progress,
        })
    }

    pub fn peer_closed(self) -> MachineFailure {
        MachineFailure::terminal(TransferError::PeerClosed)
    }

    pub fn cancelled(self) -> MachineFailure {
        MachineFailure::for_local(
            TransferError::Cancelled,
            Some(self.request.transfer_id),
            ProtocolReason::Cancelled,
        )
    }

    pub fn paused(self) -> MachineFailure {
        MachineFailure::for_local(
            TransferError::Paused,
            Some(self.request.transfer_id),
            ProtocolReason::Paused,
        )
    }
}

pub enum SenderStep {
    Chunk {
        state: SenderSending,
        frame: Frame,
        progress: SenderProgress,
    },
    Complete {
        state: SenderAwaitAck,
        frame: Frame,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SenderCompleted {
    pub transfer_id: TransferId,
    pub file_size: ByteCount,
    pub file_hash: ContentHash,
}

impl SenderAwaitAck {
    pub const fn ingress_state(&self) -> IngressState {
        IngressState::AwaitCompleteAck
    }

    pub fn receive_ack(
        self,
        frame: Frame,
        now: MonotonicMillis,
    ) -> Result<SenderCompleted, MachineFailure> {
        let state = self.0.into_live(now)?;
        match frame {
            Frame::CompleteAck(ack) if ack.transfer_id == state.completed.transfer_id => {
                Ok(state.completed)
            }
            Frame::CompleteAck(_) => Err(MachineFailure::from_engine_error(
                TransferError::Protocol(ProtocolViolation::TransferIdMismatch),
                state.completed.transfer_id,
            )),
            Frame::Abort(abort) => Err(peer_abort(abort)),
            other => Err(protocol_failure(
                state.completed.transfer_id,
                IngressState::AwaitCompleteAck,
                other.kind(),
            )),
        }
    }
}

impl ReceiverAwaitHello {
    pub const fn ingress_state(&self) -> IngressState {
        IngressState::AwaitHello
    }

    pub fn receive_hello(
        self,
        frame: Frame,
        now: MonotonicMillis,
        header_deadline: Deadline,
    ) -> Result<(ReceiverAwaitHeader, Frame), MachineFailure> {
        let state = self.0.into_live(now)?;
        match frame {
            Frame::Hello(_) => Ok((
                ReceiverAwaitHeader(Wait::new(
                    ReceiverHeaderState {
                        chunk_size: state.chunk_size,
                    },
                    header_deadline,
                )),
                Frame::Ready(Ready),
            )),
            Frame::Abort(abort) => Err(peer_abort(abort)),
            other => Err(MachineFailure::for_local(
                TransferError::Protocol(ProtocolViolation::UnexpectedFrame {
                    state: IngressState::AwaitHello,
                    actual: other.kind(),
                }),
                None,
                ProtocolReason::ProtocolViolation,
            )),
        }
    }
}

impl ReceiverAwaitHeader {
    pub const fn ingress_state(&self) -> IngressState {
        IngressState::AwaitFileHeader
    }

    pub fn receive_header(
        self,
        frame: Frame,
        now: MonotonicMillis,
        data_deadline: Deadline,
        claim: Option<ClaimedComplete>,
        sink: &mut impl StagingSink,
    ) -> Result<(ReceiverReceiving, Frame), MachineFailure> {
        let state = self.0.into_live(now)?;
        let header = match frame {
            Frame::FileHeader(header) => header,
            Frame::Abort(abort) => return Err(peer_abort(abort)),
            other => {
                return Err(MachineFailure::for_local(
                    TransferError::Protocol(ProtocolViolation::UnexpectedFrame {
                        state: IngressState::AwaitFileHeader,
                        actual: other.kind(),
                    }),
                    None,
                    ProtocolReason::ProtocolViolation,
                ));
            }
        };
        validate_header(&header, state.chunk_size)
            .map_err(|error| MachineFailure::from_engine_error(error, header.transfer_id))?;

        let claim = if matches!(header.resume, ResumeMode::Allowed) {
            claim
        } else {
            None
        };
        let (mode, staged, hasher, prefix_hash) = match claim {
            Some(claim) => {
                if claim.file_size != header.file_size {
                    return Err(MachineFailure::from_engine_error(
                        TransferError::IntegrityMismatch,
                        header.transfer_id,
                    ));
                }
                (
                    ReceiveMode::Claim(claim),
                    claim.file_size,
                    Box::new(Hasher::new()),
                    claim.file_hash,
                )
            }
            None => {
                let prepared = prepare_staging(&header, sink)?;
                (
                    ReceiveMode::Staging,
                    prepared.prefix.length,
                    prepared.hasher,
                    prepared.prefix_hash,
                )
            }
        };

        // DERIVED, here, from the length and the chunk size the header
        // negotiated. It used to be stored beside the length and re-validated
        // against it on every resume, which is a conclusion kept next to its own
        // premise — and a whole protocol violation existed for when the two
        // disagreed.
        let next_chunk_index = validated_next_index(staged.get(), header.chunk_size.get());
        let status = ResumeStatus {
            transfer_id: header.transfer_id,
            next_chunk_index,
            bytes_received: staged,
            prefix_hash: Some(prefix_hash),
        };
        let receiving = ReceiverReceiving {
            header,
            mode,
            expected_offset: staged.get(),
            expected_index: next_chunk_index,
            resumed_bytes: staged.get(),
            last_checkpoint: staged.get(),
            hasher,
            deadline: data_deadline,
        };
        Ok((receiving, Frame::ResumeStatus(status)))
    }
}

enum ReceiveMode {
    Staging,
    Claim(ClaimedComplete),
}

pub struct ReceiverReceiving {
    header: FileHeader,
    mode: ReceiveMode,
    expected_offset: u64,
    expected_index: u64,
    resumed_bytes: u64,
    last_checkpoint: u64,
    hasher: Box<Hasher>,
    deadline: Deadline,
}

impl ReceiverReceiving {
    pub const fn ingress_state(&self) -> IngressState {
        IngressState::ReceivingData
    }

    pub const fn deadline(&self) -> Deadline {
        self.deadline
    }

    pub const fn progress(&self) -> ReceiverProgress {
        ReceiverProgress {
            bytes_staged: ByteCount::new(self.expected_offset),
            file_size: self.header.file_size,
            resumed_bytes: ByteCount::new(self.resumed_bytes),
        }
    }

    pub fn receive(
        self,
        frame: Frame,
        now: MonotonicMillis,
        next_deadline: Deadline,
        sink: &mut impl StagingSink,
    ) -> Result<ReceiverStep, MachineFailure> {
        if self.deadline.elapsed(now) {
            self.best_effort_checkpoint(sink);
            return Err(MachineFailure::terminal(TransferError::Timeout));
        }
        if let Frame::Abort(abort) = frame {
            self.best_effort_checkpoint(sink);
            return Err(peer_abort(abort));
        }

        match self.mode {
            ReceiveMode::Claim(claim) => self.receive_claim(frame, claim),
            ReceiveMode::Staging => match frame {
                Frame::Chunk(chunk) => self.receive_chunk(chunk, next_deadline, sink),
                Frame::Complete(complete) => self.receive_complete(complete, sink),
                other => {
                    self.best_effort_checkpoint(sink);
                    Err(protocol_failure(
                        self.header.transfer_id,
                        IngressState::ReceivingData,
                        other.kind(),
                    ))
                }
            },
        }
    }

    pub fn deadline_exceeded(
        self,
        now: MonotonicMillis,
        sink: &mut impl StagingSink,
    ) -> Result<Self, MachineFailure> {
        if self.deadline.elapsed(now) {
            self.best_effort_checkpoint(sink);
            Err(MachineFailure::terminal(TransferError::Timeout))
        } else {
            Ok(self)
        }
    }

    pub fn peer_closed(self, sink: &mut impl StagingSink) -> MachineFailure {
        self.best_effort_checkpoint(sink);
        MachineFailure::terminal(TransferError::PeerClosed)
    }

    pub fn cancelled(self, sink: &mut impl StagingSink) -> MachineFailure {
        self.best_effort_checkpoint(sink);
        MachineFailure::for_local(
            TransferError::Cancelled,
            Some(self.header.transfer_id),
            ProtocolReason::Cancelled,
        )
    }

    pub fn paused(self, sink: &mut impl StagingSink) -> MachineFailure {
        self.best_effort_checkpoint(sink);
        MachineFailure::for_local(
            TransferError::Paused,
            Some(self.header.transfer_id),
            ProtocolReason::Paused,
        )
    }

    fn receive_claim(
        self,
        frame: Frame,
        claim: ClaimedComplete,
    ) -> Result<ReceiverStep, MachineFailure> {
        match frame {
            Frame::Complete(complete) => {
                validate_transfer_id(self.header.transfer_id, complete.transfer_id)?;
                if complete.file_hash != claim.file_hash {
                    return Err(MachineFailure::from_engine_error(
                        TransferError::IntegrityMismatch,
                        self.header.transfer_id,
                    ));
                }
                Ok(ReceiverStep::ReadyToCommit(ReceiverReadyToCommit::new(
                    self.header.transfer_id,
                    self.header.file_size,
                    claim.file_hash,
                    true,
                    ByteCount::new(self.resumed_bytes),
                )))
            }
            Frame::Chunk(chunk) => {
                validate_transfer_id(self.header.transfer_id, chunk.transfer_id)?;
                Err(MachineFailure::from_engine_error(
                    TransferError::IntegrityMismatch,
                    self.header.transfer_id,
                ))
            }
            other => Err(protocol_failure(
                self.header.transfer_id,
                IngressState::ReceivingData,
                other.kind(),
            )),
        }
    }

    fn receive_chunk(
        mut self,
        chunk: Chunk,
        next_deadline: Deadline,
        sink: &mut impl StagingSink,
    ) -> Result<ReceiverStep, MachineFailure> {
        if self.expected_offset > 0 && chunk.index == 0 && chunk.offset.get() == 0 {
            self.reset_staging(sink)?;
        }
        validate_chunk(
            &self.header,
            &chunk,
            self.expected_index,
            self.expected_offset,
        )
        .map_err(|error| {
            self.best_effort_checkpoint(sink);
            MachineFailure::from_engine_error(error, self.header.transfer_id)
        })?;

        if let Err(fault) = sink.append(ByteCount::new(self.expected_offset), &chunk.bytes) {
            self.best_effort_checkpoint(sink);
            return Err(MachineFailure::from_engine_error(
                TransferError::Storage(fault),
                self.header.transfer_id,
            ));
        }
        self.hasher.update(&chunk.bytes);
        self.expected_offset += chunk.bytes.len() as u64;
        self.expected_index += 1;

        if self.expected_offset.saturating_sub(self.last_checkpoint) >= CHECKPOINT_INTERVAL {
            if let Err(fault) = sink.checkpoint(self.accepted_prefix()) {
                return Err(MachineFailure::from_engine_error(
                    TransferError::Storage(fault),
                    self.header.transfer_id,
                ));
            }
            self.last_checkpoint = self.expected_offset;
        }
        self.deadline = next_deadline;
        let progress = self.progress();
        Ok(ReceiverStep::Continue {
            state: self,
            progress,
        })
    }

    fn receive_complete(
        self,
        complete: Complete,
        sink: &mut impl StagingSink,
    ) -> Result<ReceiverStep, MachineFailure> {
        validate_transfer_id(self.header.transfer_id, complete.transfer_id)?;
        if self.expected_offset != self.header.file_size.get() {
            self.best_effort_checkpoint(sink);
            return Err(MachineFailure::from_engine_error(
                TransferError::Protocol(ProtocolViolation::CompleteBeforeEnd {
                    received: self.expected_offset,
                    expected: self.header.file_size.get(),
                }),
                self.header.transfer_id,
            ));
        }
        let actual_hash = content_hash(&self.hasher);
        if actual_hash != complete.file_hash {
            self.best_effort_checkpoint(sink);
            return Err(MachineFailure::from_engine_error(
                TransferError::IntegrityMismatch,
                self.header.transfer_id,
            ));
        }

        sink.checkpoint(self.accepted_prefix()).map_err(|fault| {
            MachineFailure::from_engine_error(
                TransferError::Storage(fault),
                self.header.transfer_id,
            )
        })?;
        Ok(ReceiverStep::ReadyToCommit(ReceiverReadyToCommit::new(
            self.header.transfer_id,
            self.header.file_size,
            actual_hash,
            false,
            ByteCount::new(self.resumed_bytes),
        )))
    }

    fn reset_staging(&mut self, sink: &mut impl StagingSink) -> Result<(), MachineFailure> {
        // ONE call. The sink owns the order — publish the zero prefix, then
        // truncate — because doing it here left a durable prefix naming bytes
        // that had already been shortened if a crash landed between the two.
        sink.reset().map_err(|fault| {
            MachineFailure::from_engine_error(
                TransferError::Storage(fault),
                self.header.transfer_id,
            )
        })?;
        self.expected_offset = 0;
        self.expected_index = 0;
        self.resumed_bytes = 0;
        self.last_checkpoint = 0;
        *self.hasher = Hasher::new();
        Ok(())
    }

    fn best_effort_checkpoint(&self, sink: &mut impl StagingSink) {
        if matches!(self.mode, ReceiveMode::Staging) {
            let _ = sink.checkpoint(self.accepted_prefix());
        }
    }

    /// The prefix this machine has ACCEPTED — never what the sink happens to
    /// hold. After a torn append those differ by the partial tail, and that is
    /// precisely the moment a checkpoint gets published.
    fn accepted_prefix(&self) -> DurablePrefix {
        DurablePrefix {
            length: ByteCount::new(self.expected_offset),
            digest: content_hash(&self.hasher),
        }
    }
}

pub enum ReceiverStep {
    Continue {
        state: ReceiverReceiving,
        progress: ReceiverProgress,
    },
    /// The payload and hash are verified, but the irreversible seal has not run.
    ReadyToCommit(ReceiverReadyToCommit),
}

pub struct ReceiverReadyToCommit {
    completed: ReceiverCompleted,
    resumed_bytes: ByteCount,
}

impl ReceiverReadyToCommit {
    const fn new(
        transfer_id: TransferId,
        file_size: ByteCount,
        file_hash: ContentHash,
        claimed_existing: bool,
        resumed_bytes: ByteCount,
    ) -> Self {
        Self {
            completed: ReceiverCompleted::new(transfer_id, file_size, file_hash, claimed_existing),
            resumed_bytes,
        }
    }

    /// How much of this file was NOT transferred in this attempt.
    ///
    /// Carried here as well as on `ReceiverProgress` because a transfer can
    /// reach completion without a single chunk — a resumed prefix that already
    /// covered the file, or a receiver that claimed to hold it — and those are
    /// exactly the runs whose resumed count nothing else would ever report.
    pub const fn resumed_bytes(&self) -> ByteCount {
        self.resumed_bytes
    }

    pub const fn transfer_id(&self) -> TransferId {
        self.completed.transfer_id
    }

    pub const fn claimed_existing(&self) -> bool {
        self.completed.claimed_existing
    }

    /// Crosses the receiver's irreversible storage boundary.
    ///
    /// The attempt executor must arbitrate retirement immediately before this
    /// synchronous call and send no completion acknowledgement unless it succeeds.
    pub fn commit<S: StagingSink>(
        self,
        sink: &mut S,
    ) -> Result<ReceiveCommit<S::Seal>, MachineFailure> {
        if self.completed.claimed_existing {
            return Ok(ReceiveCommit::AlreadyHeld {
                completed: self.completed,
            });
        }
        let seal = sink
            .seal(self.completed.file_size, self.completed.file_hash)
            .map_err(|fault| {
                MachineFailure::from_engine_error(
                    TransferError::Storage(fault),
                    self.completed.transfer_id,
                )
            })?;
        Ok(ReceiveCommit::Sealed {
            completed: self.completed,
            seal,
        })
    }

    pub fn cancelled(self) -> MachineFailure {
        MachineFailure::for_local(
            TransferError::Cancelled,
            Some(self.completed.transfer_id),
            ProtocolReason::Cancelled,
        )
    }

    pub fn paused(self) -> MachineFailure {
        MachineFailure::for_local(
            TransferError::Paused,
            Some(self.completed.transfer_id),
            ProtocolReason::Paused,
        )
    }
}

/// What crossing the commit boundary established.
///
/// The seal used to be discarded here. That put the card back to trusting a
/// worker's word about bytes it never saw — the same thing the send side stopped
/// doing when possession became witnessed. A sink's seal is the strongest
/// statement anything in this system makes, and dropping it at the one boundary
/// that produces it meant "the file is complete" had to be re-derived from
/// storage every time it was asked.
///
/// Two arms rather than an `Option`, because the absence has a reason and the
/// card acts differently on it: a claimed file was never written by this run, so
/// there is nothing for it to witness and whatever durable reference exists came
/// from before.
pub enum ReceiveCommit<S> {
    Sealed {
        completed: ReceiverCompleted,
        seal: S,
    },
    AlreadyHeld {
        completed: ReceiverCompleted,
    },
}

impl<S> ReceiveCommit<S> {
    pub const fn completed(&self) -> ReceiverCompleted {
        match self {
            Self::Sealed { completed, .. } | Self::AlreadyHeld { completed } => *completed,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceiverCompleted {
    pub transfer_id: TransferId,
    pub file_size: ByteCount,
    pub file_hash: ContentHash,
    pub claimed_existing: bool,
}

impl ReceiverCompleted {
    const fn new(
        transfer_id: TransferId,
        file_size: ByteCount,
        file_hash: ContentHash,
        claimed_existing: bool,
    ) -> Self {
        Self {
            transfer_id,
            file_size,
            file_hash,
            claimed_existing,
        }
    }

    /// The file is already durable; delivery of this acknowledgement is best-effort.
    pub const fn acknowledgement(self) -> Frame {
        Frame::CompleteAck(CompleteAck {
            transfer_id: self.transfer_id,
        })
    }
}

struct PreparedStaging {
    prefix: DurablePrefix,
    hasher: Box<Hasher>,
    prefix_hash: ContentHash,
}

fn prepare_staging(
    header: &FileHeader,
    sink: &mut impl StagingSink,
) -> Result<PreparedStaging, MachineFailure> {
    if matches!(header.resume, ResumeMode::Disabled) {
        return fresh_staging(header, sink);
    }

    let prefix = sink.resume().map_err(|fault| {
        MachineFailure::from_engine_error(TransferError::Storage(fault), header.transfer_id)
    })?;
    if prefix.length.get() == 0 {
        return fresh_staging(header, sink);
    }
    if prefix.length.get() > header.file_size.get() {
        return Err(MachineFailure::from_engine_error(
            TransferError::Protocol(ProtocolViolation::ResumeOffsetExceedsFile {
                offset: prefix.length.get(),
                file_size: header.file_size.get(),
            }),
            header.transfer_id,
        ));
    }

    // Re-read the prefix rather than persist a hasher, and get a second answer
    // for free: the sink's own digest says these are the bytes it promised. That
    // is LOCAL evidence — the peer still gets what was recomputed here.
    let mut hasher = Box::new(Hasher::new());
    let complete = hash_staged_prefix(
        sink,
        prefix.length.get(),
        header.chunk_size.get() as usize,
        &mut hasher,
    )
    .map_err(|fault| {
        MachineFailure::from_engine_error(TransferError::Storage(fault), header.transfer_id)
    })?;
    let recomputed = content_hash(&hasher);
    if !complete || recomputed != prefix.digest {
        return fresh_staging(header, sink);
    }
    // No truncate: the sink opened at its own durable prefix and the tail is
    // already gone. Doing it here was the caller re-deciding something the sink
    // had settled.
    Ok(PreparedStaging {
        prefix,
        prefix_hash: recomputed,
        hasher,
    })
}

fn fresh_staging(
    header: &FileHeader,
    sink: &mut impl StagingSink,
) -> Result<PreparedStaging, MachineFailure> {
    sink.reset().map_err(|fault| {
        MachineFailure::from_engine_error(TransferError::Storage(fault), header.transfer_id)
    })?;
    let hasher = Box::new(Hasher::new());
    Ok(PreparedStaging {
        prefix: DurablePrefix {
            length: ByteCount::new(0),
            digest: content_hash(&hasher),
        },
        prefix_hash: content_hash(&hasher),
        hasher,
    })
}

fn validate_header(
    header: &FileHeader,
    receiver_chunk_size: ByteCount,
) -> Result<(), TransferError> {
    validate_chunk_size(receiver_chunk_size.get()).map_err(TransferError::Protocol)?;
    validate_chunk_size(header.chunk_size.get()).map_err(TransferError::Protocol)?;
    if header.chunk_size != receiver_chunk_size {
        return Err(TransferError::Protocol(
            ProtocolViolation::ChunkSizeMismatch {
                sender: header.chunk_size.get(),
                receiver: receiver_chunk_size.get(),
            },
        ));
    }
    Ok(())
}

fn validate_resume_status(
    request: &SenderRequest,
    status: &ResumeStatus,
) -> Result<(), TransferError> {
    if status.transfer_id != request.transfer_id {
        return Err(TransferError::Protocol(
            ProtocolViolation::TransferIdMismatch,
        ));
    }
    if status.bytes_received.get() > request.file_size.get() {
        return Err(TransferError::Protocol(
            ProtocolViolation::ResumeOffsetExceedsFile {
                offset: status.bytes_received.get(),
                file_size: request.file_size.get(),
            },
        ));
    }
    if matches!(request.resume, ResumeMode::Disabled) && status.bytes_received.get() > 0 {
        return Err(TransferError::Protocol(
            ProtocolViolation::ResumeNotAllowed {
                offset: status.bytes_received.get(),
            },
        ));
    }
    let expected = validated_next_index(status.bytes_received.get(), request.chunk_size.get());
    if status.next_chunk_index != expected {
        return Err(TransferError::Protocol(
            ProtocolViolation::ResumeIndexInconsistent {
                actual: status.next_chunk_index,
                expected,
            },
        ));
    }
    Ok(())
}

fn validate_chunk(
    header: &FileHeader,
    chunk: &Chunk,
    expected_index: u64,
    expected_offset: u64,
) -> Result<(), TransferError> {
    if chunk.transfer_id != header.transfer_id {
        return Err(TransferError::Protocol(
            ProtocolViolation::TransferIdMismatch,
        ));
    }
    if chunk.index != expected_index {
        return Err(TransferError::Protocol(ProtocolViolation::ChunkIndex {
            actual: chunk.index,
            expected: expected_index,
        }));
    }
    if chunk.offset.get() != expected_offset {
        return Err(TransferError::Protocol(ProtocolViolation::ChunkOffset {
            actual: chunk.offset.get(),
            expected: expected_offset,
        }));
    }
    let remaining = header.file_size.get().saturating_sub(expected_offset);
    let expected_len = remaining.min(header.chunk_size.get()) as usize;
    if expected_len == 0 || chunk.bytes.is_empty() || chunk.bytes.len() != expected_len {
        return Err(TransferError::Protocol(ProtocolViolation::ChunkLength {
            actual: chunk.bytes.len(),
            expected: expected_len,
        }));
    }
    Ok(())
}

fn validate_chunk_size(chunk_size: u64) -> Result<(), ProtocolViolation> {
    if chunk_size == 0 || chunk_size > MAX_CHUNK_SIZE as u64 {
        Err(ProtocolViolation::InvalidChunkSize {
            actual: chunk_size,
            maximum: MAX_CHUNK_SIZE,
        })
    } else {
        Ok(())
    }
}

fn validate_transfer_id(expected: TransferId, actual: TransferId) -> Result<(), MachineFailure> {
    if actual == expected {
        Ok(())
    } else {
        Err(MachineFailure::from_engine_error(
            TransferError::Protocol(ProtocolViolation::TransferIdMismatch),
            expected,
        ))
    }
}

fn hash_source_prefix(
    source: &mut impl SourceReader,
    bytes_to_hash: u64,
    buffer_size: usize,
    hasher: &mut Hasher,
) -> Result<(), TransferError> {
    let mut offset = 0;
    let mut buffer = vec![0_u8; buffer_size.max(1)];
    while offset < bytes_to_hash {
        let count = (bytes_to_hash - offset).min(buffer.len() as u64) as usize;
        read_source_exact(source, offset, &mut buffer[..count], bytes_to_hash)?;
        hasher.update(&buffer[..count]);
        offset += count as u64;
    }
    Ok(())
}

fn read_source_exact(
    source: &mut impl SourceReader,
    start_offset: u64,
    destination: &mut [u8],
    expected_end: u64,
) -> Result<(), TransferError> {
    let mut filled = 0;
    while filled < destination.len() {
        let offset = start_offset + filled as u64;
        let read = source
            .read_at(ByteCount::new(offset), &mut destination[filled..])
            .map_err(TransferError::Storage)?;
        if read == 0 {
            return Err(TransferError::UnexpectedSourceEnd {
                offset,
                expected: expected_end,
            });
        }
        if read > destination.len() - filled {
            return Err(TransferError::Storage(StorageFault::new(
                crate::StorageOperation::ReadSource,
            )));
        }
        filled += read;
    }
    Ok(())
}

fn hash_staged_prefix(
    sink: &mut impl StagingSink,
    bytes_to_hash: u64,
    buffer_size: usize,
    hasher: &mut Hasher,
) -> Result<bool, StorageFault> {
    let mut offset = 0;
    let mut buffer = vec![0_u8; buffer_size.max(1)];
    while offset < bytes_to_hash {
        let count = (bytes_to_hash - offset).min(buffer.len() as u64) as usize;
        let read = sink.read_partial_at(ByteCount::new(offset), &mut buffer[..count])?;
        if read == 0 {
            return Ok(false);
        }
        if read > count {
            return Err(StorageFault::new(crate::StorageOperation::ReadStaging));
        }
        hasher.update(&buffer[..read]);
        offset += read as u64;
    }
    Ok(true)
}

fn content_hash(hasher: &Hasher) -> ContentHash {
    ContentHash::from_bytes(*hasher.finalize().as_bytes())
}

fn protocol_failure(
    transfer_id: TransferId,
    state: IngressState,
    actual: FrameKind,
) -> MachineFailure {
    MachineFailure::from_engine_error(
        TransferError::Protocol(ProtocolViolation::UnexpectedFrame { state, actual }),
        transfer_id,
    )
}

fn peer_abort(abort: Abort) -> MachineFailure {
    MachineFailure::terminal(TransferError::PeerAborted(abort.reason))
}

pub const fn next_chunk_index(bytes_received: u64, chunk_size: u64) -> Option<u64> {
    if chunk_size == 0 {
        None
    } else if bytes_received == 0 {
        Some(0)
    } else {
        Some(bytes_received.div_ceil(chunk_size))
    }
}

fn validated_next_index(bytes_received: u64, chunk_size: u64) -> u64 {
    next_chunk_index(bytes_received, chunk_size)
        .expect("chunk size is validated before calculating an index")
}
