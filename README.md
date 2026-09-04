# Envoix

Envoix is a native, authenticated device-to-device transfer application for
files, folders, Photos, and Share-provider content. Every transfer uses one
canonical Manifest v2 job from local preparation through receiver save and
delivery proof.

v0.3.0 is an active architecture-reset release. Its scope, compatibility
policy, engineering rules, and milestone gates are documented in the
[v0.3 architecture index](docs/v0.3/README.md). The v0.2.2 download page
remains the current public release until those gates are complete.

**[Download Envoix / 下载 Envoix](https://ece4410j-nuub.github.io/envoix/)**

## Core behavior

- One immutable sealed job can contain multiple files and folders, including
  empty directories and multiple roots.
- Source enumeration and validation start locally when items are selected.
  Hashing may finish opportunistically while payload reading begins, so a slow
  preflight never blocks connection startup. No offer is sent before the
  explicit Send action seals the job.
- The receiver can inspect the authenticated inventory before payload and must
  choose its save method before accepting it.
- A sender remains in “waiting for receiver to save” after payload transfer.
  Delivered is reported only after the receiver has saved the verified roots
  and returned a persistent delivery proof.
- Interrupted jobs resume from durable per-entry checkpoints. A completed
  delivery proof can be replayed without retransmitting payload.
- Existing destination names use keep-both allocation. The receiver's final
  saved names are part of the result set and delivery proof.

Manifest v1 and the former single-file protocol are not supported.

## CLI

### Persistent desktop Agent

The Agent gives Linux/WSL and Windows a durable Inbox and Outbox while the CLI
remains its local controller:

#### Linux/WSL

```bash
scripts/with-build-cache-guard.sh cargo build -p envoix-agent -p envoix-cli

# Install both binaries and start a systemd user service. This keeps the Inbox
# in the repository for the current development workflow.
target/debug/envoix agent install --inbox "$PWD/inbox" --device-name WSL

# In another shell, create an ordinary Room plus a one-time verification code.
~/.local/bin/envoix agent status
~/.local/bin/envoix agent pair --name MacBook

# On the Mac, enter the printed Room code, then enter the six-digit code when
# Envoix asks to verify WSL. On macOS the signed helper reconnects and owns the
# credential commit. No transfer is required to finish pairing.
~/.local/bin/envoix devices list
~/.local/bin/envoix devices forget MacBook --yes
~/.local/bin/envoix transfers create --device MacBook ./photo.jpg ./folder
~/.local/bin/envoix transfers list
~/.local/bin/envoix transfers show '<transfer-id>'
~/.local/bin/envoix inbox list
~/.local/bin/envoix inbox latest
~/.local/bin/envoix inbox set-directory /absolute/path
~/.local/bin/envoix agent diagnostics

# Replace both installed binaries from a newly built pair. This preserves
# settings and Agent data when the Engine schema is compatible.
target/debug/envoix agent update --agent-binary target/debug/envoix-agent
```

Use `envoix agent stop`, `start`, and `restart` to manage the installed service.
`envoix agent uninstall` removes the service and installed binaries while
preserving settings, Engine state, credentials, and Inbox files. The explicit
`uninstall --delete-state --yes` form also removes allowlisted Engine state and
credentials, but still never removes received Inbox files. The installer
enables systemd user-service autostart but does not edit `/etc/wsl.conf`; if
systemd is unavailable, its error includes the equivalent foreground command.

The v0.3 test cycle intentionally breaks v0.2 ProductStore and Engine schema
v1 state. If startup reports `unsupported legacy state`, reset Agent-owned
state with `target/debug/envoix agent uninstall --delete-state --yes`, install
the new binary pair again, and re-pair devices. This removes Relationships,
credentials, and transfer history; the allowlisted cleanup still preserves
the configured Inbox and unknown files.

#### Windows 10/11

Keep the three Windows release binaries together. Start the graphical client;
if no Agent is installed, its recovery screen can install and start the
current-user Agent without administrator privileges:

```powershell
.\Envoix-Windows-x86_64.exe
```

The same lifecycle remains available from PowerShell:

```powershell
.\envoix-cli-windows-x86_64.exe agent install `
  --agent-binary .\envoix-agent-windows-x86_64.exe `
  --device-name Windows
& "$env:LOCALAPPDATA\Envoix\bin\envoix.exe" agent status
& "$env:LOCALAPPDATA\Envoix\bin\envoix.exe" agent restart
```

The GUI provides paired-device Room cards, file/folder selection, transfer
activity with delivery-state distinctions, pending-offer approval, Inbox
reveal, pairing, revocation, and Agent diagnostics. It controls the Agent over
the owner-only Named Pipe; it never loads the Engine store or raw credentials.
Closing the GUI does not stop queued or active transfers.

The installer copies the CLI/Agent pair under `%LOCALAPPDATA%\Envoix\bin`, keeps settings
under `%LOCALAPPDATA%\Envoix\config`, and registers `Envoix Agent <user-SID>` as
a current-user Task Scheduler task. It runs only with an interactive user token
at limited privilege, starts at logon, and retries failures without storing a
password. The same update and uninstall commands and Inbox-preservation policy
shown above apply on Windows.

`devices forget <ID-or-label> --yes` revokes that remembered credential and
stops future reconnects without deleting completed Inbox files or history.
`transfers create` seals the selected roots into durable content and queues one
Transfer. If the remembered Room is connected, the peer immediately receives
the normal file offer; otherwise the Agent dispatches it after reconnect. A
process restart preserves queued and in-flight state.

On macOS, remembered peers appear as devices. Choose **Send**, or drag files
and folders directly onto a device; Envoix opens the normal review screen and
still requires the explicit final **Send** action. An installed app also adds
**Finder > Services > Send with Envoix** for selected files and folders.
**Paste File or Image** remains available for Finder items, existing paths, and
clipboard images. Clipboard images are materialized in Envoix's durable source
cache before entering the same Manifest v2 and remembered-room queue used by
ordinary files.

See [the Agent MVP design](docs/design/agent-mvp.md) for trust, persistence,
network-path behavior, and the WSL networking modes.

### QR/direct invite

```bash
# Receiver
cargo run -p envoix-cli -- receive --enable-mdns --output ./received

# Sender: paste the invite printed by the receiver
cargo run -p envoix-cli -- send --invite '<invite>' ./photo.jpg ./folder
```

### LAN mDNS with a shared token

```bash
# Receiver
cargo run -p envoix-cli -- receive --enable-mdns \
  --token shared-token-123 --output ./received

# Sender
cargo run -p envoix-cli -- send --enable-mdns \
  --token shared-token-123 ./photo.jpg ./folder
```

### Manual endpoint

```bash
# Receiver prints its peer descriptor
cargo run -p envoix-cli -- receive \
  --token shared-token-123 --output ./received

# Sender uses that descriptor
cargo run -p envoix-cli -- send \
  --peer '<endpoint-id>@<address>' \
  --token shared-token-123 ./photo.jpg ./folder
```

The sender accepts additional positional files/folders as roots of the same
job. `--compression never|always|smart` selects the sealed compression policy.
The receiver defaults to direct save; `--save-mode copy-after-verify` explicitly
accepts an additional verified copy and its peak-space cost. Exceptionally
large offers require `--approve-large-transfer`.

Runtime TOML configuration is transport-only:

```toml
data_stream_window = "32MB"

[candidates]
allow = ["192.168.0.0/16"]
deny = ["100.64.0.0/10"]
```

## Repository layout

- `crates/envoix-protocol`: authentication envelope and Manifest v2 frames.
- `crates/envoix-transfer`: canonical job preparation, sequential data plane,
  destination planning, checkpoints, compression, and delivery authority.
- `crates/envoix-session`: authenticated iroh, Room, mDNS, and resume sessions.
- `crates/envoix-ffi`: UniFFI surface for Apple and shared native semantics.
- `crates/envoix-client::product`: remembered-device, Inbox, and local Agent
  command contract.
- `apps/envoix-agent`: persistent Windows/Linux/WSL Engine, Inbox, and Outbox
  owner.
- `apps/envoix-windows`: Windows graphical controller over the typed Agent API.
- `crates/envoix-ffi/src/android_jni`: exceptional Android runtime and platform save JNI gate inside the typed native core.
- `apps/envoix-apple`, `android`, `apps/envoix-cli`: native front ends.

See [the Manifest v2 contract](docs/design/manifest-v2-goal0-contract.md) for
the protocol and persistence boundaries, and [authentication](docs/auth.md) for
the SPAKE2/channel-binding model. [Rendezvous deployment](docs/rendezvous-deployment.md)
covers running your own broker.

## Debug build marker

Before distributing a new device build, keep these labels identical so testers
can identify the installed build:

- Apple: `apps/envoix-apple/Sources/Support.swift`
- Android: `android/app/src/main/java/dev/envoix/app/DebugBuild.kt`

UDP GSO remains disabled on Android for emulator and device compatibility.
