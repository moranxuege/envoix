## Title

Back Apple Activity with Transfer Records

## Problem

The Apple app currently has transfer state, but it does not have transfer records.

`Sender` and `Receiver` each own a `TransferViewModel`. The Activity tab reads those two view models directly, so Activity is only a summary of the two tabs. It is not yet a real transfer center.

This is acceptable for the current single-send / single-receive prototype, but it will not scale to:

- sender-initiated transfer flows;
- multiple transfers;
- trusted devices;
- retry/resume;
- multi-file transfer;
- useful diagnostics.

Before those features are added, the Apple app needs a shared Activity store that represents each transfer attempt as a structured record.

## Current Behavior

- Sender owns send state.
- Receiver owns receive state.
- Activity reads Sender and Receiver state directly.
- There is no stable transfer record for an active, completed, failed, or canceled attempt.
- Sender and Receiver cannot jump to a specific Activity item.
- Activity cannot reliably show a transfer timeline.

## Goal

Make Activity the central place for detailed transfer state in the Apple app.

Sender and Receiver should still show minimal local feedback, but detailed transfer state should live in Activity records.

## Dependency

This issue should be implemented after, or together with, structured FFI transfer events.

Activity should consume typed event data, not parse free-form status strings.

## Required Changes

### 1. Add a Transfer Activity Record

Add an Apple app model for one transfer attempt.

At minimum:

```text
id
direction: send | receive
mode: invite | room | mdns | manual
phase: waiting | pairing | connecting | transferring | verifying | completed | failed | canceled
file_name
bytes_transferred
total_bytes
average_speed
data_path: direct | relay | unknown
last_error
created_at
updated_at
```

This should be UI state, not transfer execution logic.

### 2. Add an Activity Store

Add a shared store owned by `AppModel`.

Responsibilities:

- create a record when a transfer starts;
- update records from structured transfer events;
- mark terminal states;
- expose active and recent records to the Activity tab;
- keep records in memory for v1.

Persistent history is out of scope.

### 3. Move Detailed State Out of Sender and Receiver

Sender and Receiver should keep only lightweight state:

- waiting;
- connecting;
- transferring;
- completed;
- failed;
- canceled;
- `View in Activity`.

Detailed information belongs in Activity:

- pairing stage;
- selected data path;
- speed;
- progress;
- error details;
- diagnostics-ready state.

### 4. Add View in Activity

When a send or receive is active, completed, or failed, Sender and Receiver should provide a clear way to open Activity.

For v1, jumping to the Activity tab is enough. Deep-linking to a specific row can be a follow-up.

## Out of Scope

- Persistent transfer history
- Retry/resume actions
- Multi-file or folder details
- Trusted-device grouping
- Full diagnostics export
- Redesigning app navigation
- Supporting more than the current send/receive concurrency model

## Acceptance Criteria

- Starting a send creates an Activity record.
- Starting a receive creates an Activity record.
- Activity updates from structured transfer events.
- Activity shows phase, progress, speed, mode, direction, path, and last error.
- Sender and Receiver can navigate to Activity.
- Sender and Receiver no longer duplicate detailed diagnostic UI.
- Activity does not own transfer execution logic.
- Existing invite, room, and mDNS flows continue to work.
- Activity does not parse free-form status strings for core state.

## Follow-up Issues

- Persist recent transfer history.
- Add retry/resume from Activity.
- Add copyable diagnostic reports.
- Extend Activity records for multi-file manifests.
- Group Activity records by trusted device.
