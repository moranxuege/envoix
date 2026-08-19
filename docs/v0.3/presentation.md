# v0.3 native presentation contract

Status: normative for M6 presentation work.

This document defines the product language, state ownership, semantic design
tokens, and interaction states shared by Envoix's native applications. It does
not require pixel-identical SwiftUI and Compose implementations.

## 1. Product language

Primary navigation and user-facing flows use these terms consistently:

| Term | Meaning | Do not present as |
| --- | --- | --- |
| Nearby | Discover devices that can open a Room | a transfer protocol |
| Devices | Remembered, verified relationships | saved room codes |
| Room | Authenticated context for offering Transfers | send or receive mode |
| Transfer | Durable movement of selected Content | an invitation session |
| Activity | Current and retained Transfer history | protocol logs |
| Settings | Device-local preferences and diagnostics | Engine product state |

`Invite` and `pairing code` describe entry mechanisms only. A primary action
may say `Send` or `Receive`, but those actions create or accept a Transfer
inside the current Room; they are not separate application modes.

## 2. State and effect ownership

Presentation follows one direction:

```text
View -> intent -> feature presenter/store -> Engine or platform adapter
Engine/platform event -> feature presenter/store -> immutable UI state -> View
```

| Lifetime | Owned state | Examples |
| --- | --- | --- |
| Process | Engine, discovery, Room control, durable outbox, vault adapters | `AppleApplicationRuntime`, Android feature stores/services |
| Scene/window | navigation, selection, sidebar visibility, window-local sheets | `MobileSceneNavigationState` |
| Screen | draft text, expanded rows, focus, a user-started picker | SwiftUI `@State`, Compose saveable state |
| Durable | Relationships, Transfer records, destination authorization | Engine store or an injected platform store |

A View or Composable may render an observed immutable projection and emit an
intent. It does not call FFI, a vault, network discovery, transfer services, or
persistence directly. OS pickers remain presentation effects, but their result
is handed to a platform adapter or presenter before durable state changes.

Only one active Apple scene owns global verification and invitation prompts.
Opening or closing another window must not create another Engine, discovery
session, Room controller, outbox dispatcher, or Keychain reader.

## 3. Semantic color roles

The following values are the v0.3 reference palette. Native dynamic-color
mechanisms remain authoritative for appearance changes.

| Role | Light | Dark | Use |
| --- | --- | --- | --- |
| `background` | `#F8FAFD` | `#061126` | page background |
| `surface` | `#FFFFFF` | `#0A1830` | cards and controls |
| `surfaceRaised` | `#FDFEFF` | `#10213D` | transient elevated content |
| `textPrimary` | `#0A1330` | `#FFFFFF` | primary text |
| `textMuted` | `#53627A` | `#B8C5D9` | secondary text |
| `separator` | `#E6ECF5` | `#263B5D` | borders and dividers |
| `accent` | `#1677FF` | `#66A9FF` | ordinary selected state |
| `accentStrong` | `#0D47A1` | `#A8CEFF` | primary action/text |
| `accentSoft` | `#EAF2FF` | `#142F55` | accent container |
| `success` | `#147A4B` | `#61D69A` | connected/delivered |
| `successSoft` | `#DDF3E7` | `#16362A` | success container |
| `warning` | `#A05A00` | `#FFC166` | unverified/waiting attention |
| `danger` | `#E74C3C` | `#F07167` | failed/destructive |
| `dangerStrong` | `#B42318` | `#FFB4AA` | emphasized destructive action |
| `dangerSoft` | `#FFF4F2` | `#3A2020` | failure container |

Color never carries state alone. Every warning, failure, or success state also
has text, an icon, or an accessibility value. System contrast settings take
precedence over a reference value.

## 4. Component states

### Actions

All primary and destructive actions expose these states where applicable:

- enabled: label and action are available;
- pressed: native feedback without changing the action meaning;
- busy: progress is visible and duplicate submission is disabled;
- disabled: the reason is visible nearby or available as an accessibility hint;
- destructive: native confirmation when data, trust, or an active Room ends.

### Device card

Device cards use the same meanings on Apple and Android:

| State | Meaning | Available action |
| --- | --- | --- |
| available | relationship can be contacted | open or offer files |
| connecting | reconnect is in progress | wait or cancel where supported |
| waiting | peer must open Envoix | keep queued Content |
| connected | authenticated Room control is active | open Room/send |
| needsRepair | credential or endpoint can no longer connect | pair again |

### Room invitation card

The fixed states are idle, creating, ready-hidden, ready-revealed, connecting,
connected, expired, and failed. Revealing a QR replaces the conflicting entry
actions within the same layout footprint. Secrets and verification codes use
privacy-sensitive rendering and protected screenshots where the OS supports it.

### Transfer row

Transfer status comes from the typed application snapshot. Presentation may
group states into preparing, waiting, transferring, verifying, saving,
delivered, paused, failed, or canceled, but must not infer terminal behavior
from localized text.

## 5. Adaptive input and layout

- Compact Apple windows use stack navigation; regular-width windows use split
  navigation. The decision follows window size class, not device model.
- iPad supports all orientations, resizing, multiple windows, keyboard focus,
  pointer feedback, context menus, file drop, and explicit destination repair.
- Android layouts respond to available width and system insets rather than a
  particular handset model.
- A drag/drop, share-extension, clipboard, file-picker, and keyboard action
  reaches the same Transfer draft intent as its visible button equivalent.
- Modal state belongs to the initiating scene/screen. Process-global prompts
  have one explicit presentation owner.

## 6. Accessibility and motion

Primary flows must remain operable with large accessibility text, screen
readers, keyboard/switch input, and increased contrast. Interactive targets
use each platform's recommended minimum size. Focus follows visual task order;
progress announcements are rate-limited; reduced-motion settings replace
decorative movement without hiding state changes.

Identifiers used by automation are stable implementation contracts, not
localized labels. Sensitive values are never placed in accessibility labels,
logs, screenshots, or test attachments.

## 7. Localization

Migrated SwiftUI screens use `Localizable.xcstrings`; migrated Compose screens
use Android string resources. English literals are stable localization keys or
catalog values, not product-policy inputs. Inline English/Chinese selection may
remain only on a screen not yet migrated, and M6 cannot close while it remains
on a primary flow.

Formatting, pluralization, and accessibility phrases belong to the native
catalog. Error codes and Engine state remain locale-independent.

Android migration is incremental and M6 remains open until the primary flows
are complete. The NFC invitation overlay, QR scanner, Room container, and the
existing shared transfer-setup labels use `values` /
`values-b+zh+Hans` resources; shared Room components, Connection Hub,
Activity, and Settings still contain inline bilingual text to be migrated.

## 8. M6 verification

A migrated feature supplies evidence for:

1. pure projection/state tests, including unavailable and invalid inputs;
2. hosted or JVM interaction tests for component states;
3. iPhone/iPad or Android UI coverage at representative widths;
4. accessibility identifiers and large-text behavior for its primary actions;
5. absence of direct FFI, vault, persistence, or network calls in its View or
   Composable;
6. native-catalog coverage for every user-facing string on the migrated screen.
