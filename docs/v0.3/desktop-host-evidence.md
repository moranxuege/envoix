# Desktop host evidence

Status: active M7 evidence registry

This document records reproducible desktop-host evidence without credentials,
stable device identifiers, invitation material, private absolute paths, or raw
logs. A row is `pass` only when the named command ran on the stated host. CI
coverage and real-host coverage are separate.

## 2026-09-03 to 2026-09-04 reference run

| Surface | Result | Evidence |
| --- | --- | --- |
| macOS signed Debug bundle | pass | `scripts/apple-dev.sh macos-debug-signed` built and verified the Team `6638TTB2SF` app and nested helper, hardened runtime, stable designated requirements, GUI-without-Keychain-group boundary, and helper-only Keychain access group. |
| macOS helper control | pass | The rebuilt CLI used its default endpoint to query the installed helper. Diagnostics reported Agent protocol 12, application contract 6, Engine schema 2, Unix-socket transport, and `apple_keychain` credential protection. |
| macOS helper fail-closed behavior | pass | `MacOSAgentControlTests` passed 14/14, including registration and unregistration failure cases that make no helper request. |
| WSL service lifecycle state | pass | At `719b5276`, the reference WSL host completed update, restart, stop/start, default uninstall, and reinstall. Every start-like command returned only after an immediate protocol-12 status request succeeded. The reinstalled systemd user service reported `Type=notify`, active/running, one retained pairing, and no automatic restarts. |
| macOS to WSL remembered Transfer | pass | A 55-byte, non-sensitive single-file fixture reached `delivered`; the WSL Inbox recorded the matching root and byte count, and sender/receiver SHA-256 values matched. |
| Linux lifecycle contract | pass | Native WSL execution passed 29 Agent tests, 9 CLI tests, and the Linux lifecycle integration test with strict Clippy. The test replaces a current unit with a legacy `Type=simple` fixture and proves update migrates it to `Type=notify`; real-host uninstall/reinstall retained byte-identical settings and Inbox evidence. |
| Windows lifecycle contract | pass | At `719b5276`, CI run `33777325117` strictly linted and tested the Windows CLI/Agent, built both binaries, and passed the isolated Scheduled Task lifecycle script. This includes the shared standalone-host lifecycle loop changed by that revision. |
| macOS clean-user Keychain prompt audit | not run | The current signed-in account is not a clean-user environment. No claim is made from the non-interactive unit contract alone. |
| Developer ID notarization and staple | not run | `macos-release` requires a Developer ID Application identity and notarytool profile. A successful signed Debug build is not notarization evidence. |

## Windows GUI decision

v0.3 does not promote or ship a Windows GUI. The supported Windows product is
the per-user Agent plus CLI over the owner-only Named Pipe. That path provides
the application-contract and lifecycle prototype evidence, but there is no
WinUI artifact to label as supported. A later WinUI promotion requires a real
prototype to pass the same protocol-version, owner-only IPC, lifecycle,
transfer, accessibility, signing, installation, and update gates; until then,
the absence of a Windows GUI is intentional rather than an omitted release
artifact.

## Remaining M7 evidence

1. Run the signed helper from a clean macOS user and record the Keychain prompt
   count across first start, app restart, helper restart, and in-place upgrade.
2. Run `macos-release` with release signing/notary inputs and retain the
   notarization, staple, Gatekeeper, entitlement, and checksum outputs.
3. Run a receive and lifecycle check on a real Windows host; CI validates the
   isolated Scheduled Task contract but is not real-host transfer evidence.
