# Apple client execution plan

Status: **In execution — bidirectional Apple Manifest, multi-Photos provider, real iOS Files/Folder payload, and symmetric Room QR intake gates green; Share host, manual Photos, and external-provider acceptance pending**

Owner: Apple client workstream

Last updated: 2026-07-16

Design reference: [`../../../Design.png`](../../../Design.png)

This document is the single execution source of truth for the Envoix Apple
client and its cross-platform dependencies. GitHub issues and local design
drafts remain canonical for subject-specific contracts; this document owns
their order, dependency gates, acceptance evidence, workstream ownership, and
decision log.

Detailed specifications are routed as follows:

| Subject | Canonical detail |
|---|---|
| Durable transfer lifecycle | [`transfer-state-machine.md`](transfer-state-machine.md) |
| Shared client and path contracts | [`client-api.md`](client-api.md) |
| Multi-file and directory transfer | [`../issues/transfer-manifest-v1.md`](../issues/transfer-manifest-v1.md) |
| Cross-platform Wi-Fi Aware vertical slice | [`wifi-aware-vertical-slice.md`](wifi-aware-vertical-slice.md) |
| Sender-first invites | [`../issues/sender-initiated-transfer-flows.md`](../issues/sender-initiated-transfer-flows.md) |
| Trusted devices | [`../issues/trusted-device-store.md`](../issues/trusted-device-store.md) |
| Reliable completion and resume | [`../issues/reliable-transfer-completion-resume.md`](../issues/reliable-transfer-completion-resume.md) |
| Active issue routing | [`../issues/README.md`](../issues/README.md) |

## 1. Outcome

Deliver an Apple client that:

1. behaves correctly on the agreed iPhone and macOS device matrix;
2. renders the durable Rust transfer record instead of implementing a second
   lifecycle in Swift;
3. can advance without continuous Android feature/UI parity, while keeping the
   current Android application compile-compatible at shared Rust/FFI gates;
4. keeps existing Rust/UniFFI callers source-compatible through additive API
   evolution;
5. accepts one regular document through the system Open In route and one or
   more Files or Photos items through a Share Extension/App Group handoff;
6. uses `ManifestV1` for multi-file and directory sharing while retaining the
   compatible single-file path;
7. adds Wi-Fi Aware only as a shared Apple/Android nearby path, while retaining
   LAN and remote rendezvous/relay reachability; and
8. keeps the macOS app usable as both a supported client and the default
   physical counterpart for iPhone transfer verification; and
9. is supported by reproducible builds, honest automated tests, and physical
   device evidence.

“Apple-independent” means that Android UI feature work does not gate Apple
delivery. It does not mean allowing shared Rust/FFI changes to break the current
Android application, or duplicating protocol, transfer, security, retry, resume,
or path-selection semantics in Swift. Each shared-boundary milestone therefore
runs targeted Android compile/APK checks using the existing Android architecture;
full Android feature parity and UI polish remain separately scheduled. A pinned
CLI, Rust integration harness, or known-good mobile peer may be used when a
second endpoint is required.

## 2. Current baseline

The source baseline is locked and the reproducible Apple build, simulator, and
physical-device UI baseline is green. Wave 0 has stable commits; later work is
kept in reviewable staged commits on the current feature branch.

- Branch: `feat/transfer-state-foundation`. The latest committed checkpoint is
  `4b7da35`; the build-cache guard, symmetric Room QR intake, pairing-code
  validation, and Share payload test entry are retained as independent stages.
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
- The current Android application has been realigned with its existing UniFFI
  architecture after the shared-boundary merge. Kotlin compilation, the Debug
  APK, and JVM unit tests pass; the APK contains the arm64-v8a
  `libenvoix_ffi.so`. No Android device was connected, so install/startup and
  Apple↔Android evidence remain pending.
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
- A paused Apple Activity now releases its Send/Receive presentation slot while
  the durable session, diagnostics, and protected cache remain owned by
  `AppModel`. Starting another transfer is allowed; resuming a parked Activity
  is disabled until its recorded parallel-transfer limit has capacity.
- `scripts/build-apple-core.sh` now regenerates the ignored
  `crates/envoix-ffi/EnvoixCore/` package successfully from the current tree;
  XcodeGen, macOS Debug, and iOS Simulator build-for-testing all pass.
- Local Apple iteration now uses `scripts/apple-dev.sh`: simulator, device, and
  macOS builds reuse three stable DerivedData directories; CLI indexing is
  disabled; hosted and App UI tests have separate schemes; and the generated
  Rust package and generated Xcode project are reused until their content
  digests change. Routine runs do not create standalone `.xcresult` bundles.
  Explicit size, log/index trim, Rust-incremental trim, and cold-cache cleanup
  commands bound disk growth. Archive validation preserves the macOS 13/iOS 16
  deployment contract without unconditionally cleaning BLAKE3 on every Core
  rebuild.
- Build-producing Apple wrapper commands now run a free-space guard. The default
  hard-minimum/target watermarks are 64/96 GiB, based on the roughly 57 GiB
  largest Cargo build tree observed on this Mac. Cleanup is serialized; a new
  build is refused below the hard minimum or while another build is active, and
  deletion toward the target is limited to explicitly marked one-off Envoix
  build/test artifacts and repository-local legacy Apple build state. The
  stable default Apple cache and this repository's Cargo `target/` are eligible,
  in that order, only when required to restore the hard minimum. Android
  diagnostic evidence, transfer staging, App Group data, received files, and
  other projects are outside its boundary.
  A process-owned writer lease covers the guard and the complete build command,
  so parallel Codex sessions cannot race between preflight and cache growth.
  Build-free test reruns use shared reader leases: paired endpoints may run
  together, while cache-mutating writers are excluded. Fifteen-second
  heartbeats make an abandoned lease reclaimable after two minutes without
  allowing a live long-running test to look stale.
  Apple, Android instrumentation, cross-device Room, NAT, optional JNI staging,
  and installed pre-commit build entry points all use the same lease.
  Build-free test reruns retain their required compiled cache and still refuse
  to start below the hard free-space minimum.
- The current default simulator suite reports 8 passes, 4 explicit skips, and
  0 failures. The four skipped methods require the opt-in
  `ENVOIX_CROSS_DEVICE_TESTING` configuration and a paired Android device; they
  no longer compile to empty passing bodies.
- A signed current-tree build was built, installed, and launched on the paired
  iPhone 15 Pro Max. Two physical UI automation attempts timed out before test
  execution; after the device was made ready, the third run executed and passed
  all three selected navigation/issue-#45 regression tests.
- The subsequent single-home UI tree passed all six app UI regressions on the
  same physical iPhone, including a deterministic stalled-acknowledgement case
  that proves `Cancelling…` returns to an actionable Cancel control after the
  five-second UI timeout.
- The iOS product now declares iPhone-only (`UIDeviceFamily=[1]`) and portrait-
  only support. The global stage swipe and duplicate fixed bottom compensation
  were removed, and simulator regression tests cover explicit stage controls
  and immediate developer-mode toggle state. The Apple Send flow now accepts
  multiple files and folders; macOS additionally permits mixed top-level
  selections. One regular file stays on the compatible single-file path, while
  folders or multiple roots require `ManifestV1`.
- The iOS Send sheet now exposes three unambiguous sources: Photos, Files, and
  Folder. Photos copies each provider representation directly into its durable
  App Group draft; Files accepts regular files only; Folder uses the system's
  dedicated directory picker and explains that **Open** confirms the current
  folder. The public document-picker API does not allow the app to rename that
  system action. Synthetic provider staging passes on the physical iPhone, and
  the three controls remain reachable in English and Chinese/dark physical UI
  regressions. Isolated single- and two-item synthetic Photos providers have
  also passed through the production iOS sender to the production macOS App
  with exact final bytes and hashes; the two-item run selected `ManifestV1`.
  The real Folder picker now has a physical-device acceptance gate: tapping the
  system **Open** action selected the current directory, and the production Send
  UI delivered its exact directory tree and payload to the production macOS
  App over Direct. The real Files picker also selected two app-owned local
  files and delivered both exact payloads through the production Manifest path
  over Direct. The real Files share sheet now also stages two files through the
  Envoix Share Extension and delivers their exact payloads to the production
  macOS App over Direct. Manual Photos payload acceptance, arbitrary File
  Provider coverage, multi-Photos share-sheet acceptance, and direct Open In
  remain pending.
- The Share Extension accepts multiple Files or Photos representations. It
  stages each provider directly into App Group `group.com.envoix.app.shared`,
  uses a validated versioned draft descriptor, checks actual available storage
  instead of imposing a fixed byte quota, and imports the draft into the
  existing Send sheet when the user next opens the main app. Unclaimed drafts
  retain a 24-hour TTL; automatic and manual cleanup protect active, paused,
  and retryable transfer sources. The app also declares `public.data` document
  handling for the separate, system-launched “Open in Envoix” path. Hosted draft/document-import
  tests pass 12/12 after the lifecycle and document-entry audit, the
  embedded-extension simulator build passes, the existing App UI suite remains
  9/9, and macOS still builds. Xcode-managed provisioning now includes the App
  Group on both targets; the signed product builds, installs, and launches on
  the physical iPhone. The first Photos invocation exposed a missing foreground
  resume check; after the `scenePhase` fix, the user confirmed that a real Photos
  item stages successfully and appears in Send with the correct name when the
  main app is manually reopened. A physical Files-host run now proves two real
  file URLs are resolved, staged, adopted, sent, and byte/hash verified by the
  macOS App. Direct Open In and arbitrary third-party File Provider acceptance
  remain.
- Apple now exposes Room Code as the only manual pairing primitive in Send and
  Receive. Either side may display its role QR while the opposite side scans;
  role-less bare codes remain compatible with both flows, while scanning a QR
  for the same local role is rejected. The obsolete Token/advanced pairing
  selector is no longer visible. The shared parser now validates the existing
  `<digits>-<word>-<word>` code shape instead of accepting every non-empty bare
  string, so an ordinary web QR stays in the scanner with a recoverable error.
  The three focused scanner UI regressions pass on one iPhone Simulator, which
  returned to Shutdown immediately afterward.
- A physical iPhone-to-macOS App hotspot gate now passes with the real macOS
  `AppModel`, canonical Activity projection, and destination publication path:
  33 bytes arrived over Direct IPv6 with an exact SHA-256 match. The receiver
  test is PID-isolated from the user's Activity store. A second physical gate
  now stages a synthetic PNG through `PhotoDraftImporter`, sends it through the
  production iOS `AppModel.send`, and verifies the macOS Activity UI resolver,
  68 final bytes, and SHA-256 after a Direct transfer. A paired follow-up stages
  two named PNG providers, selects the production Manifest path, and verifies 2
  roots, 2 files, 136 bytes, and both hashes over Direct IPv6. These are stronger
  than the earlier CLI/core gate, but the final Photos UI → iOS App → macOS App
  → Finder manual acceptance and multi-Photos Share Extension host acceptance
  are still pending.
- The reverse compatible single-file gate is also green: the production macOS
  `AppModel` sent 37 bytes through an Invite/Relay path to the production iPhone
  `AppModel`, and both canonical Activities completed with the exact
  `7168fd00a9cc516cb7502c53760d5740f38c0671edc338f32ab6ce606fb32165`
  SHA-256 and the Manifest-aware final iOS destination. This run fixed two
  production defects without changing the existing FFI surface: Manifest now
  emits an immediate native snapshot when the transport advertises an invite,
  and terminal cleanup touches receive-publication staging only when that
  staging was actually registered. Personal Hotspot Mac→iPhone Room/mDNS
  discovery and canonical Auto→Relay retry policy retention remain open.
- The reverse Manifest/multi-root gate is now green as well. The production
  macOS `AppModel` sent one folder containing a file and an empty directory plus
  one loose file through Invite/Relay to the physical iPhone. The production
  iOS receiver used its app-private staging and multi-root publication path;
  both Activities completed with 2 roots, 2/2 files, 2 directories, 75/75
  bytes, the exact final tree, and both SHA-256 values. This Swift-only slice
  added an optional defaulted Manifest Invite path policy and did not change the
  Rust/UniFFI public surface.
- `ManifestV1` is implemented through protocol, independent wire frames,
  sequential engine, authenticated direct/mDNS/Room session routing, durable
  client Activity, additive FFI, and Apple selection/publication UI. The
  negotiated receiver retains legacy single-file support on the same endpoint.
  Activity cards now show aggregate inventory, current item, top-level roots,
  exceptional per-item results, and the correct completed destination. Generic
  iOS build/build-for-testing and macOS hosted tests are green. The
  physical Manifest production payload gates are green in both Apple
  directions. Share-provider multi-item and Apple↔Android Manifest evidence
  remain pending.
- Apple build and test jobs are not currently part of `.github/workflows/ci.yml`.
- GitHub issues [#44](https://github.com/ECE4410J-NUUB/envoix/issues/44)
  and [#45](https://github.com/ECE4410J-NUUB/envoix/issues/45) remain the primary
  Apple UX issues. Cross-platform records and discovery are tracked in
  [#40](https://github.com/ECE4410J-NUUB/envoix/issues/40) and
  [#41](https://github.com/ECE4410J-NUUB/envoix/issues/41).

The Rust/UniFFI merge and Apple build, simulator, and targeted physical UI
boundaries are green. G0 is closed; later product tracks retain their own
automated and physical-device acceptance gates.

## 3. Confirmed decisions

| ID | Decision | Accepted direction | Status |
|---|---|---|---|
| D1 | Supported devices and orientations | iPhone + macOS; iPhone portrait; no iPad or landscape promise in this milestone | **Confirmed** |
| D2 | iOS navigation interaction | One iPhone home screen; Send, Receive, Activity, and Settings open as sheets; no permanent bottom stage bar or global stage swipe | **Confirmed and implemented** |
| D3 | First system entry | “Open in Envoix” directly handles one regular document; Share Extension stages one or more Files/Photos items for import when the main app next becomes active; in-app Send separately exposes Photos, Files, and Folder; multiple items/folders use Manifest | **Implemented in source; single-Photos Share Extension entry/adoption, two-file Files-host Share Extension→macOS payload, explicit-source physical UI/provider staging, synthetic single-/multi-Photos provider→macOS payload, and real Files-/Folder-picker→macOS payload acceptance pass; Open In, arbitrary File Provider, multi-Photos share-sheet, and manual Photos payload acceptance pending** |
| D4 | Baseline and shared-core authority | Full-tree checkpoint, latest dev merge, Android App compile compatibility at shared boundaries, additive Apple Rust/UniFFI evolution | **Confirmed and executed** |
| D5 | Multi-file and directory priority | Move `ManifestV1` into the first parallel product wave; do not ship an app-side zip detour | **Confirmed** |
| D6 | Wi-Fi Aware device matrix | iPhone↔supported Android is required; Android↔Android gets baseline evidence; macOS keeps LAN/relay until separately proven | **Confirmed** |
| D7 | Nearby versus remote reachability | Wi-Fi Aware is nearby-only; trusted identity, presence, rendezvous, mailbox, and relay own remote reachability | **Confirmed** |
| D8 | QR scan ownership | Send and receive choose opposite roles, but either device may show its role QR and the other may scan; the UI must not prescribe a fixed scanner | **Confirmed, implemented, and covered in both directions** |
| D9 | Apple cross-device acceptance | Keep the macOS app in sync and use iPhone↔macOS as the default physical payload test; opening Send alone is entry evidence, not transfer acceptance | **Confirmed; production AppModel single-file and Manifest payload gates pass in both directions, while manual UI-to-UI acceptance remains pending** |

For D3, a Photos item means an image or video representation supplied by the
Photos share sheet. Multiple selected assets now use `ManifestV1`. Preserving a
Live Photo as paired resources remains unsupported and must be described
truthfully.

### D4 resolution

The full-tree checkpoint and dev merge confirm that the Apple client cannot be
versioned safely by copying only `apps/envoix-apple/`. Apple development now
starts from `2084944` and follows these ownership rules:

1. the existing Android App architecture remains the Android application
   authority; it is not continuously rewritten to mirror Apple UI/features;
2. Apple work does not wait for subsequent Android UI work;
3. shared Rust/UniFFI changes are additive and must compile both native callers;
4. every shared-boundary milestone includes targeted Android Kotlin/APK
   compatibility; physical Android validation is added when the feature is
   cross-device or transport-specific;
5. cross-platform protocol behavior is validated with pinned loopback/harness or
   a known-good peer instead of depending on unfinished Android feature work;
6. any future dev merge repeats the checkpoint → merge → compatibility tests
   sequence used here.

## 4. Scope

### In scope

- iPhone and macOS SwiftUI layout, navigation, accessibility, localization, and
  platform-specific file interactions.
- Durable Activity rendering and Apple-owned publication resources.
- Additive Rust client/UniFFI contracts required to keep the Apple UI truthful.
- Apple build generation, simulator tests, CI, previews, and physical-device
  validation.
- macOS app maintenance as a supported product and as the stable iPhone
  counterpart for bidirectional Room/Auto and LAN path verification.
- Share Extension for one or more Files or Photos items, subject to the
  Manifest entry-count boundary and the extension's runtime budget.
- `ManifestV1` protocol, core, FFI, and Apple integration for multiple files and
  directories.
- A Wi-Fi Aware vertical slice shared with Android, subject to capability,
  entitlement, and physical-device evidence.

### Out of scope unless separately approved

- Continuous Android UI parity, broad refactoring, or unrelated Android repair.
- Targeted Android adapters required to preserve a shared Rust/FFI contract are
  in scope, but must follow the current Android App architecture.
- Replacing the working transfer engine or wire protocol.
- Ad-hoc archive/zip packaging as a substitute for `ManifestV1`.
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

Wave 0 removed the confirmed duplicate `88` point bottom compensation and the
global stage-switch drag, including its `UIScreen.main.bounds` dependency.
Wave 1 then removed the iPhone stage bar entirely: one home screen presents
Send, Receive, Activity, and Settings as sheets, so each transfer sheet now
owns its own bottom safe area. The remaining responsive-layout risks are:

1. Fixed QR and control dimensions need coverage on small screens, large
   Dynamic Type, landscape if supported, and translated strings.
2. macOS card content needs an agreed maximum readable width.
3. Camera-denied UX needs a direct route to system Settings.
4. The dark-theme brand colors and destructive colors require contrast review.

## 7. Execution stages and gates

### Stage 0 — Lock decisions and preserve the baseline

Completed:

- D1–D8 confirmed and recorded;
- full-tree checkpoint `ceff278` created and pushed before merging;
- latest `origin/dev` (`5577194`) merged as `2084944` without unresolved files;
- shared-core/FFI/JNI compile check passed;
- 89 `envoix-client` and 40 `envoix-ffi` tests passed, including durable
  restore, QR/room pause-resume, mailbox receipt, publication, and legacy-ID
  migration;
- `EnvoixCore` regenerated and the Xcode project regenerated;
- macOS Debug and iOS Simulator build-for-testing passed;
- default simulator tests now report 8 passed, 4 explicitly skipped, and 0
  failed;
- the current signed build was installed and launched on the physical iPhone;
- three targeted physical UI tests passed on the iPhone 15 Pro Max;
- iPhone-only portrait settings, explicit stage navigation, immediate
  developer-mode toggle coverage, bottom-padding cleanup, and truthful
  single-file/Manifest messaging were implemented.
- the subsequent iPhone UI wave replaced explicit stages with one home screen,
  sheet-based Send/Receive/Activity/Settings flows, a canonical Activity
  capsule, symmetric QR guidance, and bounded command feedback.

Remaining actions:

- keep later feature slices in independent commits with their own evidence;
- create additional worktrees only when concurrently executing a separately
  owned track.

Gate G0 evidence:

- checkpoint commit/branch identifier and clean implementation worktree;
- successful Apple Core generation;
- exact build commands and exit status for macOS and iOS;
- honest executed/skipped test counts;
- physical-device version, installed build identity, and reproduction result for
  issue #45.

G0 is closed. The build, simulator, and targeted physical UI portions are
green. The first two physical XCTest attempts are retained as environment-
failure evidence; the third run executed the actual tests and passed.

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

Implemented in the first iPhone UI wave:

- removed the competing app-level bottom safe-area owner;
- moved Send and Receive into large sheets whose CTA bars own their local safe
  area;
- moved Activity and Settings to toolbar sheets and retained an active-transfer
  capsule derived from canonical Activity records;
- added post-command snapshot refreshes and a five-second acknowledgement
  timeout so Activity controls cannot display an infinite transitional spinner.
- added a DEBUG-only stalled-command fixture and a semantic UI regression that
  observes the pending command, waits for its bounded timeout, and verifies the
  restored Cancel control is hittable.
- added XCUITest accessibility audits for Home, Send, Receive, empty Activity,
  Settings, and every Activity fixture state; corrected opaque button contrast,
  QR descriptions, 44-point sheet dismissal, Dynamic Type action stacking, and
  long record/helper text wrapping.
- kept each transfer CTA fixed to its sheet safe area, then added a small-screen
  geometry assertion proving that a scrolled room-code Copy action is hittable
  and does not intersect the CTA. The compact input placeholder is intentionally
  `Enter code` because the adjacent pairing panel already owns the scan guidance.
- passed the complete six-test app UI suite on the small iPhone SE simulator;
  a seventh targeted regression also passed there with Simplified Chinese,
  dark appearance, and the largest accessibility content-size category.

Still required before declaring all of G2 complete:

- visual review and screenshot baselines for the named lifecycle and permission
  states, not only semantic control existence;
- VoiceOver order/labels, keyboard focus/dismissal, camera denial/recovery, and
  foreground/background restoration;
- minimum, normal, and wide macOS window review;
- the remaining light/dark and English/Chinese state combinations where long
  record names, failures, or permission copy can change layout.

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

Completed in the current G3 slices:

- Apple rejects a snapshot with an older canonical sequence, uses timestamp
  only to order equal-sequence deliveries, and has hosted tests for reordered
  callbacks;
- Activity pruning always retains non-terminal records and spends the history
  limit only on the newest terminal records;
- additive `FfiTransferActivityActions` makes pause/resume/cancel/delete and
  finalizing availability a Rust-owned policy. Swift controls no longer parse
  `diagnosticMessage`; the legacy Confirming encoding remains contained inside
  the compatible FFI adapter until a versioned record extension is safe;
- additive `FfiCoreInfo` reports FFI API version, crate version, and named
  capabilities. Settings exposes the loaded core/API version and highlights an
  unexpected API version;
- native publication failures, including a rejected completion callback, now
  set structured failure and retryability fields before Apple projects actions;
- additive `FfiNativePublicationTarget`, `set_publication_target`,
  `publication_target`, and
  `publication_failed` reuse the canonical record's atomic `platform_extras`
  store. Apple restores the destination from the durable session getter, with
  its former UserDefaults entry retained only as a migration fallback. A staged
  receive therefore remains `Publishing` across restart, keeps its typed
  recovery reason, and can replace the Files/Finder destination without
  retransmission;
- Activity renders publication failure as “Save failed”: ordinary retry keeps
  the existing target, while `ChooseFolder` opens the platform folder picker
  and immediately republishes the bytes already received. A persisted failure
  waits for user action after restore instead of entering an automatic retry
  loop;
- `TransferViewModel.Phase` is now a presentation-only projection of the
  canonical Activity record whenever one exists. Raw observer callbacks retain
  only a pre-record fallback, so they cannot independently drive the durable
  lifecycle;
- additive `MailboxObserverV2`, `start_durable_transfer_v2`, and
  `restore_durable_transfer_v2` carry the normalized receipt endpoint frozen in
  the durable session. Existing mailbox and start/restore APIs are unchanged;
  records created before the endpoint field use the frontend's current
  configured endpoint as an explicit migration fallback.

G3 status:

- complete. Share Activity creation is no longer blocked by canonical-state or
  per-session receipt-endpoint work.

Gate G3 evidence:

- reducer/record tests cover stale events, every state/action combination,
  publication retry, and terminal-history pruning;
- existing FFI callers still compile;
- Rust tests and Apple hosted tests pass;
- no Apple control decision depends on a diagnostic-message string.

### Stage 4 — Establish Apple CI

Actions:

- use `scripts/apple-dev.sh` as the canonical local fast path, keeping stable
  platform-specific caches and reserving unique `.xcresult` paths for milestone
  evidence rather than unique DerivedData paths;
- fingerprint Rust/binding contents and the XcodeGen input file list so a pure
  Swift edit neither regenerates the four-platform Core package nor rewrites the
  Xcode project; validate archive deployment targets before accepting cached C
  objects, and clean BLAKE3 only as an automatic repair path;
- require the generated project and every shared scheme used by the wrapper to
  exist before accepting a matching XcodeGen input digest;
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

### Stage 5 — Run the first parallel product wave

The product wave may begin after G0, but each integration point remains gated:
Share Activity creation waits for G3, user-facing layout waits for G2, and
shared provider APIs remain additive until both Apple and Android compile.

#### Track M — Accelerated `ManifestV1`

Status: **protocol, wire codec, sequential engine, authenticated
direct/mDNS/Room routing, durable Activity, additive FFI, Apple app
selection/publication/Activity UI, and Share Extension multi-item source intake
complete; physical Manifest AppModel acceptance complete in both Apple
directions; compatible single-file macOS→iPhone Invite/Relay acceptance
complete; Share Extension multi-item and Apple↔Android physical acceptance
remain**.

The contract now freezes the additive compatibility direction: existing
`envoix/1` and all single-file APIs remain unchanged, while manifest transfers
use `envoix/manifest/1`. It also defines the entry model, BLAKE3 integrity,
10,000-entry/4 MiB manifest limits, portable relative-path validation,
sequential entry lifecycle, partial-result semantics, and non-overwriting
conflict mapping. In particular, a colliding selected top-level directory is
renamed as a unit rather than silently merged into existing user content.

First slices:

1. **Completed:** freeze versioned manifest types, named limits, exact ALPN/mode
   selection, checked aggregates, and unsafe relative-path/parent rejection
   with Rust tests;
2. **Completed:** add a separate frame family with IDs 16 through 26, full
   lifecycle round trips, hostile-decode revalidation, cross-family rejection,
   and a borrowed chunk writer without changing the existing public
   single-file `Frame`/`FrameConnection` APIs;
3. **Completed:** implement the sequential multi-file/directory engine and
   authenticated ALPN routing over manual/direct, the existing mDNS discovery
   loop, and the existing Room rendezvous flow while retaining legacy
   `single_file_v1` entry points;
4. **Completed:** expose additive `Client::send_manifest`, negotiated
   `Client::receive_transfer`, `TransferSet`, typed summaries, aggregate and
   per-entry events, and all existing source modes without adding fields to
   `TransferRequest` or changing `Transfer::wait`;
5. **Completed:** expose aggregate and per-item result data through durable
   records and additive FFI without changing existing single-file APIs;
6. **Completed:** add Apple multi-selection, directory selection on macOS,
   Manifest preparation cancellation, multi-item publication, and Activity
   inventory/current-item/result/destination reporting;
7. **Completed:** send one folder containing a regular file and an empty
   directory plus one loose file from a physical iPhone to the production
   macOS `AppModel`, then verify both canonical Activities, the final directory
   tree, exact bytes, aggregate counts, and per-file SHA-256;
8. **Completed for the main-app Files path and in Share source/hosted tests:**
   the real iOS Files picker sends two selected files through production
   Manifest to macOS; multi-item Files/Photos Share Extension intake uses the
   same Manifest path, while physical Share host acceptance remains;
9. **Completed for the compatible single-file boundary:** send from the
   production macOS `AppModel` to the production physical-iPhone `AppModel`
   through Invite/Relay and verify the exact final file, size, SHA-256, selected
   path, and canonical Activities;
10. **Completed:** send one folder containing a regular file and an empty
    directory plus one loose file from the production macOS `AppModel` through
    Invite/Relay to the physical iPhone; verify both canonical Activities,
    app-private receive staging, multi-root publication, the final tree, exact
    bytes/counts, and both SHA-256 values.

Acceptance follows `transfer-manifest-v1.md`: no default overwrite, identical
files may be skipped by hash, differing files keep both, path traversal is
rejected before writes, and older peers fail clearly before payload transfer.

#### Track S — Files + Photos Share Extension

Status: **multi-item source intake, direct document-open entry, explicit in-app
Photos/Files/Folder sources, automated cache/lifecycle gates, provisioning,
physical install, launch, and single-Photos entry/adoption acceptance complete;
synthetic single- and multi-Photos provider→production iOS Send→macOS App
payload acceptance complete; the real Files and Folder pickers→production
Send→macOS App payload gates are complete; the real Files host→Share Extension
two-item→production Send→macOS App payload gate is complete; multi-Photos
share-sheet acceptance, manual Photos payload acceptance, arbitrary File
Provider behavior, and direct Open In provider acceptance remain**.

First slice:

- expose separate Photos, Files, and Folder choices in the main Send sheet so
  users do not have to infer directory selection from the generic Files UI;
- expose “Send with Envoix” for one or more items from Files or Photos;
- asynchronously load and copy each selected representation directly into App
  Group staging while the provider callback is alive, without an intermediate
  scratch copy;
- persist a validated draft descriptor with identifier, item list, media types,
  names, sizes, creation time, and staged relative paths;
- preserve the draft until the user opens the main app, then import it when the
  scene becomes active and create the canonical transfer Activity only when the
  user actually starts sending;
- register `public.data` document handling so Open In-capable source apps can
  launch Envoix directly with one security-scoped regular file;
- preflight actual available storage, apply TTL/manual cleanup, retain
  collision-safe names, and clean up cancellation without imposing a fixed byte
  quota;
- protect active, paused, and retryable transfer sources from automatic and
  manual cleanup;
- explain that paired Live Photo preservation remains unsupported instead of
  silently dropping resources.

Acceptance:

- the in-app Photos, Files, and Folder controls each open the intended system
  picker; in the Folder picker, **Open** with no selected child confirms the
  current directory;
- Files and Photos each produce a visible send draft with the correct type,
  name, and size;
- a staged Photos or Files item is not considered transfer-accepted until it is
  sent to the macOS app, received under the expected name, and verified by size
  and hash after final publication;
- a PDF or regular file opened through the system document route launches
  Envoix directly and produces the same visible Send selection;
- extension termination does not invalidate the staged source;
- iCloud-backed item loading has progress/failure UX and no false success;
- cancelling/import failure cleans or expires the staged file;
- sending still follows the canonical Activity lifecycle.

Implemented contract for the first slice:

- the main iOS Send sheet has distinct Photos, Files, and Folder controls;
  Apple owns the directory picker's **Open** label, so Envoix documents that it
  uploads the current folder instead of depending on private UIKit mutation;
- the real system Folder picker and its **Open** action pass on the physical
  iPhone, followed by exact Manifest directory/file publication to the macOS
  app; this does not claim every third-party File Provider behaves identically;
- the real system Files picker selects two app-owned local files on the
  physical iPhone, followed by exact two-root Manifest publication, bytes, and
  per-file SHA-256 verification in the production macOS app;
- the real Files share sheet selects two app-owned local files, the Share
  Extension resolves their `public.file-url` property-list representations to
  the underlying files, and the production macOS App verifies the two final
  names, 97 aggregate bytes, both payloads, both SHA-256 values, and Direct path;
- App Group: `group.com.envoix.app.shared`;
- one or more regular file, image, or video representations per draft; folders,
  symlinks, special files, and paired Live Photos are rejected explicitly;
- UUID-scoped staging, atomic versioned descriptor and pending pointer;
- no fixed staged-byte quota; actual available capacity and write-out-of-space
  errors are authoritative;
- 10,000 items is the Manifest protocol entry-count boundary, not a promise
  about Share Extension runtime capacity;
- unclaimed drafts expire after 24 hours; manual cleanup is available in
  Settings and startup cleanup preserves all resumable state;
- the main app retains the staged file through transfer and acknowledges the
  pending pointer only when Send is actually requested.

Publication and disk-I/O boundary:

- macOS ordinary receive never routes through the App Group: Rust writes into
  the selected/default destination and finalizes with a same-filesystem hard
  link or checked rename, so no full-payload migration copy is added;
- iOS default local receive likewise writes directly to its output directory;
- only an iOS user-selected Files/FileProvider destination uses app-private
  receive staging so verified bytes remain available for publication retry;
- a staged regular file tries `clonefile` first. On local same-volume APFS this
  is copy-on-write metadata work; unsupported filesystems, cross-volume targets,
  and FileProvider destinations fall back to a full copy;
- top-level staged directories still use a recursive copy during publication.

Paused-session parking is now implemented at the Apple presentation boundary.
When a canonical Activity reaches `Paused`, its Send/Receive view model snapshots
diagnostics and returns to setup while `AppModel` retains the durable session and
resource access. Resume is admitted against the Activity's recorded
`maxParallelTransfers` limit, so a parked transfer cannot silently overbook the
engine when another transfer is executing.

#### Track W — Cross-platform Wi-Fi Aware vertical slice

Status: **platform/provider contract drafted; capability, pairing, and QUIC
interoperability remain unproven on an Apple↔Android device pair**.

The detailed contract lives in
[`wifi-aware-vertical-slice.md`](wifi-aware-vertical-slice.md). It keeps remote
rendezvous/relay untouched, requires Android API-34 NAN pairing capability for
Apple interoperability, and treats Apple `WAEndpoint` as an opaque native
Network.framework endpoint. The proposed additive boundary injects one reliable
native byte channel into the existing Rust `FrameConnection`; Rust continues to
own authentication, protocol frames, hashing, resume, receipts, Manifest, and
canonical Activity state.

First slice:

- verify API, OS, hardware, entitlement, and permission availability on the
  target iPhone and Android devices;
- define additive discovery/data-path provider events and capabilities in the
  shared boundary;
- implement Apple and Android providers against the same service and identity
  contract;
- establish authenticated iPhone↔Android and Android↔Android data paths without
  requiring an access point or pre-existing Internet connection;
- hand the usable path to the existing Rust transfer/session layer;
- retain QR/mDNS/rendezvous/direct/relay fallback and resume semantics.

Acceptance:

- physical iPhone↔Android discovery and file transfer passes final size/hash
  verification; Android↔Android has at least one baseline transfer;
- loss of the Aware path follows a typed fallback/resume path without creating
  a duplicate Activity;
- unsupported devices show capability-based UX rather than a dead control;
- macOS remains supported through existing LAN/direct/relay paths and is not
  falsely advertised as Wi-Fi Aware capable.

#### Track R — Trusted devices and remote presence

This follows the first nearby vertical slice. It is a shared-core product
milestone, not an Apple-only data store. It requires durable identity/trust
policy, revocation, remote presence, rendezvous/relay reachability, and clear
privacy semantics before UI work.

Acceptance must include offline, revoked, identity-changed, relay-only, and
remote-to-local transitions. The same logical device must not appear as
separate Aware, mDNS, and relay peers.

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
| iPhone size | Small simulator, standard simulator, physical iPhone 15 Pro Max; portrait only |
| macOS | Minimum supported window, normal window, wide window |
| Appearance | Light, dark |
| Language | English, Simplified Chinese |
| Dynamic Type | Default, largest supported accessibility size |
| Input | Touch, keyboard where applicable, camera, paste, Files/Finder picker, Files share sheet, Photos share sheet |
| State | Empty, waiting, pairing, transferring, paused, confirming, publishing, failed, completed |
| Permissions | First prompt, allowed, denied, re-enabled |
| Destination | App-local, user-selected local folder, iCloud/FileProvider where available |
| Lifecycle | Foreground, background/foreground, killed/restored |
| Cross-device | iPhone↔macOS app in both directions; Room/Auto on normal LAN and iPhone Personal Hotspot where relevant; record selected Direct/Relay path and verify final size/hash |

iPad, iPhone landscape, Split View, and Stage Manager are excluded from this
milestone and must not be implied by target settings or documentation.

## 9. Parallel execution model

Parallel work begins only after the checkpoint and file ownership are recorded.

| Workstream | Primary scope | Must not own |
|---|---|---|
| A — UI shell | `ContentView.swift`, responsive navigation, shared visual components | Core lifecycle or FFI policy |
| B — State boundary | `TransferViewModel.swift`, `Support.swift`, durable Apple resources, additive Rust/FFI contract | Navigation redesign |
| C — Verification | Apple tests, `project.yml`, Apple CI, build/readme commands | Product behavior not covered by an approved acceptance case |
| D — Share Extension | App Group staging, draft import, Files/Photos provider tests | Transfer lifecycle or Manifest framing |
| E — Manifest | Protocol/core/record/FFI manifest path and compatibility tests | Apple navigation or platform provider loading |
| F — Wi-Fi Aware | Shared provider contract plus Apple/Android native adapters | File framing, hashing, resume, or remote presence |

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
| 2026-07-13 | `2c91047` + Wave 0 tree | G0 Apple core | macOS/Xcode 26.6 | `scripts/build-apple-core.sh`; `xcodegen generate` | pass | generated ignored `crates/envoix-ffi/EnvoixCore/` and Xcode project |
| 2026-07-13 | `2c91047` + Wave 0 tree | G0 macOS | macOS 26.5 SDK, arm64 | `xcodebuild -project Envoix.xcodeproj -scheme Envoix -configuration Debug -destination 'platform=macOS,arch=arm64' -derivedDataPath /private/tmp/envoix-g0-macos-2c91047 CODE_SIGNING_ALLOWED=NO build` | pass | `/private/tmp/envoix-g0-macos-2c91047` |
| 2026-07-13 | `2c91047` + Wave 0 tree | G0 iOS build | iPhone 16 Pro simulator, iOS 18.3.1 | `xcodebuild ... -scheme Envoix-iOS ... build-for-testing` | pass | `/private/tmp/envoix-g0-ios-2c91047` |
| 2026-07-14 | `2c91047` + Wave 0 tree | G0 iOS tests | iPhone 16 Pro simulator, iOS 18.3.1 | `xcodebuild ... test-without-building` | 8 passed, 4 explicitly skipped, 0 failed | `/private/tmp/envoix-g0-ios-results-final-20260714-3.xcresult` |
| 2026-07-13 | `2c91047` + Wave 0 tree | G0 physical install | iPhone 15 Pro Max | signed `xcodebuild`; `devicectl device install app`; `devicectl device process launch` | build, install, and launch pass | `/private/tmp/envoix-g0-device-2c91047` |
| 2026-07-13/14 | `2c91047` + Wave 0 tree | G0 physical UI | iPhone 15 Pro Max | run three issue-#45/navigation UI tests, then retry one | both blocked before test execution: timed out enabling automation mode | `/private/tmp/envoix-g0-device-ui-20260713.xcresult`; `/private/tmp/envoix-g0-device-ui-retry-20260714.xcresult` |
| 2026-07-14 | `2c91047` + Wave 0 tree | G0 physical UI | iPhone 15 Pro Max | rerun transfer controls, explicit navigation, and developer-mode toggle tests on final tree | 3 passed, 0 failed | `/private/tmp/envoix-g0-device-ui-final-20260714.xcresult` |
| 2026-07-14 | `2c91047` + Wave 0 tree | D1 product settings | built simulator product | inspect `UIDeviceFamily` and `UISupportedInterfaceOrientations` | `[1]`; portrait only | built `Info.plist` |
| 2026-07-14 | `2c91047` + UI Wave 1 tree | G2 macOS regression | macOS arm64 | `xcodebuild ... -scheme Envoix ... build` | pass | `/private/tmp/envoix-ui-wave2-macos` |
| 2026-07-14 | `2c91047` + UI Wave 1 tree | G2 iPhone UI | iPhone 16 Pro simulator, iOS 18.3.1 | build-for-testing; run complete hosted and app UI suites | 8 hosted tests (4 explicit cross-device skips) plus 5 app UI tests passed; Send and Receive CTA controls hittable in sheets | `/private/tmp/envoix-ui-wave2-all-sim-20260714-0142.xcresult` |
| 2026-07-14 | `2c91047` + UI Wave 1 tree | cancel regression | Rust host | run pairing cancel and pause-transition cancel unit tests | 2 passed, 0 failed | terminal output |
| 2026-07-14 | `2c91047` + UI Wave 1 tree | G2 physical UI | iPhone 15 Pro Max | signed build-for-testing, then run Send/Receive sheet regression | build passed; test did not execute because the phone remained locked and was interrupted after 325 seconds | `/private/tmp/envoix-ui-wave2-device-transfer-20260714-0140.xcresult` |
| 2026-07-14 | `2c91047` + UI Wave 1 tree | bounded command UI | iPhone 16 Pro simulator, iOS 18.3.1 | run deterministic stalled-acknowledgement Cancel regression | 1 passed, 0 failed; pending indicator appeared and Cancel returned after the five-second timeout | `/private/tmp/envoix-ui-cancel-regression-sim-20260714.xcresult` |
| 2026-07-14 | `2c91047` + UI Wave 1 tree | G2 physical UI | iPhone 15 Pro Max | rebuild current tree and run complete app UI suite | 6 passed, 0 failed; single home, all sheets, hittable Send/Receive actions, Activity capsule, Settings, and bounded Cancelling recovery executed | `/private/tmp/envoix-ui-wave2-device-current-6-20260714.xcresult` |
| 2026-07-14 | `2c91047` + UI Wave 1 tree | G2 small-screen UI | iPhone SE (3rd generation) simulator, iOS 18.3.1 | run complete app UI suite at default appearance/content size | 6 passed, 0 failed | `/private/tmp/envoix-ui-wave2-small-current-6-20260714.xcresult` |
| 2026-07-14 | `2c91047` + UI Wave 1 tree | G2 localized accessibility layout | iPhone SE (3rd generation) simulator, iOS 18.3.1 | set dark appearance and `accessibility-extra-extra-extra-large`, then run Chinese primary-action regression and restore light/large | 1 passed, 0 failed; Send/Receive remained reachable and Settings exposed `深色` | `/private/tmp/envoix-ui-wave2-se-zh-dark-axxxl-20260714.xcresult` |
| 2026-07-14 | `2c91047` + UI Wave 1 accessibility tree | G2 semantic accessibility | iPhone 16 Pro simulator, iOS 18.3.1 | run complete app UI suite including five-surface and Activity-fixture accessibility audits | 9 passed, 0 failed | `/private/tmp/envoix-ui-wave2-a11y-full-sim-20260714.xcresult` |
| 2026-07-14 | `2c91047` + UI Wave 1 accessibility tree | G2 small-screen accessibility | iPhone SE (3rd generation) simulator, iOS 18.3.1 | run final complete app UI suite; require scrolled Copy controls to be hittable and geometrically clear of the fixed CTA | 9 passed, 0 failed | `/private/tmp/envoix-ui-wave2-a11y-full-se-final-20260714.xcresult` |
| 2026-07-14 | `2c91047` + UI Wave 1 accessibility tree | G2 maximum Dynamic Type audit | iPhone SE (3rd generation) simulator, iOS 18.3.1 | set the system content size to `accessibility-extra-extra-extra-large` and run the complete Activity fixture audit | pass; validates the one exact standard-size Xcode text-clipping prediction without disabling the audit category | `/private/tmp/envoix-ui-a11y-activity-se-axxxl-20260714.xcresult` |
| 2026-07-14 | `2c91047` + UI Wave 1 accessibility tree | G2 macOS regression | macOS arm64, Xcode 26.6 | `xcodebuild -project Envoix.xcodeproj -scheme Envoix -configuration Debug -destination 'platform=macOS' build` | pass | terminal output |
| 2026-07-14 | `2c91047` + UI Wave 1 accessibility tree | G2 physical accessibility | iPhone 15 Pro Max | run complete nine-test app UI suite | 8 passed, 1 accessibility failure; the physical device exposed clipped `Scan QR or enter code` placeholder text | `/private/tmp/envoix-ui-wave2-a11y-device-final-20260714.xcresult` |
| 2026-07-14 | `2c91047` + UI Wave 1 accessibility tree | G2 physical fix verification | iPhone 15 Pro Max | shorten the redundant field placeholder, then rerun the five-surface accessibility audit and stalled-cancel regression | 2 passed, 0 failed | `/private/tmp/envoix-ui-wave2-a11y-device-targeted-20260714.xcresult` |
| 2026-07-14 | `2c91047` + UI Wave 1 accessibility tree | G2 final physical UI | iPhone 15 Pro Max | rerun the complete app UI suite on the final source tree | 9 passed, 0 failed in 126 seconds | `/private/tmp/envoix-ui-wave2-a11y-device-final-pass-20260714.xcresult` |
| 2026-07-14 | `2c91047` + G3 projection tree | G3 Rust boundary | macOS host | `cargo test -p envoix-ffi` with local socket access | 42 passed, 0 failed; includes typed actions, core info, and existing durable/loopback coverage | terminal output |
| 2026-07-14 | `2c91047` + G3 projection tree | G3 Apple hosted | iPhone 16 Pro simulator, iOS 18.3.1 | run complete `Envoix-iOSUITests` hosted suite against regenerated core | 6 passed, 4 explicit cross-device skips, 0 failed; includes sequence ordering, terminal pruning, typed action policy, and core API reporting | `/private/tmp/envoix-g3-typed-actions-hosted-20260714.xcresult` |
| 2026-07-14 | `2c91047` + G3 projection tree | G3 macOS | macOS arm64, Xcode 26.6 | `xcodebuild -project Envoix.xcodeproj -scheme Envoix -configuration Debug -destination 'platform=macOS' build` | pass | terminal output |
| 2026-07-14 | `2c91047` + G3 projection tree | compatibility boundary | Android arm64-v8a / Gradle 8.9 | regenerate additive Kotlin binding; `./gradlew :app:assembleDebug --no-daemon` | Rust release `.so` built and generated binding reached Kotlin compile without binding errors; APK blocked by pre-existing Android source drift (`NativeSession`, `LogSink`, `renderConfig`, and missing `Publishing` branch) | terminal output |
| 2026-07-14 | `2c91047` + G3 publication tree | G3 durable publication | macOS host | persist target/failure through canonical `platform_extras`, read the target back from the restored durable session, replace it, and clear failure without retransmission | final dedicated restart/replacement/getter test passed; the slice reached a full 43/43 before the additive getter, while later full reruns exposed the existing Iroh socket-cleanup timeout and all reported root loopbacks passed independently | terminal output |
| 2026-07-14 | `2c91047` + G3 publication tree | G3 Apple hosted | iPhone 16 Pro simulator, iOS 18.3.1 | build all iOS test targets and run complete hosted suite | 6 passed, 4 explicit cross-device skips, 0 failed; loaded core advertises `durable_publication_recovery_v1` | `/private/tmp/envoix-g3-publication-hosted-20260714.xcresult` |
| 2026-07-14 | `2c91047` + G3 publication tree | G3 publication UI | iPhone 16 Pro simulator, iOS 18.3.1 | run complete app UI suite with a retryable `Publishing/ChooseFolder` fixture | 9 passed, 0 failed; choose-folder replaces Resume, Cancelling remains bounded, and all accessibility audits pass | `/private/tmp/envoix-g3-publication-app-ui-20260714.xcresult` |
| 2026-07-14 | `2c91047` + G3 publication tree | G3 macOS | macOS arm64, Xcode 26.6 | build desktop app against regenerated additive core | pass | `/private/tmp/envoix-g3-publication-macos` |
| 2026-07-14 | `2c91047` + G3 final tree | G3 canonical phase + receipt endpoint | macOS host | `cargo test -p envoix-ffi` after adding the compatible V2 mailbox path | 43 passed, 0 failed; includes persisted per-session endpoint restore and legacy API coverage | terminal output |
| 2026-07-14 | `2c91047` + G3 final tree | G3 Apple hosted | iPhone 16 Pro simulator, iOS 18.3.1 | build all test targets and run complete `Envoix-iOSUITests` suite | 7 passed, 4 explicit cross-device skips, 0 failed; includes pure canonical Phase projection and `per_session_receipt_endpoint_v1` capability | `/private/tmp/envoix-g3-final-hosted-20260714-1358.xcresult` |
| 2026-07-14 | `2c91047` + G3 final tree | G2/G3 app UI regression | iPhone 16 Pro simulator, iOS 18.3.1 | run complete app UI suite after the Phase reducer change | 9 passed, 0 failed; single home, sheet flows, accessibility, and bounded Cancelling recovery remain green | `/private/tmp/envoix-g3-final-app-ui-20260714-1359.xcresult` |
| 2026-07-14 | `2c91047` + G3 final tree | G3 macOS + Android compatibility | macOS arm64 / Android Gradle 8.9 | build macOS; regenerate additive Kotlin binding; run `:app:compileDebugKotlin` | macOS passed; Android reached client compilation with no generated-binding errors and remains blocked only by the pre-existing `LogSink`, `NativeSession`, `renderConfig`, and `Publishing` source drift | terminal output |
| 2026-07-14 | `2c91047` + Android compatibility tree | Android shared-boundary compatibility | Android Gradle 8.9 / arm64-v8a | run `:app:compileDebugKotlin`, `:app:assembleDebug`, and `:app:testDebugUnitTest`; inspect APK entries | all pass; APK contains `lib/arm64-v8a/libenvoix_ffi.so`; no Android device connected for startup smoke | terminal output and `android/app/build/outputs/apk/debug/app-debug.apk` |
| 2026-07-14 | `2c91047` + Share Extension tree | Track S hosted contract | iPhone 16 Pro simulator, iOS 18.3.1 | exercise App Group staging/load/discard, folder/quota rejection, TTL cleanup, traversal rejection, deep-link parsing, and AppModel import | 6 passed, 0 failed | `/private/tmp/envoix-share-integration/Logs/Test/Test-Envoix-iOS-2026.07.14_14-59-44-+0800.xcresult` |
| 2026-07-14 | `2c91047` + Share Extension tree | Track S integration regression | iPhone 16 Pro simulator, iOS 18.3.1 / macOS arm64 | build full iOS product with embedded `EnvoixShare.appex`; run existing App UI suite; build macOS | iOS build passed, App UI 9/9 passed, macOS build passed | `/private/tmp/envoix-share-integration`; `/private/tmp/envoix-share-integration/Logs/Test/Test-Envoix-iOS-2026.07.14_15-18-47-+0800.xcresult`; `/private/tmp/envoix-share-macos` |
| 2026-07-14 | `2c91047` + Share Extension tree | Track S physical provisioning | iPhone 15 Pro Max | signed device build without provisioning updates | source compilation did not start: existing app profile lacks App Groups support and no profile exists for `com.envoix.app.ios.share`; physical Files/Photos acceptance remains pending | `/private/tmp/envoix-share-physical` and terminal output |
| 2026-07-14 | `2c91047` + audited Share Extension tree | Track S boundary audit | iPhone 16 Pro simulator, iOS 18.3.1 / macOS arm64 | reject non-canonical deep links and cross-draft path aliases; arbitrate cancellation against asynchronous staging; rebuild signed simulator product; rerun Share contract, App UI, and macOS gates | Share contract 9/9, App UI 9/9, iOS build, and macOS build passed | `/private/tmp/envoix-share-audit-tests-signed.xcresult`; `/private/tmp/envoix-share-audit-app-ui.xcresult`; `/private/tmp/envoix-share-audit-ios`; `/private/tmp/envoix-share-audit-macos` |
| 2026-07-14 | `2c91047` + audited Share Extension tree | Track S managed provisioning | iPhone 15 Pro Max | allow Xcode to update Apple Developer provisioning, require both targets to carry `group.com.envoix.app.shared`, then build, install, and launch | Xcode created/updated development profiles for `com.envoix.app.ios` and `com.envoix.app.ios.share`; independent entitlement inspection found the shared App Group on both signed bundles; device build, install, and launch passed; Files/Photos share-sheet invocation remains pending | `/private/tmp/envoix-share-physical-provisioned` and `devicectl` output |
| 2026-07-14 | `2c91047` + managed Share tree | Track S first user acceptance | iPhone 15 Pro Max / Photos | invoke Envoix for one image and expect the main app to present a selected Send flow | failed: no Send flow appeared; code audit found that an already-running app did not check pending drafts when returning to the foreground | user observation and source audit |
| 2026-07-14 | `2c91047` + Share resume/Open In tree | Track S recovery + document entry | iPhone 16 Pro simulator / iPhone 15 Pro Max | observe active scene transitions; declare and validate `public.data`; retain security-scoped file URLs; rerun hosted Share contract; sign, build, and install | hosted contract 12/12 passed; signed physical build and install passed; user retest remains pending | `/private/tmp/envoix-share-resume-openin-2.xcresult`; `/private/tmp/envoix-share-resume-openin-physical` |
| 2026-07-14 | `2c91047` + Share foreground regression tree | Track S lifecycle regression | iPhone 16 Pro simulator, iOS 18.3.1 | place the running app in the background, stage a Share fixture in the App Group, reactivate the app, and require Send to show the selected fixture; rerun the prior full App UI suite | targeted foreground recovery 1/1 passed; existing App UI 9/9 passed | `/private/tmp/envoix-share-foreground-ui.xcresult`; `/private/tmp/envoix-share-resume-openin-app-ui.xcresult` |
| 2026-07-14 | `2c91047` + Share foreground regression tree | Track S physical lifecycle regression | iPhone 15 Pro Max | run the same background staging and foreground reactivation scenario on-device, require the selected fixture in Send, then relaunch normally without test arguments | targeted recovery 1/1 passed; normal relaunch passed | `/private/tmp/envoix-share-foreground-device.xcresult` and `devicectl` output |
| 2026-07-14 | `2c91047` + Share foreground regression tree | Track S Photos provider acceptance | iPhone 15 Pro Max / Photos | share one ordinary image to Envoix, confirm the extension reports ready, finish the extension, manually reopen Envoix, and inspect the Send selection | passed: Send opened automatically and displayed the correct image name; the manual app switch remains required by the Share Extension platform boundary | user observation |
| 2026-07-14 | `2c91047` + Share foreground regression tree | Track S direct document UI regression | iPhone 16 Pro simulator, iOS 18.3.1 | open a regular file URL through the system application entry and require Send to show the selected name | 1 passed, 0 failed; this proves entry/adoption only, not payload delivery | `/private/tmp/envoix-share-openin-ui-3.xcresult` |
| 2026-07-14 | `2c91047` + D9 Apple cross-device tree | Current macOS counterpart build | macOS arm64, macOS 26.5 SDK | build the macOS app from the same source/core tree after the Share foreground and document-entry changes | passed; current macOS app is buildable for the next physical iPhone↔macOS payload run | `/private/tmp/envoix-share-macos-current` |
| 2026-07-14 | `2c91047` + D9 Apple cross-device tree | iPhone→Mac hotspot Core path | iPhone 15 Pro Max / Mac on iPhone Personal Hotspot | run the dedicated `testCrossDeviceSendIosToMacOSRoom` against the current Mac CLI/core receiver using Room/Auto; require a selected path, final bytes, and matching SHA-256 | passed: 33/33 bytes; Direct path; received SHA-256 `9a6e0868c7da6c4a5801723bf2505033e62a72af21edd9ce310299cfa93feaf1` matches the sender fixture | `/private/tmp/envoix-apple-hotspot-macos-ios.xcresult`; `/private/tmp/envoix-apple-hotspot-macos-received/envoix-manual-ios-to-macos.bin`; terminal path log |
| 2026-07-14 | `2c91047` + Apple build-cache tree | Apple build iteration | macOS arm64, Xcode 26.6 | replace mtime freshness and unconditional BLAKE3 cleanup with content-digest invalidation, archive deployment inspection, stable platform caches, and build-without-testing reruns; exercise unchanged and file-list invalidation paths | passed; full Core regeneration 34.96 s versus 105.05 s before, unchanged Core/project preparation 1.02 s, cold macOS hosted test 27.36 s, warm test 6.00 s, and `test-without-building` 2.23 s on this Mac; archive objects remain within macOS 13/iOS 16 limits | terminal timing and `otool` inspection output |
| 2026-07-14 | `2c91047` + macOS App-hosted tree | Honest default macOS test gate | macOS arm64, Xcode 26.6 | run the dedicated macOS App-hosted cross-device method without `ENVOIX_CROSS_DEVICE_TESTING` | 1 explicit skip, 0 failures; the default suite cannot report a fake network success | `/private/tmp/envoix-macos-hosted-default-final-20260714.xcresult` |
| 2026-07-14 | `2c91047` + Apple build-cache tree | Default iOS target + honest skip | iPhone 16 Pro simulator, iOS 18.3.1 | resolve the default installed simulator by identifier, then run `testCrossDeviceSendIosToMacOSRoom` without `ENVOIX_CROSS_DEVICE_TESTING` | automatic target resolution selected `72787ED4-8E08-485B-93CF-50290C5F9F8E`; 1 explicit skip, 0 failures | `$TMPDIR/envoix-apple-cache/ios-simulator-debug/Logs/Test/Test-Envoix-iOS-Hosted-2026.07.14_19-54-08-+0800.xcresult` |
| 2026-07-14 | `2c91047` + macOS App-hosted tree | iPhone→macOS App hotspot payload | iPhone 15 Pro Max / `Envoix.app` on Mac connected to iPhone Personal Hotspot | run the physical iPhone sender against `EnvoixMacOSHostedTests.testReceiveIosToMacOSAppRoom`; require canonical Activity `Completed`, selected path, exact completed path, filename, size, file existence, and SHA-256 | passed: sender and receiver both completed; Direct IPv6 selected; 33/33 bytes; receiver Activity `0D23D28C-FFB4-4D85-9A08-FFA65D2E722F`; PID-scoped published file SHA-256 `9a6e0868c7da6c4a5801723bf2505033e62a72af21edd9ce310299cfa93feaf1` matched exactly | `/private/tmp/envoix-hotspot-macos-app-ios-sender-20260714.xcresult`; `/private/tmp/envoix-hotspot-macos-app-20260714-2.xcresult`; `$TMPDIR/envoix-macos-hosted-23906/received/envoix-manual-ios-to-macos.bin` |
| 2026-07-14 | `2c91047` + Manifest contract/wire tree | Track M protocol contract and codec | macOS Rust host | run `cargo test -p envoix-protocol`, strict clippy, `cargo test -p envoix-session --lib`, `cargo test -p envoix-transfer --lib`, and compile client/FFI/Android JNI | protocol 23/23, session 24/24, and original single-file transfer 32/32 passed; frame IDs 1–9 and 16–26, lifecycle round trips, borrowed chunk output, hostile decode, and cross-family rejection are covered; clippy with warnings denied and all three native boundary checks passed | terminal output |
| 2026-07-14 | `2c91047` + Manifest contract/build-cache tree | Track M native consumers | macOS arm64 / iPhone 16 Pro simulator iOS 18.3.1 / Android Gradle 8.9 | invalidate and regenerate Apple Core from the new Rust source; build macOS and iOS apps; compile current Android App Kotlin; remove one generated required scheme and rerun prepare | Apple Core regenerated; macOS and iOS builds passed; Android `:app:compileDebugKotlin` passed. The first macOS attempt exposed a missing explicit `Envoix` scheme after XcodeGen switched to declared schemes. After adding it, the missing-output probe caught and corrected a Bash condition that treated the completeness function name as a string; the final probe regenerated the project and restored the scheme | stable `$TMPDIR/envoix-apple-cache` products and terminal output |
| 2026-07-14 | `38d1fd5`–`84dab08` | Track M engine + authenticated session routing | macOS Rust host | exercise the Manifest engine and real iroh routing over manual/direct, real mDNS discovery, and loopback Room rendezvous; retain old single-file endpoints and legacy-peer rejection | session 35/35 passed with strict clippy; tests cover multiple files, ordinary/empty directories, safe conflict mapping, resume, dual-ALPN legacy compatibility, `manifest.unsupported_peer`, mDNS, and Room; client, FFI, and Android JNI compile checks passed | commits and terminal output |
| 2026-07-14 | `a3d6120` | Track M additive client facade | macOS Rust host | expose Manifest send and negotiated receive for Manual/Invite/mDNS/Room without changing `TransferRequest` or `Transfer`; run all client tests, strict clippy, rustdoc, and native consumer checks | 91 unit tests plus 3 real iroh loopbacks passed; the loopbacks prove old API single-file, two-file/two-directory Manifest transfer, and a legacy sender reaching the new negotiated receiver; FFI and Android JNI compile checks passed | commit and terminal output |
| 2026-07-15 | `efa3ac6`–`01a417c` | Track M durable client + FFI | macOS Rust host / native consumers | persist accepted plans and per-entry results in one durable Manifest Activity; expose additive UniFFI session, observer, runner, and record types; keep negotiated legacy single-file receive available | targeted Rust tests and native boundary compile gates passed; existing single-file APIs remain additive-compatible | commits and terminal output |
| 2026-07-15 | `055cdbc`–`dae6154` | Track M Apple app integration | generic iOS / macOS arm64 host | build Manifests from validated selections; route one regular file through the legacy path and folders/multiple roots through Manifest; publish received roots; render Manifest Activity inventory and results | generic iOS build and build-for-testing passed; macOS hosted suite executed 8 tests with 7 passes, 1 explicit live-device skip, and 0 failures; no Simulator was launched | commits and terminal output |
| 2026-07-15 | `9999b81` | Track M physical iPhone→macOS Manifest | iPhone 15 Pro Max / production macOS `AppModel` | send a folder containing `photo.bin` and an empty directory plus one loose file through Room/Auto; require canonical sender/receiver completion, exact root/file/directory counts, final tree and bytes, per-file SHA-256, and a selected data path | sender 1/1 and receiver 1/1 passed; Direct IPv6 selected; 2 roots, 2/2 files, 2 directories, 63/63 bytes; SHA-256 `36f392e18be2a72d6220d2773fc572aa8d9332bcf0a37c1ebba7a0c81e34b9c4` and `3d6a25e6964bb76c6bb916f991b370d0f9f301f00bc30c735082c162bc7e001b` matched | `/private/tmp/envoix-manifest-ios-20260715-physical02.xcresult`; `/private/tmp/envoix-manifest-macos-20260715-physical02.xcresult` |
| 2026-07-15 | `bab895b` | Track S multi-item + cache contract | iPhone 16 Pro simulator, iOS 18.3.1 | exercise v1 compatibility, multi-item direct App Group staging, available-capacity failure, collision/path validation, claims, startup/manual cleanup, and paused/retryable receive protection | 20 passed, 0 failed; no fixed Envoix byte quota remains | `/private/tmp/envoix-share-cache-contract-20260715.xcresult` |
| 2026-07-15 | `9c34353` | iOS Files publication I/O | iPhone 16 Pro simulator on APFS | materialize a 1 MiB verified staging file through the production publication helper; require copy-on-write clone selection and exact source/destination bytes | 1 passed, 0 failed; `.clone` selected | `/private/tmp/envoix-publication-clone-20260715.xcresult` |
| 2026-07-15 | `bab895b` | Transfer cache UI entry | iPhone 16 Pro simulator, iOS 18.3.1 | open Settings, require the manual cache cleanup control, and retain the existing immediate developer-mode toggle regression | 1 passed, 0 failed; the single Simulator returned to Shutdown | `/private/tmp/envoix-cache-ui-20260715.xcresult` |
| 2026-07-15 | `fcb47cb` | Apple paused-session parking | physical iPhone 15 Pro Max / macOS arm64 | park a paused Activity without releasing its durable owner; require active records to retain the setup slot, paused records to release it, and resume controls to obey the recorded concurrency limit; rerun Activity UI in a Chinese environment and compile the shared macOS app | state tests 3/3 and Activity UI 1/1 passed on the physical iPhone; macOS Debug build passed | `/private/tmp/envoix-paused-slot-device-20260715.xcresult`; `/private/tmp/envoix-paused-slot-ui-device-language-fix-20260715.xcresult`; `/private/tmp/envoix-paused-slot-macos` |
| 2026-07-15 | `e64b7e8` | Track S in-app Photos staging | physical iPhone 15 Pro Max | register a JPEG `NSItemProvider`, require direct provider-callback copy into a versioned App Group draft, retain the name/type/bytes, and publish the pending pointer | 1 passed, 0 failed; no personal Photos data was read | `/private/tmp/envoix-source-picker-importer-device-pass-20260715.xcresult` |
| 2026-07-15 | `e64b7e8` | Track S explicit source UI | physical iPhone 15 Pro Max | open Send in the default and Chinese/dark layouts; require Photos, Files, and Folder controls plus the fixed Send action to remain reachable | 2 passed, 0 failed | `/private/tmp/envoix-source-picker-ui-refactor-device-20260715.xcresult` |
| 2026-07-15 | `e64b7e8` | Track S macOS compatibility | macOS arm64 | build the shared macOS app after isolating the iOS-only picker/importer | build passed | `/private/tmp/envoix-source-picker-macos` |
| 2026-07-15 | `82e9cf7` | Track S Photos-provider production payload | physical iPhone 15 Pro Max / production macOS `AppModel` | stage a valid synthetic PNG from `NSItemProvider` through `PhotoDraftImporter`, send it with production `AppModel.send`, resolve the negotiated single-root Manifest through the same Activity UI helper, and verify final name, bytes, SHA-256, and data path | sender 1/1 and receiver 1/1 passed; Direct selected; `envoix-manual-photo.png`, 68/68 bytes, SHA-256 `431ced6916a2a21a156e38701afe55bbd7f88969fbbfc56d7fe099d47f265460`; no personal Photos or pending App Group draft was read or replaced | `/private/tmp/envoix-photo-production-ios-photo20260715c.xcresult`; `/private/tmp/envoix-photo-production-macos-photo20260715c.xcresult` |
| 2026-07-15 | `a1d0dd5` | D9 reverse Apple production payload | production macOS `AppModel` / physical iPhone 15 Pro Max on Personal Hotspot | start the iPhone production Manifest receiver, hand its Invite once to the macOS hosted sender, force Relay-only for this topology, and require canonical completion plus the exact final file, size, SHA-256, selected path, and Manifest-aware iOS resolver | sender 1/1 and receiver 1/1 passed; Relay selected through `https://envoix.chkxwlyh.us:8444/`; `envoix-manual-macos-to-ios.bin`, 37/37 bytes, SHA-256 `7168fd00a9cc516cb7502c53760d5740f38c0671edc338f32ab6ce606fb32165`; `envoix-ffi` 47/47 passed and no Simulator was launched | `/private/tmp/envoix-macos-to-ios-ios-invite-20260715g.xcresult`; `/private/tmp/envoix-macos-to-ios-macos-invite-20260715g.xcresult` |
| 2026-07-15 | `78fde8d` | Track M physical macOS→iPhone Manifest publication | production macOS `AppModel` / physical iPhone 15 Pro Max on Personal Hotspot | send one folder containing `photo.bin` and an empty directory plus one loose file through Invite/Relay; require canonical sender/receiver completion, app-private iOS staging-to-destination publication, exact root/file/directory counts, final tree and bytes, both SHA-256 values, and selected path | sender 1/1 and receiver 1/1 passed; Relay selected through `https://envoix.chkxwlyh.us:8444/`; 2 roots, 2/2 files, 2 directories, 75/75 bytes; SHA-256 `23ca433fae5ce4a6564cd115b38c8cd327e0d4dc9ae0b463fee48bcd55fc0b4d` and `4ab2bfb892f14047d3fc1550fd45414a74927e8f07d36c1c4cc5293ad0cc1736` matched; no Simulator was launched | `/private/tmp/envoix-macos-to-ios-manifest-ios-20260715a.xcresult`; `/private/tmp/envoix-macos-to-ios-manifest-macos-20260715a.xcresult` |
| 2026-07-15 | `e1b6c0e` | Track S multi-Photos provider Manifest payload | physical iPhone 15 Pro Max / production macOS `AppModel` | stage two named synthetic PNG providers through `PhotoDraftImporter` into an isolated v2 draft, require Manifest selection, send through production `AppModel.send`, and verify both final files, aggregate counts, exact bytes, per-file SHA-256, and selected path | sender 1/1 and receiver 1/1 passed; Direct IPv6 selected; 2 roots, 2/2 files, 0 directories, 136/136 bytes; both SHA-256 values were `431ced6916a2a21a156e38701afe55bbd7f88969fbbfc56d7fe099d47f265460`; no personal Photos data or live App Group draft was read or replaced, and no Simulator was launched | `/private/tmp/envoix-multi-photo-ios-20260715a.xcresult`; `/private/tmp/envoix-multi-photo-macos-20260715a.xcresult` |
| 2026-07-15 | `0a98c3a` | Track S current-folder system action | physical iPhone 15 Pro Max / real iOS Folder picker | open Send, enter the dedicated Folder picker, tap Apple's system **Open/打开** action without selecting a child, and require the current Documents directory to return as one selected folder | 1 passed, 0 failed, 0 skipped; the real picker returned the current directory; no Simulator was launched | `/private/tmp/envoix-folder-picker-device-20260715b.xcresult` |
| 2026-07-15 | `19ce9da` | Track S Folder-picker production payload | physical iPhone 15 Pro Max / production macOS `AppModel` on Personal Hotspot | prepare one isolated directory containing `payload.txt`, select the current directory through the real Folder picker, enter the Room code through the production Send UI, and require both Activity completion plus exact Manifest tree, bytes, hash, and selected data path | sender 1/1 and receiver 1/1 passed with 0 failures/skips; Direct `172.20.10.1:58075` selected; 1 root, 1/1 file, 1 directory, 36/36 bytes; SHA-256 `8c809b310917bd7eb88dc6ba24b1f11f340e24c934db1575f00e9a57c4a72e54` matched; the isolated iPhone fixture was cleaned and no Simulator was launched | `/private/tmp/envoix-folder-picker-ios-20260715d.xcresult`; `/private/tmp/envoix-folder-picker-macos-20260715f.xcresult` |
| 2026-07-15 | `7387599` | Track S Files-picker system multi-selection | physical iPhone 15 Pro Max / real iOS Files picker | open Send, enter the dedicated Files picker at an isolated app-owned directory, select two regular files, tap Apple's system **Open/打开** action, and require the Send selection summary to report both items | 1 passed, 0 failed, 0 skipped; both system-visible files were selected and returned through the production picker delegate; no Simulator was launched | `/private/tmp/envoix-file-picker-selection-20260715d.xcresult` |
| 2026-07-15 | `4ec2a9b` | Track S Files-picker production payload | physical iPhone 15 Pro Max / production macOS `AppModel` on Personal Hotspot | select two isolated regular files through the real Files picker, enter the Room code through the production Send UI, require Manifest selection and both Activity completions, then verify final names, counts, exact bytes, both hashes, destination, and selected data path | sender 1/1 and receiver 1/1 passed with 0 failures/skips; Direct `172.20.10.1:57115` selected; 2 roots, 2/2 files, 0 directories, 81/81 bytes; SHA-256 `7ddd6832a64a5f1b7612cc58ef96f855539c4a9391e41f32f32c2066bf42305f` and `b059309e001627222aada44fd13e85b6ae347bb954bab646a1a38630ca66ff03` matched; fixtures were cleaned and no Simulator was launched | `/private/tmp/envoix-file-picker-ios-20260715a.xcresult`; `/private/tmp/envoix-file-picker-macos-20260715a.xcresult` |
| 2026-07-15 | cache-guard working tree | Local build-cache recovery | development Mac, APFS | stop cross-machine validation; inspect filesystem and active build processes; clean only repository Cargo output and Envoix build/test artifacts; exercise syntax/status, marked allowlist and unsafe-directory preservation, hard-minimum refusal, healthy-range stable-cache retention, emergency-only stable/target deletion, stale/recent heartbeat handling, shared readers, writer exclusion, and reader-mutation rejection | available space increased from 5.1 GiB to about 93.5 GiB (about 88 GiB physically reclaimed); `cargo clean` reported 57.0 GiB/185,701 files and temporary Envoix artifacts accounted for about 116 GiB logically before APFS accounting; the final 64/96 GiB guard and build lease passed the bounded no-build checks without launching Simulator or rebuilding Core | terminal output; `scripts/build-cache-guard.sh`; `scripts/with-build-cache-guard.sh` |
| 2026-07-16 | `633ce0b` | Room Code parser boundary | Rust host / native FFI | reject ordinary URLs and malformed bare strings while retaining legacy four-digit and current six-digit Room Code compatibility; preserve the existing QR URL and FFI interfaces | focused `envoix-client` 9/9 and `envoix-ffi` pairing 4/4 passed; strict `clippy -D warnings` passed for both crates | terminal output |
| 2026-07-16 | `eb5799a`–`4b7da35` | Symmetric Room QR intake | iPhone 16 Pro simulator, iOS 18.3.1 / generic iOS / macOS arm64 | scan Receive QR into Send, scan Send QR into Receive, keep an ordinary web QR in the scanner with a visible error, remove the obsolete Token selector, and compile the conditional Files Share Extension payload pair on both Apple endpoints | focused App UI 3/3 passed; ordinary and cross-device-conditional iOS build-for-testing passed; ordinary and cross-device-conditional macOS builds passed; Share payload execution remains a separate physical gate; the single Simulator returned to Shutdown | `/var/folders/dn/xmztcp9551z4m0kqfbr74m_m0000gn/T/envoix-apple-cache/ios-simulator-debug/Logs/Test/Test-Envoix-iOS-AppUI-2026.07.16_09-56-40-+0800.xcresult`; terminal output |
| 2026-07-16 | `8a9a5e2` | Track S Files Share Extension production payload | physical iPhone 15 Pro Max / real Files share sheet / production macOS `AppModel` on Personal Hotspot | select two isolated files in Files, invoke the Envoix Share Extension, resolve Files-host `public.file-url` property lists to the real security-scoped URLs, finish the extension, adopt the draft in the main app, enter the Room Code through the fully visible Send field, and verify both production Activities plus final payloads | sender 1/1 and receiver 1/1 passed; Direct `172.20.10.1:53304` selected; 2 roots, 2/2 files, 0 directories, 97/97 bytes; SHA-256 `10a3424372e0389307c9bafb51e92d43d095693e218697ea5df3e756605aa975` and `057ee6ebeedb3e66b94e6accb30bb49a3ef9fc8d250d549e26990836912b9e92` matched; provider validation passed 25/25; the test cancels its setup picker instead of leaving a modal page, injects the run ID into both `.xctestrun` environments, and finishes with no Simulator running | `/private/tmp/envoix-share-host-ios-20260716h.xcresult`; `/private/tmp/envoix-share-host-macos-20260716h.xcresult`; guarded hosted-test output |

## 11. Definition of done

The Apple milestone is complete only when all of the following are true:

- D1–D9 are recorded as accepted decisions;
- G0–G5 evidence exists and has been inspected;
- supported UI configurations pass the frozen visual/accessibility matrix;
- canonical record and publication semantics are enforced across Rust/UniFFI and
  Swift without breaking existing callers;
- Apple CI runs real tests and distinguishes skipped cross-device methods;
- Share Extension, Manifest, and Wi-Fi Aware each pass their own automated and
  physical acceptance gates before being advertised as supported;
- current iPhone and macOS builds correspond to the recorded source revision;
- every user-facing iPhone send entry accepted in this milestone has at least
  one iPhone↔macOS payload run with selected-path, final size, hash, and
  publication evidence;
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
  publication/listener/string-ID boundary.
- 2026-07-13: D1–D3 confirmed. D3 initially included one item from both Files
  and Photos. D5 accelerates Manifest without an app-side zip interim, D6 requires
  iPhone↔Android and Android↔Android Wi-Fi Aware evidence, and D7 keeps remote
  reachability on the trusted-presence/rendezvous/relay path. The plan changed
  from one selected feature to a gated parallel product wave.
- 2026-07-14: Wave 0 implementation and validation completed on the working
  tree. Apple Core, macOS, simulator build/tests, and three targeted physical UI
  regressions are green; disabled cross-device tests are explicit skips. The UI
  now truthfully rejects multi-file/folder input pending Manifest. G0 awaits only
  a stable implementation commit before parallel worktrees begin.
- 2026-07-14: D2 was refined after physical-device feedback. The iPhone no
  longer exposes Transfer and Activity as permanent bottom stages. A single
  home screen opens Send, Receive, Activity, and Settings as sheets; active
  canonical Activity is projected as a compact capsule. D8 records that QR
  scanning is symmetric. Command snapshot refresh and bounded pending feedback
  address an observed indefinite Cancelling indicator. The current tree then
  passed all six app UI regressions on the physical iPhone, including a direct
  stalled-acknowledgement recovery test.
- 2026-07-14: G2 semantic accessibility coverage was expanded to every primary
  sheet and canonical Activity fixture. Standard and small-screen simulator
  suites pass 9/9. A full physical run exposed one clipped pairing placeholder;
  the redundant scan wording was shortened and the physical accessibility plus
  stalled-cancel regressions then passed 2/2; the final full physical suite
  subsequently passed 9/9. Manual VoiceOver order, permission recovery,
  screenshot baselines, and the full visual matrix remain G2 work.
- 2026-07-14: G3 projection slices now reject reordered snapshots, preserve all
  non-terminal Activity cards during pruning, and source action availability
  from an additive Rust capability record. Apple no longer parses diagnostic
  strings for controls or finalizing state. Runtime core/API/capability reporting
  is visible in Settings, and structured publication retryability replaces the
  prior display-string policy. Rust 42/42, Apple hosted tests, and macOS build
  pass. Android's new Rust library builds, while the existing Android app source
  still fails independently at its known legacy/current API drift; that client
  repair remains outside the Apple workstream.
- 2026-07-14: G3 publication recovery now persists the Apple destination and
  structured save failure inside the canonical record. Restart preserves the
  `Publishing` card and staged bytes; Apple reads the destination from the
  restored durable session, Retry reuses it, and Choose folder replaces it and
  republishes in place. The dedicated final Rust recovery/getter test, all
  independently retried loopback roots, Apple hosted 6 pass plus 4 explicit
  skips, App UI 9/9, final iOS test build, and macOS build pass. A pre-getter
  full Rust run reached 43/43; later aggregate reruns remain timing-sensitive to
  the existing Iroh socket-cleanup flake rather than this publication path.
- 2026-07-14: G3 closed. Apple Phase is a pure canonical-record presentation
  projection, while a compatible V2 mailbox contract freezes and restores each
  session's receipt endpoint without changing legacy callers. Final validation
  passed Rust 43/43, Apple hosted 7 pass plus 4 explicit skips, App UI 9/9, and
  macOS build; regenerated Kotlin bindings introduced no Android binding errors.
- 2026-07-14: The Android maintenance boundary was refined: Apple remains the
  primary feature track, but shared Rust/FFI milestones must keep the current
  Android App compile-compatible instead of accepting client drift. The targeted
  UniFFI alignment now passes Kotlin compilation, Debug APK assembly, and JVM
  tests; physical Android startup remains pending because no device is attached.
- 2026-07-14: Track S first slice implemented one-item Files/Photos intake,
  validated App Group staging, deep-link/pending import, and Send-sheet adoption.
  Hosted contract tests pass 6/6, the embedded-extension build passes, the full
  App UI regression remains 9/9, and macOS builds. Physical iPhone installation
  is blocked only by missing App Group and extension provisioning; no Apple
  Developer resources were changed automatically.
- 2026-07-14: With explicit user authorization, Xcode-managed provisioning
  created/updated development profiles for the main iOS app and Share Extension
  and enabled `group.com.envoix.app.shared` on both signed targets. The audited
  Share contract now passes 9/9, App UI remains 9/9, macOS builds, and the signed
  iOS product builds, installs, and launches on iPhone 15 Pro Max. The earlier
  provisioning failure is retained above as historical evidence; only manual
  Files and Photos share-sheet invocation remains before Track S physical
  acceptance can be marked complete.
- 2026-07-14: The first real Photos Share invocation did not present Send. The
  subsequent source audit found a concrete recovery defect: the app relied only
  on SwiftUI `onAppear`, which foreground restoration does not necessarily
  re-run.
  The app now imports pending drafts whenever `scenePhase` becomes active. In
  parallel, the main app declares `public.data` and handles security-scoped file
  URLs for a distinct “Open in Envoix” route. The expanded hosted suite passes
  12/12, the dedicated background-to-foreground UI regression passes 1/1 on
  both simulator and iPhone 15 Pro Max, the prior App UI suite remains 9/9, and
  the user-confirmed physical Photos entry/adoption flow now passes. The actual
  Photos payload run plus Share-hosted Files and direct Open In provider
  acceptance remain pending; manually returning to the containing app after the
  Share Extension finishes is expected platform behavior, not an Envoix
  transfer failure.
- 2026-07-14: The user confirmed the corrected Photos provider flow on the
  physical iPhone, including the extension-ready state, manual return to Envoix,
  automatic Send presentation, and correct image name. This closes only the
  entry/adoption gate. D9 now keeps the macOS app synchronized and requires a
  real iPhone↔macOS payload, final size/hash, publication, and selected-path
  record before the Photos send flow is called end-to-end accepted.
- 2026-07-14: D9 gained a dedicated iOS-to-macOS hosted test instead of
  repurposing an Android-named case. On the current iPhone Personal Hotspot,
  iPhone 15 Pro Max sent 33 bytes to the Mac core receiver over Room/Auto; the
  selected payload path was Direct and the received SHA-256 matched exactly.
  The Mac advertised `172.20.10.7`, and path probing also established the
  hotspot host candidate `172.20.10.1`. This proves Apple hardware/core network
  reachability, not yet the Photos UI → iOS App → macOS App product flow.
- 2026-07-14: The side-chat build optimization was integrated before further
  feature work. Core and Xcode project freshness now use content digests rather
  than mtimes, platform builds reuse stable caches, hosted tests can rerun
  without rebuilding, and every produced archive object is checked before the
  deployment-target guard accepts cached BLAKE3 output. On this Mac the full
  Core regeneration fell from 105.05 seconds to 34.96 seconds, while an
  unchanged prepare takes 1.02 seconds. File addition/removal invalidation and
  the Bash 3.2 paths were exercised explicitly. A final default-suite check
  exposed Xcode's model-only destination ambiguity across installed runtimes;
  the wrapper now resolves an actually installed iPhone simulator by identifier
  while retaining an explicit environment override.
- 2026-07-14: D9 advanced from a CLI/core receiver to the actual macOS app
  boundary. A physical iPhone sent the 33-byte fixture over Direct IPv6 while a
  hosted test drove the production macOS `AppModel`; canonical Activity reached
  `Completed`, the exact PID-isolated destination existed, and its SHA-256
  matched. The ordinary build keeps this network method as an explicit skip.
  The evidence still does not claim the final Photos UI → iOS Send UI → macOS
  Receive UI → Finder manual flow.
- 2026-07-14: Track M began with the additive protocol contract rather than a
  zip or repeated single-file workaround. `ManifestV1` now has stable IDs,
  entry kinds, BLAKE3 metadata, exact named limits, checked aggregate sizes,
  parent-before-child validation, and typed portable-path failures. Protocol
  selection keeps one regular file on `envoix/1` and requires
  `envoix/manifest/1` for multi-file or directory shapes. The independent
  Manifest codec now freezes frame IDs 16 through 26, round-trips the full
  sequential lifecycle, rejects single-file frames, revalidates decoded offers,
  and provides a borrowed chunk writer without changing the existing public
  single-file frame APIs. At that contract-only slice, the new ALPN remained
  deliberately unadvertised until session routing and engine support landed.
  Existing protocol/session/single-file tests, native Rust callers, both Apple
  builds, and Android Kotlin compilation remain green. This validation also
  exposed and fixed the missing explicit macOS scheme and strengthened Xcode
  project-cache completeness checks. A deliberate missing-scheme probe caught
  a Bash conditional that did not execute the completeness function; after the
  correction, the same probe forced XcodeGen and restored the required output.
- 2026-07-14: Track M advanced through engine, session, and client without
  reimplementing existing mDNS or Room foundations. The sequential engine now
  transfers multiple files plus explicit/empty directories with receiver-owned
  safe conflict mapping and resume. Additive negotiated receivers advertise
  both ALPNs after authentication and route Manifest over manual/direct, the
  existing mDNS discovery loop, or the existing Room rendezvous flow; legacy
  single-file entry points remain unchanged. The additive client facade exposes
  `send_manifest`, `receive_transfer`, `TransferSet`, typed summaries, and all
  Manifest events while preserving `TransferRequest`, `send`, `receive`, `run`,
  and `Transfer::wait`. Commits `38d1fd5` through `a3d6120` carry these slices.
  Session 35/35, client 91 unit plus 3 real iroh loopbacks, strict clippy,
  rustdoc, and client/FFI/Android JNI compile gates pass. At that stage, the
  remaining Track M critical path was durable Activity projection → additive
  FFI → Apple multi-selection/directory publication → multi-item Share
  intake.
- 2026-07-15: Track M reached the Apple app boundary in staged commits
  `efa3ac6` through `dae6154`. Durable Manifest runners and additive FFI project
  accepted plans, aggregate/current-item progress, and final per-entry results.
  The Apple Send flow accepts multiple files, folders, and mixed roots; keeps
  one regular file on the legacy-compatible path; retains security-scoped
  resources through asynchronous Manifest preparation; and provides explicit
  cancellation. Receive publication handles multiple top-level roots without
  overwriting existing content. Activity shows Manifest inventory, root
  preview, current item, exceptional results, and the correct completed item or
  destination directory. Generic iOS build/build-for-testing and the macOS
  hosted suite passed without launching Simulator. Commit `9999b81` then added
  a paired physical gate: iPhone 15 Pro Max sent one folder containing a file
  and an empty directory plus one loose file to the production macOS
  `AppModel`; both canonical Activities completed over Direct IPv6, and the
  receiver verified 2 roots, 2 files, 2 directories, 63 bytes, the exact final
  tree, and both SHA-256 values. Commit `78fde8d` later closed the reverse
  direction: macOS sent the same two-root shape through Invite/Relay, while the
  physical iPhone completed app-private staging and production multi-root
  publication with exact counts, 75 bytes, final tree, and both SHA-256 values.
  Multi-item Share Extension intake is source-complete and hosted-tested; its
  physical Photos/Files gate, full manual UI acceptance, and Apple↔Android
  Manifest payload evidence remain.
- 2026-07-15: Removed the Apple Share staging 4 GiB policy limit. The extension
  now performs a direct provider-to-App-Group copy, preflights real available
  capacity, supports multi-item Manifest drafts, and records claims so startup
  and Settings cleanup preserve active, paused, and retryable sessions. The
  receive-path audit confirmed that macOS and default iOS outputs are direct;
  iOS custom Files publication now uses APFS copy-on-write cloning when possible
  and a full-copy fallback where required.
- 2026-07-15: Apple paused-session parking now detaches the Send/Receive
  presentation slot from a canonical paused Activity while retaining its
  durable session and protected resources in `AppModel`. Resume admission
  follows the Activity's recorded parallel-transfer limit, and the physical
  iPhone Activity UI regression is language-independent.
- 2026-07-15: The main iOS Send entry is now split into Photos, Files, and
  Folder. Photos uses the same direct provider-to-App-Group staging contract as
  the Share Extension, Files rejects directories, and Folder uses a dedicated
  directory picker. Apple's public API keeps the final system button titled
  **Open**; Envoix explicitly explains that this action uploads the current
  folder instead of mutating private picker views. Synthetic provider staging
  passed 1/1, the physical source-entry UI passed 2/2, and an isolated synthetic
  Photos provider passed production iPhone→macOS payload acceptance 1/1 on both
  peers with exact bytes/hash over Direct. The real Files and Folder interfaces
  are now covered below; manual Photos payload evidence, Share Extension
  multi-item host acceptance, arbitrary File Providers, and Open In remain.
- 2026-07-15: The reverse production single-file gate now passes from the
  macOS `AppModel` to the physical-iPhone `AppModel`. The first attempt exposed
  unconditional cleanup of receive-publication staging on sessions that never
  registered it; cleanup is now scoped to a real publication record. The QR
  path then exposed that a Manifest transport `Advertised` event updated only
  internal state, leaving Swift without the Invite; the existing native
  observer now receives an immediate updated record without an FFI API change.
  The final paired tests transferred 37/37 bytes over Relay and matched SHA-256
  `7168fd00a9cc516cb7502c53760d5740f38c0671edc338f32ab6ce606fb32165`.
  Failed Room/Auto probes on the same Personal Hotspot also established two
  remaining core tasks: Mac→iPhone mDNS discovery is directionally unreliable,
  and canonical Invite de-duplication currently loses the per-attempt
  Auto→Relay path-policy override. Neither is claimed fixed.
- 2026-07-15: The reverse Manifest production gate now passes from macOS to the
  physical iPhone. `startSendingManifestWithInvite` gained a source-compatible
  defaulted path-policy parameter so the Personal Hotspot test can select
  Relay-only without changing normal callers or Rust/UniFFI. The paired tests
  verified 2 roots, 2/2 files, 2 directories including an empty directory,
  75/75 bytes, exact final contents, and both SHA-256 values. The iOS receiver
  used the production app-private staging and multi-root publication path, not
  a direct test copy. Both result bundles contain one executed pass, the
  one-shot Invite key was removed, and no Simulator was launched.
- 2026-07-15: Two named synthetic PNG `NSItemProvider`s now pass through
  `PhotoDraftImporter`, an isolated v2 draft, production Manifest selection,
  the physical-iPhone sender, and the production macOS receiver. The paired
  tests completed 2 roots, 2/2 files, 136/136 exact bytes, and both SHA-256
  checks over Direct IPv6. This proves the main-app multi-Photos provider
  payload path; it does not substitute for Photos' system share-sheet or Share
  Extension multi-item host acceptance. No Simulator was launched.
- 2026-07-15: The dedicated iOS Folder picker now passes its real system-action
  and payload gates on the physical iPhone. Apple's **Open/打开** action selected
  the current directory without a child selection; the same production Send UI
  then transferred one directory containing `payload.txt` to the production
  macOS `AppModel`. Both result bundles contain one executed pass, Direct over
  the Personal Hotspot network was selected, and the receiver verified 1 root,
  1/1 file, 1 directory, 36 exact bytes, and SHA-256
  `8c809b310917bd7eb88dc6ba24b1f11f340e24c934db1575f00e9a57c4a72e54`.
  Arbitrary third-party File Provider behavior remains a separate gate; no
  Simulator was launched.
- 2026-07-15: The dedicated iOS Files picker now passes real two-file system
  selection and production payload gates on the physical iPhone. Both files
  were selected in Apple's Files UI and returned through the public document
  picker delegate; the production Send UI then selected Manifest and delivered
  both roots to the production macOS `AppModel` over Direct
  `172.20.10.1:57115`. The receiver verified 2 roots, 2/2 files, 0 directories,
  81 exact bytes, the final names and destination, plus SHA-256
  `7ddd6832a64a5f1b7612cc58ef96f855539c4a9391e41f32f32c2066bf42305f`
  and `b059309e001627222aada44fd13e85b6ae347bb954bab646a1a38630ca66ff03`.
  Each result bundle contains one executed pass with no failure or skip; the
  fixtures were cleaned and no Simulator was launched. This does not claim
  Share Extension host behavior for Photos or arbitrary external File Provider
  coverage.
- 2026-07-16: Bare pairing input now has a real protocol boundary. The shared
  parser accepts the existing legacy/current Room Code nameplate envelope and
  rejects URLs, missing components, extra components, non-numeric nameplates,
  non-alphabetic words, and overlong nameplates before they reach native UI.
  Focused client/FFI tests and strict Clippy pass without changing the public
  Invite or UniFFI interfaces.
- 2026-07-16: Apple Send and Receive now expose one Room Code pairing model
  instead of a separate Token/advanced selector. Either role can scan the
  opposite role QR, a role-less Room Code remains usable by either flow, and an
  invalid QR reports an error without dismissing the scanner. All three focused
  UI cases pass on the current generated Core.
- 2026-07-16: The physical Files-host Share Extension gate now passes end to
  end. The first execution exposed that Files supplies a binary property-list
  `public.file-url`, not a transferable file representation; the extension now
  resolves and security-scopes the underlying URL before staging. Test audit
  also found and fixed an unclosed setup picker, a partially clipped Room field
  that XCTest incorrectly treated as actionable, and a hosted-test environment
  fallback that falsely expected 81 bytes after 97 bytes had arrived. The final
  paired run executed one passing test on each peer, published two exact files
  over Direct `172.20.10.1:53304`, matched both SHA-256 values, and launched no
  Simulator. Multi-Photos share-sheet, arbitrary File Provider, and direct Open
  In payload acceptance remain open.
