## Title

Expose Structured Transfer Events over FFI

## Problem

The Rust client API already emits structured transfer events, but the FFI layer collapses many of them into free-form status strings before they reach native apps.

For example, structured events such as:

```text
Binding { direction, mode }
Pairing { step }
Connected { path }
PathChanged { path }
Failed { direction, reason }
```

are currently surfaced to Swift mostly as:

```text
on_status("binding ...")
on_status("pairing: ...")
on_status("connected via ...")
```

This makes the Apple app depend on display text instead of typed state. It also blocks reliable Activity, diagnostics, localization, and future retry/resume behavior.

## Current Behavior

- `envoix-client` exposes a rich `TransferEvent` enum.
- `envoix-ffi` exposes a smaller `TransferObserver` callback interface.
- Progress and terminal success are structured enough.
- Pairing, connection mode, selected data path, phase changes, and some errors are reduced to strings.
- Native clients cannot reconstruct the full transfer timeline without parsing human-readable messages.

## Goal

Expose enough structured transfer event data through FFI so native apps can render transfer state without parsing status strings.

`on_status(String)` may remain for human-readable logs, but it must not be the primary data source for transfer state.

## Required Changes

### 1. Add FFI-Safe Event Types

Add UniFFI-compatible records/enums for transfer events.

At minimum, native clients should receive structured data for:

- transfer direction;
- rendezvous mode;
- transfer phase;
- transfer id when available;
- file name;
- progress bytes;
- pairing step;
- selected data path;
- terminal error.

These types should avoid Rust-only or iroh-specific public types at the FFI boundary.

### 2. Add a Structured Event Callback

Add a callback such as:

```text
on_transfer_event(event)
```

or equivalent typed callbacks if UniFFI constraints make one enum awkward.

The callback should preserve the meaning of `envoix_client::api::TransferEvent`.

### 3. Keep Status Text Secondary

Keep `on_status(String)` only for human-readable display/logging.

Native clients should not need to parse strings like:

```text
"pairing: peer matched"
"connected via relay (...)"
```

to determine app state.

### 4. Preserve Existing FFI Behavior During Migration

Existing callbacks should continue to work while native clients migrate:

- `on_invite_ready`
- `on_started`
- `on_progress`
- `on_completed`
- `on_failed`
- `on_status`

The new structured callback can be additive.

## Out of Scope

- Apple Activity UI redesign
- Transfer history persistence
- Retry/resume actions
- Multi-file manifests
- Trusted devices
- Changing the core transfer protocol
- Replacing all existing observer callbacks in one step

## Acceptance Criteria

- Swift can receive structured pairing step updates.
- Swift can receive structured data path updates.
- Swift can receive transfer direction and mode without parsing text.
- Swift can receive transfer id and file metadata when available.
- Existing FFI tests still pass.
- New FFI tests cover at least one structured event sequence.
- `on_status(String)` is no longer required for core transfer state.
- The generated Swift bindings expose the new structured event types cleanly.

## Follow-up Issues

- Back Apple Activity with transfer records.
- Add copyable diagnostics from structured event history.
- Persist recent transfer records.
