# v0.3 milestone plan

Status: active execution plan.

This plan is dependency-ordered. A milestone is complete only when its exit
criteria and evidence are satisfied, not when its implementation has merely
started. Each milestone should end in one or more focused commits pushed to the
v0.3 branch.

## Summary

| Milestone | Objective | Depends on | Primary evidence |
| --- | --- | --- | --- |
| M0 | Freeze architecture, scope, compatibility, and engineering rules | none | reviewed documents, version baseline, clean tree |
| M1 | Establish trustworthy behavior and security baselines | M0 | characterization tests, CI gates, dependency audit |
| M2 | Turn `envoix-client` into a narrow application boundary | M1 | dependency checks and contract tests |
| M3 | Move Room, Relationship, and Transfer policy into shared reducers | M2 | shared transition suites and reference transfer |
| M4 | Establish durable product storage and Agent control protocol | M3 | migration fixtures, restart tests, CLI/Agent tests |
| M5 | Replace parallel Swift/Kotlin orchestration with typed bindings and ports | M3, M4 | binding contract tests and device-hosted tests |
| M6 | Rebuild native presentation and the Apple universal app | M5 | hosted UI tests, iPad adaptability evidence |
| M7 | Complete desktop host topology and supported CLI operations | M4, M5 | macOS/Windows/WSL lifecycle and transfer evidence |
| M8 | Harden distribution, security, documentation, and cross-platform release | M6, M7 | signed artifacts, SBOM/audit, release matrix |

Cross-device clipboard synchronization is not a v0.3 deliverable. v0.3 creates
the `Content` and platform-port boundaries needed to design it without another
architecture rewrite.

## M0 — Architecture and execution contract

### Deliverables

- authoritative v0.3 architecture and domain vocabulary;
- compatibility and migration policy;
- engineering and documentation standard;
- milestone and verification matrix;
- source version moved to `0.3.0` without replacing the current public v0.2.2
  download page;
- dedicated v0.3 development branch;
- list of historical documents that are superseded but not yet deleted.

### Exit criteria

- all authoritative documents link to each other and contain no unresolved
  contradiction about platform ownership;
- the repository reports v0.3.0 from Rust, Android, and Apple build metadata;
- the working tree is clean after the milestone commit;
- no production behavior changes in this milestone.

### Verification

- Markdown link and formatting inspection;
- repository-wide version search;
- Cargo metadata validation;
- Android and Apple project-generation configuration inspection.

## M1 — Behavior baseline and release blockers

### Deliverables

- characterization tests for current Room code, verification pairing,
  remembered reconnect, revocation, Transfer resume, and delivery proof;
- golden fixtures for Agent control messages and binding events that must
  survive the first migration;
- CI triggered for pushes to the v0.3 branch, not only pull requests;
- one pinned Rust toolchain used locally and in CI;
- RustSec audit and dependency/license policy in CI;
- fixes or documented reachability decisions for current RustSec findings;
- fail-closed HTTPS diagnostic upload and bounded/authenticated log ingestion;
- removal of the temporary egui desktop application from the workspace and
  release workflow;
- release workflow no longer describes debug-signed artifacts as production
  deliverables.

### Exit criteria

- baseline tests pass before application-boundary changes begin;
- no known unaccepted critical/high release vulnerability;
- ordinary branch pushes execute relevant Rust and platform gates;
- release jobs build only supported product forms;
- the temporary desktop package and its release artifacts are absent.

### Verification

- guarded Rust format, lint, and workspace tests;
- workflow syntax and matrix-contract tests;
- current `cargo audit` report attached to milestone evidence;
- targeted server and diagnostic-upload tests;
- a release-workflow dry run where practical.

## M2 — Application boundary

### Deliverables

- logical `model`, `command`, `event`, `snapshot`, `ports`, and `runtime`
  modules inside `envoix-client`;
- stable identifiers and typed application errors;
- one ordered Engine event contract;
- immutable snapshot reconstruction;
- explicit configuration ownership for broker, relay, data window, and retry;
- compatibility adapters around old entry points while consumers migrate;
- removal of wildcard `envoix-session` and `envoix-transfer` re-exports from
  the app-facing API.

### Exit criteria

- a small application contract can represent create/join Room, verified
  pairing, remembered reconnect, create Transfer, progress, completion,
  cancellation, and revocation;
- contract tests rebuild an identical snapshot from events;
- no frontend needs a raw session object for the migrated reference slice;
- no new crate is introduced unless an ADR demonstrates the need.

### Verification

- reducer and serialization tests;
- public API/dependency checks;
- CLI and Agent compile against the compatibility adapter;
- existing reference transfer behavior remains unchanged.

## M3 — Shared product state

### Deliverables

- pure Relationship reducer including verify, trust, rotate, revoke, and
  generation mismatch;
- pure Room reducer including admission, authentication, expiry, disconnect,
  and replacement;
- pure Transfer reducer including offer, accept/reject, progress, pause,
  recovery, delivery proof, failure, cancellation, and removal;
- explicit effects emitted by reducers rather than performed inside them;
- remembered-device send selected as the first vertical slice because it has a
  verified macOS-to-WSL reference path;
- old send/invite product paths removed after all consumers migrate; low-level
  Invite capability types remain in protocol/invite modules where required.

### Exit criteria

- legal and illegal transitions have one shared Rust test suite;
- Room expiry does not alter trusted Relationship or durable Transfer state;
- Swift and Kotlin no longer independently classify outcomes or generation
  fallback for the migrated slice;
- macOS -> WSL remembered transfer and revocation pass on the new state model;
- incompatible v0.2 clients receive a typed/versioned failure where wire
  compatibility cannot be retained.

### Verification

- table-driven and property tests for reducers;
- restart and duplicate-event tests;
- guarded Rust tests plus Apple and Android hosted tests;
- recorded reference-device transfer evidence.

## M4 — Persistence and Agent control plane

### Deliverables

- one Engine-owned, versioned product schema;
- ADR selecting SQLite or an atomic-file store based on migrations,
  concurrency, corruption recovery, packaging, and testability;
- vault references separated from non-secret metadata;
- atomic v0.2 import with backup and explicit re-pair fallback;
- versioned Agent Command/Event/Snapshot protocol;
- owner-only Unix socket and Windows Named Pipe transport contracts;
- one durable Engine owner and explicit process/state locking;
- CLI commands for Agent lifecycle, device management, pairing, Transfer
  creation/status, Inbox inspection, and diagnostics.

### Exit criteria

- process death and restart do not lose a durable Transfer or Relationship;
- a failed migration leaves the old store and received files intact;
- two local controllers cannot create competing durable Engine owners;
- control messages are bounded and contract-tested;
- the CLI never loads platform credentials directly.

### Verification

- versioned fixture migrations, including malformed and interrupted input;
- concurrent-controller and peer-credential tests;
- Agent restart/revoke/resume tests on Linux/WSL;
- Windows IPC contract tests even if the GUI is not yet implemented.

## M5 — Typed bindings and platform ports

### Deliverables

- UniFFI projection of the reduced Command/Event/Snapshot surface for Swift
  and Kotlin;
- one Swift concurrency adapter and one Kotlin coroutine adapter;
- typed platform ports for vault, content source/destination, discovery,
  clipboard intake, background work, and notifications;
- removal of JSON orchestration across the Android in-process application
  boundary for migrated operations;
- hand-written JNI retained only for documented exceptional boundaries;
- binding version and capability negotiation.

### Exit criteria

- Apple and Android receive equivalent event fixtures and snapshots;
- no binding independently implements retry or product terminal-state policy;
- binding cancellation, object lifetime, callback threading, and event gaps are
  tested;
- platform code cannot obtain raw secret values unless implementing the vault
  port at the trusted boundary.

### Verification

- generated-binding compatibility checks;
- Swift hosted tests and Kotlin JVM tests;
- Android instrumentation tests for platform ports;
- leak/lifetime tests for long-running transfers and cancellation.

## M6 — Native presentation and Apple universal app

### Deliverables

- feature-level Swift and Kotlin presentation stores that project Engine state
  without owning product policy;
- semantic design tokens, component-state definitions, and native string
  catalogs;
- decomposition of oversized Apple views/view models and Android service/UI
  files along existing feature boundaries;
- Apple universal iPhone/iPad target;
- independent iPhone compact shell and iPad adaptive shell;
- iPad split navigation, resizing, rotation, multi-window scene ownership,
  drag/drop, keyboard, pointer, context menu, and file destination behavior;
- accessibility labels, focus order, dynamic type/font scaling, contrast, and
  reduced-motion behavior for primary flows.

### Exit criteria

- Views/Composables do not access network, vault, or persistence APIs;
- one Engine serves multiple iPad scenes while presentation state stays
  scene-local;
- the app behaves correctly from compact width through full-screen iPad;
- iPhone, iPad, macOS, and Android use the same product terminology and state
  meanings;
- no source-inline bilingual string mechanism remains on migrated screens.

### Verification

- Apple hosted presentation and clipboard-intake tests;
- iPhone and iPad UI tests across representative window sizes;
- Android JVM and emulator instrumentation tests;
- accessibility inspection and visual evidence for primary screens.

## M7 — Desktop hosts and CLI

### Deliverables

- signed macOS per-user helper/Agent packaging using the paid Developer Team;
- stable bundle identifiers, designated requirements, App/Keychain groups,
  and no ad-hoc credential fallback in distributable builds;
- controlled Keychain access at Engine lifecycle boundaries, with no prompt
  retry loop;
- Windows per-user Agent lifecycle and owner-only Named Pipe API;
- supported Windows CLI build and installation path;
- decision and prototype evidence for WinUI before a Windows GUI is promoted;
- maintained Linux/WSL systemd user service and CLI installation/update path.

### Exit criteria

- GUI and CLI do not directly own desktop credentials;
- app/helper upgrades retain stable identity and do not repeatedly request
  Keychain authorization;
- macOS, Windows, and WSL Agent lifecycle tests cover install, start, stop,
  restart, update, revoke, and uninstall-with-data-policy;
- Windows and WSL can receive a reference Transfer without a temporary GUI;
- a Windows GUI is shipped only if it passes the same application contract and
  lifecycle gates.

### Verification

- signed macOS development and notarization dry-run artifacts;
- clean-user macOS Keychain prompt audit;
- Windows Agent/CLI automated and real-host tests;
- WSL systemd and NAT-path reference tests.

## M8 — Release and security closure

### Deliverables

- public threat model and security review updated for the as-built v0.3
  architecture;
- HTTPS-only diagnostics, authentication, rate limits, retention, consent,
  and redaction policy;
- pinned release toolchains and immutable or reviewed CI dependencies;
- production Android signing and Apple signing/notarization/TestFlight paths;
- SBOM, checksums, provenance/signature, vulnerability, license, and secret
  scanning;
- real test execution for Rust, macOS, iPhone, iPad, Android, Windows, and
  Linux/WSL;
- current architecture, operations, recovery, and release documentation;
- historical v0.2 design documents clearly archived or marked superseded.

### Exit criteria

- all release artifacts have an owner, signing policy, installation guide, and
  update strategy;
- no release artifact is debug/test/ad-hoc signed without being labeled and
  intentionally scoped as development-only;
- no unaccepted high-severity security finding;
- a full reference matrix demonstrates Room pairing, remembered reconnect,
  Transfer, resume, revoke, and migration behavior;
- v0.3.0 release notes accurately describe supported and unavailable features.

### Verification

- release candidate generated from an immutable tag;
- artifact signature, checksum, SBOM, and install verification;
- dependency/security reports;
- cross-device evidence registry and clean-install/upgrade tests.

## Change sequencing within a milestone

Every implementation slice follows the same order:

1. record the behavior or decision being changed;
2. add a failing or characterization test at the owning layer;
3. implement the smallest vertical change;
4. migrate one consumer;
5. verify supported affected targets;
6. remove only the compatibility code made unreachable by that slice;
7. update authoritative documentation;
8. commit with a focused message and push;
9. record verification evidence before starting the next slice.

Large mechanical deletions, generated binding updates, and version migrations
must be separate from semantic changes so regressions remain reviewable.
