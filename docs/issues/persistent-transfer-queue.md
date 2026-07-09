## Title

Persistent Transfer Queue and Transfer Records

## Problem

Envoix currently starts transfers as direct operations.

In the Apple app, `AppModel` owns two long-lived `TransferViewModel` instances: one for receiving and one for sending. The Activity tab reads those view models directly. This works for the current prototype, but it does not scale to:

- multiple pending transfers;
- retry and resume;
- trusted-device auto receive;
- sender/receiver flows that jump into Activity;
- transfer history;
- Android and desktop clients using the same transfer model.

The product needs a transfer queue and transfer records as a shared system concept, not only a Swift UI array.

## Existing Implementation Found

Already present:

- `envoix-client::Transfer` is a handle for one running transfer.
- `TransferStats` is derived from the event stream.
- Apple `AppModel` shares `send` and `receive` view models across window and menu bar.
- Apple Activity exists, but reads the two view models directly.
- `docs/issues/apple-activity-transfer-records.md` already covers an Apple in-memory Activity store.

Missing:

- queue item model;
- stable attempt ids;
- persisted transfer records;
- retry/restart state;
- transfer queue API in Rust/FFI;
- platform-independent state machine;
- persisted partial-transfer visibility.

## Goal

Introduce a cross-platform transfer queue model that can represent active, pending, completed, failed, canceled, and resumable transfer items.

Apple Activity should become one consumer of this model. Future Android and desktop clients should use the same queue semantics through FFI or a platform binding.

## Required Changes

### 1. Define queue item and attempt records

Create a shared model similar to:

```text
TransferQueueItem {
  queue_id,
  created_at,
  updated_at,
  direction,
  peer_source,
  peer_identity,
  local_source,
  destination,
  status,
  current_attempt_id,
  attempts,
  resumable_state,
}

TransferAttempt {
  attempt_id,
  started_at,
  ended_at,
  mode,
  transfer_id,
  selected_path,
  bytes_transferred,
  total_bytes,
  result,
  failure,
}
```

The exact storage fields can be smaller in v1, but the queue must distinguish a logical transfer item from one network attempt.

### 2. Add queue state machine

Suggested statuses:

```text
draft
queued
waiting_for_peer
connecting
transferring
verifying
committing
completed
failed
canceled
paused
resumable
discarded
```

The queue should consume structured transfer events and structured failures.

### 3. Persist minimal records

Persist enough information to show recent transfer history and resume incomplete work after app restart.

v1 persistence can be conservative:

- completed/failed/canceled metadata;
- resumable receive sidecar references;
- user-visible file name and size;
- peer label when available;
- no sensitive token or invite persistence unless explicitly designed.

### 4. Add FFI API

Expose queue operations through FFI, for example:

```text
list_transfer_items()
start_queue_item(queue_id)
cancel_queue_item(queue_id)
retry_queue_item(queue_id)
discard_partial(queue_id)
observe_queue_events()
```

The exact interface can be adapted to UniFFI constraints, but Apple and Android should not each invent their own queue semantics.

### 5. Update Apple Activity

Apple Activity should render queue records instead of reading only `send` and `receive` view models.

Sender and Receiver can still show compact local state, but detailed state and retry actions should live in Activity.

## System Boundary

Queue state, attempt identity, retry eligibility, and persistence semantics belong in Rust client/FFI or a shared platform-neutral layer.

Platform apps own:

- file picker integration;
- permission prompts;
- platform-specific storage locations;
- native navigation and presentation.

## Dependencies

GitHub issue: #40

Hard dependencies:

- Structured Transfer Events over FFI, because queue items should update from typed lifecycle events.
- #38 Structured Error Model and Diagnostics Pipeline, because failed queue items need structured failure data.
- Reliable Transfer Completion, Commit, and Resume Semantics, because queue terminal states must not be inferred from progress alone.

Related issues:

- Apple Activity transfer records, because Apple Activity is the first UI consumer.
- #39 Cancellation and Retry UX, because retry actions should eventually operate on queue items.
- #43 Parallel Transfer Design, because queue scheduling becomes important once one user action can own several concurrent transfer units.

## Out of Scope

- Long-running background service
- Trusted-device auto send
- Cloud sync of transfer history
- Full multi-file manifest execution
- Automatic retry scheduling
- End-to-end encrypted queue persistence

## Acceptance Criteria

- A transfer queue item model is documented and implemented in the shared layer.
- Running transfers are associated with attempt ids.
- Queue records can represent completed, failed, canceled, and resumable states.
- Native clients can observe queue changes through a stable API.
- Apple Activity consumes queue records rather than only the two current view models.
- Android can implement a UI against the same queue contract later.
- Sensitive pairing tokens and invite secrets are not persisted accidentally.

## Follow-up Issues

- Persist recent transfer history.
- Add Resume Inbox for partial transfers.
- Group queue records by trusted device.
- Add background availability policies per platform.
