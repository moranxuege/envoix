# Apple client execution plan

Status: **Draft — product decisions pending**

Owner: Apple client workstream

Last updated: 2026-07-13

Design reference: [`../../../Design.png`](../../../Design.png)

This document is the execution source of truth for the Envoix iOS and macOS
clients. GitHub issues remain canonical for issue-specific discussion; this
document owns cross-issue sequencing, gates, acceptance evidence, and the
current decision log.

No product implementation may start from this draft until Decisions D1–D4 are
confirmed. Read-only audits and plan maintenance are allowed before that gate.

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

The baseline is intentionally not called green yet.

- Branch: `feat/transfer-state-foundation`, tracking its origin branch.
- `HEAD` and `origin/feat/transfer-state-foundation` both point to `1ca1888`;
  there are no committed ahead/behind changes. Every newer change is currently
  worktree-only.
- The worktree contains extensive intentional, uncommitted Apple, Android, Rust,
  test, script, and documentation changes. The tracked diff currently spans 85
  files with 15,312 insertions and 6,074 deletions, plus untracked Apple, Android,
  FFI, scripts, tests, and this plan. They must be preserved.
- The generated `crates/envoix-ffi/EnvoixCore/` package was removed during disk
  cleanup. An exact-source Apple build therefore requires a full
  `scripts/build-apple-core.sh` run first.
- The iPhone 15 Pro Max has an installed debug app, but it predates the latest
  shared-source rename and cannot prove that the phone matches the current tree.
- Historical simulator output reported ten tests, but four cross-device methods
  compile to empty bodies when `ENVOIX_CROSS_DEVICE_TESTING` is absent. The
  effective default coverage was six tests.
- Apple build and test jobs are not currently part of `.github/workflows/ci.yml`.
- GitHub issues [#44](https://github.com/ECE4410J-NUUB/envoix/issues/44)
  and [#45](https://github.com/ECE4410J-NUUB/envoix/issues/45) remain the primary
  Apple UX issues. Cross-platform records and discovery are tracked in
  [#40](https://github.com/ECE4410J-NUUB/envoix/issues/40) and
  [#41](https://github.com/ECE4410J-NUUB/envoix/issues/41).

The July canonical lifecycle and strict transfer results are useful prior
evidence, but they do not replace a rebuild and verification against the exact
baseline selected in D4.

## 3. Decisions required before implementation

| ID | Decision | Proposed default | Status |
|---|---|---|---|
| D1 | Supported devices and orientations | iPhone + macOS; iPhone portrait first; no iPad promise in this milestone | Pending user confirmation |
| D2 | iOS navigation interaction | Keep the branded floating stage bar, but remove the global horizontal stage-swipe gesture | Pending user confirmation |
| D3 | First post-foundation feature | Single-file Share Extension first; design the Wi-Fi Aware provider boundary in parallel | Pending user confirmation |
| D4 | Baseline and shared-core authority | Preserve all current work in a local checkpoint; allow additive Rust/UniFFI changes while preserving existing callers | Pending user confirmation |

D4 must also identify whether the entire current dirty tree is accepted as the
new baseline. If not, the user must name the commit/branch that owns the desired
shared-core state; the Apple changes can then be selected explicitly instead of
silently inheriting or discarding Android-side work.

### D4 audit conclusion

The Apple work cannot be safely checkpointed by copying only
`apps/envoix-apple/`. Its current UI and durable-state code consumes the
worktree-only `envoix-client` lifecycle, UniFFI implementation, generated Swift
binding, storage/publication behavior, and Apple build scripts. Treating the
tracked `1ca1888` commit as the Apple baseline would therefore discard required
shared-core work, not merely Android UI work.

The proposed safe interpretation of “stop depending on Android fixes” is:

1. preserve the entire current worktree once in a local checkpoint;
2. branch Apple development from that exact checkpoint;
3. make no further Android application changes in the Apple workstream;
4. accept only additive shared Rust/UniFFI changes required by the Apple client;
5. validate cross-platform protocol behavior with a pinned peer/harness rather
   than waiting for Android application development.

If the intent is instead to reject some current shared-core work, D4 must name
the exact last accepted Rust/FFI state. File-path filtering is not a safe proxy
for that architectural decision.

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

Actions:

- confirm D1–D4;
- record `HEAD`, branch, `git status`, and scoped diff statistics;
- create a user-approved local checkpoint before touching existing source;
- do not push the checkpoint unless separately requested;
- create the Apple feature branch/worktrees from the accepted checkpoint;
- rebuild `EnvoixCore` and regenerate the Xcode project;
- build macOS Debug and iOS Simulator `build-for-testing` from exact current
  sources;
- reinstall the physical iPhone before recording behavior against the baseline;
- change disabled cross-device tests to report `XCTSkip` or isolate them in an
  explicit cross-device scheme.

Gate G0 evidence:

- checkpoint commit/branch identifier and clean implementation worktree;
- successful Apple Core generation;
- exact build commands and exit status for macOS and iOS;
- honest executed/skipped test counts;
- physical-device version, installed build identity, and reproduction result for
  issue #45.

Recommended checkpoint sequence after D4 approval:

1. capture the full status, diff statistics, untracked-file list, and current
   `HEAD` in the evidence log;
2. create a local `checkpoint/apple-baseline-2026-07-13` branch from the current
   worktree and commit all accepted files without rewriting history;
3. create `feat/apple-client-roadmap` from the checkpoint;
4. keep the checkpoint local until the user separately approves publication;
5. create additional worktrees only after G0 builds prove the checkpoint is
   reproducible.

The actual branch names may change at execution time to avoid collisions. No
checkpoint command is authorized by this draft alone.

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

Actions:

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
  generated XCFramework.

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
