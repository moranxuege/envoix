# Apple physical-test remediation plan — 2026-07-16

Status: **Implementation in progress; the M0/M1 liveness and M4 state/UI
slices are automated, physical revalidation is pending**

This document is a physical-test remediation proposal and evidence appendix.
`docs/design/apple-client-execution-plan.md` remains the canonical cross-issue
schedule, while GitHub issues remain the canonical active-work records. After
the milestone order is confirmed, reconcile the accepted work into those
sources; this proposal does not independently supersede them.

Source evidence:

- `docs/handoffs/2026-07-16-apple-manual-test-ledger.md`
- user-supplied iOS/macOS Activity reports from 2026-07-16
- read-only persisted Manifest records recovered from the physical iPhone
- current `feat/transfer-state-foundation` source at `f74852e`

## Consolidated closure status — 2026-07-17

This table is the short operational summary for the full manual-test campaign.
It separates defects that are already physically accepted from the current
state/UI repair and from observations that still lack a stable reproduction.

| Area | Status | Current conclusion |
| --- | --- | --- |
| Post-connect Manifest source re-read and zero-byte connection loss | Fixed and physically accepted | The redundant source preflight hash was removed; fresh three-file Manifest transfers start payload promptly after connection. |
| Retained single-file confirmation, receiver lifecycle, and missing destination behind a stale receipt | Fixed and physically accepted | Tests 21–23 prove real rematerialization when the file is missing and explicit zero-payload reuse when it is present. |
| Share Extension draft disappearing across app activation/relaunch | Fixed and physically accepted | Draft ownership no longer deletes durable staging during lease release; Test 27 reopened and transferred both items. |
| Single-file pause/resume | Fixed and physically accepted | Test 28 reused the same `142,606,336`-byte prefix on both peers and completed. |
| Release Rust core and App Group staging | Fixed and physically accepted for the normal path | Apple builds use the Release core; staging requests an APFS clone first and falls back to ordinary copy. This does not eliminate Photos/iCloud provider waits. |
| Activity badge placement and canonical Pause/Cancel availability | Fixed; automated coverage present | The badge belongs to Activity, and visible commands are derived from canonical lifecycle/action availability rather than the setup card's local phase. |
| Terminal Activity leaking back into Transfer setup | Implemented; physical revalidation pending | Terminal snapshots remain in Activity while the setup presentation slot returns to idle. |
| Ambiguous “Send Again” / “Receive Again” actions | Implemented; physical revalidation pending | Generic repeat labels are removed; idle setup uses explicit fresh Send/Start Receiving actions, while Resume remains Activity-owned. |
| Scanning a QR for the wrong local role | Implemented; automated UI coverage passed, physical revalidation pending | The app switches to the compatible Send/Receive setup and preserves an unfinished send selection instead of only showing a passive warning. |
| Activity deletion flicker, stale badge, or reappearing record | Implemented; automated failure-path coverage passed, physical relaunch revalidation pending | The card is tombstoned only while cleanup is pending, persisted records are polled for acknowledgement, resources are released only after disappearance, and timeout restores the card with a retry message. |
| macOS destination permission appearing after pairing | Open (`R4`) | Destination access must still be proven/pre-authorized before a live receive; no new transport code is included in this UI/state slice. |
| Approximately 30-second Room receiver wait before joining | Recorded and deferred | Tests 25, 27, and 28 point to relay-home readiness before Room join. A later fix must preserve relay fallback. |
| Hotspot Direct-candidate stalls/path black holes | Recorded; controlled workaround validated | Candidate allow-list tests were stable, but the general multi-candidate policy remains transport work. Wi-Fi Aware is being developed separately and is outside this change. |
| Some/hidden Photos videos stuck in preparation | Recorded, not currently reproducible | No durable Activity exists for the failed cases. Provider progress, representation type, iCloud retrieval, and APFS-clone/copy duration must be instrumented before changing integrity or staging behavior. |
| Logical/skipped/resumed/payload byte domains, speed, and ETA | Deferred by product decision | Transfers are correct, but Manifest summaries cannot yet state the exact physical payload separately. Speed/ETA work is intentionally later. |
| WeChat accepting one/two/three exported videos inconsistently | Observation, not yet an Envoix defect | The system share target's acceptance changes with the selected asset set. Capture a repeatable asset/entry-path case before changing Envoix export behavior. |

The current repair deliberately does not alter Android's JNI session transport,
Iroh candidate selection, relay readiness, Wi-Fi Aware, or the Photos provider
pipeline. Those areas have separate compatibility and reproduction gates.

This plan covers the Apple product behavior exercised by the physical iPhone
15 Pro Max and macOS app. Shared Rust/UniFFI changes must remain compatible with
the current Android client. It does not treat wired transport, Wi-Fi Aware, or
parallel transfer as part of the immediate bug-fix scope.

## Implementation update — 2026-07-16

The current remediation worktree, based on `f74852e`, implements the first
diagnostic and P0-liveness slice without claiming that the remaining physical
product issues are complete.

Implemented and covered by automated regressions:

- reports now identify iOS and macOS correctly and include core version, FFI
  API version, sorted capabilities, and a short executable SHA-256 fingerprint;
- diagnostic-only text such as `accept stream opened` remains in the Activity
  section and no longer creates a false `[failure]` section;
- pasteboard writes return and verify a result, so a failed write no longer
  produces a false success toast; Share/Save fallback remains pending;
- an additive `ManifestTransferObserverV2` forwards rate-limited structured
  Manifest/per-entry events to Apple diagnostics, while the original observer
  and start/restore entry points remain available for existing clients;
- Manifest peers exchange `Hello` before source validation, and post-connect
  preflight now checks only regular-file type, size, and recorded modification
  time instead of performing another full BLAKE3 read;
- stream-time BLAKE3 remains authoritative, and a same-size source mutation is
  rejected before receiver publication;
- negotiated single-file receive events now update canonical Started,
  Progress, Verifying, and Verified lifecycle state as they occur;
- the compatibility projection consumes the already verified transfer hash
  instead of reopening and hashing the completed file, and terminal receiver
  records receive a non-zero `started_at`;
- retained single-file re-confirmation is covered over dual-ALPN and exact Room
  paths. Those current-source regressions complete without creating a duplicate
  destination file. The specific physical Test 02 failure was not reproduced,
  so the next device run remains the acceptance gate for that incident.

Compatibility and automated evidence:

- the FFI API is now version 3 with capability
  `manifest_diagnostic_events_v1`; V1 observer symbols remain generated;
- `envoix-transfer`, `envoix-client`, `envoix-session`, and `envoix-ffi` tests
  pass, including the new mutation, lifecycle, event-bridge, and retained-file
  regressions;
- Clippy passes for all four affected crates with warnings denied;
- macOS hosted tests pass (20 executed, 11 expected cross-device skips) and iOS
  hosted tests pass (48 executed, 10 expected cross-device skips);
- focused M4 regressions pass: one hosted terminal-slot test plus five UI tests
  covering wrong-role QR switching, canonical lifecycle actions, cancel timeout,
  and durable-removal timeout; a macOS Debug build also succeeds with the shared
  Send/Receive/Activity changes;
- the Android Debug APK builds successfully with the updated Rust core, which
  verifies that the additive FFI change does not break the current Android
  client build.

Still pending and intentionally outside this slice:

- pre-authorizing the destination before pairing (`R4`);
- initial Photos/App Group materialization and initial Manifest hashing progress
  (`M2`);
- separate logical, skipped, resumed, and physical-payload byte domains (`M3`);
- physical acceptance of the implemented terminal Activity ownership, explicit
  fresh-action labels, wrong-role QR switching, and acknowledged deletion
  behavior (`M4`);
- wired transport investigation (`M5`).

Physical revalidation update:

- post-fix Test 08 passed the retained-single-file gate: both peers completed
  with the same transfer ID, the sender reused all `371,926,650` bytes, and no
  delivery-unconfirmed or connection-loss fallback occurred;
- the fixed reports passed the installation/diagnostic gate (`core_ffi_api=3`,
  endpoint fingerprints, populated Manifest events, no false failure section);
- existing-destination and full-prefix BLAKE3 verification still consumed
  roughly five seconds per side, and receiver reused-byte accounting remained
  zero; these remain M2/M3 work rather than regressions in the M1 fix;
- a pre-test cancelled receive exposed stale Pause/Cancel actions and labelled
  a Room-with-mDNS-fallback Activity as `mDNS`; this is added to M4's terminal
  presentation/action-semantics scope.
- post-fix Test 09 transferred a three-file, 1.29 GB Manifest in approximately
  37 seconds after connection. All lightweight source checks and first payload
  progress occurred in the connection timestamp second, so the former
  connection-time full-source preflight stall did not recur;
- the user-visible iOS preparation before sender Activity creation remained
  long and remains unmeasured, reinforcing M2's requirement to create the
  preparation Activity before materialization/staging/initial hashing;
- the macOS sidebar placed a pending-Activity count on `Transfer`; the badge
  belongs on `Activity` and is tracked as a small M4 presentation correction.

## Product facts that should not be regressed

- Fresh Direct payload throughput is healthy in the observed topology:
  approximately 27–32 MB/s for 372–862 MB transfers.
- Single-file and Manifest transfers work in both Apple directions.
- Manifest conflict planning correctly classified retained files as
  `skipped_identical` in Tests 04 and 05.
- Test 05 transferred only the new 489,982,341-byte file while retaining two
  existing files.
- BLAKE3 end-to-end integrity, resume safety, and skip-identical identity are
  required product properties. The plan removes redundant work, not integrity
  verification.
- Direct and relay fallback remain product requirements. A nearby-device fix
  must not silently make Auto direct-only.
- `FfiTransferActivityRecord` remains the canonical UI lifecycle source. Raw
  diagnostic events must not become a second native state machine.

## Consolidated findings

| ID | Priority | Finding | Evidence and current cause boundary |
| --- | --- | --- | --- |
| R1 | P0 | Manifest can leave an authenticated connection idle while re-reading every source file | The earlier approximately 992 MB two-video run connected but transferred 0 bytes and ended `connection lost`. Tests 03/04/05/07 show 11, 23, 29, and 12–15 second post-connection waits. `send_manifest_with_cancel` runs `preflight_sources` before `Hello`, and `preflight_sources` performs a full BLAKE3 pass. |
| R2 | P0 | Re-sending one already-complete single file can fail during completion confirmation | Test 02 recognized all 371,926,650 bytes as resumed, sent no ordinary payload, then the sender became `delivery unconfirmed`, the receiver reported connection loss, and fallback ended paused. The full-resume `Complete`/`CompleteAck`/close path needs a dedicated reproduction. |
| R3 | P0 | A negotiated single-file receive is projected incorrectly through the Manifest receiver | Test 01 sender completed while macOS remained `connecting`; Test 06 completed with receiver `started_at=0`. The Manifest driver ignores single-file lifecycle events and only adopts the result after the run, then reopens and hashes the completed file through `ManifestSendRequest::from_paths`. |
| R4 | P1 | Destination permission can interrupt a live transfer | The first Downloads run paused while macOS requested access. Destination authorization should be established before advertising/joining, not during payload publication. |
| P1 | P1 | Multi-file preparation is long and mostly invisible | The delay occurs for iOS Photos and ordinary macOS file selection, proving it is not solely Photos/App Group staging. Initial Manifest construction hashes every regular file before the durable sender Activity exists. |
| P2 | P1 | iOS Photos can materialize data more than once | `loadFileRepresentation` supplies a temporary representation and Envoix persists it to App Group for extension/main-app handoff. The persistent copy has a lifecycle purpose, but the current data path does not fuse durable writing with hashing and exposes poor progress. |
| P3 | P1 | Repeated identical-file planning still requires long reads | Test 04 sent zero payload but took about 23 seconds after connection. The receiver hashes existing destination files to prove identity; the sender also performs redundant source verification. |
| P4 | P1 | Current test builds do not represent an optimized Rust core | `build-apple-core.sh` defaulted to Rust Debug; Test 08's approximately 5 seconds per 371.9 MB scales closely to the approximately 17-second three-video preparation baseline. The default-profile fix is implemented pending a Release-core physical A/B; the hash loop remains sequential until that evidence justifies further complexity. |
| P5 | P1 | Some Photos videos can remain in an opaque preparation stage before an Activity exists | Both Apple import paths use `NSItemProvider.loadFileRepresentation`. The returned `Progress` is kept only for cancellation, and the following App Group materialization discards whether APFS cloned or fell back to a full copy. The current UI and report cannot distinguish Photos retrieval/export, iCloud download, or durable-copy latency from a hang. |
| A1 | P1 | Logical completion bytes and physical payload bytes are conflated | Test 05 jumped immediately by exactly 800,127,969 skipped bytes and then transferred 489,982,341 new bytes. The aggregate Activity reports 1,290,110,310 completed bytes and zero resumed bytes. |
| A2 | P1 | Average speed can be computed from the wrong byte domain | Apple `averageBps` divides aggregate logical bytes by transfer duration. Test 05 can therefore imply about 71.7 MB/s although the new payload rate was about 27.2 MB/s. Core live-rate tracking partially avoids the first skipped baseline, but the UI/report does not explain the distinction. |
| A3 | P1 | `resumed_bytes` does not describe skip-identical work | Tests 04/05 report zero resumed bytes even though 800,127,969 bytes were skipped as identical. Resume and skip are different concepts, but no separate skipped-byte counter is exposed. |
| D1 | P1 | A non-failure diagnostic can create a `[failure]` report section | `TransferDiagnostics.report` emits `[failure]` whenever `diagnosticMessage` is non-empty. `accept stream opened` is diagnostic-only but appeared as an unknown setup failure in Test 01. |
| D2 | P1 | Manifest reports often have an empty `[transfer_events]` section | Tests 03–07 retained an Activity timeline but no structured transfer-event lines, reducing phase and per-entry evidence. |
| D3 | P1 | Report timestamps and lifecycle fields can contradict terminal state | Test 06 receiver completed with `started_at=0`; Test 01 remained connecting after sender completion. This overlaps R3 but also requires report-level regression coverage. |
| D4 | P1 | iOS report copy can fail without trustworthy feedback | Test 04's copy action failed. `copyToPasteboard` returns no result and the UI unconditionally announces success. The root cause is not yet reproduced. |
| D5 | P2 | Build identity is too weak for cross-device diagnosis | All reports show `0.1.0 (1)`. The platform header fix is present locally, but reports still lack a source/core fingerprint sufficient to prove both installed apps are synchronized. |
| U1 | P1 | macOS Transfer retains the last terminal operation | `TransferViewModel` keeps `transferActivity` after completion/failure; `TransferSetupStageView` treats every non-idle phase as recent Activity. Terminal history therefore leaks into setup. |
| U2 | P1 | “Send Again” and “Receive Again” have undefined product semantics | These labels are selected for every completed/canceled/failed phase. They create a fresh operation but do not state whether source items, Room, destination, transfer identity, or resume data are reused. |
| U3 | P1 | QR role mismatch stops at a passive message | Send rejects a Send-role QR and Receive rejects a Receive-role QR with one line of text. The parsed role is known, but no transition to the compatible local role is offered. |
| U4 | P1 | “Preparing” does not tell the user what work is occurring | Provider materialization, durable staging, source hashing, destination comparison, pairing, and connection are compressed into similar waiting labels with little byte progress. |
| U5 | P1 | Activity deletion can fail without an acknowledged result | The Apple client removes the card and releases resources while ignoring durable-session `remove()` results. FFI reports only command enqueue success; asynchronous durable-record/partial cleanup has no completion acknowledgement. A failure can therefore look like frontend flicker, stale badge state, or a record returning after relaunch even when the cause is below SwiftUI. |
| C1 | Deferred | Envoix does not deliberately select a wired Apple path | AirDrop reported a wired connection in the comparison run. This is a capability investigation, not evidence that the current Direct payload path is broken. |

## Non-bugs and wording corrections

- Repeated direct IPv4/IPv6 path landmarks are not by themselves a failure;
  Iroh may migrate between viable Direct addresses.
- Test 04 did not retransmit 800.1 MB. Both raw sender entries were
  `skipped_identical`; aggregate bytes represented logical completion.
- The two 1202-character Test 04 reports were identical copies of the same
  macOS record, not two independent endpoints.
- A Share Extension-to-main-app durable handoff cannot retain an
  `NSItemProvider` temporary URL after the provider callback. Eliminating every
  persistent copy is not a valid requirement; eliminating avoidable extra
  materialization and combining work is.

## Required semantic contracts before coding

### Byte domains

Keep the existing aggregate field compatible, then add explicit counters:

- `logical_completed_bytes`: entries completed, resumed, or skipped;
- `attempt_payload_bytes`: bytes actually read from the source and sent during
  this attempt;
- `resumed_bytes`: verified partial bytes reused for the same transfer;
- `skipped_identical_bytes`: completed destination content reused without
  payload transfer;
- `verified_bytes`: optional phase counter for hashing/comparison progress.

Existing `bytes_transferred` remains wire/source compatible and is documented
as aggregate logical completion for Manifest. Speed and ETA must use
`attempt_payload_bytes`, never aggregate logical bytes.

### Action semantics

| Context | Allowed primary action | Meaning |
| --- | --- | --- |
| Transfer setup, idle | Send / Start receiving | Create a new Activity from the visible setup fields. |
| Active transfer | View Activity | Do not duplicate lifecycle controls in setup. |
| Paused or retryable network loss | Resume | Continue the same Activity/transfer using retained partial state. |
| Publication failure | Retry saving / Choose folder | Re-publish staged received content without retransmitting. |
| Completed Activity | No generic retry | History is terminal. A future convenience action must say “Use these items for a new send” or “Receive a new transfer” and create a fresh Activity. |
| Failed, non-retryable Activity | Start a new transfer | Do not label a fresh operation as Retry. |
| Terminal Activity removal | Remove from Activity | Delete the durable record according to canonical cleanup policy. |

The first UI correction should remove generic “Send Again” and “Receive Again”
rather than invent hidden reuse behavior.

## Implementation milestones

### M0 — Diagnostics and reproducible regressions

Goal: make the next physical run self-explanatory before changing performance.

1. Land the existing platform-specific report header fix and tests.
2. Emit `[failure]` only for canonical failure-like states/metadata; keep
   diagnostic-only messages in a separate diagnostics field or log section.
3. Make pasteboard copy return success/failure, verify the written value where
   the platform permits, and offer Share/Save Report as a fallback.
4. Include app version, build, core crate/API version, capability list, and a
   source/build fingerprint in developer diagnostics.
5. Preserve Manifest transport and per-entry events in `[transfer_events]`.
6. Add monotonic phase timings and byte progress for:
   provider materialization, durable staging, Manifest hashing, source
   preflight, pairing, authentication, receiver conflict planning, payload,
   verification, and publication.
7. Add small deterministic regressions with injected slow hashing/ACK behavior;
   do not require gigabyte fixtures to reproduce timeouts.

M0 acceptance:

- `accept stream opened` never appears under `[failure]` for a connecting
  record;
- a failed pasteboard write never shows “copied”;
- a Manifest report contains phase/per-entry evidence;
- installed iOS and macOS builds can be distinguished without relying on the
  user remembering which bundle was installed.

### M1 — P0 liveness and completion correctness

Goal: no authenticated connection should die because Envoix performs avoidable
local work, and both peers must agree on terminal completion.

1. Remove the connection-time full source BLAKE3 pass from
   `preflight_sources`.
2. Before connecting, validate regular-file type, size, and a cheap source
   fingerprint. After authentication, send Manifest `Hello` and `Offer`
   immediately.
3. Keep stream-time BLAKE3 and receiver final verification. If a same-size
   source changes after preparation, fail before publication when the streamed
   hash differs from the offered hash.
4. Keep transport liveness active while the receiver compares existing files;
   this is defense in depth, not a substitute for removing redundant work.
5. Reproduce and fix the full-resume single-file `CompleteAck`/close race from
   Test 02. A fully reused file must send zero payload and still complete on
   both peers.
6. Normalize negotiated single-file receive lifecycle into timely canonical
   snapshots. Do not wait until the end to show Started/Progress/Verifying.
7. Carry the already verified single-file hash/result into the compatibility
   projection; do not reopen and hash the completed file merely to synthesize
   a one-entry Manifest Activity.
8. Establish destination access before Room join/advertisement so a macOS
   permission prompt cannot stall a live connection.

M1 acceptance:

- Manifest sends perform no full source re-read after `Connected`;
- Manifest `Hello` is emitted promptly after authentication;
- same-size source mutation still cannot be published;
- Test 02's retained single file completes on both peers with zero payload;
- a negotiated single-file receiver reports non-zero `started_at`, file name,
  transfer ID, progress, and terminal state without a post-transfer hash delay;
- failure classification for connection loss is structured and retryable when
  retained state permits recovery.

### M2 — Multi-file preparation and Photos data path

Goal: one necessary data pass per purpose, visible and cancellable.

1. Create/persist the preparation Activity before initial Manifest hashing so
   the user sees byte/item progress immediately.
2. For iOS staging, compute BLAKE3 while writing the one durable App Group copy
   and persist the hash with the draft descriptor; Manifest construction reuses
   it after validating source identity.
3. In the main-app Photos path, evaluate `PHAssetResourceManager` direct writes
   to the durable destination with progress and iCloud download reporting.
4. In Share Extension, attempt in-place provider access when offered; otherwise
   retain the single durable-copy fallback. Do not depend on extension lifetime
   for a gigabyte network transfer.
5. Observe and persist the `NSItemProvider` progress, provider callback
   duration, selected type identifier, staged byte count, APFS cloned/copied
   result, and materialization duration. Keep asset names and paths out of
   routine logs; include them only in explicit diagnostic exports.
6. For ordinary macOS/Files URLs, keep one initial source hash but expose its
   progress and cancellation. Benchmark a larger buffered blocking/file I/O
   loop and Release Rust core before enabling Rayon or other parallel hashing.
7. Add a trusted completed-file hash cache keyed by destination identity, size,
   mtime/change token, and completed receipt. Rehash when any identity fact is
   unavailable or changed, especially for external File Providers.
8. Never let a cache hit weaken final payload verification or resume-prefix
   validation.

M2 acceptance:

- tapping Send produces visible preparation state promptly;
- provider/staging/hash phases show current item and byte progress and can be
  canceled;
- a fresh multi-file send has no duplicate source hash pass;
- an unchanged local destination can reach `skipped_identical` without a full
  rehash, while modified/suspicious files fall back to hashing;
- Release and Debug measurements are reported separately, with CPU time, read
  bytes, and wall time per phase.

### M3 — Honest progress, speed, and reports

Goal: progress may be logical, but speed must describe physical work.

1. Add the byte-domain counters additively through Rust records, UniFFI, Swift,
   and generated Android bindings.
2. Derive live/average/peak speed and ETA from `attempt_payload_bytes` only.
3. Show a clear summary such as “800.1 MB already present · 490.0 MB sent” for
   Test 05-shaped transfers.
4. Report skipped file count/bytes, resumed bytes, payload bytes, logical total,
   and per-entry results explicitly.
5. Preserve existing aggregate fields for older native consumers; do not rename
   or silently change their wire meaning.

M3 exact regression fixture:

- logical total: `1,290,110,310`;
- skipped identical: `800,127,969` across two files;
- payload this attempt: `489,982,341` across one file;
- resumed: `0`;
- speed/ETA inputs: only `489,982,341` physical payload bytes.

### M4 — Transfer setup and role UX

Goal: setup creates transfers; Activity owns lifecycle and history.

1. Detach terminal `TransferViewModel` presentation from the macOS Transfer
   page after snapshotting diagnostics. Keep the durable record in Activity.
2. On completion, show a bounded confirmation/toast and return setup to idle;
   reopening Transfer must not resurrect the last completed/failed operation.
3. Remove generic “Send Again” and “Receive Again”. Keep Resume/Retry only where
   canonical `recovery_action` defines same-Activity work.
4. If convenience replay is added later, use explicit labels and show exactly
   which source/destination fields are reused before creating a new Activity.
5. When a scanner sees a QR for the same local role, parse the known remote role
   and offer one clear action: “Switch to Receive” or “Switch to Send”.
6. Do not silently discard a selected send draft. A role switch must preserve
   it for later or ask for confirmation.
7. Replace generic “Preparing” with the typed phase labels introduced in M0/M2.
8. Make Activity removal transactional from the user's perspective: retain the
   card until durable cleanup is acknowledged, surface failures, and reconcile
   a timed-out response from persisted records. Do not release ShareDraft or
   publication resources before the owning cleanup outcome is known.

M4 acceptance:

- terminal history appears only in Activity, not Transfer setup;
- every visible action maps to one canonical command or an explicitly fresh
  Activity;
- scanning a sender QR in Send offers a one-step transition to Receive without
  losing selected items;
- deleting an Activity removes its durable record and exact owned sidecars,
  stays deleted after relaunch, and reports a cleanup failure instead of
  optimistically hiding the card;
- macOS and iOS hosted/UI tests cover completed, paused, retryable failure,
  publication failure, role mismatch, and fresh setup.

### M5 — Deferred transport capability investigation

After M0–M4 are stable:

1. Determine what Apple means by AirDrop's reported wired connection in the
   tested topology and whether a public API exposes a usable interface/path.
2. Determine whether Iroh already sees that interface as a Direct candidate or
   needs an additive native transport provider.
3. Keep this separate from Wi-Fi Aware and from current Room/Direct/relay
   correctness. Do not advertise wired support before a physical payload/hash
   gate proves it.

## Verification strategy

### Automated gates

- Add Rust unit/integration tests before each transport fix.
- Cover source-mutation, full-resume ACK, delayed receiver planning,
  skip-identical counters, and negotiated single-file projection.
- Add focused Swift hosted tests for report sections, copy result handling,
  terminal setup reset, action labels, and QR role transition.
- Regenerate UniFFI bindings for additive fields and compile the current Android
  client; Apple work may not leave generated Android consumers broken.
- Wrap direct Cargo/Xcode builds with `scripts/with-build-cache-guard.sh`; use
  `scripts/apple-dev.sh` and stable DerivedData roots as required by repository
  policy.
- Create standalone `.xcresult` bundles only for milestone evidence.

### Physical acceptance matrix

| Gate | Scenario | Required evidence |
| --- | --- | --- |
| G1 | iOS -> macOS, fresh single approximately 372 MB | Both terminal Completed; receiver lifecycle populated; exact final size/hash; Direct path. |
| G2 | Repeat G1 with destination retained | Zero payload; both terminal Completed; no delivery-unconfirmed fallback. |
| G3 | iOS -> macOS, fresh two-file approximately 800 MB Manifest | No post-connect source rehash; exact two files/hash; honest phase timing. |
| G4 | Repeat G3 with both files retained | `skipped_identical_bytes=800127969`; payload 0; prompt terminal completion. |
| G5 | Two retained files plus one new file | Exact Test 05 counters; speed uses only 489,982,341 payload bytes. |
| G6 | macOS -> iOS, fresh two-file approximately 862 MB Manifest | Same liveness/counter/integrity requirements in reverse. |
| G7 | Photos Share Extension -> macOS, large multi-item | One durable staging pass, visible progress, no connection idle timeout, exact final files/hash. |
| G8 | Fresh macOS Downloads authorization | Permission established before pairing; no live-transfer permission stall. |

Each gate records source/build fingerprint, phase durations, physical/logical
byte counters, selected data path, terminal state on both peers, destination
paths, final sizes, and independent hashes.

## Commit and review slices

Keep changes surgical and reviewable:

1. current report-header fix + ledger/plan;
2. report truthfulness, copy result, and phase instrumentation;
3. Manifest post-connect liveness fix;
4. single-file full-resume and negotiated-receiver lifecycle fix;
5. additive byte accounting and native bindings;
6. Transfer/action/QR UX semantics;
7. Photos/staging/hash fusion and destination hash cache;
8. final automated and physical acceptance evidence.

Do not combine protocol correctness, Photos provider redesign, UI navigation,
and wired transport into one patch.

## Recommended starting point

Start with M0, then M1. These two milestones make diagnostics trustworthy and
remove the paths that can lose a connection or disagree on completion. M2–M4
then improve preparation, metrics, and product semantics without obscuring P0
regressions. M5 remains a separate capability decision.
