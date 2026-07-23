# Unified discovery and connection workflow

Status: **Issue #59 implementation map; native handoff continuity automated gates complete, physical BLE pending**

Last reviewed: 2026-07-24

This document turns Issue #59 into independently reviewable slices. It does not
replace the issue's security and acceptance criteria.

## Product workflow

Apple and Android use the same product sequence even when their native controls
look different:

```text
untrusted observation
-> user selects a nearby card
-> explicit Send / Receive / Exchange intent
-> one canonical transfer draft
-> source or destination authorization
-> final Start
-> invitation delivery or accepted incoming invitation
-> #57 authentication and role binding
-> viable connection candidates
-> selected authenticated path
-> #55 Manifest transfer and #56 recovery
```

Discovery metadata never becomes peer identity. A selected card contributes
only temporary display context and a provider-specific invitation opportunity.
The in-memory setup draft owns the selected source or destination, role,
invitation context, and eventual Activity identity. Durable recovery remains
owned by #56 rather than being implied by discovery state.

## Slice D0: native handoff continuity

This branch converges Apple on the ordering already used by Android:

- keep discovery, role selection, and transfer setup inside one stable
  presentation;
- carry the selected nearby context into Send or Receive;
- prepare Photos, Files, Folder, or receive destination before BLE delivery;
- deliver an outbound BLE invitation only from the final Start action;
- allow one delivery in flight;
- ignore duplicate or late completion callbacks; and
- create the canonical transfer Activity only after successful delivery.

The slice is complete when hosted delivery tests pass and the iOS simulator can
open Photos, Files, and Folder after a nearby sender handoff. Physical BLE
sender gates remain required in both directions because simulator fixtures do
not prove GATT behavior or security-scoped access on a real device.

## Slice D1: shared typed boundaries

Add bounded shared models for:

```text
DiscoveryObservation
AuthenticatedPeer
ConnectionCandidate
```

The Rust/client boundary owns normalization, candidate semantics, privacy-safe
decision events, and deterministic policy inputs. Swift and Kotlin retain
permission, provider lifecycle, native endpoints, and platform presentation.
No address, name, RSSI, presence key, pairing alias, or provider badge may
construct an `AuthenticatedPeer`.

This slice can add model and serialization tests before secure handoff is
available, but it must not fabricate the identity binding owned by #57.

## Slice D2: deterministic candidate policy

Implement pure policy tests before transport integration:

1. validate and reject malformed, expired, unauthenticated, or disallowed
   candidates;
2. rank a bounded top tier from injected measurements and cost policy;
3. produce a bounded staggered attempt plan;
4. select the first authenticated viable path;
5. retain a healthy path with hold time, cooldown, hysteresis, and a material
   improvement threshold; and
6. emit one typed reason for fallback or upgrade.

IPv4 and IPv6 receive no unconditional family preference. BLE RSSI remains a
presentation hint and is not a path-quality score.

## Slice D3: authenticated integration

Integrate candidate attempts only after #57 supplies the authenticated peer and
role binding. Coordinate retry budgets with #56 so discovery, dialing, and
recovery cannot multiply attempts. A reconnect, fallback, or upgrade must keep
the same peer, role, draft, Activity, Manifest, and security transcript.

Direct failure may fall back to Relay. Relay may upgrade only when the policy
reports a material improvement and the active Manifest session can preserve
identity and transfer state.

## Slice D4: physical acceptance

Record exact build, device, OS, network topology, direction, selected path, and
privacy-safe failure reason for:

- Apple to Android and Android to Apple on the same LAN;
- unrelated-network Relay fallback;
- one rejected or failed candidate; and
- one supported safe path upgrade.

The gate also repeats the nearby Photos, Files, Folder, receive-destination, and
duplicate-Start checks on physical devices.

## Ownership and stop lines

- #55 owns Manifest payload semantics.
- #56 owns recovery authorization, budgets, and cache lifecycle.
- #57 owns secure invitation, authenticated identity, and explicit roles.
- #58 owns durable trust and persistent Exchange.
- #59 owns provider-independent candidate selection, fallback, and upgrade.
- #60 may add Wi-Fi Aware only as another provider/candidate after its own
  physical capability and interoperability gates pass.

The currently attached Android endpoint does not expose the required Wi-Fi
Aware pairing capability, and the attached iPhones were offline during this
slice. Therefore #60 remains hardware-gated. Compile success, simulator state,
an entitlement, or a discovered name must not be reported as cross-platform
Wi-Fi Aware support.
