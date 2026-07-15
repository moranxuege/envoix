# Envoix — Apple app

Native SwiftUI macOS and iOS client for envoix. The UI is a thin layer over the
Rust core (`envoix-client`), reached through the `EnvoixCore` Swift package
generated from `crates/envoix-ffi` (UniFFI).

The canonical product sequence, accepted device matrix, feature dependencies,
and verification gates live in
[`docs/design/apple-client-execution-plan.md`](../../docs/design/apple-client-execution-plan.md).
This README only documents current build and use instructions.

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
   - The Share Extension additionally requires App Group
     `group.com.envoix.app.shared` on both `Envoix-iOS` and `EnvoixShare`, plus
     a development profile for bundle ID `com.envoix.app.ios.share`. Xcode may
     need permission to update these resources in the Apple Developer account.

5. Press **Run** (`⌘R`). On first launch, iOS may ask for local-network access;
   allow it, or LAN discovery/transfer will fail.

For a quick demo, run the macOS app on the Mac and the iOS app on the iPhone.
On iPhone, choose **Send a file** or **Receive a file** from the single home
screen; the selected flow opens as a sheet. Either device can show its role's
QR code and the opposite role can scan it, so there is no fixed “sender scans”
or “receiver scans” rule. On iOS, the default receive folder is visible in
Files as **On My iPhone > Envoix > Downloads**; choose another Files folder only
when you need a custom location.

### Share from Files or Photos

Inside Envoix on iOS, Send exposes three explicit sources: **Photos** accepts
one or more images/videos, **Files** accepts one or more regular files, and
**Folder** opens a dedicated directory picker. In the Folder picker, navigate
to the directory and tap the system **Open** action without selecting a child;
that uploads the current folder. Apple owns this system action title and does
not expose a public API for renaming it, so Envoix explains the behavior beside
the picker entry instead of modifying private UIKit views. macOS accepts
multiple files/folders, including mixed selections, through its picker or
drag-and-drop. One regular file uses the legacy-compatible single-file
protocol; a folder or multiple roots uses `ManifestV1`. Manifest preparation
may hash many files, so the Send sheet stays open and exposes cancellation
until a durable Activity has been created.

For a PDF or another regular document, choose **Open in Envoix** when the source
app offers an Open In destination. iOS launches the main app directly, and
Envoix presents the normal Send sheet while retaining security-scoped access to
one file or folder supplied by that route.

For Photos and generic share sheets, choose one or more files, images, or videos
and select the **Envoix** Share Extension. The extension copies each selected
representation directly into the shared App Group. Tap **Done**, then open
Envoix manually; when the app becomes active it imports the pending draft and
presents the Send sheet. iOS does not allow a Share Extension to launch its
containing app.

One item uses the compatible single-file path; multiple items use `ManifestV1`.
The 10,000-item boundary comes from the Manifest protocol and is not a practical
promise that iOS will let an extension process that many providers before its
execution budget expires. Paired Live Photo preservation, symbolic links, and
special files remain unsupported. There is no fixed Envoix byte quota: staging
preflights the device's available capacity and reports a storage error only when
the copy cannot fit. Unclaimed drafts expire after 24 hours. Settings also
offers manual cache cleanup, while startup cleanup and manual cleanup always
protect active, paused, and retryable transfers.

macOS receives directly into the selected output directory and does not copy a
payload from the App Group. Core finalization uses a same-filesystem hard link
or checked rename. iOS also receives directly into its default local output;
only a user-selected Files/FileProvider directory uses app-private staging.
Publishing a regular file tries same-volume copy-on-write cloning first and
falls back to a full copy for unsupported, cross-volume, or FileProvider
destinations. Publishing a staged directory still requires a recursive copy.

## Test

Use the repository wrapper for local iteration. It keeps one DerivedData cache
per platform, disables CLI-only indexing, regenerates `EnvoixCore` only after a
Rust/binding content digest changes, and runs XcodeGen only when `project.yml`
or the Apple source-file list changes. It also exposes smaller hosted/UI test
schemes. The first run establishes those fingerprints and may perform one full
Core generation; subsequent Swift-only iterations reuse the Core and project.

```bash
export ENVOIX_IOS_SIM_DESTINATION='platform=iOS Simulator,id=<SIMULATOR_UUID>'

# Build and run only the hosted contract tests. This remains incremental.
scripts/apple-dev.sh ios-test hosted

# Rerun the already-built test bundle without compiling again.
scripts/apple-dev.sh ios-test-rerun hosted

# Run UI tests or the complete pair only when their wider coverage is needed.
scripts/apple-dev.sh ios-test ui
scripts/apple-dev.sh ios-test all

# The equivalent macOS App-hosted paths.
scripts/apple-dev.sh macos-test
scripts/apple-dev.sh macos-test-rerun
```

Without `ENVOIX_IOS_SIM_DESTINATION`, the wrapper chooses an installed iPhone
16 Pro simulator by identifier, then falls back to the first available iPhone.
Using an identifier avoids Xcode interpreting a model-only destination against
an installed newer runtime that does not contain that model.

Do not create a new `-derivedDataPath` for each run. Set
`ENVOIX_XCRESULT_PATH=/private/tmp/<milestone>.xcresult` only when a milestone
needs a retained result bundle; routine runs keep their logs inside the stable
cache. To inspect or reclaim disk space:

```bash
scripts/apple-dev.sh cache-size
scripts/apple-dev.sh trim-cache             # keeps compiled products
scripts/apple-dev.sh trim-rust-incremental  # saves more; next Rust build is slower
scripts/apple-dev.sh clean-cache            # cold Xcode build next time
```

The default cache root is `$TMPDIR/envoix-apple-cache`; override it with
`ENVOIX_APPLE_CACHE_ROOT` when necessary. A signed device build uses a separate
stable cache so it cannot invalidate the simulator products:

```bash
export ENVOIX_IOS_DEVICE_DESTINATION='platform=iOS,id=<DEVICE_UUID>'
scripts/apple-dev.sh ios-device-build
```

`EnvoixCore` generation first reuses Cargo's target cache, then inspects every
object in the produced macOS/iOS static archives. If an object requires a newer
OS than macOS 13 or iOS 16, the wrapper cleans only the BLAKE3 Apple targets and
regenerates once. This retains the deployment-target safety check without
forcing that cleanup on every Rust change. Use `scripts/apple-dev.sh core-force`
to rebuild Core, or set `ENVOIX_APPLE_FORCE_PROJECT_REBUILD=1` to rerun XcodeGen,
only while diagnosing stale generated artifacts.

Project reuse also requires every shared scheme used by the wrapper to exist.
If the project bundle is incomplete even though its input digest matches, the
wrapper regenerates it instead of accepting a partial generated project.

On the 2026-07-14 development Mac, the measured paths after this change were:
full Core regeneration 34.96 s (previous unconditional-clean path 105.05 s),
unchanged Core plus Xcode project check 1.02 s, cold macOS hosted build/test
27.36 s, warm build/test 6.00 s, and `test-without-building` 2.23 s. Treat these
as a comparison on one machine, not a CI performance guarantee.

Cross-device methods report `XCTSkip` in the default suite. They execute only
with the explicit `ENVOIX_CROSS_DEVICE_TESTING` configuration and a live peer;
skipped output must not be reported as cross-device success. In addition to the
Android matrix, `testCrossDeviceSendIosToMacOSRoom` provides a specifically
named iPhone-to-Mac Room/Auto network gate.
Hosted coverage also verifies canonical snapshot ordering, terminal-only
history pruning, Rust-owned Activity action availability, and the loaded core
API/capability report. Durable publication coverage verifies that a save
failure and destination survive restart, and that replacing the destination
reuses the staged receive instead of retransmitting it. It also verifies that
the view model projects Phase from the canonical record and that the loaded
core advertises per-session receipt-endpoint support.
The app UI suite also includes a stalled-command fixture that verifies an
accepted Cancel action leaves its pending indicator and becomes actionable
again when no canonical state acknowledgement arrives within five seconds.
It audits the primary Home, Send, Receive, Activity, and Settings surfaces for
clipping, contrast, descriptions, hit regions, and supported Dynamic Type. The
small-screen checks also scroll each room-code Copy action above the fixed
transfer CTA and assert that their frames do not overlap.
Its localized layout regression can be paired with `simctl ui ... appearance`
and `content_size` to exercise Chinese, dark appearance, and accessibility text
sizes on the small-screen simulator without changing production behavior.

For a physical iPhone Personal Hotspot App-level path probe, start the hosted
receiver inside `Envoix.app`. The records and received file are isolated under
`/private/tmp`; the default Room matches the dedicated iPhone sender test:

```bash
export ENVOIX_XCRESULT_PATH=/private/tmp/envoix-hotspot-macos-app.xcresult

scripts/apple-dev.sh macos-test \
  'OTHER_SWIFT_FLAGS=$(inherited) -D ENVOIX_CROSS_DEVICE_TESTING' \
  -only-testing:Envoix-macOSTests/EnvoixMacOSHostedTests/testReceiveIosToMacOSAppRoom
```

The hosted scheme passes an explicit test-host argument. The App records and
received file are written below a PID-scoped
`$TMPDIR/envoix-macos-hosted-<PID>/` directory; the final evidence marker prints
the exact file path. This avoids modifying the user's normal Activity store and
keeps parallel sessions isolated.

With that receiver waiting, run the physical iPhone test from a second shell:

```bash
xcodebuild -project apps/envoix-apple/Envoix.xcodeproj \
  -scheme Envoix-iOS-Hosted -configuration Debug \
  -destination 'platform=iOS,id=<DEVICE_UUID>' \
  -derivedDataPath /private/tmp/envoix-apple-hotspot-ios \
  -allowProvisioningUpdates \
  'OTHER_SWIFT_FLAGS=$(inherited) -D ENVOIX_CROSS_DEVICE_TESTING' \
  test \
  -only-testing:Envoix-iOSUITests/EnvoixIOSLoopbackTests/testCrossDeviceSendIosToMacOSRoom
```

Require the macOS App activity to reach `Completed`, a non-empty Direct/Relay
path, and matching filename, size, user-visible resolved destination, and
SHA-256. This exercises the production macOS `AppModel`, `TransferViewModel`,
Activity projection, and destination path.

The physical Photos-provider payload gate uses the same receiver-first order.
Start
`EnvoixMacOSHostedTests.testReceiveIosPhotoDraftToMacOSAppRoom`, wait for its
`photo-receiver-ready` marker, and then run
`EnvoixIOSLoopbackTests.testCrossDeviceSendPhotoDraftIosToMacOSAppRoom` on the
physical iPhone. The sender creates a valid synthetic PNG provider in an
isolated test draft, stages it through `PhotoDraftImporter`, and starts the
production `AppModel.send` Room path. It does not read the user's Photos library
or replace a pending App Group draft. The macOS production receiver uses the
negotiated Manifest model even for this single item, so its canonical completed
path is the destination root; acceptance requires the same Manifest-aware URL
resolver used by Activity UI to identify the final file, followed by exact
filename, size, SHA-256, and Direct/Relay checks. This gate still does not
replace final manual Photos UI → iOS App → macOS App acceptance.

The physical Manifest gate uses the same two-peer order. Set the same run ID
and Room code in both shells, start
`EnvoixMacOSHostedTests.testReceiveIosToMacOSAppManifestRoom`, wait for its
`manifest-receiver-ready` marker, and then run
`EnvoixIOSLoopbackTests.testCrossDeviceSendIosToMacOSManifestRoom` on the
physical iPhone. The fixture sends one folder containing a regular file and an
empty directory plus one loose file. The receiver verifies the final tree,
exact payload bytes, aggregate root/file/directory counts, and both SHA-256
values. Cross-device compilation or an explicit skip does not satisfy this
gate; both result bundles must contain one executed passing test.

The reverse compatible single-file gate starts
`EnvoixIOSLoopbackTests.testCrossDeviceReceiveMacOSToIosAppInvite` on the
physical iPhone first. Copy only the payload printed after
`[cross-device] iOS App invite`, hand it to the hosted macOS app through the
one-shot test key, and then run
`EnvoixMacOSHostedTests.testSendMacOSToIosAppInvite` with the same explicit
cross-device build flag:

```bash
defaults write com.envoix.app envoix.test.macOSToIosInvite -string '<INVITE>'
```

The macOS test consumes and immediately removes this key. On Personal Hotspot
this gate deliberately requests Relay-only: Mac-to-iPhone mDNS discovery and
the canonical Auto-to-Relay retry are not yet reliable in this topology. Both
result bundles must contain one executed passing test, and acceptance requires
the Relay path plus the exact final size, SHA-256, and Manifest-aware resolved
destination on iOS. This is reverse compatible single-file evidence, not
macOS-to-iPhone multi-root Manifest acceptance.

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

The iPhone client has one home screen. **Send** and **Receive** open as sheets;
**Activity** and **Settings** are toolbar sheets instead of permanent bottom
tabs. An active transfer appears as a compact home-screen activity capsule.
Both roles can show an Android-compatible `envoix://pair/<code>` QR plus the
same short code, and the opposite role can scan the QR or enter the code. The
rendezvous broker only pairs devices, and the file still moves over the
encrypted transfer path.

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
- On iPhone, each transfer setup sheet owns its bottom safe area, so the Send
  and Receive actions remain reachable without competing with app navigation.
- Accepted Activity commands refresh their canonical snapshot after dispatch;
  a command indicator times out instead of spinning indefinitely when no state
  acknowledgement arrives.
- Once a canonical Activity record exists, it is the sole lifecycle source for
  the transfer screen; raw callbacks are retained only as startup presentation
  fallback.
- Activity buttons use the Rust core's typed action policy instead of parsing
  status text. Settings shows the loaded core and FFI API versions to make a
  stale generated package visible during development.
- A receive that finished transferring but could not publish to Files/Finder
  stays in Activity as **Save failed**. Retry reuses the current folder;
  **Choose folder** replaces the destination and saves the staged file without
  receiving it again, including after an app restart. The restored destination
  comes from the Rust durable session; the former native store is migration-only.
- New durable sessions freeze their configured receipt endpoint in Rust and
  restore it through the versioned mailbox courier. The legacy courier and
  start/restore functions remain available for existing clients.
- **Send** accepts multiple files/folders from its picker. On macOS,
  drag-and-drop accepts multiple roots, while *Paste Path* imports one file or
  folder path from the clipboard.
- During a transfer the status line shows live throughput and an ETA based on a
  short rolling average; on macOS completion includes *Reveal in Finder* and a
  copyable absolute path.
- A **menu-bar item** shows transfer status and an *Open Envoix* action; closing
  the main window keeps the app running there. The window is resizable and
  supports full screen.

## Planned work

Do not maintain a second roadmap here. The canonical execution plan currently
prioritizes responsive Apple UI and canonical state projection, then runs three
gated tracks in parallel: Files/Photos Share Extension, `ManifestV1`, and a
cross-platform Wi-Fi Aware vertical slice. Trusted devices and remote presence
follow as shared-core work. Speed limiting, parallel transport, signing, and
distribution remain later milestones in that plan.

## Notes

- `project.yml` is the source of truth; `Envoix.xcodeproj` is generated and
  git-ignored.
- The Rust static library links several Apple frameworks
  (`SystemConfiguration`, `Security`, `SecurityFoundation`, `CoreWLAN`); these
  are set in `project.yml` under `OTHER_LDFLAGS`. `CoreWLAN` in particular is
  resolved dynamically at runtime, so it must be linked even though it produces
  no link-time error when missing.
