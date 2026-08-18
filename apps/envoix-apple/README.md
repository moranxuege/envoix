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

Remembered peers are presented as devices on macOS. A device's **Send** button
opens the selection screen, while dropping files or folders onto that device
opens the same screen with those roots preselected. The app also advertises a
Finder service named **Send with Envoix**. It imports all selected Finder URLs,
brings the main window forward, and waits for device selection and the explicit
final **Send** action; invoking the service never starts network transfer by
itself.

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
