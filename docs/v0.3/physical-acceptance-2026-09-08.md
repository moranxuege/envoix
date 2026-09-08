# v0.3 physical acceptance — 2026-09-08

Status: in progress; no stable-release approval.

## Current checkpoint: build 8

Candidate source: `a7da56a5414f635ac580242474e26d2f960203b4`, version 0.3.0 (8).
[CI](https://github.com/moranxuege/envoix/actions/runs/34225697498) and the
[full signed release rehearsal](https://github.com/moranxuege/envoix/actions/runs/34225295953)
passed. Independent verification passed all 18 checksums and 20 provenance/SBOM
policy checks. No stable tag was created.

| Physical build 8 check | Result | Evidence boundary |
| --- | --- | --- |
| Windows upgrade/start/restart | Passed | Original pairs/settings and all 18 Inbox files retained; immediate Status ready. |
| WSL upgrade/start/restart/stop | Passed | Original pairs/settings and all 74 Inbox files retained; restart changed PID. |
| Windows → WSL pause/restart/resume | Passed | 256 MiB, paused state persisted, delivered SHA-256 matched; 59.72 s including reconnection. |
| WSL → Windows pause/restart/resume | Passed | 256 MiB, paused state persisted, delivered SHA-256 matched; 19.69 s. |
| WSL → Windows single file | Passed | 65,536 bytes; received SHA-256 matched. |
| Windows → WSL nested directory | Passed | Three files including Unicode names and an empty file; all hashes matched. |
| macOS notarization | Passed | Strict codesign, stapler and Gatekeeper accepted build 8; evidence under candidate-build8/macos. |
| macOS installed helper startup | Passed after scoped registration repair | Unchanged notarized build 8 runs with protocol 16 and both original pairings; registered-service restart also passed. |

The initial Mac upgrade failed with CODESIGNING 4 / AMFI c[5]p[1]m[1]e[0], and
the old application was temporarily restored. Follow-up diagnosis found 84 historical
Envoix main/helper Launch Services records (55 paths no longer existed). Only these
Envoix records were unregistered with the documented `lsregister -u` operation; no
application or user data was deleted. Reinstalling the unchanged notarized build 8
at `/Applications/Envoix.app`, force-registering that path and enabling its helper
resolved the failure. No global BTM reset, system restart or security bypass occurred.
This demonstrates a local registration repair; it does not identify which individual
old record caused the mismatch.

The installed build 8 helper now answers protocol 16 with both original pairings,
and all 11 original Inbox files match their pre-upgrade hashes. A registered-service
restart replaced the PID and returned ready with both pairings. A real GUI file picker
queued a 256 MiB file to the original WSL pairing; after closing the send window and
exiting the GUI, the transfer was Delivered and its receiver SHA-256 matched. WSL →
Mac background reception also delivered 286,720 bytes with an exact hash match.
Evidence: `dist/physical-20260908/macos/build8-registration-repair-and-transfer.json`.

The source also fixes a separate first-enable UI issue: after enabling or refreshing
the service, Settings refreshes the helper's device list and transfer/inbox snapshot.
The targeted macOS Release compile passed. This small macOS-only UI correction is
not yet included in the installed, already-notarized a7da56a5 binary; collect remaining
acceptance fixes before the next distribution packaging pass. No build number changed.

iOS build 8 archived and uploaded successfully, finished App Store Connect processing,
and is assigned to the existing Envoix-Internal-Test group. No new testers were added.
The connected iPad still has 0.2.2 (4); the owner was asked to update via TestFlight
and verify preserved files. Android remains physically connected per the owner, but
current adb and USB registry queries enumerate no Android device; owner-side unlock,
USB mode and accessory/debug authorization checks are pending.

Physical TestFlight installation/retention, Android production-key migration and the
remaining mobile/foreground UI matrix are still pending. Windows Authenticode remains
unsigned by the owner's explicit decision.

Release workflow correction: finish targeted runtime regression and physical desktop
checks before another Apple distribution cycle. Local rebuilds and test retries do
not by themselves require a new build number; immutable uploaded distribution builds
do. Android and Apple numbers remain aligned by the repository release contract.

The following sections retain earlier checkpoints and explain the fixes; their pending
statements are superseded only where the build 8 table above supplies new evidence.

## Candidate identities

- Core / build 6 candidate: `a24b209bda2221e00ff55969872347e83c2c7ac1`.
  [Full signed Android / desktop rehearsal](https://github.com/moranxuege/envoix/actions/runs/34214600608)
  passed; independent verification passed 18 checksums and 20 provenance/SBOM checks.
- Apple archives: `188c5eb1b600c976fe1771228da3eed1485b8017`, which adds the
  archive-only helper command without changing the preceding runtime source.
  [Apple CI](https://github.com/moranxuege/envoix/actions/runs/34216103011) passed.
- Desktop CLI readiness fix: `2a6641c8c476a4393710d040ebce72ad46fb7987`.
  [Desktop rehearsal](https://github.com/moranxuege/envoix/actions/runs/34218113642)
  passed. The three downloaded Windows executables passed exact-source,
  workflow, repository and hosted-runner provenance checks.

## Apple distribution

macOS 0.3.0 (6) was archived with Developer ID signing, submitted through the
existing Xcode Apple account, and exported with `xcodebuild -exportNotarizedApp`.
Independent `codesign --verify --deep --strict`, `stapler validate` and Gatekeeper
assessment passed. Gatekeeper reported `accepted; source=Notarized Developer ID`.
The exported app executable SHA-256 is
`703235dd0dc28dcdb98303a4fe834abad65ef33bdbcdfe15a3a73a39e178484a`.
This proves the build 6 archive; a subsequent runtime fix requires a fresh archive
and notarization before that newer candidate can be approved.

Xcode's existing Apple account is a working notarization route; recovering an
old `notarytool` Keychain profile name is no longer necessary. The sequence is
Developer ID archive, `-exportArchive` with `method=developer-id` and
`destination=upload`, then `-exportNotarizedApp`, followed by independent checks.

The iOS app and Share extension were archived and successfully uploaded as
0.3.0 (6). App Store Connect finished processing and automatically assigned the
build to the existing internal test group. The group's two existing testers can
access it; no new testers or invitations were added. The installed Store profiles
are Xcode-managed, so export uses `signingStyle=automatic`, Team 6638TTB2SF and
`method=app-store-connect`; explicitly selecting them under manual signing fails.
Physical TestFlight upgrade/launch and file-retention verification remain pending.

## Desktop physical results

| Check | Result | Scope |
| --- | --- | --- |
| WSL retained-state upgrade, protocol 12 → 16 | Passed | Original pairing, settings and all 61 Inbox files preserved; process replaced across restart. |
| Windows retained-state upgrade, protocol 12 → 16 | Passed for preservation | Initial configuration and all 3 Inbox files matched the pre-upgrade backup byte-for-byte. |
| Windows startup readiness | Passed after CLI fix | Update, restart and start followed immediately by Status succeeded; pair count, configuration and all 12 then-current Inbox files retained. |
| Windows ↔ WSL remembered pairing | Passed | Added only dedicated acceptance relationships; both pre-existing relationships retained. |
| WSL → Windows single file | Passed | 65,536 bytes, delivered state and received SHA-256 verified. |
| Windows → WSL nested directory | Passed | Three files, including Unicode/space paths and an empty file; delivered state and every received SHA-256 verified. |
| WSL → Windows 256 MiB pause/restart/resume | **Failed** | Pause survived Agent restart; Resume remained connecting for 240 seconds. |

The recovery failure is reproducible: startup parks the remembered listener as a
responder while the Transfer is paused. Resume/Retry only wakes an already active
Room, whereas Create also restarts a parked listener so it can become a connector.
Creating a separate diagnostic Transfer activated that existing path and allowed
the original 256 MiB transfer to finish with the correct SHA-256. This diagnostic
workaround is **not** counted as a passing recovery gate. A fix and a fresh test
without the workaround are required.

The CLI readiness fix passed nine local unit tests and Clippy. Its initial Linux
CI lifecycle fixture acknowledged fake systemctl operations without providing a
real control socket, and therefore failed the new readiness condition. The fixture
was updated to exchange an actual protocol-16 Status request/response; CI recheck
is pending. No production readiness bypass was introduced.

## Mobile state preservation

The USB Android installation is an older debug-signed 0.3.0 (5). Its 531 private
files were backed up (25,279,595,222 original bytes; 9,610,671,796 compressed bytes).
The compressed archive was fully read and its SHA-256 verified. Changing to the
long-term production signing identity requires uninstalling the incompatible old
signature. Owner authorization for that destructive migration is pending.
Android Keystore keys cannot be exported; old pairing credentials cannot be
restored after uninstall. Received file bytes remain in the protected backup.

The USB iPad currently has 0.2.2 (4). Its Documents, Library and shared Library
were backed up: 68 files, 21,699,933 bytes, with a per-file SHA-256 manifest.
The owner has been asked to update through TestFlight and verify retained files.
The iPhone is not currently connected for physical acceptance.

The Android matrix evidence collector previously required `run-as`, which is
unavailable for production non-debuggable APKs. Test-only evidence now travels
through instrumentation status, with bounded decoding and removal from sanitized
logs. This does not change the application APK or enable debugging. The 52 matrix
script tests passed; compilation and physical execution remain in progress.

## Owner-accepted limitations and outstanding gates

The owner explicitly chose not to purchase Windows signing at this time.
Windows remains unsigned, with that limitation disclosed; no Authenticode,
SmartScreen reputation or publisher identity is claimed.

Outstanding: fresh recovery-fix validation, final-candidate macOS notarization,
physical Android signing migration/upgrade, iPad TestFlight upgrade/launch,
mobile cross-device transfers/recovery, and remaining GUI/foreground integration
checks. Keep `v0.3.0` unpublished while product correctness gates remain open.

## Build 7 follow-up and build 8 correction

Build 7 (`75855ddc5f0e1b1cf24d9e11a5e6a3156c9ebe35`) passed CI and the full
signed release rehearsal (runs 34221543456 and 34221543678). Independent artifact
verification passed 18 checksums and 20 provenance/SBOM checks. Its macOS app
passed notarization, stapling and Gatekeeper; its iOS archive uploaded successfully.
Physical TestFlight installation is still pending.

Windows and WSL retained-state upgrades and immediate startup readiness passed.
Both single-file and nested-directory transfers passed with received SHA-256
verification. WSL → Windows 256 MiB pause/restart/resume passed without a workaround.
The reverse Windows → WSL case failed: a late attempt event replaced the persisted
Paused state with Failed/InternalError after restart. This is a release blocker.

Build 8 preserves durable Paused/Canceled states when late progress or attempt
results arrive. Settlement reads and updates the state under the same store lock.
Regression coverage exercises success, internal error and network loss after both
controls, persistence across reopen, and late progress after pause. All 31 Agent
tests and Clippy with warnings denied passed locally. Fresh candidate CI, Apple
distribution and both physical recovery directions must pass before this fix is
considered accepted. Android signing migration still awaits permission to uninstall
the old debug-key app; the verified backup does not preserve Android Keystore keys.
