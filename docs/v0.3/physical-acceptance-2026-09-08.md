# v0.3 physical acceptance — 2026-09-08

Status: in progress; no stable-release approval.

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
