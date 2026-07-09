## Title

Cross-Platform Nearby Discovery v1

## Problem

Envoix already has a same-LAN mDNS transfer flow:

- receiver advertises an iroh endpoint through mDNS;
- sender discovers candidates on the local network;
- both sides authenticate with a shared token;
- Apple UI exposes this as the token mode.

That is useful, but it is not the same as a product-level nearby device list.

Trusted-device flow needs a presence layer where the app can show known nearby devices before a transfer starts. This layer must be cross-platform and reusable by Android, Windows, Linux, Harmony, and Apple clients. It should not be implemented as Apple-only UI state.

## Existing Implementation Found

Already present:

- `crates/envoix-session` uses `iroh-mdns-address-lookup`.
- `PeerSource::Mdns` exists in `envoix-client`.
- CLI supports `--enable-mdns` / auto LAN flow.
- FFI exposes `receive_mdns` and `send_mdns`.
- Apple `PairingMode.token` uses mDNS auto-discovery.
- `docs/mdns-testing.md` documents same-LAN mDNS testing and limitations.
- `docs/design/client-api.md` states serverless modes must keep working without broker/relay/DNS/internet.

Missing:

- discovery provider abstraction;
- presence records independent of one transfer;
- trusted-device metadata in discovery advertisements;
- list/browse API for native clients;
- capability negotiation for discovered devices;
- platform-specific permission and backend matrix;
- Android/Harmony/Windows/Linux discovery adapters.

## Goal

Create a v1 nearby discovery layer that can advertise and browse online Envoix devices on the same local network.

This should be a low-cost presence mechanism, not a file transfer channel.

## Non-Goal

Nearby discovery v1 does not provide cross-internet presence.

mDNS/Bonjour works on the local multicast domain. It does not make two devices in different cities discover each other. Remote trusted-device reachability must use a separate rendezvous or relay-aware presence design.

## Required Changes

### 1. Define a discovery provider interface

Add a shared abstraction such as:

```text
DiscoveryProvider {
  advertise(local_presence)
  browse(filter)
  stop()
}
```

The interface should allow multiple backends later:

- mDNS / Bonjour / DNS-SD;
- Android NSD;
- Wi-Fi Aware, if a later Android implementation chooses it;
- Windows/Linux zeroconf implementations;
- future rendezvous-backed remote presence.

### 2. Define `PresenceRecord`

Presence must be explicit and bounded:

```text
PresenceRecord {
  device_id,
  display_name,
  platform,
  app_version,
  protocol_versions,
  capabilities,
  trust_hint,
  endpoint_candidates,
  expires_at,
}
```

Do not advertise secrets, pairing tokens, full trust material, or unnecessary local paths.

### 3. Separate discovery from transfer direction

Nearby presence should not imply sender/receiver direction.

The user should be able to:

- select a nearby trusted device and send to it;
- receive an offer from a nearby trusted device;
- fall back to manual invite, room code, or token flow.

Actual transfer direction must still be negotiated by the transfer protocol.

### 4. Preserve serverless invariants

Same-LAN discovery must not depend on the rendezvous broker, relay, DNS, or public internet.

If a relay/broker is configured globally, it must not make LAN discovery slower or change the exposure scope unless the user opts in.

### 5. Add platform permission notes

Document backend requirements:

- Apple: local network permission and Bonjour/mDNS behavior;
- Android: NSD/mDNS support varies by OS/network; Wi-Fi Aware is a separate future backend;
- Windows/Linux: firewall and zeroconf service behavior may differ;
- Harmony/OpenHarmony: backend to be researched before implementation.

## System Boundary

The discovery API and presence record belong in the shared Rust/client/FFI design.

Platform clients own:

- permission prompts;
- showing nearby devices;
- platform-specific service registration where Rust cannot directly provide it;
- mapping OS discovery callbacks into the shared model.

## Dependencies

GitHub issue: #41

Hard dependencies:

- None for raw same-LAN presence browsing.

Full-scope dependencies:

- Trusted Device Store v1, to classify discovered peers as trusted devices, my devices, or unknown devices.
- Sender-Initiated Transfer Flows, so selecting a nearby device can start a send without forcing the receiver-first flow.
- Structured Transfer Events over FFI, so native clients can observe discovery and connection transitions without parsing status text.

## Out of Scope

- Cross-internet trusted-device presence
- Auto-accept policy
- Background receive on iOS
- Wi-Fi Aware implementation
- Bluetooth implementation
- Transfer queue execution
- Replacing existing token mDNS flow

## Acceptance Criteria

- A platform-neutral `PresenceRecord` is specified.
- A discovery provider abstraction is specified or implemented in the shared layer.
- Existing mDNS token transfer still works.
- Nearby discovery does not require broker/relay/DNS/internet.
- Native clients can list nearby Envoix devices without starting a transfer.
- Advertisements contain no pairing tokens or sensitive secrets.
- Platform limitations are documented for Apple, Android, Windows, Linux, and Harmony/OpenHarmony.

## Follow-up Issues

- Implement mDNS-backed presence provider.
- Add Apple nearby device list UI.
- Add Android NSD discovery adapter.
- Add remote trusted-device presence design.
- Add capability-based device filtering.
