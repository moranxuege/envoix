# Unified discovery and connection workflow

Status: **Focused Issue #59 product contract; persistent Exchange remains deferred**

Last reviewed: 2026-07-25

This document defines the graduation-safe part of Issue #59. It does not
replace the issue's security criteria and does not claim the persistent
Exchange, trust, or candidate-policy work owned by later issues.

## Product workflow

iOS and Android use the same four product surfaces even when their native
controls look different:

```text
Connect
├─> One-time Room -> one or more independent Manifest transfers
├─> Activity
└─> Settings
```

The Connect surface contains only nearby discovery, Scan QR, Show QR, Enter
code, Activity, and Settings. Selecting a method creates an ephemeral local room
context. It does not authenticate a device or establish a durable connection.

The One-time Room contains an unverified peer/context header, one file composer,
incoming-offer confirmation, and a timeline of transfers started while that
room is open. Either endpoint may offer a file, but every offer still uses an
independent Invite v1 transfer. The UI therefore says **Unverified** and **Each
transfer connects separately**. It must not say Connected until a live transfer
reports an actual connection event.

Activity is a separate full surface containing canonical transfer records. It is
not embedded in Connect or the room. Closing a room discards only its unstarted
draft; active transfers continue in Activity.

Discovery metadata never becomes peer identity. A selected card contributes
only temporary display context and a provider-specific invitation opportunity.
Room codes, display names, network addresses, and foreground presence keys must
not be used as room or peer identity.

## Implemented foundation: native handoff continuity

The Manifest v2 foundation already guarantees:

- keep discovery and transfer setup inside one stable presentation;
- carry the selected nearby context into Send or Receive;
- prepare Photos, Files, Folder, or receive destination before BLE delivery;
- deliver an outbound BLE invitation only from the final Start action;
- allow one delivery in flight;
- ignore duplicate or late completion callbacks; and
- create the canonical transfer Activity only after successful invitation
  delivery and local session start.

The internal Send/Receive direction remains an Invite v1 compatibility adapter.
It is not presented as the top-level product workflow.

## Current slice: workflow ownership

Each native client owns one explicit workflow state:

```text
Connect
OneTimeRoom(ephemeralRoomId, untrustedContext, draftId)
Activity
Settings
```

Workflow-scoped state, rather than individual views, owns navigation, the
selected observation, inbound offers, pending external shares, and the active
transfer draft. Android gives an in-memory draft one stable identifier and
permits it to start at most once. iOS deliberately does not restore unstarted
drafts across process death in this slice. Any future job restoration must be
keyed to an explicit workflow draft; a global "latest preparing job" must never
leak sources between rooms.

Incoming unauthenticated BLE offers are bounded, expire, are deduplicated, and
always require Accept or Reject. They never navigate on their own.

Discovery is leased by the visible workflow:

- Connect browses BLE and mDNS.
- A nearby room keeps the current provider lease so an invitation can complete,
  but the workflow exposes only the selected peer and rejects offers from other
  presence keys. The platform providers still scan broadly underneath this UI
  filter; a true provider-level selected-peer lease is deferred.
- QR/manual rooms, Activity, and Settings do not run foreground discovery.
- Returning from a system picker must reacquire the selected observation before
  BLE delivery is enabled.

## Current slice: privacy-safe path presentation

The existing Room, mDNS, and iroh Direct/Relay behavior remains unchanged.
Iroh stays responsible for path selection and upgrade. Existing Connected and
PathChanged events are projected through the FFI as a structured `Direct`,
`Relay`, or `Other` path event. Product UI never formats raw IP addresses or
relay URLs.

## Acceptance target

Automated state tests and current physical-device evidence must cover:

- iOS to Android and Android to iOS on the same LAN;
- QR and manual-code entry in both directions;
- each physically supported BLE invitation direction;
- incoming-offer accept, reject, deduplication, and expiry;
- Photos, Files, Folder, Android external-share, and destination continuity;
- peer loss during a picker and later reacquisition;
- duplicate Start suppression and draft isolation;
- Direct/Relay presentation without an address or credential; and
- discovery stopped on Activity and Settings.

Legacy discovery and transfer runs remain useful downstream evidence, but they
do not by themselves validate this new room UI. The branch verification ledger
must distinguish the directions and interactions rerun on these binaries from
requirements that remain pending.

## Verification ledger: 2026-07-25

Passed on the current branch:

- Rust workspace tests and warning-free Clippy;
- Android JVM tests, ktlint, lint, and debug application/instrumentation builds;
- the bilingual room workflow on Android model `25060RK16C`, including
  Hub/Room Activity and Settings round trips, rotation continuity, pending
  invitation continuity, and both transfer setup sheets;
- a signed iPhone 15 Pro Max build on iOS 26.5.2, the hosted suite, and five
  physical UI smoke tests covering both languages, Activity/Settings
  separation, explicit incoming-offer acceptance, and nearby source pickers;
- the macOS hosted suite; and
- Android-to-iOS and iOS-to-Android single-file Manifest transfers, plus local
  unreadable-Share recovery on each mobile platform.

Still pending physical evidence on this UI generation:

- closing a room while a genuinely live transfer remains pending;
- complete QR/manual-code and BLE handoff interactions in both directions; and
- Relay fallback or a safe live path upgrade.

## Ownership and stop lines

- #55 owns Manifest payload semantics.
- #56 owns recovery authorization, budgets, and cache lifecycle.
- #57 owns secure invitation, authenticated identity, and explicit roles.
- #58 owns durable trust and persistent Exchange.
- #59 ultimately owns provider-independent candidate policy, fallback, and
  upgrade. This focused branch references but does not close it.
- #60 may add Wi-Fi Aware only as another provider/candidate after its own
  physical capability and interoperability gates pass.

The current slice does not add an authenticated peer model, Invite v2 Exchange,
automatic acceptance, a candidate planner, persistent room history, or
room-wide partial-file deletion. The attached Android hardware does not expose
the required Wi-Fi Aware pairing capability, so #60 remains hardware-gated.
Compile success, simulator state, an entitlement, or a discovered name must not
be reported as cross-platform Wi-Fi Aware support.
