## Title

Speed Limit and Backpressure

## Problem

Envoix has a speed limit setting placeholder, but transfer speed is not enforced.

The current Apple settings UI even hides the speed limit control because the core does not throttle bandwidth. This is the correct UI behavior for now, but the product needs a real speed limit and backpressure model before exposing the setting.

Speed limiting must be implemented in the shared transfer layer, not independently in each native client.

## Existing Implementation Found

Already present:

- `EnvoixRuntimeSettings.speed_limit_mbps` exists in FFI.
- The FFI comment says it is reserved for future throttling and currently advisory only.
- Apple `RuntimeSettingsProvider` passes `speedLimitMbps` to FFI.
- Apple settings show a message that speed limiting is not exposed because transfers do not enforce it.
- `TransferStats` and Apple `RateTracker` measure throughput.
- README documents enforced speed limits as not implemented.

Missing:

- core transfer throttle;
- backpressure strategy;
- speed limit in `TransferOptions`;
- pause/resume API;
- dynamic speed limit update;
- multi-transfer fairness;
- tests proving bandwidth is limited.

## Goal

Implement real upload/download speed limiting and backpressure in the shared Rust transfer/client layer.

Native apps should only configure and display the policy.

## Required Changes

### 1. Add speed policy to transfer options

Introduce a shared policy such as:

```text
TransferRatePolicy {
  upload_limit_bytes_per_sec: optional,
  download_limit_bytes_per_sec: optional,
  burst_bytes: optional,
}
```

Do not rely on Apple-only settings for behavior.

### 2. Implement sender-side throttling

Sender-side upload limiting should regulate how quickly payload chunks are read and sent.

This is the simplest v1 enforcement point and avoids uncontrolled memory buffering.

### 3. Define receiver-side backpressure

Receiver should naturally apply backpressure through the transport when disk writes or verification cannot keep up.

If explicit receiver download limiting is added, it must not corrupt timeout behavior or make sender completion semantics ambiguous.

### 4. Support pause and resume later

Speed limiting should be designed so that pause/resume can be added cleanly.

Pause is not just "speed limit 0" unless the protocol, UI state, and timeout behavior explicitly support that.

### 5. Multi-transfer fairness

When simultaneous transfers are allowed, the policy should define whether limits are:

- per transfer;
- per app;
- per direction;
- or inherited from a queue scheduler.

v1 can start with per-transfer limits, but the limitation must be documented.

### 6. Native UI exposure

Only expose the speed limit UI after the core enforces it.

Apple and future Android clients should use the same FFI policy fields.

## System Boundary

Throttling and backpressure belong in the Rust transfer/client layer.

Platform apps own:

- settings UI;
- low-power mode hints;
- metered network hints;
- foreground/background policy.

## Dependencies

GitHub issue: #42

Hard dependencies:

- Reliable Transfer Completion, Commit, and Resume Semantics, because throttling must not break timeout, commit, or final-ack behavior.
- Structured Transfer Events over FFI, because native clients need typed throughput and policy state.

Related issues:

- #40 Persistent Transfer Queue and Transfer Records, for future app-wide bandwidth fairness.
- #43 Parallel Transfer Design, because parallel scheduling must respect bandwidth limits.

## Out of Scope

- Parallel transfer scheduler
- Adaptive congestion control replacement
- OS-level QoS integration
- Background transfer service
- Per-peer trust policy
- UI exposure before enforcement

## Acceptance Criteria

- A transfer option can specify an enforced speed limit.
- Sender-side throttling limits observed throughput within a reasonable tolerance in tests.
- No unbounded buffering is introduced.
- Completion and timeout semantics remain correct under throttling.
- FFI exposes the enforced speed policy to native clients.
- Apple speed limit UI is only enabled once enforcement exists.
- Documentation no longer describes speed limit as merely advisory after implementation.

## Follow-up Issues

- Pause/resume controls.
- App-wide bandwidth budgeting.
- Low-power or metered-network policies.
- Interaction with parallel transfer scheduler.
- Adaptive transfer profiles.
