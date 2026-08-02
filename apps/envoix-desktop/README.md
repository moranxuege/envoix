# envoix-desktop

A demo desktop front end for Windows and Linux. It is deliberately temporary:
one transfer route, happy path only, no settings, no persistence.

## Why egui

It links `envoix-client` directly, so there is no FFI bridge to maintain, and it
produces one self-contained binary per platform with no runtime dependency on a
system webview. Tauri would need `webkit2gtk` installed system-wide on Linux,
and Flutter cannot cross-compile a Windows desktop binary from Linux at all.

## Looking like the app

`src/theme.rs` ports the colour tokens from
`android/app/src/main/java/dev/envoix/app/ui/Theme.kt` verbatim, both palettes.
Corner radii and type sizes follow the dominant Compose values in
`android/app/src/main/java/dev/envoix/app/ui/`: 16dp cards, 12dp controls,
11-13sp body text, monospace for technical values.

Roboto Regular and Bold are vendored under `assets/` (Apache-2.0, the family
the Android app renders with) so Windows and Linux show the same weights rather
than falling back to whatever each platform ships.

The mobile app stacks bottom nav, transfer list, and the "New transfer" sheet
vertically. On a wide window those become three side-by-side panes: navigation
rail, transfer activity, composer.

## The one route

A directional invitation through the deployed rendezvous, using the same
`BROKER` and `RELAY` constants as `Endpoints` in
`android/app/src/main/java/dev/envoix/app/TransferRepository.kt`.

    Receiver                          Sender
    ---------------------------       ---------------------------
    Receive tab -> "Receive"          Send tab -> "Choose files"
    QR + room code appear             paste the invite
    "Copy invite" -> send to peer     -> "Send"
    offer arrives
    "Accept and save"

The receiver parks on the authenticated offer until it is accepted, which is
the product's inspect-before-payload behaviour rather than an auto-accept.

Joining needs the full `envoix://invite/v2/...` payload, not the room code
alone; the naked room-code route was retired. The room code is shown because it
identifies the room to a human, and the QR encodes the same payload the button
copies.

Files dropped anywhere on the window queue for sending and switch the composer
to Send, which is the gesture a desktop user reaches for before hunting for a
file picker.

## Running it

    cargo run -p envoix-desktop

Two instances on one machine work: point them at different save directories.

## Building both binaries

    # Linux
    cargo build -p envoix-desktop --release
    # -> target/release/envoix-desktop

    # Windows, cross-compiled from Linux
    rustup target add x86_64-pc-windows-gnu
    cargo install cargo-zigbuild        # needs zig on PATH or the ziglang module
    cargo zigbuild -p envoix-desktop --release --target x86_64-pc-windows-gnu
    # -> target/x86_64-pc-windows-gnu/release/envoix-desktop.exe

Both are self-contained. The `.exe` needs no runtime, no webview, and no
Visual C++ redistributable.

## Demo runbook

Two machines, one transfer, roughly ninety seconds.

1. **Receiver**: launch, leave the composer on **Receive**, set **Save to** if
   the default is wrong, press **Receive**. A QR and a room code appear.
2. **Sender**: launch, press **Send**, then drag the files onto the window (or
   **Choose files**). The card reports the item count and total size.
3. **Receiver**: press **Copy invite** - the button confirms with "Copied" -
   and get that string to the sender by any means. The QR carries the same
   payload for a phone.
4. **Sender**: paste into **Invite from the receiver**, press **Send**.
5. **Receiver**: the card reaches *Needs approval* with the inventory. Press
   **Accept and save**; the QR withdraws, because the invitation is spent.
6. The bar turns green on delivery and **Open folder** reveals the files.

If the venue's NAT defeats hole punching, start both sides with
`ENVOIX_DESKTOP_RELAY_ONLY=1`. The transfer then rides the relay rather than a
direct path, which is slower but far harder to break.

The **Logs** tab in the left rail carries the whole lifecycle, which is the
first place to look if a step stalls.

Run it twice before presenting. A second transfer through the same window is
covered by `two_transfers_in_a_row`, and cancelling mid-flight then retrying is
covered by `cancelling_mid_transfer_leaves_the_engines_usable`, but neither
substitutes for rehearsing on the actual machines and network.

## Verified

- `a_file_crosses_the_deployed_rendezvous` (ignored by default, needs the
  deployed broker alive) drives the same `receive`/`send` functions the UI
  drives, over the real rendezvous, and compares the received bytes to the
  source:

      cargo test -p envoix-desktop -- --ignored --nocapture

- `waiting_for_a_sender_light` and `transferring_dark` render the shell
  offscreen through wgpu into `target/ui-preview/`, so the layout can be
  reviewed and regressed without a display server.

## Windows verification status

Verified on real hardware: Windows 10 22H2 (build 19045), x86-64, reached over
Tailscale.

| | Result |
|---|---|
| PE loads, Rust runtime starts | works |
| `local_allocatable_bytes` (`GetDiskFreeSpaceExW`) | works |
| Invitation, SPAKE2 pairing, rendezvous | works |
| Sending to a native Linux peer | works |
| **Receiving from a native Linux peer, hash compared** | **works** |

The receive leg was proven with `envoix-cli` cross-built for
`x86_64-pc-windows-gnu`, because it drives the same `envoix-transfer`
destination and save path this app does, at half the transfer size:

    # on Windows
    envoix.exe receive --create-invite --rendezvous <broker> --relay <relay> \
        --output C:\recv
    # on Linux, with the invitation it printed
    envoix send --invite '<payload>' ./payload.bin

256 KiB arrived byte-identical, the receiver returned its delivery proof, and
the data path negotiated direct rather than falling back to the relay.

### Wine is not a substitute, and says so loudly

Wine 10.0 runs the executable and gets through pairing, but two failures there
are Wine's own and do not reproduce on Windows:

- Receiving fails with `receiver_save_failed: OS error 4390`
  (`ERROR_NOT_A_REPARSE_POINT`). `exclusive_rename` in
  `crates/envoix-transfer/src/destination_v2.rs` calls `MoveFileExW` without
  `MOVEFILE_REPLACE_EXISTING` and maps a collision to `AlreadyExists` from
  error 80 or 183; Wine returns 4390 instead, escaping the mapping. Real
  Windows returns `ERROR_ALREADY_EXISTS`, and the save path works there.
- Two peers both under Wine fail earlier: Wine leaves `IP_ECN`
  (`setsockopt` optname 50), `SIO_UDP_CONNRESET`, and a UDP vendor ioctl
  unimplemented, all of which QUIC uses. Rendezvous traffic survives it; a
  direct peer-to-peer session does not.

`interop_receive` and `interop_send` remain useful for pairing one native peer
against one emulated or remote peer through a published invitation file:

    ENVOIX_INTEROP_INVITE=/tmp/i.txt ENVOIX_INTEROP_SAVE=/tmp/recv \
      cargo test -p envoix-desktop interop_receive -- --ignored --nocapture

## Forcing the relay

Set `ENVOIX_DESKTOP_RELAY_ONLY=1` to force the relay path when a venue's NAT
defeats hole punching, or to separate a transport fault from an application one.

## Not covered

mDNS/LAN discovery, manual peer descriptors, resume, cancel mid-payload beyond
dropping the session, multi-transfer history, settings, and QR *scanning*
(display only). The transfer list holds one transfer at a time.
