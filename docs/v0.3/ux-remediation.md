# Envoix v0.3 experience remediation

Status: normative release gate for M6 and M7 presentation work.

This document closes the gap between the v0.3 architecture and the product
that people see. Native applications may follow platform conventions, but
they must share one information hierarchy, vocabulary, visual identity, and
Transfer meaning.

## 1. Audience layers

Every piece of presentation belongs to exactly one layer:

| Layer | Audience | Examples |
| --- | --- | --- |
| primary flow | every user | device name, selected content, progress, saved destination, next action |
| recovery | a user whose action failed | actionable cause, retry, repair, choose another destination |
| advanced | a user changing deployment or troubleshooting | broker, relay, connection path, diagnostic export |
| diagnostic | developers and support | Agent protocol, Engine schema, IPC transport, credential provider |

Primary flows must not expose `Agent`, `helper`, `Engine`, IPC, DPAPI,
Keychain, schema numbers, protocol numbers, or credential references. A
recovery message describes the user-visible capability first (for example,
"Background transfers are unavailable") and may link to technical details.

Explanatory copy is justified only when it answers one of these questions:

1. What will happen if I perform this action?
2. Why is the action unavailable?
3. Where did my content go?
4. Is the transfer still running, safely saved, or in need of attention?

Implementation rationale belongs in documentation or the diagnostic layer.

## 2. Shared application topology

The primary topology is the same on Apple, Android, and Windows:

1. **Devices** is the start surface and contains nearby discovery, verified
   devices, and the action to add a device.
2. Selecting a device opens its **Room**, where content is selected or dropped
   and offered to that device.
3. **Activity** shows active and retained Transfers from every Room.
4. **Inbox** shows content durably saved on this device and its destination.
5. **Settings** contains ordinary preferences. Deployment and diagnostics are
   collapsed under Advanced.

One-time and remembered Rooms may differ in lifetime, but the UI must not
describe them as different transfer engines or split Activity into
"helper" and "legacy" sources.

## 3. Visual identity

The semantic palette in `presentation.md` is exact, not illustrative. Windows
must use the same named roles and values already used by SwiftUI and Compose.
Native typography, focus, window chrome, menus, and picker behavior remain
platform-owned.

Primary hierarchy uses no more than one emphasized action per task region.
Cards group one user task rather than one implementation subsystem. Desktop
layouts must use the available width for useful parallel information or bound
their readable content width; they must not stretch a phone card into a wide,
mostly empty panel.

## 4. Transfer presentation projection

The domain `Transfer` remains the authoritative state machine and must not gain
UI-only policy. Each host consumes a versioned, secret-free presentation
projection with at least:

- Transfer, Relationship, and Content identifiers;
- direction and authoritative state;
- bounded root-name preview, item count, directory count, and total bytes;
- transferred bytes, current path, and sampled throughput where available;
- created, last-updated, and terminal timestamps;
- durable destination display name for completed receives;
- typed failure/rejection code and supported actions.

Speed and ETA are ephemeral projections derived from bounded samples; they are
not durable Engine truth. A sender displays **Delivered** only after receiver
save proof. A receiver displays **Saved** only after durable publication.

## 5. Legacy-removal sequence

Cleanup is performed behind characterization tests and in this order:

1. make the typed Agent/Engine projection cover the active user flow;
2. migrate one platform consumer;
3. prove current Room, Transfer, restart, and revoke behavior;
4. remove only the now-unreachable compatibility path;
5. repeat for the next consumer.

The targeted removals are:

- the temporary `envoix-client::api` compatibility surface after Agent, CLI,
  and FFI consumers use explicit application modules;
- macOS legacy remembered-peer/outbox ownership after all durable desktop
  operations use helper control;
- Android `RoomControlPhase.Legacy` after share, deep-link, nearby, and manual
  entry use the current Room workflow;
- Android `Transfer.log`/`addLog` and duplicated `OpLog` events after the
  activity drawer reads the structured timeline;
- view-local product policy while decomposing oversized Apple, Android, and
  Windows presentation files along feature boundaries.

Legacy rejection fixtures remain where they prove a typed failure. They must
not decode old state into current domain objects.

## 6. Release evidence

M6 and M7 presentation work cannot close until evidence includes:

- the same terminology and semantic colors on Apple, Android, and Windows;
- screenshots of Devices, Room, Activity, Inbox, Settings, empty, active,
  delivered, and failed states at representative window sizes;
- screen-reader/accessibility-tree and keyboard-focus inspection;
- filename, progress, destination, and next-action visibility in real
  cross-device transfers;
- source scans proving diagnostic terminology is absent from primary copy;
- a recorded list of compatibility paths removed in each migration slice.

