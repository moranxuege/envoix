use envoix_outcomes::OutcomeCode;
use envoix_protocol::{
    Abort, Chunk, Complete, ContentHash, Frame, FrameKind, IngressState, ProtocolReason,
    ResumeMode, ResumeStatus,
};
use envoix_types::{ByteCount, OfferedName, TransferId};

use crate::{
    CHECKPOINT_INTERVAL, ClaimedComplete, Deadline, DurablePrefix, MachineFailure, MonotonicMillis,
    ProtocolViolation, ReceiverCompleted, ReceiverReceiving, ReceiverStep, SenderRequest,
    SenderSending, SenderStep, SourceReader, StagingSink, StorageFault, StorageOperation,
    TransferError, next_chunk_index, receiver_start, sender_start,
};

const NOW: MonotonicMillis = MonotonicMillis(10);
const DEADLINE: Deadline = Deadline::at(MonotonicMillis(100));
const EXPIRED: MonotonicMillis = MonotonicMillis(100);

fn transfer_id(byte: u8) -> TransferId {
    TransferId::from_bytes([byte; 16])
}

/// A send request over exactly `bytes`.
///
/// Takes the bytes rather than their length because the request now states what
/// they must hash to, and a length cannot say that. A test whose source and
/// request disagree is testing the mismatch guard, which is what
/// [`a_source_that_changed_under_the_sender_cannot_complete`] does deliberately.
fn request(id: TransferId, bytes: &[u8], chunk_size: usize, resume: ResumeMode) -> SenderRequest {
    SenderRequest::new(
        id,
        OfferedName::from_untrusted("report.bin").unwrap(),
        ByteCount::new(bytes.len() as u64),
        ByteCount::new(chunk_size as u64),
        resume,
        hash_of(bytes),
    )
    .unwrap()
}

fn hash_of(bytes: &[u8]) -> ContentHash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(bytes);
    ContentHash::from_bytes(*hasher.finalize().as_bytes())
}

struct MemorySource {
    bytes: Vec<u8>,
    max_read: usize,
}

impl MemorySource {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            max_read: usize::MAX,
        }
    }
}

impl SourceReader for MemorySource {
    fn read_at(
        &mut self,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, StorageFault> {
        let offset = offset.get() as usize;
        if offset >= self.bytes.len() {
            return Ok(0);
        }
        let count = destination
            .len()
            .min(self.max_read)
            .min(self.bytes.len() - offset);
        destination[..count].copy_from_slice(&self.bytes[offset..offset + count]);
        Ok(count)
    }
}

/// One bound artifact, in memory. The sink is per-artifact now, so the double
/// holds one buffer rather than a table keyed by whatever the caller passed.
#[derive(Default)]
struct MemorySink {
    staged: Vec<u8>,
    prefix: Option<DurablePrefix>,
    sealed: Option<Vec<u8>>,
    sealed_hash: Option<ContentHash>,
    append_calls: usize,
    fail_append_at: Option<usize>,
    partial_write_on_failure: usize,
    fail_seal: bool,
}

impl StagingSink for MemorySink {
    type Seal = ();

    fn resume(&mut self) -> Result<DurablePrefix, StorageFault> {
        let prefix = self.prefix.unwrap_or(DurablePrefix {
            length: ByteCount::new(0),
            digest: ContentHash::from_bytes(*blake3::hash(&[]).as_bytes()),
        });
        // Opening discards any tail past the promised prefix, exactly as a real
        // lease does. `prepare_staging` relies on this and no longer truncates.
        self.staged.truncate(prefix.length.get() as usize);
        Ok(prefix)
    }

    fn read_partial_at(
        &mut self,
        offset: ByteCount,
        destination: &mut [u8],
    ) -> Result<usize, StorageFault> {
        let offset = offset.get() as usize;
        if offset >= self.staged.len() {
            return Ok(0);
        }
        let count = destination.len().min(self.staged.len() - offset);
        destination[..count].copy_from_slice(&self.staged[offset..offset + count]);
        Ok(count)
    }

    fn append(&mut self, offset: ByteCount, bytes: &[u8]) -> Result<(), StorageFault> {
        let call = self.append_calls;
        self.append_calls += 1;
        if self.fail_append_at == Some(call) {
            self.fail_append_at = None;
            let count = self.partial_write_on_failure.min(bytes.len());
            if self.staged.len() == offset.get() as usize {
                self.staged.extend_from_slice(&bytes[..count]);
            }
            return Err(StorageFault::new(StorageOperation::AppendStaging));
        }
        if self.staged.len() != offset.get() as usize {
            return Err(StorageFault::new(StorageOperation::AppendStaging));
        }
        self.staged.extend_from_slice(bytes);
        Ok(())
    }

    fn checkpoint(&mut self, prefix: DurablePrefix) -> Result<(), StorageFault> {
        assert!(
            prefix.length.get() as usize <= self.staged.len(),
            "a sink cannot promise bytes it was never given"
        );
        self.prefix = Some(prefix);
        Ok(())
    }

    fn reset(&mut self) -> Result<(), StorageFault> {
        // Publish first, then truncate — the order the real store enforces.
        self.prefix = Some(DurablePrefix {
            length: ByteCount::new(0),
            digest: ContentHash::from_bytes(*blake3::hash(&[]).as_bytes()),
        });
        self.staged.clear();
        Ok(())
    }

    fn seal(
        &mut self,
        expected_size: ByteCount,
        digest: ContentHash,
    ) -> Result<Self::Seal, StorageFault> {
        if self.fail_seal {
            return Err(StorageFault::new(StorageOperation::Seal));
        }
        if self.staged.len() as u64 != expected_size.get()
            || ContentHash::from_bytes(*blake3::hash(&self.staged).as_bytes()) != digest
        {
            return Err(StorageFault::new(StorageOperation::Seal));
        }
        self.sealed = Some(self.staged.clone());
        self.sealed_hash = Some(digest);
        self.prefix = None;
        Ok(())
    }
}

fn begin(
    request: SenderRequest,
    source: &mut MemorySource,
    sink: &mut MemorySink,
    claim: Option<ClaimedComplete>,
) -> Result<(SenderSending, ReceiverReceiving), MachineFailure> {
    let chunk_size = request.chunk_size();
    let (sender, hello) = sender_start(request, DEADLINE);
    let receiver = receiver_start(chunk_size, DEADLINE).unwrap();
    let (receiver, ready) = receiver.receive_hello(hello, NOW, DEADLINE)?;
    let (sender, header) = sender.receive_ready(ready, NOW, DEADLINE)?;
    let (receiver, status) = receiver.receive_header(header, NOW, DEADLINE, claim, sink)?;
    let sender = sender.receive_resume(status, NOW, DEADLINE, source)?;
    Ok((sender, receiver))
}

struct Driven {
    sender: crate::SenderCompleted,
    receiver: ReceiverCompleted,
    chunk_bytes: usize,
    chunks: usize,
}

fn drive(
    mut sender: SenderSending,
    mut receiver: ReceiverReceiving,
    source: &mut MemorySource,
    sink: &mut MemorySink,
) -> Result<Driven, MachineFailure> {
    let mut chunk_bytes = 0;
    let mut chunks = 0;
    loop {
        match sender.next_frame(source)? {
            SenderStep::Chunk { state, frame, .. } => {
                if let Frame::Chunk(chunk) = &frame {
                    chunks += 1;
                    chunk_bytes += chunk.bytes.len();
                }
                sender = state;
                receiver = match receiver.receive(frame, NOW, DEADLINE, sink)? {
                    ReceiverStep::Continue { state, .. } => state,
                    ReceiverStep::ReadyToCommit(_) => panic!("chunk completed transfer"),
                };
            }
            SenderStep::Complete { state, frame } => {
                let completed = match receiver.receive(frame, NOW, DEADLINE, sink)? {
                    ReceiverStep::ReadyToCommit(ready) => ready.commit(sink)?,
                    ReceiverStep::Continue { .. } => panic!("complete frame did not complete"),
                };
                let sender = state.receive_ack(completed.acknowledgement(), NOW)?;
                return Ok(Driven {
                    sender,
                    receiver: completed,
                    chunk_bytes,
                    chunks,
                });
            }
        }
    }
}

fn failure<T>(result: Result<T, MachineFailure>) -> MachineFailure {
    match result {
        Ok(_) => panic!("expected machine failure"),
        Err(failure) => failure,
    }
}

/// A provider that swaps the document under a live send cannot complete it.
///
/// The sender hashes what it reads and puts that hash into `Complete`. Without
/// an expectation to hold it against, a document replaced mid-send produced a
/// `Complete` declaring the NEW bytes' hash, the receiver verified against that,
/// and both sides agreed on a file the authority never staged — the exact
/// failure the staged digest exists to prevent, silently passing every check.
///
/// The bytes are still on the wire when this fires. That is the accepted cost of
/// reading the source twice: the network is wasted, the transfer is not wrong.
///
/// The error is `SourceChanged`, not the peer-facing `IntegrityMismatch`: this
/// is our own document moving under us, and the only thing that resolves it is
/// the person choosing a source again — which is what `SourceUnreadable` and its
/// `RePickSource` recovery say, and what `Internal` said nothing about.
#[test]
fn a_source_that_changed_under_the_sender_cannot_complete() {
    let id = transfer_id(31);
    let staged = vec![7_u8; 8];
    let promised = request(id, &staged, 4, ResumeMode::Allowed);

    // The provider answers different bytes of the same length — the case a size
    // check cannot see.
    let mut source = MemorySource::new(vec![9_u8; 8]);
    let mut sink = MemorySink::default();
    let (sender, _receiver) = begin(promised, &mut source, &mut sink, None).unwrap();
    let (sender, _) = one_sender_chunk(sender, &mut source);
    let (sender, _) = one_sender_chunk(sender, &mut source);

    assert_eq!(
        failure(sender.next_frame(&mut source)).error(),
        TransferError::SourceChanged,
        "a swapped document completed as if it were the staged one"
    );

    // And the same drive over the bytes the request names DOES complete, so the
    // refusal above is not passing because completion is unreachable.
    let honest = request(id, &staged, 4, ResumeMode::Allowed);
    let mut source = MemorySource::new(staged);
    let mut sink = MemorySink::default();
    let (sender, _receiver) = begin(honest, &mut source, &mut sink, None).unwrap();
    let (sender, _) = one_sender_chunk(sender, &mut source);
    let (sender, _) = one_sender_chunk(sender, &mut source);
    assert!(matches!(
        sender.next_frame(&mut source).unwrap(),
        SenderStep::Complete { .. }
    ));
}

#[test]
fn transfer_resume_hash_characterization() {
    full_transfer_and_short_reads();
    matching_resume_sends_only_tail();
    corrupted_prefix_restarts_both_sides();
    self_consistent_prefix_from_another_source_restarts();
    claim_complete_redelivers_ack_without_chunks();
    fresh_offer_ignores_claim_complete();
    exact_ingress_and_resume_validation();
    completion_order_and_strict_ack();
}

fn full_transfer_and_short_reads() {
    let id = transfer_id(1);
    let bytes = b"abcdefghij".to_vec();
    let mut source = MemorySource::new(bytes.clone());
    source.max_read = 2;
    let mut sink = MemorySink::default();
    let request = request(id, &bytes, 4, ResumeMode::Allowed);
    let (sender, receiver) = begin(request, &mut source, &mut sink, None).unwrap();
    let driven = drive(sender, receiver, &mut source, &mut sink).unwrap();

    assert_eq!(driven.chunks, 3);
    assert_eq!(driven.chunk_bytes, bytes.len());
    assert_eq!(sink.sealed.as_ref(), Some(&bytes));
    assert_eq!(
        driven.sender.file_hash,
        ContentHash::from_bytes(*blake3::hash(&bytes).as_bytes())
    );
    assert_eq!(driven.sender.file_hash, driven.receiver.file_hash);
}

fn matching_resume_sends_only_tail() {
    let id = transfer_id(2);
    let bytes = (0_u8..20).collect::<Vec<_>>();
    let mut source = MemorySource::new(bytes.clone());
    let mut sink = MemorySink::default();
    let request = request(id, &bytes, 4, ResumeMode::Allowed);
    let (mut sender, mut receiver) = begin(request.clone(), &mut source, &mut sink, None).unwrap();

    for _ in 0..2 {
        let (next_sender, frame) = one_sender_chunk(sender, &mut source);
        sender = next_sender;
        receiver = continue_receiver(receiver, frame, &mut sink);
    }
    drop(sender);
    let stopped = receiver.peer_closed(&mut sink);
    assert_eq!(stopped.error(), TransferError::PeerClosed);
    assert_eq!(
        sink.prefix.map(|prefix| prefix.length),
        Some(ByteCount::new(8))
    );

    let (sender, receiver) = begin(request, &mut source, &mut sink, None).unwrap();
    assert_eq!(sender.progress().resumed_bytes, ByteCount::new(8));
    let driven = drive(sender, receiver, &mut source, &mut sink).unwrap();
    assert_eq!(driven.chunk_bytes, bytes.len() - 8);
    assert_eq!(sink.sealed.as_ref(), Some(&bytes));
}

fn corrupted_prefix_restarts_both_sides() {
    let id = transfer_id(3);
    let bytes = (20_u8..40).collect::<Vec<_>>();
    let mut source = MemorySource::new(bytes.clone());
    let mut sink = MemorySink::default();
    let request = request(id, &bytes, 4, ResumeMode::Allowed);
    let (sender, receiver) = begin(request.clone(), &mut source, &mut sink, None).unwrap();
    let (sender, first) = one_sender_chunk(sender, &mut source);
    let receiver = continue_receiver(receiver, first, &mut sink);
    let (sender, second) = one_sender_chunk(sender, &mut source);
    let receiver = continue_receiver(receiver, second, &mut sink);
    drop(sender);
    let paused = receiver.paused(&mut sink);
    assert_eq!(paused.error(), TransferError::Paused);
    sink.staged[0] ^= 0xff;

    let (sender, receiver) = begin(request, &mut source, &mut sink, None).unwrap();
    assert_eq!(sender.progress().resumed_bytes, ByteCount::new(0));
    let (sender, restart) = one_sender_chunk(sender, &mut source);
    let Frame::Chunk(restart_chunk) = &restart else {
        panic!("expected restart chunk");
    };
    assert_eq!(restart_chunk.index, 0);
    assert_eq!(restart_chunk.offset, ByteCount::new(0));
    let receiver = continue_receiver(receiver, restart, &mut sink);
    assert_eq!(receiver.progress().resumed_bytes, ByteCount::new(0));
    let driven = drive(sender, receiver, &mut source, &mut sink).unwrap();
    assert_eq!(driven.chunk_bytes + 4, bytes.len());
    assert_eq!(sink.sealed.as_ref(), Some(&bytes));
}

/// The receiver's local digest check catches a TORN prefix. It cannot catch a
/// prefix that is internally consistent but came from different content — the
/// receiver has no way to know what the sender holds. That is what the wire
/// comparison is for, and only this case reaches it.
///
/// Written when the local check landed: without it, the sender's prefix-hash
/// path lost its only coverage, because every corruption test now stops one
/// step earlier.
fn self_consistent_prefix_from_another_source_restarts() {
    let id = transfer_id(11);
    let bytes = (60_u8..80).collect::<Vec<_>>();
    let mut sink = MemorySink::default();

    // Stage eight bytes of SOMETHING ELSE, checkpointed honestly: the digest
    // names exactly the bytes on disk, so the receiver's own check is satisfied.
    let other = (200_u8..208).collect::<Vec<_>>();
    sink.append(ByteCount::new(0), &other).unwrap();
    sink.checkpoint(DurablePrefix {
        length: ByteCount::new(other.len() as u64),
        digest: ContentHash::from_bytes(*blake3::hash(&other).as_bytes()),
    })
    .unwrap();

    let mut source = MemorySource::new(bytes.clone());
    let request = request(id, &bytes, 4, ResumeMode::Allowed);
    let (sender, receiver) = begin(request, &mut source, &mut sink, None).unwrap();

    // The receiver offered its prefix in good faith; the sender rejected it.
    assert_eq!(receiver.progress().resumed_bytes, ByteCount::new(8));
    assert_eq!(sender.progress().resumed_bytes, ByteCount::new(0));

    let (sender, restart) = one_sender_chunk(sender, &mut source);
    let Frame::Chunk(restart_chunk) = &restart else {
        panic!("expected restart chunk");
    };
    assert_eq!(restart_chunk.offset, ByteCount::new(0));
    let receiver = continue_receiver(receiver, restart, &mut sink);
    let driven = drive(sender, receiver, &mut source, &mut sink).unwrap();
    assert_eq!(driven.chunk_bytes + 4, bytes.len());
    assert_eq!(sink.sealed.as_ref(), Some(&bytes));
}

fn claim_complete_redelivers_ack_without_chunks() {
    let id = transfer_id(4);
    let bytes = b"already-present".to_vec();
    let known_hash = ContentHash::from_bytes(*blake3::hash(&bytes).as_bytes());
    let claim = ClaimedComplete {
        file_size: ByteCount::new(bytes.len() as u64),
        file_hash: known_hash,
    };
    let transfer_request = request(id, &bytes, 4, ResumeMode::Allowed);
    let mut sink = MemorySink::default();

    let mut source = MemorySource::new(bytes.clone());
    let (sender, receiver) = begin(
        transfer_request.clone(),
        &mut source,
        &mut sink,
        Some(claim),
    )
    .unwrap();
    let SenderStep::Complete { state, frame } = sender.next_frame(&mut source).unwrap() else {
        panic!("claim-complete sent a chunk");
    };
    let ReceiverStep::ReadyToCommit(ready) =
        receiver.receive(frame, NOW, DEADLINE, &mut sink).unwrap()
    else {
        panic!("claim-complete did not complete");
    };
    let completed = ready.commit(&mut sink).unwrap();
    assert!(completed.claimed_existing);
    let _lost_ack = state;

    let mut source = MemorySource::new(bytes.clone());
    let (sender, receiver) = begin(
        transfer_request.clone(),
        &mut source,
        &mut sink,
        Some(claim),
    )
    .unwrap();
    let driven = drive(sender, receiver, &mut source, &mut sink).unwrap();
    assert_eq!(driven.chunks, 0);

    let mut different = MemorySource::new(b"different-data!".to_vec());
    let (sender, receiver) =
        begin(transfer_request, &mut different, &mut sink, Some(claim)).unwrap();
    let (_sender, frame) = one_sender_chunk(sender, &mut different);
    let rejected = failure(receiver.receive(frame, NOW, DEADLINE, &mut sink));
    assert_eq!(rejected.error(), TransferError::IntegrityMismatch);
    assert_eq!(
        rejected.abort().map(|abort| abort.reason),
        Some(ProtocolReason::IntegrityMismatch)
    );

    let mut source = MemorySource::new(bytes);
    let request = request(id, &source.bytes.clone(), 4, ResumeMode::Allowed);
    let (_sender, receiver) = begin(request, &mut source, &mut sink, Some(claim)).unwrap();
    let mismatch = Frame::Complete(Complete {
        transfer_id: id,
        file_hash: ContentHash::from_bytes([0x55; 32]),
    });
    assert_eq!(
        failure(receiver.receive(mismatch, NOW, DEADLINE, &mut sink)).error(),
        TransferError::IntegrityMismatch
    );
}

fn fresh_offer_ignores_claim_complete() {
    let id = transfer_id(12);
    let bytes = b"fresh-means-bytes".to_vec();
    let claim = ClaimedComplete {
        file_size: ByteCount::new(bytes.len() as u64),
        file_hash: ContentHash::from_bytes(*blake3::hash(&bytes).as_bytes()),
    };
    let request = request(id, &bytes, 4, ResumeMode::Disabled);
    let mut source = MemorySource::new(bytes.clone());
    let mut sink = MemorySink::default();
    let (sender, receiver) = begin(request, &mut source, &mut sink, Some(claim)).unwrap();
    let driven = drive(sender, receiver, &mut source, &mut sink).unwrap();
    assert!(driven.chunks > 0);
    assert_eq!(driven.chunk_bytes, bytes.len());
    assert_eq!(sink.sealed.as_ref(), Some(&bytes));
}

fn exact_ingress_and_resume_validation() {
    assert_eq!(next_chunk_index(0, 4), Some(0));
    assert_eq!(next_chunk_index(1, 4), Some(1));
    assert_eq!(next_chunk_index(4, 4), Some(1));
    assert_eq!(next_chunk_index(5, 4), Some(2));
    assert_eq!(next_chunk_index(5, 0), None);

    let wrong_id = exact_chunk_failure(Chunk {
        transfer_id: transfer_id(99),
        index: 0,
        offset: ByteCount::new(0),
        bytes: vec![0; 4],
    });
    assert_eq!(
        wrong_id,
        TransferError::Protocol(ProtocolViolation::TransferIdMismatch)
    );
    let wrong_index = exact_chunk_failure(Chunk {
        transfer_id: transfer_id(5),
        index: 1,
        offset: ByteCount::new(0),
        bytes: vec![0; 4],
    });
    assert_eq!(
        wrong_index,
        TransferError::Protocol(ProtocolViolation::ChunkIndex {
            actual: 1,
            expected: 0,
        })
    );
    let wrong_offset = exact_chunk_failure(Chunk {
        transfer_id: transfer_id(5),
        index: 0,
        offset: ByteCount::new(1),
        bytes: vec![0; 4],
    });
    assert_eq!(
        wrong_offset,
        TransferError::Protocol(ProtocolViolation::ChunkOffset {
            actual: 1,
            expected: 0,
        })
    );
    for length in [0, 3, 5] {
        let error = exact_chunk_failure(Chunk {
            transfer_id: transfer_id(5),
            index: 0,
            offset: ByteCount::new(0),
            bytes: vec![0; length],
        });
        assert_eq!(
            error,
            TransferError::Protocol(ProtocolViolation::ChunkLength {
                actual: length,
                expected: 4,
            })
        );
    }

    chunk_size_mismatch_is_refused();
    oversized_resume_is_refused();
    disabled_resume_is_refused();
    inconsistent_resume_index_on_the_wire_is_refused();
}

fn completion_order_and_strict_ack() {
    let id = transfer_id(6);
    let bytes = b"verify-before-seal".to_vec();
    let request = request(id, &bytes, 4, ResumeMode::Allowed);
    let mut source = MemorySource::new(bytes.clone());
    let mut sink = MemorySink::default();
    let (mut sender, mut receiver) = begin(request.clone(), &mut source, &mut sink, None).unwrap();
    while sender.progress().bytes_sent != sender.progress().file_size {
        let (next, frame) = one_sender_chunk(sender, &mut source);
        sender = next;
        receiver = continue_receiver(receiver, frame, &mut sink);
    }
    let mismatch = Frame::Complete(Complete {
        transfer_id: id,
        file_hash: ContentHash::from_bytes([0xaa; 32]),
    });
    let mismatch = failure(receiver.receive(mismatch, NOW, DEADLINE, &mut sink));
    assert_eq!(mismatch.error(), TransferError::IntegrityMismatch);
    assert_eq!(
        mismatch.abort().map(|abort| abort.reason),
        Some(ProtocolReason::IntegrityMismatch)
    );
    assert!(sink.sealed.is_none());

    let mut source = MemorySource::new(bytes.clone());
    let mut sink = MemorySink {
        fail_seal: true,
        ..MemorySink::default()
    };
    let (sender, receiver) = begin(request.clone(), &mut source, &mut sink, None).unwrap();
    let seal_failure = failure(drive(sender, receiver, &mut source, &mut sink));
    assert_eq!(
        seal_failure.error(),
        TransferError::Storage(StorageFault::new(StorageOperation::Seal))
    );
    assert_eq!(
        seal_failure.abort().map(|abort| abort.reason),
        Some(ProtocolReason::StorageFault)
    );
    assert!(sink.sealed.is_none());

    let mut source = MemorySource::new(bytes);
    let mut sink = MemorySink::default();
    let (mut sender, mut receiver) = begin(request, &mut source, &mut sink, None).unwrap();
    let (await_ack, completed) = loop {
        match sender.next_frame(&mut source).unwrap() {
            SenderStep::Chunk { state, frame, .. } => {
                sender = state;
                receiver = continue_receiver(receiver, frame, &mut sink);
            }
            SenderStep::Complete { state, frame } => {
                let ReceiverStep::ReadyToCommit(ready) =
                    receiver.receive(frame, NOW, DEADLINE, &mut sink).unwrap()
                else {
                    panic!("receiver did not complete");
                };
                assert!(
                    sink.sealed.is_none(),
                    "verification must not seal before the attempt commit gate"
                );
                let completed = ready.commit(&mut sink).unwrap();
                break (state, completed);
            }
        }
    };
    assert_eq!(sink.sealed.as_ref().map(Vec::len), Some(18));
    drop(completed.acknowledgement());
    let timed_out = failure(await_ack.deadline_exceeded(EXPIRED));
    assert_eq!(timed_out.error(), TransferError::Timeout);
    assert_eq!(timed_out.error().outcome_code(), OutcomeCode::Timeout);
}

fn exact_chunk_failure(chunk: Chunk) -> TransferError {
    let id = transfer_id(5);
    let bytes = vec![7; 10];
    let request = request(id, &bytes, 4, ResumeMode::Allowed);
    let mut source = MemorySource::new(bytes);
    let mut sink = MemorySink::default();
    let (_sender, receiver) = begin(request, &mut source, &mut sink, None).unwrap();
    let failure = failure(receiver.receive(Frame::Chunk(chunk), NOW, DEADLINE, &mut sink));
    assert_eq!(
        failure.abort().map(|abort| abort.reason),
        Some(ProtocolReason::ProtocolViolation)
    );
    failure.error()
}

fn chunk_size_mismatch_is_refused() {
    let id = transfer_id(7);
    let request = request(id, &[0_u8; 8], 4, ResumeMode::Allowed);
    let (sender, hello) = sender_start(request, DEADLINE);
    let receiver = receiver_start(ByteCount::new(8), DEADLINE).unwrap();
    let (receiver, ready) = receiver.receive_hello(hello, NOW, DEADLINE).unwrap();
    let (_sender, header) = sender.receive_ready(ready, NOW, DEADLINE).unwrap();
    let failure =
        failure(receiver.receive_header(header, NOW, DEADLINE, None, &mut MemorySink::default()));
    assert_eq!(
        failure.error(),
        TransferError::Protocol(ProtocolViolation::ChunkSizeMismatch {
            sender: 4,
            receiver: 8,
        })
    );
}

fn oversized_resume_is_refused() {
    let id = transfer_id(8);
    let request = request(id, &[0_u8; 8], 4, ResumeMode::Allowed);
    let mut source = MemorySource::new(vec![0; 8]);
    let (sender, _hello) = sender_start(request, DEADLINE);
    let (sender, header) = sender
        .receive_ready(Frame::Ready(envoix_protocol::Ready), NOW, DEADLINE)
        .unwrap();
    let Frame::FileHeader(_) = header else {
        panic!("expected header");
    };
    let status = Frame::ResumeStatus(ResumeStatus {
        transfer_id: id,
        next_chunk_index: 3,
        bytes_received: ByteCount::new(9),
        prefix_hash: Some(ContentHash::from_bytes([0; 32])),
    });
    assert_eq!(
        failure(sender.receive_resume(status, NOW, DEADLINE, &mut source)).error(),
        TransferError::Protocol(ProtocolViolation::ResumeOffsetExceedsFile {
            offset: 9,
            file_size: 8,
        })
    );
}

fn disabled_resume_is_refused() {
    let id = transfer_id(13);
    let request = request(id, &[0_u8; 8], 4, ResumeMode::Disabled);
    let mut source = MemorySource::new(vec![0; 8]);
    let (sender, _hello) = sender_start(request, DEADLINE);
    let (sender, _header) = sender
        .receive_ready(Frame::Ready(envoix_protocol::Ready), NOW, DEADLINE)
        .unwrap();
    let status = Frame::ResumeStatus(ResumeStatus {
        transfer_id: id,
        next_chunk_index: 1,
        bytes_received: ByteCount::new(4),
        prefix_hash: Some(ContentHash::from_bytes(*blake3::hash(&[0; 4]).as_bytes())),
    });
    assert_eq!(
        failure(sender.receive_resume(status, NOW, DEADLINE, &mut source)).error(),
        TransferError::Protocol(ProtocolViolation::ResumeNotAllowed { offset: 4 })
    );
}

/// The receiver no longer STORES an index to disagree with its own offset — it
/// derives one. But a peer can still send an index that disagrees with the
/// offset beside it, and that claim comes from another machine, so the sender
/// still checks it. This test moved here from the sink side when the stored
/// field went away; the violation did not.
fn inconsistent_resume_index_on_the_wire_is_refused() {
    let id = transfer_id(9);
    let bytes = vec![0_u8; 8];
    let request = request(id, &bytes, 4, ResumeMode::Allowed);
    let mut source = MemorySource::new(bytes);
    let mut sink = MemorySink::default();

    let (sender, hello) = sender_start(request, DEADLINE);
    let receiver = receiver_start(ByteCount::new(4), DEADLINE).unwrap();
    let (receiver, ready) = receiver.receive_hello(hello, NOW, DEADLINE).unwrap();
    let (sender, header) = sender.receive_ready(ready, NOW, DEADLINE).unwrap();
    let (_receiver, honest) = receiver
        .receive_header(header, NOW, DEADLINE, None, &mut sink)
        .unwrap();

    // An honest receiver at offset 0 is at index 0. Claim otherwise.
    let Frame::ResumeStatus(mut status) = honest else {
        panic!("expected a resume status");
    };
    assert_eq!(status.next_chunk_index, 0);
    status.next_chunk_index = 2;

    assert_eq!(
        failure(sender.receive_resume(Frame::ResumeStatus(status), NOW, DEADLINE, &mut source))
            .error(),
        TransferError::Protocol(ProtocolViolation::ResumeIndexInconsistent {
            actual: 2,
            expected: 0,
        })
    );
}

fn one_sender_chunk(sender: SenderSending, source: &mut MemorySource) -> (SenderSending, Frame) {
    match sender.next_frame(source).unwrap() {
        SenderStep::Chunk { state, frame, .. } => (state, frame),
        SenderStep::Complete { .. } => panic!("expected chunk"),
    }
}

fn continue_receiver(
    receiver: ReceiverReceiving,
    frame: Frame,
    sink: &mut MemorySink,
) -> ReceiverReceiving {
    match receiver.receive(frame, NOW, DEADLINE, sink).unwrap() {
        ReceiverStep::Continue { state, .. } => state,
        ReceiverStep::ReadyToCommit(_) => panic!("expected receiving state"),
    }
}

#[test]
fn storage_injected_fault_resume() {
    let id = transfer_id(10);
    let chunk_size = 1024 * 1024;
    let bytes = (0..CHECKPOINT_INTERVAL as usize + chunk_size + 17)
        .map(|index| index as u8)
        .collect::<Vec<_>>();
    let request = request(id, &bytes, chunk_size, ResumeMode::Allowed);
    let mut source = MemorySource::new(bytes.clone());
    let mut sink = MemorySink {
        fail_append_at: Some(8),
        partial_write_on_failure: 257,
        ..MemorySink::default()
    };
    let (mut sender, mut receiver) = begin(request.clone(), &mut source, &mut sink, None).unwrap();

    for _ in 0..8 {
        let (next, frame) = one_sender_chunk(sender, &mut source);
        sender = next;
        receiver = continue_receiver(receiver, frame, &mut sink);
    }
    assert_eq!(
        sink.prefix.map(|prefix| prefix.length),
        Some(ByteCount::new(CHECKPOINT_INTERVAL))
    );
    let (_sender, failed_frame) = one_sender_chunk(sender, &mut source);
    let failed = failure(receiver.receive(failed_frame, NOW, DEADLINE, &mut sink));
    assert_eq!(
        failed.error(),
        TransferError::Storage(StorageFault::new(StorageOperation::AppendStaging))
    );
    assert_eq!(
        failed.abort().map(|abort| abort.reason),
        Some(ProtocolReason::StorageFault)
    );
    assert_eq!(
        sink.prefix.map(|prefix| prefix.length),
        Some(ByteCount::new(CHECKPOINT_INTERVAL))
    );
    assert_eq!(
        Some(sink.staged.len()),
        Some(CHECKPOINT_INTERVAL as usize + 257)
    );

    let (sender, receiver) = begin(request, &mut source, &mut sink, None).unwrap();
    assert_eq!(
        sender.progress().resumed_bytes,
        ByteCount::new(CHECKPOINT_INTERVAL)
    );
    assert_eq!(Some(sink.staged.len()), Some(CHECKPOINT_INTERVAL as usize));
    let driven = drive(sender, receiver, &mut source, &mut sink).unwrap();
    assert_eq!(
        driven.chunk_bytes,
        bytes.len() - CHECKPOINT_INTERVAL as usize
    );
    assert_eq!(sink.sealed.as_ref(), Some(&bytes));
    assert_eq!(
        sink.sealed_hash,
        Some(ContentHash::from_bytes(*blake3::hash(&bytes).as_bytes()))
    );
}

#[test]
fn transfer_wait_closure() {
    let id = transfer_id(11);
    // `[1; 4]` because this case drives a send to completion below, and a
    // request now states what its source must hash to.
    let request = request(id, &[1_u8; 4], 4, ResumeMode::Allowed);
    let (ready, hello) = sender_start(request.clone(), DEADLINE);
    assert_eq!(
        failure(ready.deadline_exceeded(EXPIRED)).error(),
        TransferError::Timeout
    );
    assert_eq!(
        sender_start(request.clone(), DEADLINE)
            .0
            .peer_closed()
            .error(),
        TransferError::PeerClosed
    );
    assert_abort(
        sender_start(request.clone(), DEADLINE).0.cancelled(),
        ProtocolReason::Cancelled,
    );
    assert_abort(
        sender_start(request.clone(), DEADLINE).0.paused(),
        ProtocolReason::Paused,
    );

    let receiver = receiver_start(ByteCount::new(4), DEADLINE).unwrap();
    assert_eq!(
        failure(receiver.deadline_exceeded(EXPIRED)).error(),
        TransferError::Timeout
    );
    let receiver = receiver_start(ByteCount::new(4), DEADLINE).unwrap();
    assert_abort(receiver.cancelled(), ProtocolReason::Cancelled);
    let receiver = receiver_start(ByteCount::new(4), DEADLINE).unwrap();
    let (header, ready_frame) = receiver.receive_hello(hello, NOW, DEADLINE).unwrap();
    assert_eq!(
        failure(header.deadline_exceeded(EXPIRED)).error(),
        TransferError::Timeout
    );

    let (sender, _hello) = sender_start(request.clone(), DEADLINE);
    let (resume, file_header) = sender.receive_ready(ready_frame, NOW, DEADLINE).unwrap();
    assert_eq!(
        failure(resume.deadline_exceeded(EXPIRED)).error(),
        TransferError::Timeout
    );

    let mut source = MemorySource::new(vec![1; 4]);
    let mut sink = MemorySink::default();
    let (sender, receiver) = begin(request.clone(), &mut source, &mut sink, None).unwrap();
    let (_sender, chunk) = one_sender_chunk(sender, &mut source);
    let receiver = continue_receiver(receiver, chunk, &mut sink);
    assert_eq!(
        failure(receiver.deadline_exceeded(EXPIRED, &mut sink)).error(),
        TransferError::Timeout
    );

    let mut source = MemorySource::new(vec![1; 4]);
    let mut sink = MemorySink::default();
    let (sender, receiver) = begin(request, &mut source, &mut sink, None).unwrap();
    let (sender, chunk) = one_sender_chunk(sender, &mut source);
    let receiver = continue_receiver(receiver, chunk, &mut sink);
    let SenderStep::Complete { state: ack, .. } = sender.next_frame(&mut source).unwrap() else {
        panic!("expected complete");
    };
    assert_eq!(
        failure(ack.deadline_exceeded(EXPIRED)).error(),
        TransferError::Timeout
    );
    assert_abort(receiver.cancelled(&mut sink), ProtocolReason::Cancelled);

    let Frame::FileHeader(_) = file_header else {
        panic!("expected file header");
    };
}

fn assert_abort(failure: MachineFailure, expected: ProtocolReason) {
    assert_eq!(
        failure.outbound(),
        Some(Frame::Abort(Abort {
            transfer_id: failure.abort().and_then(|abort| abort.transfer_id),
            reason: expected,
        }))
    );
}

#[test]
fn errors_do_not_echo_peer_payloads() {
    let violation = TransferError::Protocol(ProtocolViolation::UnexpectedFrame {
        state: IngressState::AwaitReady,
        actual: FrameKind::Chunk,
    });
    let display = violation.to_string();
    assert!(display.contains("Chunk"));
    assert!(!display.contains("payload"));

    let receiver = receiver_start(ByteCount::new(4), DEADLINE).unwrap();
    let rejected =
        failure(receiver.receive_hello(Frame::Ready(envoix_protocol::Ready), NOW, DEADLINE));
    assert_eq!(
        rejected.abort().map(|abort| abort.reason),
        Some(ProtocolReason::ProtocolViolation)
    );
}
