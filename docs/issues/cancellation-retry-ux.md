## Title

Cancellation and Retry UX

## Problem

Envoix supports cancellation and resumable single-file transfer at the core level, but the product semantics are still too coarse.

Today, cancellation and failures can look similar once they reach native UI. The Apple app suppresses the next failure after user cancellation and shows a local canceled state, but the lower layers and future clients still need a shared model for:

- who canceled;
- whether partial state was kept;
- whether retry is safe;
- whether retry should resume or restart;
- what action the user should see.

Without structured cancellation and retry semantics, Activity, trusted-device flow, Android clients, and transfer queue behavior will diverge.

## Existing Implementation Found

Already present:

- `Transfer::cancel()` exists in `envoix-client`.
- `TransferCancelToken` notifies transfer loops.
- The transfer protocol sends an interruption error when possible.
- Resume sidecar state exists for single-file receive.
- Apple `TransferViewModel.cancel()` calls `session.cancel()`.
- Apple UI differentiates `.canceled` from `.failed(String)`.
- Apple send/receive buttons become `Cancel Transfer` while busy.

Missing:

- structured cancel origin;
- structured retryability;
- explicit "resume vs restart" UI state;
- user-visible partial-file management;
- discard-partial action;
- retry action tied to a previous transfer attempt;
- shared cross-platform policy.

## Goal

Define how a transfer attempt can be canceled, failed, retried, resumed, or discarded in a way that works across Apple and future Android/desktop clients.

The v1 product rule should be:

```text
Cancellation is not a generic failure.
Retry must be explicit and based on structured failure data.
```

## Required Changes

### 1. Cancellation semantics

Represent cancellation explicitly:

```text
Cancellation {
  origin: local_user | remote_user | system | unknown,
  phase,
  partial_state: none | retained | discarded | unknown,
}
```

User-initiated cancellation should not appear as a scary transfer failure.

Remote cancellation should be visible as "The other device canceled" rather than "connection lost" when the protocol can tell.

### 2. Partial state policy

Define what happens to receiver-side partial files and sidecars:

- default: keep compatible partial state for future resume;
- explicit action: discard partial state;
- failed validation: delete or ignore unsafe partial state;
- completed transfer: clean sidecar state.

The UI must not expose `.part` files as completed user files.

### 3. Retry eligibility

Use structured failure data, not strings, to decide whether retry is offered.

Expected retry policy:

- retry/resume allowed: network lost, peer temporarily unreachable, missing final ack, local app interruption;
- retry after user action: permission denied, disk full, bookmark expired, no local network permission;
- retry not automatic: authentication failure, protocol error, unsupported feature, hash mismatch;
- cancel: show restart/resume options only if partial state exists.

### 4. Retry action model

A retry should be attached to a previous transfer record or transfer queue item.

Retry input should include:

- direction;
- source file or receive directory;
- peer source or peer identity;
- transfer mode;
- resume preference;
- previous attempt id;
- known partial state, if any.

Do not implement blind automatic retry loops in this issue.

### 5. UI requirements

Native clients should support these states:

- canceled by me;
- canceled by peer;
- failed but retryable;
- failed and needs user action;
- failed and cannot be retried safely;
- partial data retained;
- partial data discarded.

Apple can be the first UI implementation, but the state model must be exposed through FFI for Android and future clients.

## System Boundary

The core/client layer should classify cancellation and retryability.

Native clients should present actions and platform-specific fixes, such as choosing a new Files folder on iOS or opening local network settings.

## Dependencies

GitHub issue: #39

Hard dependencies:

- #38 Structured Error Model and Diagnostics Pipeline, because retry eligibility must not be inferred from strings.
- Reliable Transfer Completion, Commit, and Resume Semantics, because retry/resume depends on knowing whether the receiver committed the file.

Full-scope dependencies:

- #40 Persistent Transfer Queue and Transfer Records, for queue-backed retry actions.
- Apple Activity transfer records, for a stable place to show retry/resume/discard actions.

## Out of Scope

- Automatic background retry
- Persistent trusted-device send queue
- Multi-file manifest retry
- Parallel retry scheduler
- OS background task implementation
- Full diagnostic report UI

## Acceptance Criteria

- Local user cancellation and remote cancellation are distinguishable when possible.
- Cancellation does not surface as a generic failure in native UI.
- Retry eligibility is derived from structured failure data.
- UI can show retry/resume/restart/discard actions without parsing error strings.
- Partial state retention and deletion rules are documented.
- Tests cover cancellation mapping and at least one retryable failure classification.

## Follow-up Issues

- Add retry/resume actions in Activity.
- Add a Resume Inbox for partial transfers.
- Add queue-backed retry after transfer queue exists.
- Extend retry semantics to `ManifestV1`.
