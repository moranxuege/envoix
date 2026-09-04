# Envoix Apple app

The macOS and iOS SwiftUI clients are thin projections of the canonical
Manifest v2 Rust core exposed through the generated `EnvoixCore` UniFFI package.

## Prerequisites

- Xcode 16+
- `cargo-swift`
- `xcodegen`
- Rust targets `aarch64-apple-ios` and `aarch64-apple-ios-sim` for iOS builds

## Build

Use the repository wrappers so the shared build-cache guard owns Cargo and
DerivedData concurrency:

```bash
scripts/build-apple-core.sh
cd apps/envoix-apple
xcodegen generate
open Envoix.xcodeproj
```

`scripts/build-apple-core.sh` generates the ignored
`crates/envoix-ffi/EnvoixCore/` package and all Swift/C bindings from the Rust
declarations. Select `Envoix` for macOS or `Envoix-iOS` for an iPhone/simulator.

The iOS app and Share Extension require App Group
`group.com.envoix.app.shared`. A physical iPhone also needs the normal local
network, camera, signing, and Share Extension entitlements.

### macOS Agent helper

The macOS application embeds `EnvoixEngineHelper.app` in
`Contents/Library/LoginItems`. Users explicitly enable it from Settings; the
GUI then talks to the shared API 25 `FfiAgentControlClient` over the helper's
owner-only Unix socket. Only the helper starts `FfiAgentHost`, owns the durable
Engine, and receives the Engine Keychain access group.

Use `scripts/apple-dev.sh macos-build` for certificate-independent compile-only
builds and `scripts/apple-dev.sh macos-helper-test` for isolated host/control
tests. These Debug artifacts intentionally omit the production helper Keychain
entitlement and cannot persist verified pairing credentials. For a locally
usable Debug app, install an Apple Development identity for Team `6638TTB2SF`
and run `scripts/apple-dev.sh macos-debug-signed`. Set
`ENVOIX_MACOS_ALLOW_PROVISIONING_UPDATES=1` only when Xcode is allowed to create
or download the required development signing assets. If the Mac is not already
registered with the team, separately set
`ENVOIX_MACOS_ALLOW_DEVICE_REGISTRATION=1` to permit that external account
change. The signed Debug command fails closed unless the GUI has no Engine
Keychain group and the embedded helper has exactly
`6638TTB2SF.com.envoix.engine.credentials`. Signed Debug uses the isolated
helper bundle identifier `com.envoix.app.engine-helper.debug`; this prevents
macOS Background Task Management from reusing an incompatible ad-hoc helper
registration while the production helper keeps `com.envoix.app.engine-helper`.

This command validates the signed helper host, its Agent control surface, and
helper-owned Keychain persistence. Agent protocol v14 keeps first-contact
`join_pairing` behind that helper: when a foreground macOS Room receives a
verification request, the GUI closes its unverified session and sends only the
bounded invitation, label, and one-time code over the owner-only socket. The
helper reconnects, verifies, commits the Relationship, and keeps the credential
inside its Keychain-backed vault. The macOS paired-device list and its Send/drop
entry points use the helper's typed `ListDevices` and `CreateTransfer` requests;
the GUI receives only non-secret device summaries and transfer identifiers. The
helper-owned paired-device room and Activity views consume its snapshot, live
rate/ETA telemetry, path projection, and Inbox preference through that same
typed boundary; they do not reopen or copy the helper credential.

API 25 also exposes the Rust-owned deployment defaults to both Apple targets.
Agent protocol v12 can update the broker and relay stored on an existing
Relationship without exporting or rotating its credential. API 26 and Agent
protocol v13 add helper-owned Transfer pause, resume, retry, cancel, and
history removal. API 27 and Agent protocol v14 add a helper-owned receive
location plus bounded ephemeral phase, rate, ETA, and file-summary telemetry;
the SwiftUI process receives only these secret-free projections. API 28 and
Agent protocol v15 move durable pairing to a four-phase Relationship upgrade,
carry the creator's complete Room route across different deployments, and mark
an interrupted post-commit Relationship as needing repair instead of presenting
it as ready.

A distributable build must use `scripts/apple-dev.sh
macos-release` with `ENVOIX_MACOS_DEVELOPER_ID` and
`ENVOIX_MACOS_NOTARY_PROFILE`; the command fails closed unless Developer ID
signing, nested entitlement checks, notarization, staple validation, and
Gatekeeper assessment all succeed.

## Transfer model

- Files, folders, Photos, and Share Extension representations all become roots
  in one canonical transfer job.
- Local enumeration and validation begin immediately after selection. Hashing
  can continue on the streaming path without a separate preflight gate.
- Send is the only seal boundary. After it is tapped, the immutable Manifest v2
  offer is sent over Room, mDNS, manual endpoint, or direct invite routing.
- The incoming authenticated inventory is visible before payload. Ordinary
  offers continue automatically; exceptionally large offers require explicit
  approval.
- Receive exposes `Save directly` and `Verify, then copy` before the session is
  accepted. The latter explicitly pays the extra time and peak-space cost.
- The UI distinguishes transferring, verifying, saving, waiting for receiver
  save, finalizing delivery, and delivered. Delivered is never inferred from
  byte transfer alone.
- Completed received roots are read-only in the transfer UI and can be
  previewed, opened in Finder/Files, or shared through platform actions.

Compression policy (`Never`, `Always`, or `Smart`) is selected in Settings and
is frozen into the job at Send. `Never` preserves the original encoding,
`Always` applies Zstandard, and `Smart` uses a conservative, case-insensitive
allowlist for the final filename extension. `Smart` does not read a file sample
or probe the network; unknown extensions, extensionless names, single-component
dotfiles such as `.env`, and already compressed formats remain uncompressed.

## Files and Photos

On iOS, Files can select multiple files, Folder can select one or more directory
roots, and Photos can select multiple assets. The Share Extension stabilizes provider
representations in the shared App Group; the main app imports them into the
same canonical job. Inaccessible descendants remain visible as source issues,
with reauthorization, accessible-only, and remove-root actions.

macOS supports mixed multi-file/folder selection and drag/drop through the same
job preparation API. Security-scoped source and destination access is retained
for the lifetime of the active job.

Helper-owned paired peers are presented as devices on macOS. A device's
**Send** button opens the native multi-item picker, while dropping files or
folders onto that device submits those roots directly to the helper. The helper
validates and seals the sources before `CreateTransfer` succeeds. The app also
advertises a Finder service named **Send with Envoix**. It imports all selected
Finder URLs and brings the main window forward; choosing a paired helper device
then queues that selection. Invoking the Finder service by itself never starts
network transfer.

## Invitations

Both Send and Receive create an `envoix://invite/v2/<payload>` QR and can copy
that complete invite link. The naked internal InviteV2 Room Code is not a
public join credential. Foreground Room Control separately accepts
`envoix://room/<dddddd-xxxx-xxxx>` links and current no-`R` Room codes. Deep
links route to the invitation's authenticated joiner role; scans and clipboard
input inside a Send or Receive flow accept only a complete InviteV2 link.
Legacy invitation formats are rejected.

Pending InviteV2 credentials are process-memory-only. Apple relaunch records do
not persist full payloads, Room Codes, or tickets, so an invitation that was
still pairing must be created or entered again after process exit. Persistent
remembered-peer credentials remain deferred to Issue 58.

## Generated interface

The Rust declarations in `crates/envoix-ffi/src` are the source of truth.
Generated Swift, C header, module map, binary framework, and Swift package are
build products and are intentionally not tracked. Regenerate `EnvoixCore` after
changing the FFI surface before opening the Xcode project.
