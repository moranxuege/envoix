# Apple client execution plan

Status: **Draft — baseline locked; product decisions D1–D3 pending**

Owner: Apple client workstream

Last updated: 2026-07-13

Design reference: [`../../../Design.png`](../../../Design.png)

This document is the execution source of truth for the Envoix iOS and macOS
clients. GitHub issues remain canonical for issue-specific discussion; this
document owns cross-issue sequencing, gates, acceptance evidence, and the
current decision log.

No product implementation may start from this draft until Decisions D1–D3 are
confirmed. D4 is now resolved by the checkpoint and dev merge recorded below.

## 1. Outcome

Deliver an Apple client that:

1. behaves correctly on the agreed iPhone and macOS device matrix;
2. renders the durable Rust transfer record instead of implementing a second
   lifecycle in Swift;
3. can advance independently of Android application changes;
4. keeps existing Rust/UniFFI callers source-compatible through additive API
   evolution;
5. ships a first Apple-focused feature after the UI and state foundations are
   proven; and
6. is supported by reproducible builds, honest automated tests, and physical
   device evidence.

“Apple-independent” means that Android UI work and unfinished Android fixes do
not gate Apple delivery. It does not mean duplicating protocol, transfer,
security, retry, resume, or path-selection semantics in Swift. A pinned CLI,
Rust integration harness, or known-good mobile peer may be used when a second
endpoint is required.

## 2. Current baseline

The source baseline is locked; the Apple build baseline is not green yet.

- Branch: `feat/transfer-state-foundation`, tracking its origin branch.
- Safety checkpoint `ceff278` (`feat: checkpoint canonical mobile transfer
  lifecycle`) contains the entire previously dirty tree and was pushed to
  `origin/feat/transfer-state-foundation` before any merge.
- `origin/dev` at `5577194` was merged into the checkpoint as merge commit
  `2084944`. All 13 textual conflicts were resolved; the Android application
  takes the latest dev implementation, while shared Rust/UniFFI files use dev
  as the authority plus additive Apple compatibility.
- The merged shared core compiles for `envoix-client`, `envoix-ffi`, and
  `envoix-android-jni`. `envoix-client` has 89 passing library tests and
  `envoix-ffi` has 40 passing library/loopback tests.
- The Android Gradle APK build has not produced code evidence on this machine:
  the wrapper download for Gradle 8.9 timed out at roughly 30%. This is recorded
  as an environment failure, not treated as a passing or failing source build.
- The merge keeps dev's record-authoritative Remove, checked commit barrier,
  stamped receipt responses, committed sent-hash verification, durable platform
  extras/receipt endpoint, atomic final naming, receipt repair, and Android UDP
  compatibility changes.
- Apple compatibility retained on top includes stable listener identity/token,
  raw diagnostic/invite events, structured failures, staged receive publication,
  final published path, and public string Activity IDs. Core records remain
  `u64`; UniFFI maps string IDs deterministically through `platform_extras` and
  migrates legacy string-ID files without duplicate cards.
- A remote pause/loss now automatically parks a new rendezvous attempt on the
  peer, while the locally paused side still requires the user's Resume action.
  This is covered by QR and room loopback tests.
- The generated `crates/envoix-ffi/EnvoixCore/` package is absent. An exact-source
  Apple build therefore requires `scripts/build-apple-core.sh` before Xcode
  build or device evidence.
- The iPhone 15 Pro Max installed debug app predates this merged source and is
  not valid evidence for the new baseline.
- Historical simulator output reported ten tests, but four cross-device methods
  compile to empty bodies when `ENVOIX_CROSS_DEVICE_TESTING` is absent. The
  effective default coverage was six tests.
- Apple build and test jobs are not currently part of `.github/workflows/ci.yml`.
- GitHub issues [#44](https://github.com/ECE4410J-NUUB/envoix/issues/44)
  and [#45](https://github.com/ECE4410J-NUUB/envoix/issues/45) remain the primary
  Apple UX issues. Cross-platform records and discovery are tracked in
  [#40](https://github.com/ECE4410J-NUUB/envoix/issues/40) and
  [#41](https://github.com/ECE4410J-NUUB/envoix/issues/41).

The Rust/UniFFI merge is green at its tested boundary. G0 remains open until the
generated Apple core, macOS build, iOS build-for-testing, honest test count, and
physical-device installation all come from `2084944` or a documented descendant.

## 3. Decisions required before implementation

| ID | Decision | Proposed default | Status |
|---|---|---|---|
| D1 | Supported devices and orientations | iPhone + macOS; iPhone portrait first; no iPad promise in this milestone | Pending user confirmation |
| D2 | iOS navigation interaction | Keep the branded floating stage bar, but remove the global horizontal stage-swipe gesture | Pending user confirmation |
| D3 | First post-foundation feature | Single-file Share Extension first; design the Wi-Fi Aware provider boundary in parallel | Pending user confirmation |
| D4 | Baseline and shared-core authority | Full-tree checkpoint, latest dev merge, dev-authoritative Android, additive Apple Rust/UniFFI compatibility | **Confirmed and executed** |

### D4 resolution

The full-tree checkpoint and dev merge confirm that the Apple client cannot be
versioned safely by copying only `apps/envoix-apple/`. Apple development now
starts from `2084944` and follows these ownership rules:

1. latest dev owns the Android application and the shared correctness fixes it
   already merged;
2. Apple work does not wait for subsequent Android UI work;
3. shared Rust/UniFFI changes are additive and must compile both native callers;
4. cross-platform protocol behavior is validated with pinned loopback/harness or
   a known-good peer instead of depending on unfinished Android feature work;
5. any future dev merge repeats the checkpoint → merge → compatibility tests
   sequence used here.

## 4. Scope

### In scope

- iPhone and macOS SwiftUI layout, navigation, accessibility, localization, and
  platform-specific file interactions.
- Durable Activity rendering and Apple-owned publication resources.
- Additive Rust client/UniFFI contracts required to keep the Apple UI truthful.
- Apple build generation, simulator tests, CI, previews, and physical-device
  validation.
- One user-approved post-foundation feature.

### Out of scope unless separately approved

- Refactoring or repairing the Android application.
- Replacing the working transfer engine or wire protocol.
- Multi-file/directory transport before `TransferManifest v1` is accepted.
- Implementing a fake speed limit, scheduler, retry policy, or path selector in
  Swift.
- Claiming iPad, landscape, background transfer, Wi-Fi Aware, or remote-device
  support without the corresponding matrix and physical evidence.
- Signing, notarization, TestFlight, or App Store release unless added as a
  later milestone.

## 5. Architecture contract

### Rust core owns

- transfer lifecycle states and legal transitions;
- `activity_id`, `attempt_id`, monotonic `sequence`, and `transfer_id` rules;
- pause, resume, cancel, remove, retry, and publication transition policy;
- discovery/path fallback and connection strategy;
- pairing, authentication, encryption, receipt, integrity, and invite formats;
- partial-file identity, cleanup, resume, storage quota, and failure policy;
- global concurrency/backpressure when those features are implemented.

### Apple platform layer owns

- SwiftUI presentation, navigation, localization, and accessibility;
- Files/Finder pickers, security-scoped access, bookmarks, and platform
  publication resources keyed by `activity_id`;
- camera permission UX, QR scanning UI, notifications, Share Extension/App
  Intent integration, and capability presentation;
- translating typed core state into user-facing copy without parsing diagnostic
  strings as state.

### Boundary invariants

1. `FfiTransferActivityRecord` is the UI source of truth. Raw transfer events are
   diagnostic or invite inputs, not a second reducer.
2. A command response means accepted/rejected; the UI waits for the next record
   snapshot instead of optimistically inventing a terminal state.
3. `Publishing` is not `Completed`. A visible completed file must be available
   at the recorded final destination.
4. Swift rejects an incoming record whose `sequence` is older than the stored
   record for the same activity.
5. History limits may evict terminal history only; active/non-terminal records
   are never dropped to satisfy a display cap.
6. Main-actor delivery is explicit for callbacks that mutate observable Apple
   state.
7. New UniFFI surface is additive until every existing caller has migrated.
8. User-facing behavior is driven by typed state, failure code, recovery action,
   and capability data. Diagnostic prose is never a control protocol.

## 6. Known UI adaptation risks

The first implementation pass must address structure rather than per-device
padding patches.

1. `ContentView` owns a custom bottom stage bar through `safeAreaInset`.
2. `SendView` and `ReceiveView` independently own bottom CTA bars through a
   second `safeAreaInset`.
3. Both transfer views add a fixed `88` point bottom padding.
4. A global stage-switch drag and the Send/Receive role drag can recognize the
   same horizontal gesture.
5. The global drag uses `UIScreen.main.bounds.width`, which is not the active
   scene/container width in every window or multitasking configuration.
6. Fixed QR and control dimensions need coverage on small screens, large
   Dynamic Type, landscape if supported, and translated strings.
7. macOS card content needs an agreed maximum readable width.
8. Camera-denied UX needs a direct route to system Settings.
9. The dark-theme brand colors and destructive colors require contrast review.

## 7. Execution stages and gates

### Stage 0 — Lock decisions and preserve the baseline

Completed:

- D4 confirmed by the user's merge instruction;
- full-tree checkpoint `ceff278` created and pushed before merging;
- latest `origin/dev` (`5577194`) merged as `2084944` without unresolved files;
- shared-core/FFI/JNI compile check passed;
- 89 `envoix-client` and 40 `envoix-ffi` tests passed, including durable
  restore, QR/room pause-resume, mailbox receipt, publication, and legacy-ID
  migration.

Remaining actions:

- confirm D1–D3;
- rebuild `EnvoixCore` and regenerate the Xcode project;
- build macOS Debug and iOS Simulator `build-for-testing` from the exact merged
  source;
- reinstall the physical iPhone before recording behavior against the baseline;
- change disabled cross-device tests to report `XCTSkip` or isolate them in an
  explicit cross-device scheme;
- create parallel Apple worktrees only after G0 build evidence is green.

Gate G0 evidence:

- checkpoint commit/branch identifier and clean implementation worktree;
- successful Apple Core generation;
- exact build commands and exit status for macOS and iOS;
- honest executed/skipped test counts;
- physical-device version, installed build identity, and reproduction result for
  issue #45.

The original recommendation to create a separate checkpoint branch was
superseded by the safer executed sequence: commit the entire accepted tree on
the current feature branch, push it, then merge dev. Additional worktrees remain
deferred until G0 proves `2084944` reproducible on Apple.

### Stage 1 — Freeze UI acceptance tests

Actions:

- define the D1 device matrix in previews and UI tests;
- add stable accessibility identifiers only where a user-visible control needs
  automation;
- capture baseline screenshots for Send, Receive, Activity, Settings, camera
  denial, empty state, active transfer, publication failure, and completion;
- add failing regression coverage for issue #45 before changing its behavior;
- define keyboard dismissal and focus behavior for room code/token fields.

Gate G1 evidence:

- the intended test fails on the old behavior or a physical reproduction is
  recorded when automation cannot reproduce it;
- every supported screen/state has a named preview or UI-test fixture;
- unsupported devices/orientations are explicitly excluded in project settings
  and documentation rather than accidentally implied.

### Stage 2 — Repair the responsive layout model

Actions:

- give bottom safe-area ownership to one container per screen;
- remove fixed compensation padding that duplicates safe-area ownership;
- resolve stage/role gesture competition according to D2;
- use container geometry instead of global screen bounds;
- make QR, button groups, Activity actions, and long localized text adaptive;
- set macOS readable-width and minimum-window behavior;
- verify 44-point iOS hit targets, focus visibility, Dynamic Type, VoiceOver
  labels/order, keyboard, and permission recovery;
- correct contrast failures without changing the approved brand palette more
  than necessary.

Gate G2 evidence:

- all D1 visual-matrix cells reviewed;
- UI tests pass without coordinates or arbitrary sleeps where semantic waits are
  possible;
- no clipping, overlap, unreachable control, unexpected horizontal scroll, or
  gesture ambiguity on supported configurations;
- issue #45 is verified on the physical iPhone, not closed from simulator output
  alone.

### Stage 3 — Make canonical state projection enforceable

Already established by the merged core:

- checked durable commit barrier before snapshots and world-facing effects;
- key-stamped receipt responses and sent-hash verification;
- typed `AwaitingPublication` → `Completed` transition with final path/URI;
- structured failure retained in durable snapshots;
- stable listener identity/token across retry and restore;
- additive raw event notices for diagnostics/invite presentation;
- string Apple Activity IDs adapted to dev's numeric record authority with
  legacy migration.

Remaining actions:

- reject stale record sequences in the Apple model and unit-test reordering;
- retain every non-terminal record when pruning terminal history;
- replace the `verifying + diagnosticMessage == "confirming"` convention with an
  additive typed state/capability;
- expose allowed actions from the canonical record or core policy instead of
  duplicating transition policy in Swift;
- make publication failure/target changes durable and recoverable without
  retransmission;
- reduce `TransferViewModel.Phase` to presentation-only projection;
- add core API/capability/build-version reporting so an app can detect an old
  generated XCFramework;
- add an additive mailbox callback/version that carries dev's durable per-session
  receipt endpoint. The current Apple courier still uses `defaultLogServer`, so
  it must not silently ignore a future custom receipt endpoint.

Gate G3 evidence:

- reducer/record tests cover stale events, every state/action combination,
  publication retry, and terminal-history pruning;
- existing FFI callers still compile;
- Rust tests and Apple hosted tests pass;
- no Apple control decision depends on a diagnostic-message string.

### Stage 4 — Establish Apple CI

Actions:

- add a pinned Apple CI job that regenerates or consumes a reproducible
  `EnvoixCore` package;
- compile macOS and run iOS Simulator hosted/UI tests;
- report executed and skipped counts;
- retain `.xcresult` and relevant logs as CI artifacts;
- document local commands in `apps/envoix-apple/README.md` without duplicating
  this roadmap.

Gate G4 evidence:

- a clean CI checkout can build the Apple core and both Apple consumers;
- CI fails when a required test does not execute;
- Rust public API compatibility and Apple-generated bindings are checked.

### Stage 5 — Implement the selected feature

The selected feature starts only after G2 and G3. Its platform shell may be
spiked earlier, but it must not bypass the architecture contract.

#### Option A — Single-file Share Extension (proposed default)

First slice:

- expose “Send with Envoix” for exactly one regular file;
- stage the file into an App Group while the extension is alive;
- pass a durable draft identifier to the main app;
- let the main app create the canonical transfer Activity;
- provide TTL/quota cleanup for abandoned staged drafts;
- reject multiple files/directories with truthful copy until manifest support
  exists.

Acceptance:

- Files → Share → Envoix produces a visible send draft with the correct name and
  size;
- extension termination does not invalidate the staged source;
- cancelling/import failure cleans or expires the staged file;
- sending still follows the canonical Activity lifecycle.

#### Option B — Wi-Fi Aware vertical slice

First slice:

- prove OS/API/entitlement availability on the target iPhone;
- define additive discovery/data-path provider events and capabilities in the
  shared boundary;
- establish discovery, authenticated pairing, and a data path with a supported
  second peer;
- hand the usable path to the existing Rust transfer/session layer;
- retain QR/rendezvous/direct fallback and resume semantics.

Acceptance:

- works without an access point or pre-existing Internet connection when the
  hardware/OS support Wi-Fi Aware;
- transfers a test file with final size/hash evidence;
- loss of the Aware path follows a typed fallback/resume path;
- unsupported devices show capability-based UX rather than a dead control.

#### Option C — Trusted devices and remote presence

This is a shared-core product milestone, not an Apple-only data store. It
requires durable identity/trust policy, revocation, remote presence,
rendezvous/relay reachability, and clear privacy semantics before UI work.

Acceptance must include offline, revoked, identity-changed, relay-only, and
remote-to-local transitions. The local draft
`docs/issues/trusted-device-store.md` is the starting design input.

### Stage 6 — Physical hardening and milestone release

Actions:

- exercise Files/Finder and iCloud/FileProvider destinations;
- test permission denial/re-enable, background/foreground, process kill/restore,
  network change, and low-storage behavior;
- verify final path/URL, byte count, and content hash;
- run Instruments for energy, memory, SwiftUI invalidation, and network behavior;
- run a strict transfer matrix with retries disabled for the first attempt;
- keep retry-success evidence separate from the original failure evidence.

Gate G5 evidence:

- physical iPhone and macOS checklist is complete;
- supported feature paths have final-file evidence;
- no P0/P1 issue in the milestone scope remains open without a documented user
  decision;
- documentation describes current behavior rather than planned behavior.

## 8. Validation matrix

The exact matrix is frozen by D1. The proposed minimum is:

| Dimension | Proposed coverage |
|---|---|
| iPhone size | Small simulator, standard simulator, physical iPhone 15 Pro Max |
| macOS | Minimum supported window, normal window, wide window |
| Appearance | Light, dark |
| Language | English, Simplified Chinese |
| Dynamic Type | Default, largest supported accessibility size |
| Input | Touch, keyboard where applicable, camera, paste, Files/Finder picker |
| State | Empty, waiting, pairing, transferring, paused, confirming, publishing, failed, completed |
| Permissions | First prompt, allowed, denied, re-enabled |
| Destination | App-local, user-selected local folder, iCloud/FileProvider where available |
| Lifecycle | Foreground, background/foreground, killed/restored |

If iPad or landscape is approved, Split View, Stage Manager, compact/regular
size-class transitions, and active-container geometry become mandatory G2
cells.

## 9. Parallel execution model

Parallel work begins only after the checkpoint and file ownership are recorded.

| Workstream | Primary scope | Must not own |
|---|---|---|
| A — UI shell | `ContentView.swift`, responsive navigation, shared visual components | Core lifecycle or FFI policy |
| B — State boundary | `TransferViewModel.swift`, `Support.swift`, durable Apple resources, additive Rust/FFI contract | Navigation redesign |
| C — Verification | Apple tests, `project.yml`, Apple CI, build/readme commands | Product behavior not covered by an approved acceptance case |
| D — Feature | New extension/provider targets and feature-specific tests | Reimplementing transfer semantics |

Rules:

- use separate branches/worktrees after D4;
- one integration owner reviews every cross-workstream API change;
- no two workstreams edit the same file concurrently without explicit handoff;
- integrate at G1–G5 rather than accumulating one large final merge;
- every change must trace to a stage action or an approved issue;
- failed experiments remain separate commits or are removed before integration.

## 10. Evidence log

Add one row only after inspecting the named artifact. “Build succeeded” without
the command, source revision, destination, and result is insufficient.

| Date | Revision | Gate | Environment | Command/scenario | Result | Artifact |
|---|---|---|---|---|---|---|
| 2026-07-13 | current dirty tree | Planning | repository audit | inspect Apple UI/tests/core boundaries | Draft plan created; implementation not started | this document |
| 2026-07-13 | `ceff278` | G0 safety | Git/local + origin | full-tree checkpoint before dev merge | checkpoint committed and pushed to the matching remote branch | Git commit `ceff278` |
| 2026-07-13 | `2084944` | G0 core | macOS/Rust | merge `origin/dev` `5577194`; resolve 13 conflicts | merge commit created; no unresolved paths | Git commit `2084944` |
| 2026-07-13 | `2084944` | G0 core | macOS/Rust | `cargo check -p envoix-client -p envoix-ffi -p envoix-android-jni` | pass | terminal output |
| 2026-07-13 | `2084944` | G0 core | macOS/Rust | `cargo test -p envoix-client --lib` | 89 passed | terminal output |
| 2026-07-13 | `2084944` | G0 boundary | macOS/Rust | `cargo test -p envoix-ffi --lib` | 40 passed | terminal output |
| 2026-07-13 | `2084944` | compatibility | macOS/Android toolchain | `./gradlew :app:assembleDebug --no-daemon` | not executed: Gradle 8.9 wrapper download timed out | terminal output |

## 11. Definition of done

The Apple milestone is complete only when all of the following are true:

- D1–D4 and the selected feature are recorded as accepted decisions;
- G0–G5 evidence exists and has been inspected;
- supported UI configurations pass the frozen visual/accessibility matrix;
- canonical record and publication semantics are enforced across Rust/UniFFI and
  Swift without breaking existing callers;
- Apple CI runs real tests and distinguishes skipped cross-device methods;
- the selected feature passes its automated and physical acceptance cases;
- current iPhone and macOS builds correspond to the recorded source revision;
- relevant issues and documentation match the shipped state;
- reusable decisions and verified outcomes are ingested into the external
  project wiki after the milestone, not before evidence exists.

## 12. Decision and change log

- 2026-07-13: Initial draft created from the current worktree, Apple UI/test
  audit, architecture documents, handoff, and existing project wiki. D1–D4 are
  intentionally unresolved; no implementation work is authorized by this draft.
- 2026-07-13: D4 resolved. Full tree checkpointed and pushed as `ceff278`, then
  latest dev merged as `2084944`. Plan updated against the dev commit barrier,
  durable extras/receipt work, Android authority, and the retained Apple
  publication/listener/string-ID boundary. D1–D3 remain pending.
