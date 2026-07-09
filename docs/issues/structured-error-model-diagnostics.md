## Title

Structured Error Model and Diagnostics Pipeline

## Problem

Envoix has a partial structured error model in `envoix-client`:

```text
TransferError { phase, kind, message }
```

However, this structure does not survive the full product boundary.

The current FFI observer still reports terminal failures as:

```text
on_failed(reason: String)
```

The Apple app then maps raw strings with `friendlyError(...)`. This is useful for a prototype, but it is not enough for debugging, localization, retry policy, Activity records, or future Android clients.

## Existing Implementation Found

Already present:

- `crates/envoix-client/src/api/error.rs` defines `Phase`, `ErrorKind`, and `TransferError`.
- `Transfer::wait()` maps core errors into `TransferError` using the latest observed phase.
- `TransferEvent::Failed` exists, but carries only `reason: String`.
- `crates/envoix-ffi/src/lib.rs` collapses `TransferError` into `error.to_string()`.
- Apple `TransferViewModel` stores `.failed(String)`.
- Apple `friendlyError(...)` maps some raw strings to friendlier text.
- `docs/design/client-api.md` already states that mobile needs finite error categories, not arbitrary error chains.

Missing:

- stable error codes;
- retryability;
- peer-vs-local origin;
- recovery actions;
- diagnostic details safe for logs;
- redaction rules;
- structured FFI records;
- structured Apple/Android UI consumption.

## Goal

Expose a machine-readable failure object from Rust core through FFI to native apps.

Native apps should not parse display strings to decide what happened or what action to show.

## Required Changes

### 1. Extend the public error shape

Introduce a stable error record, either by extending `TransferError` or by adding a UI-facing wrapper:

```text
TransferFailure {
  code: FailureCode,
  category: FailureCategory,
  phase: FailurePhase,
  origin: local | peer,
  direction: send | receive | unknown,
  transfer_id: optional,
  attempt_id: optional,
  retryable: bool,
  recovery_action: optional,
  user_message_key: String,
  diagnostic_message: String,
}
```

The exact Rust/UniFFI shape can differ, but the boundary must be machine-readable.

### 2. Define bounded failure codes

Start with a small v1 set:

```text
user_canceled
network_lost
peer_unreachable
authentication_failed
permission_denied
disk_full
hash_mismatch
protocol_error
destination_conflict
unsupported_feature
timeout
internal_error
unknown
```

Avoid leaking arbitrary internal error strings into decision logic.

### 3. Define failure phases

The existing `Phase` enum is a good start, but UI and diagnostics need enough detail for real debugging:

```text
setup
binding
advertising
pairing
connecting
authenticating
negotiating
transferring
verifying
committing
acknowledging
cleaning_up
```

This can be introduced incrementally, but the public model should not be limited to one generic `transfer` phase forever.

### 4. Add recovery actions

Expose suggested user actions as data, for example:

```text
retry
resume
choose_folder
open_settings
re_pair
update_app
switch_pairing_method
discard_partial
none
```

The UI decides how to present the action, but the classification should come from core/client logic.

### 5. Add peer-safe error frames

The current protocol `ErrorFrame` carries only a human-readable message.

Replace or extend it with a safe peer error payload:

```text
PeerErrorFrame {
  code,
  phase,
  retryable,
  safe_message
}
```

Do not send local paths, invite tokens, full peer identifiers, or sensitive OS details to the remote peer.

### 6. Preserve diagnostic detail locally

Local logs and Activity diagnostics may include more detail than peer-visible errors, but they must follow redaction rules.

At minimum, structured logs should include:

- transfer id when known;
- attempt id when available;
- direction;
- mode;
- phase;
- error code;
- retryable flag;
- selected data path when relevant.

## System Boundary

This must be solved in Rust core/client/FFI first.

Apple UI should consume structured failure records. Future Android, Windows, Linux, and Harmony clients should receive the same data model through their binding layer instead of re-implementing string matching.

## Dependencies

GitHub issue: #38

Hard dependencies:

- None.

Opened-batch dependents:

- #39 Cancellation and Retry UX
- #40 Persistent Transfer Queue and Transfer Records
- #44 Polish Receive Destination UX on Apple Platforms

Related issues:

- Structured Transfer Events over FFI, because failure records should travel through the same typed native boundary.
- Reliable Transfer Completion, Commit, and Resume Semantics, because completion and retryable final-ack failures need stable error codes.
- Apple Activity transfer records, because Activity should display structured diagnostics rather than raw strings.

## Out of Scope

- Full Activity redesign
- Automatic retry scheduling
- Persistent transfer queue
- Uploading diagnostic reports
- Crash reporting service integration
- Security audit

## Acceptance Criteria

- Public API exposes structured failure data beyond `Display`.
- FFI exposes structured failure data to native clients.
- `on_failed(reason: String)` is either replaced or kept only as a compatibility callback.
- Native clients can distinguish cancellation, network failure, permission failure, disk failure, auth failure, hash mismatch, and protocol mismatch without parsing strings.
- Peer-visible error frames are redacted and safe.
- Apple UI no longer relies on `friendlyError(...)` string matching for core failure classification.
- Tests cover several representative mappings from `CoreError` to public failure data.

## Follow-up Issues

- Cancellation and Retry UX
- Activity diagnostics from structured failures
- Copyable diagnostic report from Activity
- Android binding consumption of structured failure records
