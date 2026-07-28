# Nearby discovery and experimental BLE rendezvous

Status: **Android and iOS foreground discovery complete; unauthenticated BLE invitation handoff physically verified from Android to iPhone**

Last reviewed: 2026-07-24

This document freezes the nearby-discovery contract and its experimental handoff
into the existing Envoix pairing flow. Discovery metadata and the current BLE
invitation carrier are untrusted. The implementation deliberately makes no
device-identity, confidentiality, anti-relay, or anti-impersonation claim for
that carrier.

## 1. Product boundary

The Android and iOS apps expose nearby devices directly on the **Connect**
surface. That surface:

- scans and advertises over Bluetooth LE while Connect is visible;
- discovers and publishes a DNS-SD service on the local network;
- merges observations from both transports by one foreground-presence key;
- exposes provider availability and permission failures instead of one Boolean;
- reserves the same provider interface for Wi-Fi Aware; and
- stops broad discovery when Connect is hidden or the app leaves the
  foreground, except while a nearby One-time Room needs its handoff lease.
  That room filters the visible observations and inbound offers to its selected
  presence key; the current platform providers still scan broadly underneath
  that workflow filter.

Selecting a BLE peer opens an explicitly experimental pairing context. The
selected context remains attached to one One-time Room while either endpoint
prepares a send source or confirms an incoming offer. Send/Receive remains an
internal Invite v1 adapter, not a top-level navigation choice. Only the final
**Start** action writes an outbound Envoix invitation to the selected peer over
BLE GATT and starts the existing SPAKE2, Direct/Relay, and transfer state
machines. The receiving app requires explicit Accept or Reject before entering
the opposite internal role. No second transfer protocol or persistent
connection is introduced.

The current BLE carrier is useful only for completing the product flow. It is
not a secure replacement for QR, NFC, or a compared short code: the SPAKE2
password is inside the same unauthenticated BLE invitation. A nearby active
attacker can therefore steal, replace, impersonate, or relay the bootstrap.
mDNS-only observations still use the existing QR/manual-code flow because the
discovery DNS-SD record carries no invitation endpoint.

## 2. Discovery identity

Each discovery coordinator generates a random 8-byte value as 16 lowercase
hexadecimal characters. BLE and mDNS share that value only for an ephemeral
foreground workflow-owner lifetime. The value is never persisted. Both clients
retain it while their workflow owner pauses and resumes discovery so an open
nearby room can reacquire the same presence after Activity, Settings, or a
system picker. Recreating the workflow owner replaces it. It exists only to
merge nearby observations and is not a public key, trusted account/device ID,
or proof that two observations came from an authentic Envoix installation.

Stopping or restarting discovery advances a coordinator generation. Provider
callbacks capture the generation in which they were registered, so a late
callback from an old scan cannot repopulate the new session.

Display names and peer keys received over BLE or mDNS must be treated as
attacker-controlled. The registry validates the peer-key shape, normalizes
names, caps all received strings, and never logs peer identifiers or network
addresses.

## 3. Bluetooth LE wire marker

Apple's `CBPeripheralManager.startAdvertising` accepts only the local-name and
service-UUID advertisement keys. The protocol therefore carries the peer key
inside a service UUID rather than service data:

```text
d5f3a2d8-8f4a-4b33-PPPP-PPPPPPPPPPPP
                        └── 16-hex peer key ──┘
```

Android scans with this masked UUID:

```text
base = d5f3a2d8-8f4a-4b33-0000-000000000000
mask = ffffffff-ffff-ffff-0000-000000000000
```

No device name, MAC address, IP address, or authenticated identity is included.
Low-latency browsing is scoped to Connect and an active nearby Room handoff
lease; Activity and Settings never keep it running.

Apple foreground support can advertise the same full UUID and scan without a
service filter before applying the prefix check in-app. Apple places service
UUIDs in a platform-specific overflow area while backgrounded, so cross-platform
background discovery is explicitly outside this phase.

## 4. mDNS wire marker

The discovery-only DNS-SD type is:

```text
_envoix-disc._udp.
```

Its TXT record contains only:

```text
v=1
id=<16-hex foreground presence key>
name=<normalized display name, at most 48 characters>
```

This service is separate from the Rust transfer-session mDNS mechanism. The
Android provider keeps a UDP port and multicast lock alive while registered,
serializes Android NSD resolution requests, and refreshes a resolved observation
until `onServiceLost` arrives. The iOS provider publishes and browses the same
record through Network.framework and refreshes live browser results every five
seconds.

mDNS is link-local. The providers can report `Ready` when their platform browse
and publication objects are running, but peers still need a shared local link
that carries multicast. BLE discovery remains available when the devices are on
different IP networks.

## 5. Experimental BLE GATT invitation carrier

The connectable discovery advertisement exposes no invitation or credential.
Invitation delivery uses a separate fixed primary service and write
characteristic:

```text
service:        d5f3a2d8-8f4a-4b33-8a01-000000000001
write (with response):
                d5f3a2d8-8f4a-4b33-8a01-000000000002
```

Android and iOS use the same version-1 binary contract. All integers are
big-endian.

```text
frame = "EX" | frame_version:u8 | type:u8 | request_id:u64 |
        total_length:u16 | offset:u16 | payload_chunk

payload = security_mode:u8 | security_payload

mode-0 security_payload = envelope_version:u8 | type:u8 |
        sender_presence_key:16 ASCII bytes |
        display_name_length:u16 | invite_length:u16 |
        display_name:utf8 | invite:utf8
```

The current `BleRendezvousSecurity` boundary has only mode `0`, named
`Insecure`/`None`; sealing and opening copy plaintext. A future authenticated
implementation must replace this module boundary without changing discovery or
the transfer state machine. A receiver rejects bad magic/version/type, unknown
security modes, invalid peer keys, malformed UTF-8, out-of-order fragments,
length mismatches, and invitations without the `envoix://pair/` prefix. Limits
are 4,096 bytes per wire payload, 2,048 invitation bytes, and 192 UTF-8 display
name bytes.

Only one outbound offer runs at a time and it times out after 15 seconds. Each
GATT write uses a response and advances the ordered fragment stream, but version
1 has no application-level authenticated delivery acknowledgement. Logs contain
only direction, state, request ID, and `auth=none`; they never contain the
invitation, password, peer key, Bluetooth address, or network address.

The service, scanner, advertisement, peripheral map, and partial frame buffers
exist only while a discovery lease is active. An invitation is sent only after
the user enters a BLE-observed One-time Room, prepares the transfer details, and
taps the final **Start** action.

## 6. Merge and lifecycle rules

Every provider emits `DiscoveryObservation` through `DiscoveryProvider`.
`DiscoveryPeerRegistry` keeps the newest observation per peer and source:

- equal peer key: one card, union of fresh source badges;
- different peer key: different cards, even when names match;
- out-of-order callback: cannot overwrite a newer source observation;
- source not refreshed for 20 seconds: remove only that source;
- no fresh sources: remove the card.

Providers report `Stopped`, `Starting`, `Ready`, `Degraded`, permission and
availability failures, or `Reserved`. `WifiAwareDiscoveryProvider` currently
reports `Reserved` through the same interface and performs no platform calls.

Provider state transitions create privacy-safe operation breadcrumbs such as:

```text
DISCOVERY provider=bluetooth state=ready
DISCOVERY provider=mdns state=degraded
```

## 7. Verification

The implementation is accepted when all of the following hold:

1. `ktlintCheck`, JVM unit tests, and `assembleDebug` pass through
   `scripts/with-build-cache-guard.sh`.
2. The page shows explicit Bluetooth, mDNS, and Wi-Fi Aware provider states.
3. A BLE UUID probe appears with a BLE badge and RSSI.
4. An `_envoix-disc._udp` probe appears with its normalized name and mDNS badge.
5. BLE and mDNS probes with the same peer key appear as one card with two badges.
6. Stopping one probe expires only that source; stopping both removes the card.
7. Leaving Connect stops scanning, advertising, DNS-SD, the UDP socket, and
   multicast lock unless a nearby room holds the handoff lease. That lease
   filters product state to the selected peer, but the providers are not yet
   narrowed below the workflow layer. Activity and Settings never hold a
   discovery lease.
8. Restarting discovery advances the callback generation so callbacks from the
   stopped generation are ignored. Presence-key lifetime follows the shared
   workflow-owner continuity rule in section 2.
9. Android JVM and Apple hosted tests decode fragmented invitation frames and
   reject invalid, out-of-order, oversized, or mismatched-security payloads.
10. Android and Apple builds include the GATT server and client implementations.

The experimental BLE handoff additionally verifies that:

- `NearbyPairingSelection` remains untrusted display context and carries no
  endpoint, credential, or long-term identity;
- tapping a BLE-observed card opens an unverified One-time Room without
  claiming device identity or a persistent connection;
- Apple and Android keep the selected nearby context attached to that stable
  room while a transfer is prepared;
- Photos, Files, and Folder selection remain usable after a nearby sender
  handoff, and receive-destination authorization occurs before invitation
  delivery;
- the selected side creates and fragments the existing full invitation, while
  the receiving side decodes it and asks the user to Accept or Reject;
- only the final Start action may deliver an outbound BLE invitation, with one
  delivery in flight and late or duplicate completions unable to create a
  second Activity;
- the selected-device context is cleared when the flow is dismissed or started;
- no invitation or password appears in advertisements or logs; and
- after handoff, both sides use the unchanged SPAKE2 and Direct/Relay transfer
  state machines.

Automated protocol tests, Android `ktlintCheck`/JVM tests/`assembleDebug`, Apple
hosted tests, an Apple simulator UI regression, and an Apple physical-device
build passed on 2026-07-19. On 2026-07-24, hosted tests additionally covered
single-flight invitation delivery, failure retry, cancellation, and duplicate
callback suppression; the simulator opened Photos, Files, and Folder from the
same nearby sender setup and retained the selected folder.

The same-day physical GATT gate then passed 1/1 on Android model `25060RK16C`
and an iPhone 15 Pro Max:

- the iPhone first merged Android's BLE and mDNS observations into one card;
- Android selected the iPhone card, chose its local Receive role, and wrote the
  existing full `envoix://pair/` invitation through the fixed GATT service;
- iPhone decoded the fragmented invitation, presented the unauthenticated
  warning, enabled only the opposite Send role, and disabled Receive;
- XCTest retained screenshots named `physical-nearby-android-ble-mdns` and
  `physical-android-to-ios-ble-invite` and emitted
  `ENVOIX_PHYSICAL_BLE_INVITE_RECEIVED`;
- Android's operation log recorded only matching `connecting` and `delivered`
  breadcrumbs with one random request ID and `auth=none`; it contained no
  invitation, code, peer key, or Bluetooth address; and
- the milestone result bundle is
  `/private/tmp/envoix-ble-rendezvous-physical-20260719.xcresult`.

This proves a real Android-central to iPhone-peripheral GATT invitation handoff
and role projection. The shared codec tests and symmetric client/server
implementations cover both platform directions; a second reverse-direction
physical gate is useful regression coverage but is not claimed by this result.

Physical Android↔iPhone discovery evidence was captured on 2026-07-18 and
re-run on the lifecycle-fixed binaries on 2026-07-19 with Android model
`25060RK16C` and an iPhone 15 Pro Max:

- the iOS physical UI test
  `testPhysicalNearbyDiscoveryFindsAndroid` passed after the presence-key
  rotation change and attached a screenshot of one `25060RK16C` card containing
  both `BLE` and `mDNS`;
- during the same foreground window, Android exposed one `iPhone` card containing
  both `BLE` and `mDNS` in its UI hierarchy;
- a Mac on the same hotspot link observed Android remove its DNS-SD instance
  when discovery stopped and restore publication when it resumed;
- observed BLE RSSI values were approximately -22 to -44 dBm; these are
  independent receiver measurements, not a distance or trust signal;
- with the iPhone initially on cellular and Android on an unrelated Wi-Fi, both
  sides correctly converged only on BLE; after Android joined the Apple-hosted
  local link, both mDNS observations merged into their BLE cards; and
- the first milestone `.xcresult` was intentionally treated as a regenerable
  build artifact and reclaimed by the build-cache guard; the final 1/1 pass
  produced
  `Test-Envoix-iOS-AppUI-2026.07.19_00-26-03-+0800.xcresult` in Xcode's
  DerivedData, while the pass condition is also recorded by this ledger.

This proves cross-platform foreground BLE/mDNS discovery and merge behavior. It
does not authenticate the displayed names or peer keys. The discovery card is
user-facing context, is not cryptographically bound to a long-term trusted
device identity, and must never be presented as one.

## 8. Earlier QR/code pairing and transfer evidence

Before the GATT carrier was implemented, the same device pair exercised the
existing Room/SPAKE2 and transfer state machines through a separately entered
Envoix invitation. These results prove the downstream transfer path, not BLE
invitation delivery or a binding between a discovery card and that path:

- iPhone to Android completed a 35-byte transfer over Direct to the Android
  hotspot client at `172.20.10.10`; Android published it through MediaStore and
  verified SHA-256
  `9d377562642d0dc00419ac7a85f44ab698e0142d1f53af86a6e3b142c0dda16e`;
- Android to iPhone completed a 35-byte relay-only transfer and iOS verified
  SHA-256
  `145408869d69b7864afad6bce966d5668ed7dc11764d7c58e601ba3ee7984d65`;
- iPhone to Android also completed relay-only, with Android publication and the
  expected SHA-256; and
- a 128 MiB iPhone-to-Android relay-only run paused after crossing 4 MiB, reached
  the canonical `Paused` state, remained paused for two seconds, resumed as a
  second attempt, completed all `134217728` bytes, and passed Android MediaStore
  SHA-256
  `c1fb2c7fcd530efc01384d2e3d72a29d3dd1ad1bba466eef6a99e88990385c9d`.

The Apple-hosted local link is directionally asymmetric for arbitrary inbound
connections. iPhone could initiate Direct to the Android hotspot client, while
Android-to-iPhone Direct timed out even though discovery mDNS worked. The
relay-only passes therefore validate the required fallback, rather than hiding
this topology limitation or treating mDNS visibility as proof of bidirectional
IP reachability.

The pause/resume physical run proves functional recovery to completion. Its
second attempt restarted byte progress from zero; persisted-prefix efficiency
remains a separate transfer-core concern and is not claimed by this discovery
milestone.

## 9. Security follow-up

The authenticated replacement is intentionally separated from this vertical
slice as [GitHub issue #52](https://github.com/ECE4410J-NUUB/envoix/issues/52).
It covers the threat model, cryptographic binding between the
selected presence and the session, replay/downgrade resistance, first-use
confirmation, secure key storage, recovery and revocation, rotating identifiers,
cross-platform vectors, negative tests, and security review. Until that issue is
implemented and reviewed, the UI and logs must continue to say `auth=none` and
must not call this flow secure pairing.

## 10. Source basis

- [Apple `CBPeripheralManager.startAdvertising`](https://developer.apple.com/documentation/corebluetooth/cbperipheralmanager/startadvertising%28_%3A%29)
- [Android Bluetooth LE overview](https://developer.android.com/develop/connectivity/bluetooth/ble/ble-overview)
- [Android network service discovery](https://developer.android.com/develop/connectivity/wifi/use-nsd)
