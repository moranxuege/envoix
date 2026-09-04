# Desktop host evidence

Status: active M7 evidence registry

This document records reproducible desktop-host evidence without credentials,
stable device identifiers, invitation material, private absolute paths, or raw
logs. A row is `pass` only when the named command ran on the stated host. CI
coverage and real-host coverage are separate.

## 2026-09-03 to 2026-09-04 reference run

| Surface | Result | Evidence |
| --- | --- | --- |
| macOS signed Debug bundle | pass | At `89b216ab`, `scripts/apple-dev.sh macos-debug-signed` with explicitly authorized automatic provisioning built and verified the Team `6638TTB2SF` app and nested helper, hardened runtime, stable designated requirements, GUI-without-Keychain-group boundary, and helper-only Keychain access group. Xcode manages this development credential outside the identity set reported by standalone `security find-identity`, so later development builds must keep the explicit provisioning opt-in. |
| iPhone development build | pass | At `89b216ab`, the universal iOS target built for the paired iPhone 15 Pro Max with the Team `6638TTB2SF` development profile. The device accepted installation and launched `com.envoix.app.ios`. This is device/development evidence, not App Store/TestFlight distribution evidence. |
| macOS helper control | pass | The rebuilt CLI used its default endpoint to query the installed helper. Diagnostics reported Agent protocol 12, application contract 6, Engine schema 2, Unix-socket transport, and `apple_keychain` credential protection. |
| macOS helper fail-closed behavior | pass | `MacOSAgentControlTests` passed 14/14, including registration and unregistration failure cases that make no helper request. |
| WSL service lifecycle state | pass | At `719b5276`, the reference WSL host completed update, restart, stop/start, default uninstall, and reinstall. Every start-like command returned only after an immediate protocol-12 status request succeeded. The reinstalled systemd user service reported `Type=notify`, active/running, one retained pairing, and no automatic restarts. |
| macOS to WSL remembered Transfer | pass | A 55-byte, non-sensitive single-file fixture reached `delivered`; the WSL Inbox recorded the matching root and byte count, and sender/receiver SHA-256 values matched. |
| Linux lifecycle contract | pass | Native WSL execution passed 29 Agent tests, 9 CLI tests, and the Linux lifecycle integration test with strict Clippy. The test replaces a current unit with a legacy `Type=simple` fixture and proves update migrates it to `Type=notify`; real-host uninstall/reinstall retained byte-identical settings and Inbox evidence. |
| Windows lifecycle contract | pass | At `719b5276`, CI run `33777325117` strictly linted and tested the Windows CLI/Agent, built both binaries, and passed the isolated Scheduled Task lifecycle script. This includes the shared standalone-host lifecycle loop changed by that revision. |
| Windows graphical shell and real-host Agent | pass | On 2026-09-04, the candidate `envoix-windows` shell, CLI, and Agent were built natively on the reference Windows 11 x86_64 host with Rust 1.96.0. Strict Clippy passed with zero warnings, GUI tests passed 2/2, and the release executable launched a responsive `Envoix` window. UI Automation/AccessKit read the paired-device Room card and send controls, the `delivered` activity with exact byte progress, the saved Inbox item, and settings diagnostics. Native 2240x1440 framebuffer captures of the Devices, Activity, and Inbox pages were inspected without activating the application or interrupting the foreground desktop; the final pass found no unsupported-glyph placeholders. The current-user install reported protocol 12, application contract 6, Engine schema 2, `windows_named_pipe`, and `windows_dpapi`. The host had Microsoft YaHei and SimSun available; the rebuilt shell loads either as a CJK fallback. No invitation, credential, or stable host identifier is retained here. |
| Windows and macOS Agent transfer | pass | The physical Windows and Mac hosts completed protocol-12 verification and one remembered Transfer in each direction over the configured production broker/relay. Windows-to-Mac delivered 55 bytes and Mac-to-Windows delivered 63 bytes. In both cases the sender reached `delivered`, the receiver Inbox recorded one saved root and the exact byte count, and the source/destination SHA-256 values matched. The Windows GUI projected the completed outgoing activity and incoming Inbox item from that same Agent. Transfer creation used the CLI control client, so this does not claim automated coverage of the native file-picker gesture. |
| macOS clean-user Keychain prompt audit | not run | The current signed-in account is not a clean-user environment. No claim is made from the non-interactive unit contract alone. |
| Developer ID notarization and staple | not run | `macos-release` requires a Developer ID Application identity and notarytool profile. A successful signed Debug build is not notarization evidence. |

## Windows GUI decision

v0.3 now includes `envoix-windows`, a Rust/egui Windows graphical shell over
the stable owner-only Agent control contract. This is an architecture change
from the earlier WinUI candidate, chosen because it reuses the typed Rust
request/response model directly and cannot silently grow a parallel JSON or
Engine implementation. It is a native executable rather than a web wrapper,
but it does not claim WinUI controls. Promotion still requires the same
protocol-version, owner-only IPC, lifecycle, transfer, accessibility, signing,
installation, and update gates. The release bundle labels the current lack of
Authenticode/SmartScreen reputation instead of implying platform signing.

## Remaining M7 evidence

1. Run the signed helper from a clean macOS user and record the Keychain prompt
   count across first start, app restart, helper restart, and in-place upgrade.
2. Run `macos-release` with release signing/notary inputs and retain the
   notarization, staple, Gatekeeper, entitlement, and checksum outputs.
3. Complete the foreground Windows proof defined in
   `apps/envoix-windows/README.md`: picker selection visible in the Room,
   final Send gesture, transfer ID, sender `delivered`, receiver Inbox root,
   size/SHA-256 equality, and one GUI-close continuation run. Background UI
   Automation verified the shell's accessible controls and post-transfer
   projections, but the modal picker correctly rejected synthetic background
   file selection.
4. Sign the release EXE with the intended Windows publisher identity and an
   RFC 3161 timestamp. Retain successful `signtool verify /pa /all /v` and
   `Get-AuthenticodeSignature` (`Valid`) output, then download the exact
   checksummed asset over HTTPS on a clean Windows environment and record the
   separate SmartScreen reputation result.
