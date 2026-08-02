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

## Running it

    cargo run -p envoix-desktop

Two instances on one machine work: point them at different save directories.

## Verified

- `a_file_crosses_the_deployed_rendezvous` (ignored by default, needs the
  deployed broker alive) drives the same `receive`/`send` functions the UI
  drives, over the real rendezvous, and compares the received bytes to the
  source:

      cargo test -p envoix-desktop -- --ignored --nocapture

- `waiting_for_a_sender_light` and `transferring_dark` render the shell
  offscreen through wgpu into `target/ui-preview/`, so the layout can be
  reviewed and regressed without a display server.

## Not covered

mDNS/LAN discovery, manual peer descriptors, resume, cancel mid-payload beyond
dropping the session, multi-transfer history, settings, and QR *scanning*
(display only). The transfer list holds one transfer at a time.
