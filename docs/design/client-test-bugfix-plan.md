# Post-demo client reliability bugfix plan

Status: **in progress**

Branch: `bugfix/client-test-reliability`

Baseline: `main@30fd70ac`, public release `v0.2.2`

Last reviewed: 2026-08-05

## 1. Goal and ownership boundary

The final demo is complete. This branch exists to make the client-diversity
evidence used by the thesis trustworthy, then fix the client defects exposed by
that evidence.

This branch owns:

- Apple, Android, and desktop client state and presentation correctness;
- source selection and composition;
- preparation, progress, and throughput instrumentation;
- persistent/remembered room and Bluetooth-history lifecycle;
- client-side capability and failure presentation for BLE, NFC, and Wi-Fi
  Aware; and
- the client dimensions of the issue #61 physical matrix.

Another team member owns controlled network-environment experiments. This
branch does not tune or compare routers, loss, latency, topology, relay
placement, IPv4, or IPv6. It records the typed selected path and client timings
needed by those experiments without inferring network causes.

## 2. Engineering rules

1. Keep every existing public API compatible unless a separately reviewed,
   additive migration is required.
2. Reproduce each bug with a deterministic test or a registered physical case
   before changing its implementation.
3. Native views render canonical state. They do not infer the current transfer,
   rate, retryability, or trust relationship from a global "latest" record.
4. Preparation, connection, payload, verification, and publication are separate
   measured phases. Payload throughput never includes preparation or final
   publication time.
5. Use named constants and validated bounds for event cadence, history expiry,
   retry budgets, and retained-record limits.
6. Do not combine Wi-Fi Aware or NFC architectural work with state, intake, or
   measurement fixes.
7. All direct Cargo, Xcode, and Gradle validation follows the repository build
   cache discipline in `AGENTS.md`.

## 3. Saved-device and room boundary

The product model is intentionally layered:

```text
SavedDeviceRecord (durable, one per authenticated remote installation)
  -> PersistentExchange (durable relationship, credentials and endpoint roles)
    -> RoomSession (ephemeral, one connection attempt/lifetime)
      -> TransferTask (explicit draft/activity/attempt ownership)
```

- Apple, Android, and desktop clients present the durable top-level object as a
  **Saved device**, not as a room. Each platform stores its own local record;
  neither endpoint may project the other endpoint's local record as a shared
  room.
- A reconnect creates or resumes a `RoomSession`; it does not turn the room
  instance into the durable device identity. Room status and transfer activity
  remain scoped to that session/exchange.
- Existing `RememberedPeerRecord`, `RememberedRoom*`, relationship IDs,
  credentials, files, and automation identifiers remain readable during the
  compatibility phase. Their visible UI labels change first; internal names
  move only through an additive, versioned migration.
- Migration maps one valid legacy remembered-peer record to one saved-device
  record and its existing default exchange. Orphaned credentials or identities
  without metadata are quarantined for deterministic cleanup; they are not
  recreated from nearby history.
- Nearby display name, RSSI, Bluetooth address, and the current ephemeral
  discovery key are hints only. Rediscovery is marked as a saved device only
  after an authenticated stable installation identity or relationship-derived
  rotating presence tag matches. A name match must never establish trust.
- One discovery advertisement cannot be assumed to represent every saved
  relationship. Multi-relationship presence requires a protocol-defined set of
  unlinkable rotating tags and bounded matching; until then the UI reports an
  ordinary nearby device and authenticates before association.

The first implementation slice changes the visible Apple/Android contract to
Saved devices while preserving legacy storage and API compatibility. The
protocol/data migration follows behind deterministic NFC and Wi-Fi Aware
client fixes; it must not be simulated with presentation-only matching.

## 4. Bug registry

| ID | Priority | Observed defect | Existing owner |
| --- | --- | --- | --- |
| `BFX-001` | P0 | A new transfer can display the previous transfer record. | #61; new dedicated regression |
| `BFX-002` | P0 | Apple/Android clients present incompatible throughput values or meanings. | #42, #61 |
| `BFX-003` | P0 | Progress advances in visible bursts instead of a coherent monotonic presentation. | #42, #61 |
| `BFX-004` | P0 | Large-file preparation is long and lacks an actionable phase breakdown. | #55, #61 |
| `BFX-005` | P1 | Persistent/remembered rooms reconnect slowly, time out, or become one-sided. | #56, #58, #59 |
| `BFX-006` | P1 | Bluetooth history can retain stale, duplicate, or incorrectly associated entries. | #58, #59; new dedicated regression |
| `BFX-007` | P1 | Photos and Files cannot be composed into one transfer job. | #55, #61; new dedicated regression |
| `BFX-008` | experimental | Wi-Fi Aware physical behavior is not reliable enough for a support claim. | #60, #61 |
| `BFX-009` | experimental | NFC handoff is not reliable enough for a support claim. | #57, #61 |

BLE foreground discovery and connection have improved in current physical
testing. That observation does not close authenticated BLE, history, or
persistent-room work.

## 5. Definition of done

The branch is ready to merge only when all applicable conditions hold:

- a new task is always projected by its explicit draft, Activity, task, and
  attempt identifiers; stale or late records cannot replace it;
- Apple and Android consume the same canonical byte/time fields and document
  the same rate units and phase boundaries;
- progress is monotonic within an attempt, stale callbacks are rejected, and
  presentation updates are bounded and cancel-responsive;
- preparation emits enough structured phase evidence to distinguish source
  staging, enumeration, hashing, compression analysis, sealing, and waiting;
- persistent-room reconnect has a bounded terminal outcome and never creates a
  second relationship, duplicate Activity, or one-sided success claim;
- ephemeral BLE observations, one-time rooms, remembered relationships, and
  trusted identity records are stored and expired under separate rules;
- one explicit draft can add Photos and Files sequentially and seal them into
  one canonical `TransferJob`/Manifest without copying a second transfer path;
- Wi-Fi Aware and NFC results are explicitly `experimental`, `failed`,
  `hardware_blocked`, or `unsupported` until their physical gates pass; and
- the selected client matrix produces machine-readable results with exact
  commit, build, endpoint role, source shape, Activity ID, phase timings, byte
  counts, hashes, terminal state, and selected path.

## 6. Execution phases

### Phase 0 — freeze the baseline and register reproductions

1. Keep `v0.2.2` immutable as the comparison baseline.
2. Add one case or deterministic fixture per `BFX-*` item before its fix.
3. Record failures without automatic rerun converting them into passes.
4. Use a fixed same-LAN setup for client comparisons. Record the path but leave
   network-environment interpretation to the network-test owner.
5. Store only test-owned fixtures and privacy-safe evidence.

Gate: every P0/P1 bug has a reproducible failing test or a physical case with an
explicit `not_reproduced` result and the missing precondition.

### Phase 1 — make Activity and performance evidence trustworthy

#### `BFX-001`: stale transfer projection

- Trace draft ID -> task ID -> Activity ID -> attempt ID across Rust, FFI, and
  native view models.
- Remove any product selection based on a global latest/preparing Activity.
- Reject late events from an older task or attempt before they mutate visible
  state.
- Cover consecutive transfers, simultaneous records, process restoration, room
  close/reopen, and a late terminal callback from the first transfer.

#### `BFX-002`: throughput contract

- Keep canonical values in bytes and monotonic milliseconds at the shared
  boundary.
- Define current rate as a named rolling payload window and average rate as
  payload bytes divided by payload-active elapsed time.
- Exclude preparation, pairing, verification, saving, and delivery-proof time
  from payload rate; expose their durations separately.
- Make Apple and Android unit conversion and labels consume the same fixtures.
- Independently recompute the final physical-test average from bytes and phase
  timestamps; UI text is not the evidence source.

#### `BFX-003`: progress cadence

- Audit whether burstiness originates in Rust chunk events, FFI coalescing,
  native projection, or actual payload stalls.
- Preserve monotonic byte progress and attempt ownership.
- Coalesce UI work under one named cadence policy without suppressing terminal
  or phase-transition events.
- Test tiny files, a normal file, a large file, pause/resume, and a deliberately
  slow receiver.

#### `BFX-004`: preparation latency

- Emit structured start/end/progress evidence for source acquisition, staging,
  enumeration, hashing, compression sampling/preparation, and Manifest seal.
- Keep cancellation responsive during every phase.
- Profile first; optimize only the measured dominant phase.
- Do not skip integrity or silently weaken immutable-source semantics to make
  the UI look faster.

Gate: the same recorded event fixture renders equivalent state, phase, bytes,
and rate meaning on Apple and Android; a large-file run explains all elapsed
time instead of leaving one opaque preparation interval.

### Phase 2 — repair durable relationship and history lifecycle

#### `BFX-005`: persistent/remembered room

- Specify the authoritative relationship ID, peer identity, local role, remote
  role, credential commit, and reconnect attempt record.
- Make the two-sided commit boundary explicit; one endpoint must not present a
  durable room when the peer did not commit the same relationship.
- Bound connection timeout, retry count, backoff, and terminal user action with
  named policy values.
- Reauthenticate the pinned peer on reconnect and reject identity/role changes.
- Test first connection, app restart on either side, both apps restarting,
  offline queueing, peer removal, identity change, and retry exhaustion.

#### `BFX-006`: Bluetooth history

- Separate ephemeral observations, incoming one-time offers, recently used
  presentation history, remembered relationships, and trusted devices.
- Define a stable deduplication key for each layer; never use display name or
  Bluetooth address as durable identity.
- Give ephemeral observations and offers explicit TTL/capacity bounds.
- Ensure removal/revocation clears only the intended layer and cannot recreate
  a ghost relationship from stale discovery data.
- Test duplicate BLE/mDNS observations, peer disappearance, rename, app restart,
  delayed callbacks, and removal followed by rediscovery.

Gate: three consecutive reconnect cycles per selected direction end in one
relationship and one current Activity, or one bounded structured failure; stale
BLE data cannot repopulate durable history.

### Phase 3 — compose Photos and Files in one job

#### `BFX-007`: mixed source draft

- Keep the platform system pickers separate, but let `Add Photos` and `Add
  Files` append to the same explicit draft before Send.
- Normalize both source types into the existing source-provider contract and
  one canonical Manifest.
- Preserve provider lifetime, security-scoped access, staging ownership,
  duplicate-name policy, cancellation, and exact cleanup.
- Show the combined inventory before sealing; no source may be silently dropped
  when the second picker returns.
- Cover photo -> file, file -> photo, canceling the second picker, duplicate
  names, multiple roots, process interruption during staging, and exact
  receiver hashes/tree.

Gate: Apple and Android each produce one Activity and one Manifest containing
the selected mixed roots; at least one Apple -> Android and one Android -> Apple
physical run publish the exact expected result.

### Phase 4 — contain experimental carriers

#### `BFX-008`: Wi-Fi Aware

- Re-run capability, pairing, path, teardown, and fallback cases with typed
  evidence.
- Do not infer support from entitlement, discovery, a display name, or one
  successful transfer.
- Fix only bounded client lifecycle defects that have a deterministic
  reproduction. Keep larger interoperability or platform limitations in #60.

#### `BFX-009`: NFC

- Separate tag/HCE detection, invitation read/write, role binding, expiry,
  replay rejection, and handoff-to-transfer results.
- Fix only bounded client defects with a reproducible physical or protocol
  case. Do not claim generic iPhone-to-iPhone NFC or NFC data transfer.
- 2026-08-05: fixed the iOS scene-lifecycle regression that cancelled Core NFC
  when Apple's system sheet made the scene temporarily inactive. The attached
  Xiaomi-to-iPhone private-AID path completed its APDU reads and the user
  confirmed that the room eventually connected. Perceived end-to-end latency
  remains open as a measured performance issue; it is not an NFC read failure.

Gate: #61 rows state the honest support result and first actionable failure.
Neither carrier blocks the stable Room/QR/BLE/local-network client matrix.

### Phase 5 — thesis client-diversity matrix

Required physical endpoint classes:

- macOS;
- iPhone/iOS;
- iPad/iPadOS compatibility mode;
- Android/Xiaomi; and
- Windows/Linux only when a real host is available; binary build success alone
  is not physical-client evidence.

Select representative rows instead of a full Cartesian product:

- macOS <-> iPhone;
- iPhone <-> Android;
- iPad <-> Android;
- one-entry Files input;
- Photos input;
- mixed Photos + Files input;
- 8 MiB baseline, 64 MiB progress/recovery profile, and a 1 GiB preparation
  profile before making a large-file claim;
- one-time Room/QR and current BLE handoff;
- persistent-room restart/reconnect; and
- explicit experimental NFC/Wi-Fi Aware rows.

For every execution record:

```text
commit / build variant / protocol version
sender / receiver / model / OS
source profile / exact bytes / hashes
draft_id / task_id / activity_id / attempt
invitation input / selected path
preparation_ms / connection_ms / payload_ms / finalize_ms
payload_bytes / independently computed average_bytes_per_second
progress_event_count / maximum_progress_gap_ms
terminal state / publication / cleanup
```

Critical stable rows require three consecutive strict successes per direction.
Experimental rows retain failures and skips as results; they never become a
pass through omission or automatic rerun.

## 7. Commit and review sequence

Use small, reviewable commits in this order:

1. plan and failing regression fixtures;
2. `BFX-001` Activity ownership;
3. `BFX-002`/`BFX-003` measurement and progress contract;
4. `BFX-004` preparation phase evidence, followed by measured optimization;
5. `BFX-005` persistent-room state;
6. `BFX-006` BLE history lifecycle;
7. `BFX-007` mixed source composition;
8. bounded `BFX-008`/`BFX-009` carrier fixes, if justified; and
9. matrix evidence and thesis-facing report.

Each implementation commit includes its regression test. Do not mix unrelated
formatting, refactoring, protocol changes, server/network tuning, or build-cache
cleanup into this branch.

## 8. Verification ladder

For each phase, run only the smallest applicable layer first, then expand:

1. pure Rust/Kotlin/Swift state and projection tests;
2. FFI/JNI and native integration tests;
3. registry validation and dry-run report generation;
4. platform release-equivalent build through the cache guard; and
5. selected physical #61 cases with exact result artifacts.

The branch does not merge on UI screenshots or free-form log messages alone.
Terminal state, bytes, hashes, identifiers, phase timestamps, publication, and
cleanup must be machine-readable.
