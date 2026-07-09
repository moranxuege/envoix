## Title

Reliable Transfer Completion, Commit, and Resume Semantics

## Problem

Envoix already has important reliability building blocks in the single-file transfer path:

- receiver-side resumable temp files;
- resume sidecar state;
- BLAKE3 file verification;
- finalization before receiver completion;
- `CompleteAck` from receiver to sender.

However, these semantics are not yet clearly documented as protocol guarantees, and the native UI/FFI layers still expose a coarse completion model.

This creates room for ambiguous states such as:

- payload bytes reached 100%, but the file has not been verified;
- the receiver verified bytes, but final file commit failed;
- the sender sent all bytes, but did not receive a receiver completion acknowledgment;
- a retry resumes from partial state that is not clearly tied to the expected file;
- UI displays completion before the receiver has durably committed the file.

The project needs a strict definition of when a transfer is actually complete.

## Goal

Formalize and harden the completion, commit, and resume semantics for single-file transfer before building higher-level features such as manifest transfer, trusted-device auto receive, and parallel transfer.

The user-facing rule should be:

```text
100% progress is not completion.
Completion means the receiver verified and committed the file, and the sender observed the receiver's completion acknowledgment.
```

## Required Semantics

### 1. Distinguish progress from completion

The implementation and UI must distinguish these states:

```text
bytes_transferred
bytes_received
verifying
verified
committing
committed
receiver_completed
sender_acknowledged_completion
failed
```

`Progress { bytes_transferred == total_bytes }` must not be treated as final completion by native apps.

### 2. Receiver commit rule

The receiver must only report completion after all of these steps succeed:

- all expected bytes have been received;
- received byte count matches the expected file size;
- BLAKE3 verification matches the sender's final hash;
- the temp file has been finalized into the destination file;
- resume sidecar state has been cleaned up or marked complete.

If any of these steps fail, the receiver must report failure and must not emit a completed event.

### 3. Sender completion rule

The sender must only report completion after:

- all file bytes have been sent;
- the final `Complete` frame has been sent;
- the receiver returned `CompleteAck` for the same transfer id.

If the final acknowledgment is not received, the sender must report a retryable failure instead of success.

### 4. Resume state validation

Resume state must be accepted only when it is consistent with the expected file.

At minimum, validation should include:

- file name;
- expected file size;
- chunk size;
- recorded byte count;
- next chunk index;
- temp file length;
- transfer id rebinding rules;
- hash checkpoint or prefix hash verification.

If resume state is inconsistent, the receiver should either safely restart from zero or fail clearly. It must not blindly append to an untrusted partial file.

### 5. Terminal event consistency

For each transfer attempt, the event stream exposed to higher layers must produce exactly one terminal result:

```text
completed | failed | canceled
```

Native layers should not infer terminal state from progress alone.

### 6. UI and FFI mapping

The FFI and Apple UI should expose enough structure to represent:

- transferring;
- verifying;
- committing;
- completed;
- failed;
- retryable failure;
- bytes already resumed.

This issue does not require the full Activity registry redesign, but the completion semantics should be compatible with the future Activity transfer records.

## Out of Scope

- Multi-file manifest transfer
- Per-file manifest resume
- Parallel transfer
- Speed limiting
- File-level E2E encryption
- Trusted-device auto receive
- Interactive conflict resolution
- Retry scheduling policy

## Acceptance Criteria

- Documentation states that 100% progress is not completion.
- Receiver completion is defined as verify plus durable commit.
- Sender completion is defined as receiving `CompleteAck`.
- Failed finalization or hash mismatch is reported as failure, not success.
- Resume state validation rules are documented and covered by tests.
- Lost or missing `CompleteAck` is surfaced as a retryable transfer failure.
- Native UI does not mark a transfer completed based only on progress reaching total bytes.
- Event/FFI behavior guarantees exactly one terminal result per transfer attempt.

## Follow-up Issues

- Add explicit commit-phase events if the current event stream is not expressive enough.
- Add FFI event variants for verifying, committing, retryable failure, and resumed bytes.
- Add UI states that distinguish 100% payload progress from verified completion.
- Extend the same semantics to `ManifestV1` per-file commit.
- Add integration tests for final-ACK loss, hash mismatch, destination commit failure, and resume sidecar corruption.
