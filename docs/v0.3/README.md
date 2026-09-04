# Envoix v0.3 architecture reset

Status: active

Target release: v0.3.0

Started: 2026-08-18

v0.3.0 is an architecture release. Its purpose is to turn the existing set of
working transfer clients into one maintainable product system before more
features are added. It is not a rewrite of the authenticated transfer core.

## Release objective

By v0.3.0, every supported host must consume the same product model and the
same application state transitions:

- `Device` identifies an endpoint.
- `Relationship` is durable trust between devices.
- `Room` is a temporary authenticated rendezvous and connection context.
- `Transfer` is a durable job whose lifetime is independent of a Room.
- `Content` describes what a Transfer carries.

`Invite` remains a low-level capability used to enter a Room. It is not a
top-level product workflow. `send` remains a convenient UI and CLI command; it
creates a Transfer rather than selecting a separate protocol mode.

## Supported product hosts

| Host | v0.3 product form | Presentation technology |
| --- | --- | --- |
| macOS | signed app plus background-capable host | SwiftUI and native macOS adapters |
| iPhone | embedded engine | SwiftUI compact presentation |
| iPad | embedded engine, native multi-window presentation | SwiftUI iPad presentation shell |
| Android | embedded engine plus OS-managed background work | Jetpack Compose and native Android adapters |
| Windows | graphical shell plus persistent per-user Agent and CLI | Rust/egui shell and Rust Agent/CLI |
| Linux/WSL | persistent per-user Agent and CLI | Rust |

All six hosts are first-class maintenance targets. Feature availability may
differ where an operating system restricts background execution, clipboard
access, discovery, or persistent file access. Such differences must be exposed
as typed capabilities, not inferred by a frontend.

## Authoritative v0.3 documents

- [Target architecture](architecture.md)
- [Milestone plan](milestones.md)
- [Compatibility and breaking-upgrade policy](compatibility.md)
- [Engineering standard](engineering.md)
- [Native presentation contract](presentation.md)
- [Typed binding contract](bindings.md)
- [Desktop host evidence](desktop-host-evidence.md)
- [Dependency security baseline](dependency-security.md)
- [As-built security model](security.md)
- [Operations and recovery](operations.md)
- [Release process](release.md)
- [Release evidence](release-evidence.md)
- [Document status and historical boundary](document-status.md)
- [ADR 0001: Engine storage](adr/0001-engine-storage.md)
- [ADR 0002: macOS Engine helper](adr/0002-macos-engine-helper.md)

When an older design document conflicts with these documents, the v0.3
documents take precedence. The document-status register classifies supporting
operations references and historical design evidence; historical text is not a
release guarantee.

## Accepted decisions

1. Preserve the Rust authentication, protocol, session, and Manifest v2
   transfer implementation.
2. Make `envoix-client` the application boundary before considering a crate
   rename or further crate split.
3. Share product state and policy in Rust; keep operating-system effects in
   platform adapters.
4. Keep platform presentation: SwiftUI for Apple, Compose for Android, and a
   Windows-native executable that remains a typed Agent client rather than an
   additional Engine owner.
5. Use one Apple universal iPhone/iPad application with an independent iPad
   presentation shell. Do not stretch the phone root view onto iPad.
6. Keep the CLI as a supported presentation and administration surface. A
   persistent desktop Agent owns durable runtime state and secrets where the
   operating system permits it.
7. Use the paid Apple Developer Program team for stable development and
   distribution signing. Ad-hoc signing is not a release path.
8. Use a breaking state boundary for the v0.3 test cycle: preserve received
   files, but reset legacy ProductStore/Engine state and re-pair instead of
   retaining an importer or parallel credential model.
9. Use one bounded atomic-file Engine store with an exclusive owner lock;
   credentials remain behind vault references.

## Non-goals

- Replacing the Rust transfer core.
- Introducing a cross-platform rendering framework in v0.3.
- Adding a generic content bus before the file-transfer architecture is
  stable.
- Making mobile applications behave like unrestricted always-on daemons.
- Preserving every internal v0.2 JSON shape or unused public function.
- Reusing the retired direct-Engine desktop demo or creating a second Windows
  product state machine inside the GUI.

## Definition of done

v0.3.0 is complete only when:

- supported applications depend on an explicit application API rather than
  re-exported session or transfer internals;
- Room, Relationship, Transfer, and retry transitions have one authoritative
  implementation and one shared transition test suite;
- old app-facing send/invite workflows and the temporary desktop demo are no
  longer shipped;
- persisted v0.2 and Engine schema v1 data fail with the documented explicit
  reset requirement, while lifecycle cleanup never deletes received files;
- Apple secrets use stable signed Keychain access without prompt loops;
- Android secrets continue to use Android Keystore;
- Windows has a supported GUI/Agent/CLI path, and Linux/WSL has a supported
  Agent/CLI lifecycle path;
- iPad has native adaptive navigation, resizing, drag/drop, and scene-state
  ownership rather than an enlarged phone layout;
- every release artifact is produced by a reproducible, audited, signed release
  path appropriate to its platform;
- the milestone verification matrix is green and attached to the release
  record.

New feature work resumes only after these conditions are met or an explicit
architecture decision changes the v0.3 scope.
