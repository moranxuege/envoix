# Envoix handoff — 2026-07-12

## Stop state

- Work stopped immediately at the user's request. The active cross-device test orchestration was interrupted; do not treat that run as passed.
- Branch: `feat/transfer-state-foundation` tracking `origin/feat/transfer-state-foundation`.
- The worktree contains extensive intentional, uncommitted work from the current UI/lifecycle effort. Preserve it. Do not reset, checkout, clean, or overwrite unrelated changes.
- The active product goal remains unfinished: unify the Android/iOS/macOS UI around the Envoix design reference, keep every action aligned with the canonical transfer lifecycle, and prove reliable bidirectional physical-device transfers without retry-masked false positives.
- Design reference: `../Design.png` (navy `#0A1330`, cobalt `#0D47A1`, azure `#1677FF`, icy gray `#E6ECF5`, white).

## Device/install state

- iPhone 15 Pro Max, CoreDevice ID `1C31E041-5DC2-5CB5-92D6-CFAE1F85B1A1` (Xcode UDID `00008130-00043154346B803A`): the latest UI Debug build was installed and launched successfully as `com.envoix.app.ios`.
- The iPhone app was installed before the final Android DNS-source refactor. That refactor is Android-specific in behavior, but rebuild/reinstall iOS before claiming the phone matches the exact current source tree.
- Android physical device: `C6TW5PW4R87HEA8P`. The APK containing the Android platform-DNS fix was built and installed by the last test script.
- No physical transfer test is currently running. Re-check processes/devices before starting another run.

## Completed implementation

### Android crash and UI

- Fixed the Logs-screen startup crash by initializing `Diagnostics` in `EnvoixApp.onCreate` before the UI can read it.
- Added `LogScreenInstrumentedTest`; it passed on the physical Android device.
- Reworked Send/Receive/Activity/Settings surfaces around the Envoix brand: floating glass navigation, QR-first send/receive, larger touch targets, paste-oriented code entry, prominent file/destination selection, calmer Activity cards, and developer-only diagnostics.
- Activity controls are derived from the canonical transfer action policy instead of ad-hoc UI state.
- Removed relay URLs from compact Activity summaries; detailed path information remains in Details/Developer diagnostics.

### Apple UI

- Reworked iOS navigation into a low-sensitivity liquid floating stage bar; macOS uses a rail.
- Normal Pairing controls are hidden; advanced pairing is only exposed in Developer Mode.
- Send/Receive are QR-first, code is fully visible, file/destination choices are prominent, and Activity actions are larger and less cramped.
- Activity controls use the canonical lifecycle action policy. Developer Mode exposes useful IDs, path, logs, and diagnostic upload controls.
- Added debug-only Activity fixtures and stable accessibility identifiers for UI automation.

### Canonical lifecycle/action policy

- Android: `TransferAction` plus `availableTransferActions` in `Transfer.kt`.
- Apple: `ActivityActionAvailability` plus `activityActionAvailability` in `Support.swift`.
- Pause/Resume/Cancel/Delete visibility now follows these policies on both platforms.

## Verification already completed

- Android `assembleDebug`, `assembleDebugAndroidTest`, `ktlintCheck`, action-policy unit tests: passed.
- Android Logs physical-device regression: `OK (1 test)`.
- Android Activity UI instrumentation passed on a clean Android 34 arm64 emulator. The temporary AVD was removed.
- iOS UI suite: 2/2 passed (`testTransferScreenShowsStableControls`, `testActivityActionsMatchCanonicalLifecycle`).
- iOS generic-device Debug unsigned build: passed.
- macOS arm64 build and visual inspection: passed.
- Rust test after the DNS refactor:
  `cargo test -p envoix-session platform_system_dns_separates_and_deduplicates_address_families` — passed.
- Android rebuild after the DNS refactor:
  `:app:assembleDebug :app:assembleDebugAndroidTest` — `BUILD SUCCESSFUL`.

## Latest strict physical-transfer failure

The first strict Android→iPhone run failed because Android's iroh/Hickory resolver could not resolve `envoix.chkxwlyh.us`, even though `adb shell ping` could resolve it. This was a real app-process/platform-DNS mismatch.

Minimal uncommitted fix applied:

- `crates/envoix-session/src/endpoint.rs`: generalized the existing Apple system resolver into a mobile `PlatformSystemDnsResolver`; Android and iOS now resolve relay A/AAAA records through `tokio::net::lookup_host`/the OS resolver.
- `crates/envoix-session/src/room.rs`: supplies that resolver to both rendezvous and data endpoints on Android/iOS.
- `crates/envoix-session/Cargo.toml`: makes `n0-error` available for both mobile targets.

The strict rerun proved the DNS portion fixed. Android logs repeatedly showed:

```text
platform system DNS lookup completed host="envoix.chkxwlyh.us" addrs=[67.230.187.238:0]
```

It then failed at a later network layer:

```text
sendmsg error: Os { code: 1, kind: PermissionDenied, message: "Operation not permitted" }
destination: 67.230.187.238:7842
error sending mDNS: Operation not permitted (os error 1)
discovery error: no iroh mDNS peers discovered within 5 seconds
```

The room route did not complete, fell back to mDNS, and the iPhone remained waiting in room pairing. The test was manually interrupted when the user requested stop, so reverse direction did not run.

Logs for the DNS-fixed rerun:

```text
/var/folders/dn/xmztcp9551z4m0kqfbr74m_m0000gn/T/envoix-cross-device-20260712-195201-64030
```

Earlier pre-fix failure logs:

```text
/var/folders/dn/xmztcp9551z4m0kqfbr74m_m0000gn/T/envoix-cross-device-20260712-192642-61616
```

## Next safe steps

1. Confirm `git status` and preserve the dirty tree. Review the three DNS-refactor files above before changing them.
2. Reproduce the post-DNS failure once, without automatic retry, while capturing Android core logs and iOS test logs.
3. Diagnose `EPERM` separately from DNS. Check the Android app process's active network, VPN/firewall/data-saver policy, socket/network binding, and whether UDP `67.230.187.238:7842` is expected/available. Do not hardcode the relay IP or disable TLS verification.
4. Add the smallest targeted Android app-context network regression probe needed to distinguish HTTPS/WebSocket relay reachability from UDP/STUN reachability. Shell `ping` alone is insufficient evidence.
5. Rebuild Apple Core/iOS after the shared source rename, reinstall the iPhone, then run the strict baseline below with `ENVOIX_CROSS_DEVICE_ALLOW_RETRY=0`.
6. Only after one clean baseline, run repeated bidirectional transfer, pause/resume, invite flow, cancellation/cleanup, destination publication, and size + SHA-256 checks. A retry-success must not replace the original failure.

Strict baseline command:

```sh
env \
  ENVOIX_IOS_DESTINATION=platform=iOS,id=1C31E041-5DC2-5CB5-92D6-CFAE1F85B1A1 \
  ENVOIX_SKIP_BUILD=1 \
  ENVOIX_CROSS_DEVICE_REPEAT=1 \
  ENVOIX_CROSS_DEVICE_ALLOW_RETRY=0 \
  ENVOIX_CROSS_DEVICE_TIMEOUT_SECONDS=240 \
  ENVOIX_CROSS_DEVICE_TIMEOUT_MS=240000 \
  ENVOIX_ANDROID_TO_IOS_BYTES=8388608 \
  ENVOIX_IOS_TO_ANDROID_BYTES=8388608 \
  scripts/mobile-cross-device-room-test.sh both
```

## High-value files

- Android startup/crash: `android/app/src/main/java/dev/envoix/app/EnvoixApp.kt`
- Android lifecycle/actions: `android/app/src/main/java/dev/envoix/app/Transfer.kt`
- Android UI: `android/app/src/main/java/dev/envoix/app/ui/HomeScreen.kt`, `NewTransferSheet.kt`, `SettingsScreen.kt`
- Android physical transfer test: `android/app/src/androidTest/java/dev/envoix/app/CrossDeviceRoomInstrumentedTest.kt`
- Apple UI/state: `apps/envoix-apple/Sources/ContentView.swift`, `Components.swift`, `Theme.swift`, `Support.swift`, `TransferViewModel.swift`
- Apple physical test: `apps/envoix-apple/Tests/EnvoixIOSUITests/EnvoixIOSLoopbackTests.swift`
- Mobile DNS/endpoints: `crates/envoix-session/src/endpoint.rs`, `crates/envoix-session/src/room.rs`
- Orchestration: `scripts/mobile-cross-device-room-test.sh`

## Important cautions

- Do not mark the active goal complete: no strict bidirectional physical transfer has passed after the latest changes.
- Do not interpret the one-second completion symptom as solved solely from UI tests; final destination publication and hash verification still require physical validation.
- Do not reintroduce fake UI controls. Every visible Pause/Resume/Cancel/Delete action must map to the canonical session and return truthful feedback.
- Keep progress callback throttling and bounded transport windows; they were introduced to address Apple heat/UI flooding and must be revalidated, not casually removed.
- The llm-wiki is lower priority until the product goal is actually complete.
