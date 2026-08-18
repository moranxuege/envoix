# Envoix

Envoix is a native, authenticated device-to-device transfer application for
files, folders, Photos, and Share-provider content. Every transfer uses one
canonical Manifest v2 job from local preparation through receiver save and
delivery proof.

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

### Persistent WSL Agent

The Agent turns WSL into a remembered receiver with a durable Inbox while the
CLI remains its local controller:

```bash
scripts/with-build-cache-guard.sh cargo build -p envoix-agent -p envoix-cli

# Install both binaries and start a systemd user service. This keeps the Inbox
# in the repository for the current development workflow.
target/debug/envoix agent install --inbox "$PWD/inbox" --device-name WSL

# In another shell, create an ordinary Room plus a one-time verification code.
~/.local/bin/envoix agent status
~/.local/bin/envoix agent pair --name MacBook

# On the Mac, enter the printed Room code, then enter the six-digit code when
# Envoix asks to verify WSL. No transfer is required to finish pairing.
~/.local/bin/envoix devices list
~/.local/bin/envoix inbox list
~/.local/bin/envoix inbox latest
```

Use `envoix agent stop` and `envoix agent start` to manage the installed
service. The installer enables autostart for the user service but does not edit
`/etc/wsl.conf`; if systemd is unavailable, its error includes the equivalent
foreground command.

On macOS, remembered peers appear as devices. Choose **Send**, or drag files
and folders directly onto a device; Envoix opens the normal review screen and
still requires the explicit final **Send** action. An installed app also adds
**Finder > Services > Send with Envoix** for selected files and folders.
**Paste File or Image** remains available for Finder items, existing paths, and
clipboard images. Clipboard images are materialized in Envoix's durable source
cache before entering the same Manifest v2 and remembered-room queue used by
ordinary files.

See [the Agent MVP design](docs/design/agent-mvp.md) for trust, persistence,
network-path behavior, and the current WSL NAT limitation.

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
- `apps/envoix-agent`: persistent Linux/WSL receiver and Inbox owner.
- `apps/envoix-android-jni`: Android JNI projection and platform save gate.
- `apps/envoix-apple`, `android`, `apps/envoix-cli`: native front ends.

See [the Manifest v2 contract](docs/design/manifest-v2-goal0-contract.md) for
the protocol and persistence boundaries, and [authentication](docs/auth.md) for
the SPAKE2/channel-binding model.

## Debug build marker

Before distributing a new device build, keep these labels identical so testers
can identify the installed build:

- Apple: `apps/envoix-apple/Sources/Support.swift`
- Android: `android/app/src/main/java/dev/envoix/app/DebugBuild.kt`

UDP GSO remains disabled on Android for emulator and device compatibility.
