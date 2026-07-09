## Title

Trusted Device Store v1: My Devices and Friends

## Problem

Envoix currently treats most transfers as temporary pairings. Room, link, and token flows can complete a transfer, but they do not create a long-term device relationship.

This causes several product limitations:

- A user's own Mac, iPhone, Windows, or Linux devices are not recognized as "my devices".
- A friend's device and the user's own device have no trust-policy difference.
- There is no local record for trusted device name, platform, last seen time, or public identity.
- Future features such as one-click send, auto-accept, and Activity grouping by device do not have a reliable foundation.
- Native apps currently do not consistently use persistent device identity as a product-level default.

## Goal

Create a cross-platform foundation for trusted devices.

At the end of this issue:

```text
this device has a persistent identity
trusted peers are stored locally
trusted peers can be classified as my_device or friend
auto-accept policy is explicit and conservative
```

This issue is about identity, trust records, and policy. It is not about building a chat UI yet.

## Required Changes

### 1. Persist Local Device Identity

Each device should have a long-term identity.

The Rust core already has persistent identity support through `IdentityConfig::Persistent(path)`, but native apps should use persistent identity as a product behavior instead of creating a fresh identity every run.

The identity should be stable across app restarts and should be stored in a platform-appropriate location.

### 2. Add a Trusted Device Store

Add a local trusted peer store.

Suggested record:

```text
TrustedDevice {
  device_id
  display_name
  platform
  public_identity
  trust_kind: my_device | friend
  created_at
  last_seen_at
  auto_accept_policy
}
```

This store should be local-first and should not require an account system.

### 3. Distinguish My Devices and Friends

Trusted devices should not all have the same trust policy.

Suggested categories:

- `my_device`: a device owned by the same user.
- `friend`: a trusted device owned by another person.

`friend` devices should not auto-accept by default. They should ask before receiving transfers unless the user explicitly changes policy later.

### 4. Define Auto-Accept Precisely

`auto-accept` must not mean "allow the peer to do anything".

It should only mean:

```text
For a verified My Device, accept transfer offers automatically when local safety limits and platform availability allow it.
```

It must not mean:

- accept unknown devices;
- overwrite existing files by default;
- let the peer write arbitrary paths;
- let the peer read local files;
- stay online forever in the background;
- execute received files.

Suggested v1 policy:

```text
AutoAcceptPolicy:
  ask_each_time
  auto_accept_my_devices
```

Required safety rules:

- receiver controls the save location;
- never overwrite by default;
- reject path traversal;
- reject if disk space is insufficient;
- reject identity mismatch;
- reject over a configured size limit if one is set.

### 5. Document Platform Availability Limits

Auto-accept is policy-gated and platform-gated.

Desktop platforms can later support a more persistent receive experience through an app, tray app, menu bar helper, or background agent.

iOS and iPadOS must be foreground-first:

- foreground app: can auto-accept when policy permits;
- short background windows: may work but must not be promised;
- long lock-screen background, killed app, or suspended app: must not be promised;
- charging state or screen-on state does not guarantee background socket availability.

The product must not claim always-on background receiving on iOS/iPadOS.

### 6. Expose Basic Native UI and FFI Support

v1 does not need a conversation-style UI, but native clients need basic trusted-device operations:

- list trusted devices;
- add or confirm a trusted device;
- rename a device;
- remove trust;
- mark as `my_device` or `friend`;
- show auto-accept policy;
- show last seen when available.

The initial UI can live in a Devices screen or Settings subpage. A full navigation redesign is out of scope.

## Out of Scope

- Conversation-style device windows
- One-click send
- Nearby trusted-device discovery browser
- Remote rendezvous presence
- Push notifications
- Full Activity grouping by device
- Multi-file transfer
- Always-on background receiver on iOS
- Account system or cloud sync for the trust store

## Acceptance Criteria

- Native apps can use a persistent local device identity.
- Trusted peer records can be created, listed, renamed, and removed.
- A trusted peer can be classified as `my_device` or `friend`.
- Auto-accept policy is stored explicitly.
- `friend` devices do not auto-accept by default.
- Identity mismatch is rejected or surfaced as a trust error.
- Receiver-side save location and overwrite policy remain locally controlled.
- Documentation clearly states that auto-accept is not an always-online guarantee, especially on iOS/iPadOS.

## Follow-up Issues

- Nearby Trusted Device Discovery
- One-click Send to Trusted Device
- Auto-accept for My Devices under platform safety limits
- Conversation-style Device Window
- Sync Trust Store Across My Devices
