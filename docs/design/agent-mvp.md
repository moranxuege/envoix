# Envoix Agent MVP: macOS to WSL Inbox

Status: first vertical slice implemented in August 2026.

## Goal

The first product workflow is deliberately narrow:

1. Run a persistent Envoix Agent inside WSL.
2. Pair the macOS app once and remember both devices.
3. Send selected files, folders, or pasted clipboard images from the Mac with
   the existing Manifest v2 engine.
4. Save completed roots in a WSL Inbox.
5. Retrieve the newest path with `envoix inbox latest`, including from an SSH
   shell that cannot accept an image paste.

The Agent is not a second transfer protocol. It owns durable product state and
drives the same `envoix-client` / `envoix-session` Manifest v2 path already used
by the Apple and Android apps.

## Boundaries and invariants

- Tailscale is an optional network path, never a device identity or trust
  database. Envoix remembered credentials authenticate the peer.
- A configured relay does not force relay traffic. Iroh may select a direct LAN
  or otherwise reachable path and falls back to the relay when required.
- The local CLI is a controller. It exchanges bounded JSON-line commands with
  the Agent over an owner-only Unix socket; it does not open a second receiver.
- Remembered credentials are stored as owner-only opaque files. Product JSON
  contains only credential references, labels, generations, and routing data.
- Every Inbox entry is written only after Manifest v2 verification, destination
  save, and delivery confirmation complete.
- The existing `envoix send` and `envoix receive` commands remain compatible.

## Shape

```text
macOS Envoix app
  device card, drag/drop, Finder Service, or Paste File or Image
           |
           | ordinary Room + explicit six-digit verification once
           | remembered Room Control after that
           | direct LAN/Tailnet when reachable; relay fallback otherwise
           v
WSL envoix-agent <--- Room offer + canonical Manifest v2 ---> WSL Inbox
       ^                                         durable files
       |
       +---- owner-only Unix socket ---- envoix CLI
                                          agent status
                                          devices list
                                          inbox list/latest
```

## Local Agent contract

`envoix-client::product` is the shared product contract. Protocol version 8
defines these commands:

- `status`
- `snapshot { inbox_limit }`
- `events { after, limit }`
- `diagnostics`
- `pair { label }`
- `list_devices`
- `revoke_device { device }`
- `create_transfer { device, paths }`
- `list_transfers`
- `list_transfer_paths`
- `get_transfer { transfer_id }`
- `list_pending_offers`
- `decide_pending_offer { offer_id, decision }`
- `list_inbox { limit }`
- `latest_inbox`

The wire format is tagged JSON with one request and response per connection.
The socket is mode `0600`; its state directory is mode `0700`. Requests are
limited to 64 KiB.

The persisted `engine-state-v2.json` records the shared Engine snapshot,
durable Relationship metadata, and completed Inbox items. Opaque credentials
live under the separate `vault/` directory and are never serialized into Agent
responses. A managed process
loads its device name and Inbox location from the versioned, owner-only
`~/.config/envoix/agent.json`; command-line arguments still take precedence for
development runs.

## Pairing and receive lifecycle

`envoix agent pair --name MacBook` creates an ordinary Room Control invitation
and a separate random six-digit verification code. The CLI prints only the
short Room code and verification code. On the Mac, the user enters the Room
code through the existing room UI. Once connected, the Agent sends an optional
Room Control verification request and the Mac prompts for the six-digit code.

The code has one attempt and is carried only inside the already encrypted Room
Control channel. A match authorizes both sides to persist the credential bound
to that room's authenticated control handshake as generation 0. A mismatch,
cancel, unsupported older peer, or storage failure leaves the ordinary room
untrusted and stores no new Agent credential. No transfer is required to finish
pairing, and there is no separate pairing rendezvous or user-facing InviteV2
flow.

The initial ordinary room remains usable for offers. After it closes, the Agent
waits as a remembered Room Control responder. The Mac reconnects through its
existing remembered-room scheduler. Successful remembered authentication
advances the current generation while retaining one previous generation for
crash recovery, matching the native-app schedule.

Each file send still creates a fresh directional InviteV2, but only as the
internal transfer ticket embedded in an authenticated Room Control offer. The
Agent starts the canonical Manifest v2 receiver before accepting that offer,
then checks the authenticated item counts, byte count, and root-name preview
against the control offer before saving anything.

The Agent automatically accepts ordinary transfers. Offers above the existing
automatic-receive threshold, or above half of currently allocatable Inbox
space, remain at the authenticated Room offer boundary until the local owner
runs `envoix transfers approve <offer-id>` or rejects them. `envoix transfers
pending` shows only bounded device, root-preview, item-count, byte-count, and
capacity metadata; the directional InviteV2 payload never crosses the local
control protocol. A Room close, device revocation, or Agent restart discards
the in-memory pending offer without creating or modifying Inbox files.

## macOS clipboard intake

The macOS send screen checks clipboard sources in a fixed order: a Finder file
URL, an existing plain-text path, then image data. Files and paths remain their
original Manifest sources. Image data is normalized to PNG and first written
to Envoix's owner-only Application Support draft directory. The resulting
draft uses the same claim, activity binding, cache reconciliation, resume, and
remembered-room outbox lifecycle as an iOS Share draft, so an asynchronous WSL
send does not depend on the clipboard contents remaining unchanged.

The remembered-device list is the primary macOS send surface. Choosing
**Send** opens the ordinary selection screen; dropping files or folders onto a
device opens it with those roots already selected. The main app also provides
the macOS Service **Send with Envoix** for Finder selections. The service keeps
security-scoped access to every selected URL, brings the existing main window
forward (or reopens it), and exposes the pending item count beside the device
list. All three entry points stop at the same explicit **Send** seal boundary.

## Network behavior on the current WSL host

The current machine uses ordinary WSL NAT. Windows owns the Tailscale adapter;
WSL can make outbound Tailnet connections, but a Mac cannot initiate Envoix
UDP/QUIC directly to WSL through the Windows Tailscale address. This does not
block the MVP because the configured relay provides fallback and same-LAN
direct paths can still form where reachability permits.

WSL mirrored networking is an optional later optimization. Enabling it requires
changing Windows `.wslconfig` and restarting WSL, so it is intentionally not an
Agent installation side effect.

The Agent projects the transfer layer's selected-path events into bounded,
in-memory telemetry. `lan` means a private or link-local direct address except
the Tailscale CGNAT and IPv6 ranges; those and other direct addresses are shown
as `tailnet/direct`. Relay, Wi-Fi Aware, and unknown custom transports remain
distinct. This classification is display-only and is never an authentication,
authorization, or candidate-filtering input. Path state disappears when the
transfer ends or the Agent restarts.

## Running the slice

Build through the repository cache guard:

```bash
scripts/with-build-cache-guard.sh cargo build -p envoix-agent -p envoix-cli
```

Run the unhosted macOS clipboard and credential-store tests without launching
Envoix or accessing real remembered-device credentials:

```bash
scripts/apple-dev.sh macos-clipboard-test
```

Install the Agent in WSL after building both binaries:

```bash
target/debug/envoix agent install --inbox "$PWD/inbox" --device-name WSL
```

The command copies `envoix` and `envoix-agent` to `~/.local/bin`, writes a
systemd user unit, enables it for future WSL sessions, and starts it now. It
does not edit `/etc/wsl.conf` or enable systemd on the user's behalf. When the
user service manager is unavailable, the installed files remain usable and
the error prints the equivalent foreground command.

Manage and use the Agent from any WSL shell:

```bash
~/.local/bin/envoix agent start
~/.local/bin/envoix agent restart
~/.local/bin/envoix agent status
~/.local/bin/envoix agent pair --name MacBook
~/.local/bin/envoix devices list
~/.local/bin/envoix devices forget MacBook --yes
~/.local/bin/envoix transfers paths
~/.local/bin/envoix transfers pending
~/.local/bin/envoix transfers approve <offer-id>
~/.local/bin/envoix transfers reject <offer-id>
~/.local/bin/envoix inbox list
~/.local/bin/envoix inbox latest
~/.local/bin/envoix agent stop
```

A newly built CLI updates both installed binaries in place and restarts the
service without rewriting settings or Agent data:

```bash
target/debug/envoix agent update --agent-binary target/debug/envoix-agent
```

`envoix agent uninstall` disables the service and removes the installed
binaries while preserving settings, Engine state, credentials, and Inbox.
`envoix agent uninstall --delete-state --yes` additionally removes only the
explicitly allowlisted Agent state and credentials. Received Inbox files and
unknown files under the state root are preserved in both modes.

Forgetting accepts a device ID or its exact label and requires `--yes`. It
removes only the remembered relationship and protected credential, cancels its
reconnect listener, and leaves completed Inbox files and history intact. A
transfer already accepted before revocation may finish; no later Room offers
or reconnects are accepted from that relationship.

For a foreground development run, use `envoix-agent` directly. Defaults can be
overridden with `--settings`, `--state-dir`, `--inbox`, `--socket`, `--broker`,
`--relay`, and `--config`. `ENVOIX_STATE_DIR` and `ENVOIX_AGENT_SOCKET` are also
honored. Pass `--relay none` only when both peers have a confirmed direct
route.

## Next slices

1. Add an optional store-and-forward relay only if offline delivery becomes a
   real requirement; it is not part of this MVP.
