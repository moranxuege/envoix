# Envoix — Apple app

Native SwiftUI Apple client for envoix. The UI is a thin layer over the Rust
core (`envoix-client`), reached through the `EnvoixCore` Swift package generated
from `crates/envoix-ffi` (uniffi). The same Swift sources are intended to port to
iOS later.

## Prerequisites

- Xcode 16+
- [`cargo-swift`](https://github.com/antoniusnaumann/cargo-swift): `cargo install cargo-swift`
- [`xcodegen`](https://github.com/yonaskolb/XcodeGen): `brew install xcodegen`
- For iPhone builds:
  ```bash
  rustup target add aarch64-apple-ios aarch64-apple-ios-sim
  ```

## Build & run

1. Generate the combined macOS + iOS Rust↔Swift bridge package (run after any
   change to `crates/envoix-ffi`):

   ```bash
   scripts/build-apple-core.sh
   ```

   This writes `crates/envoix-ffi/EnvoixCore/` (xcframework + Swift bindings).
   It is git-ignored and must be regenerated locally. The script fixes the
   deployment targets at macOS 13 and iOS 16, preserves reviewed UniFFI binding
   files, and configures the Apple framework linker settings.

2. Generate the Xcode project and run:

   ```bash
   cd apps/envoix-apple
   xcodegen generate
   open Envoix.xcodeproj   # then ⌘R in Xcode
   ```

   Or build/run from the command line:

   ```bash
   xcodebuild -project Envoix.xcodeproj -scheme Envoix \
     -configuration Debug -derivedDataPath build build
   open build/Build/Products/Debug/Envoix.app
   ```

### Run on iPhone

1. Install the Rust iOS targets:

   ```bash
   rustup target add aarch64-apple-ios aarch64-apple-ios-sim
   ```

   If downloads are slow, temporarily use a mirror:

   ```bash
   export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
   export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
   rustup target add aarch64-apple-ios aarch64-apple-ios-sim
   ```

2. Regenerate `EnvoixCore` with an iOS slice:

   ```bash
   scripts/build-apple-core.sh
   ```

   `--exclude-arch x86_64-apple-ios` skips the Intel simulator slice. This is
   fine for Apple Silicon simulator builds and real iPhone demos.

3. Regenerate the Xcode project:

   ```bash
   cd apps/envoix-apple
   xcodegen generate
   open Envoix.xcodeproj
   ```

4. Connect the iPhone:
   - Use a USB cable or enable wireless debugging after first pairing.
   - Unlock the iPhone and tap **Trust This Computer** if prompted.
   - If Xcode says the iOS platform is not installed or shows no eligible
     destinations, open **Xcode > Settings > Platforms** and install the iOS
     platform/runtime matching this Xcode.
   - In Xcode, select the `Envoix-iOS` scheme and your iPhone as the run
     destination.
   - Open target **Envoix-iOS > Signing & Capabilities**, choose your Apple
     Development Team, and let Xcode manage signing.

5. Press **Run** (`⌘R`). On first launch, iOS may ask for local-network access;
   allow it, or LAN discovery/transfer will fail.

For a quick demo, run the macOS app on the Mac and the iOS app on the iPhone.
Use **QR / Code** first because it avoids typing peer addresses. Start
receiving on one device, scan or enter that code on the other device, choose a
small file, then send. On iOS, the default receive folder is visible in Files
as **On My iPhone > Envoix > Downloads**; choose another Files folder only when
you need a custom location.

## UI iteration workflow

For layout and visual work, use Xcode previews instead of repeatedly launching
the whole app:

1. Open `apps/envoix-apple/Envoix.xcodeproj`.
2. Open `Sources/PreviewFixtures.swift`.
3. Use the canvas previews for app shell, send progress, receive invite,
   completed receive, and failure states.

Only regenerate `EnvoixCore` when the Rust FFI surface changes. Pure SwiftUI
edits under `apps/envoix-apple/Sources` should refresh through the preview
canvas or Xcode's incremental build. If the canvas stalls, use **Editor >
Canvas > Reload Canvas** before doing a full app rebuild.

## Using it

Each tab uses **QR / Code** as the default path. Both send and receive can show
an Android-compatible `envoix://pair/<code>` QR plus the same short code, and
the opposite side can scan the QR or enter the code. The rendezvous broker only
pairs devices, and the file still moves over the encrypted transfer path.

Developer mode exposes **Shared Token** for same-LAN mDNS discovery without the
broker. The legacy `envoix:…` direct invite path remains as a compatibility
fallback but is no longer part of the normal UI.

On macOS, the receive folder defaults to `~/Downloads` until you pick another.
On iOS, Envoix defaults to its Files-visible Documents/Downloads folder and
remembers a custom Files folder only after you choose one. The first transfer
may trigger a local-network access prompt; allow it or LAN discovery/transfer
will fail.

Quality-of-life:

- **QR / Code** starts ready on both sides with *New* and *Copy*. The send side
  can either share its own QR/code or join the receiver's QR/code.
- **Send** accepts a file by drag-and-drop or *Paste Path* (from the clipboard),
  as well as the file panel.
- During a transfer the status line shows live throughput and an ETA based on a
  short rolling average; on macOS completion includes *Reveal in Finder* and a
  copyable absolute path.
- A **menu-bar item** shows transfer status and an *Open Envoix* action; closing
  the main window keeps the app running there. The window is resizable and
  supports full screen.

## Roadmap (not yet implemented)

Planned follow-ups, captured here so they are not lost:

- Multi-file / folder transfer (near-term: app-side zip; later: core manifest).
- Enforced speed limits. The settings model has a reserved field, but current
  transfers do not throttle bandwidth yet.
- Parallel chunk transport / out-of-order recovery. The current core still
  sends sequential resumable chunks.
- Global hotkey to send a chosen file fast.
- Saved peers: fixed token per known machine, so reconnecting needs no re-entry.
- Launch-at-login option.
- Proper code signing + notarization for distribution beyond the build machine.

## Notes

- `project.yml` is the source of truth; `Envoix.xcodeproj` is generated and
  git-ignored.
- The Rust static library links several Apple frameworks
  (`SystemConfiguration`, `Security`, `SecurityFoundation`, `CoreWLAN`); these
  are set in `project.yml` under `OTHER_LDFLAGS`. `CoreWLAN` in particular is
  resolved dynamically at runtime, so it must be linked even though it produces
  no link-time error when missing.
