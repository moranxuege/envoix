# Apple Room transfer diagnostic handoff — 2026-07-16

Status: **Paused at the user's request. Do not start another cross-device run
until the manual-test host lifetime is understood.**

Scope: iPhone 15 Pro Max ↔ macOS Envoix App, Photos Share Extension → iOS Send
→ Room → macOS receiver. No Simulator was started in this diagnostic cycle.

## Starting point

- Repository: `ECE4410J-NUUB`
- Branch: `feat/transfer-state-foundation`
- Latest pushed commit: `4fa0efb fix(ffi): expose manifest transport landmarks`
- Worktree at pause: clean (`git status --short` produced no entries)
- Current paired phone: `1C31E041-5DC2-5CB5-92D6-CFAE1F85B1A1`
- A signed Debug iOS build containing `4fa0efb` was built, installed, and
  launched on that phone. Existing app data was retained.

## What is already proven

1. A real two-image Photos Share Extension flow has succeeded end-to-end:
   Photos → Share → Envoix → manually return to Envoix → Send → Room → macOS
   app receiver. The macOS test receiver verified exactly two files, two roots,
   `1,071,694` bytes, and recomputed Manifest BLAKE3 hashes. The selected path
   was direct IPv6. This is evidence for the product payload path, although the
   test receiver's output is intentionally temporary rather than Finder's
   Downloads folder.
2. The user's successful report for that run had an exact 30-second pre-start
   gap: `created_at=10:44:15`, `started_at=10:44:45`, then immediate completion.
   The payload itself was not slow.
3. Manifest diagnostics now preserve useful phase history without changing the
   durable Activity contract or native observer interface:

   | Commit | Change | Verification |
   | --- | --- | --- |
   | `4d50c40` | Manual two-Photos cross-device acceptance gate | physical successful payload/hash run |
   | `23da10b` | Merge AppModel Activity timeline into Manifest diagnostic report | focused macOS hosted test passed |
   | `4fa0efb` | Forward ephemeral `binding`, `pairing`, `connecting`, and path landmarks to native observers while leaving canonical durable state untouched | `cargo fmt --check`; focused and full `envoix-ffi` Manifest tests; focused macOS hosted diagnostics test |

4. The apparent “same Room” issue is not an error: a Room is a rendezvous
   secret shared by exactly one sender and one receiver. Opposite `JoinIntent`s
   are required by the broker. Same-direction peers do not match.

## Latest failed diagnostic run: facts only

The second manual run was launched with the already-built macOS test bundle:

```sh
scripts/apple-dev.sh macos-test-rerun \
  -only-testing:Envoix-macOSTests/EnvoixMacOSHostedTests/testReceiveIosManualPhotosShareToMacOSAppManifestRoom
```

The receiver printed ready at `2026-07-16 19:28:39 CST`.

### iPhone durable record

The current iPhone Manifest record was copied read-only to
`/private/tmp/envoix-ios-current-record.json`. It proves:

- direction: `send`; mode: `room`; two selected JPEGs; Manifest total
  `1,071,694` bytes;
- the sender did use the receiver's Room code and selected a direct IPv6
  endpoint, `[240a:42a3:fe00:17e8:7c91:4bfa:eb75:fab4]:60260`;
- it created at `19:31:19`, failed at `19:32:49`, transferred `0` bytes;
- terminal error: `I/O error during transfer: connection lost`;
- failure classification at present is `internal_error / internal / transferring`.

### macOS test host

The associated Xcode result bundle is:

```text
$TMPDIR/envoix-apple-cache/macos-debug/Logs/Test/
Test-Envoix-macOS-Hosted-2026.07.16_19-28-35-+0800.xcresult
```

It started at `19:28:35` and ended at `19:32:29`, before the iPhone observed
the lost connection. Its sole failure was:

```text
InvalidTransition {
  phase: idle
  targetPhase: failed(removedFromContainer)
}
```

`removedFromContainer` does not occur in the Envoix source tree. Do **not**
conclude that AppModel removed a production Activity or that Room pairing is
broken from this error alone; it is currently most plausibly an XCTest/test-host
lifetime failure. The macOS process disappearing before the iPhone's
`connection lost` is enough to make this run unusable as a transport verdict.

### Fixed-code test limitation

`EnvoixMacOSHostedTests.roomCode` is declared as:

```swift
environment("ENVOIX_IOS_TO_MACOS_CODE") ?? "741205-silver-forest"
```

in `apps/envoix-apple/Tests/EnvoixMacOSHostedTests/EnvoixMacOSHostedTests.swift`.
Passing `ENVOIX_IOS_TO_MACOS_CODE=...` to `xcodebuild` did **not** reach the
test-host process: the persisted receiver record used the hard-coded fallback.
That fixed public test code can collide with stale/manual peers. It must not be
used as a reliable unique-code mechanism for manual acceptance.

## Transport hypothesis — not yet a fix

There is one concrete candidate for the prior 30-second gap:

- `crates/envoix-session/src/room.rs`, `pair_room_receiver`, calls
  `bound.ready_endpoint_addr(config.data_relay().is_some())` before joining the
  Room.
- `crates/envoix-session/src/endpoint.rs` has a 30-second endpoint-address
  wait. With a configured relay it waits for a relay home even when a direct
  address may already be usable.

This is a plausible explanation, not proof. The relay remains necessary for
remote/NAT reachability, so do **not** simply disable relay, lower the timeout,
or turn Auto into Direct-only. First obtain a clean trace showing whether the
gap is before `pairing`, between pairing landmarks, or during dialing.

## Recommended next session sequence

1. **Repair the manual acceptance harness before changing transport.**
   Inspect the Xcode test plan/scheme timeout and extract the full test activity
   log from the result bundle. Establish why the host ends around four minutes
   even though `manualPhotosTimeout` is 300 seconds.
2. **Give the test receiver a real unique input channel.**
   Do not assume a shell environment variable reaches XCTest. Choose a
   supported launch/test configuration mechanism or a deliberately created,
   test-owned input file; preserve the default behavior for normal automated
   tests. Ensure the ready marker prints the exact code actually in the durable
   receiver record.
3. **Run one clean manual attempt.**
   Keep the receiver alive, have the user send two small Photos through the
   Share Extension, and collect both:
   - iPhone `App Diagnostic Report`, especially `[activity_events]`; and
   - the macOS test result plus final temporary receiver output and hashes.
4. **Only then decide the transport change.**
   If the phase trace confirms the relay-address wait, design an additive
   solution that advertises/direct-dials promptly while retaining a relay path
   for remote devices. Add a targeted Rust regression before a physical rerun.
5. **Keep the independent product boundary open:** macOS Receive UI →
   user-selected/default Downloads → Finder must still receive its own manual
   acceptance test. The current hosted receiver writes only under a PID-scoped
   temporary directory by design.

## Useful locations and commands

| Purpose | Location / command |
| --- | --- |
| Manual macOS receiver test | `apps/envoix-apple/Tests/EnvoixMacOSHostedTests/EnvoixMacOSHostedTests.swift` (`testReceiveIosManualPhotosShareToMacOSAppManifestRoom`) |
| Test Room default | same file, `roomCode` near line 889 |
| Room receiver setup | `crates/envoix-session/src/room.rs` (`pair_room_receiver`) |
| Endpoint address wait | `crates/envoix-session/src/endpoint.rs` (`ready_endpoint_addr`) |
| Raw Manifest transport landmarks | `crates/envoix-ffi/src/manifest.rs` (`observe_manifest_transport_event`) |
| Apple diagnostic report merge | `apps/envoix-apple/Sources/TransferViewModel.swift` (`diagnosticsSnapshot`) |
| Build/install physical iOS app | `ENVOIX_IOS_DEVICE_DESTINATION='platform=iOS,id=1C31E041-5DC2-5CB5-92D6-CFAE1F85B1A1' scripts/apple-dev.sh ios-device-build`, then `xcrun devicectl device install app ...` |
| Read app-owned iPhone files | `xcrun devicectl device info files --device 1C31E041-5DC2-5CB5-92D6-CFAE1F85B1A1 --domain-type appDataContainer --domain-identifier com.envoix.app.ios ...` |
| Export one iPhone record read-only | `xcrun devicectl device copy from --device 1C31E041-5DC2-5CB5-92D6-CFAE1F85B1A1 --domain-type appDataContainer --domain-identifier com.envoix.app.ios --source 'Library/Application Support/envoix/transfer-records/<record>.json' --destination /private/tmp/<record>.json` |

## Constraints to preserve

- The user explicitly wants remote-capable behavior; do not sacrifice relay
  fallback merely to remove a nearby-device delay.
- Do not revive the obsolete standalone short-code flow; Room Code is the
  manual primitive.
- Avoid long-lived or multiple Simulators because of a confirmed audio artifact
  on this Mac. None was launched during this diagnostic run.
- Keep changes additive at the Rust/UniFFI boundary and compile-compatible with
  the current Android app.
- Use structured diagnostics and regression tests. Do not turn an unconfirmed
  timing hypothesis into a product behavior change.
