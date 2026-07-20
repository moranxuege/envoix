# Cross-platform Wi-Fi Aware vertical slice

Status: **W0 passed on Apple hardware; W1/W2 diagnostics are experimental and
the product must not advertise Wi-Fi Aware transfer support yet**

Last reviewed: 2026-07-20

This document defines the smallest honest path to an Envoix Wi-Fi Aware data
path. The first development pair is iPhone↔iPad; the release target remains a
cross-platform Apple↔Android path. Wi-Fi Aware does not replace the remote
rendezvous, mailbox, or relay paths, and an Apple `WAEndpoint` cannot be passed
directly to the existing Rust iroh endpoint.

## 1. Product outcome

For a nearby, explicitly selected device, Envoix should be able to:

1. discover and pair without a router, internet connection, or cloud service;
2. create an authenticated Wi-Fi Aware data path;
3. run the existing Envoix authentication, framing, hashing, pause/resume,
   receipt, publication, and canonical Activity semantics over that path; and
4. fall back visibly to the existing QR/link/room/direct/relay flows when Wi-Fi
   Aware is unsupported, unavailable, or the peer is remote.

The active development matrix is iPhone 15 Pro Max↔iPad Air 5 in both
directions. The release matrix adds Apple↔Android after pairing-capable Android
hardware is available. Android implementation work is paused until then;
existing Android probes remain in the branch but are not a current milestone
dependency. macOS is not included because Apple does not list it among devices
that support the public Wi-Fi Aware framework.

## 2. Verified platform boundary

### Apple

- The public framework requires iOS/iPadOS 26 and supported hardware. Apple's
  supported list includes iPad Air 4 and later, including the target Air 5.
- The app needs `com.apple.developer.wifi-aware` with both `Publish` and
  `Subscribe`, and every service must be declared in `WiFiAwareServices`.
- Pairing is system-owned through `DeviceDiscoveryUI` or `AccessorySetupKit`.
  File transfer is a device-to-device use case, so Envoix uses
  `DeviceDiscoveryUI`, not accessory ownership semantics.
- Discovery returns an opaque `WAEndpoint`. The supported route is
  `NetworkBrowser`/`NetworkListener` followed by `NetworkConnection`; there is
  no documented raw socket or iroh endpoint handle to extract.
- Apple recommends `.bulk` for almost all cases. Envoix must not use
  `.realtime` for file transfer because it increases energy use.
- `NetworkConnection<QUIC>` exposes bidirectional streams, negotiated ALPN,
  Wi-Fi Aware path metadata, and TLS security metadata. Security.framework can
  export connection-specific keying material from that metadata.

### Android

- Basic Wi-Fi Aware discovery/data paths exist from API 26, but Apple-style NAN
  pairing requires Android API 34 plus hardware reporting
  `Characteristics.isAwarePairingSupported()`.
- The current Android app has `minSdk=29` and `compileSdk/targetSdk=34`, so the
  pairing APIs can be compiled behind runtime API and capability gates without
  raising the minimum SDK.
- Android must declare Wi-Fi Aware and nearby-Wi-Fi permissions, check
  `FEATURE_WIFI_AWARE`, observe availability changes, and request the runtime
  `NEARBY_WIFI_DEVICES` permission where required.
- `AwarePairingConfig`, bootstrapping callbacks, pairing setup/verification,
  and cached aliases are native Android responsibilities.
- A successful data-path request returns an Android `Network`. Java sockets or
  a file descriptor can be bound to that network, and `getNetworkHandle()` is
  available for NDK integration.

An Android device that supports basic API-26 Wi-Fi Aware but not NAN pairing is
not accepted as an Apple-interoperable device. It may be evaluated later for an
Android-only compatibility path.

## 3. Service and roles

The proposed service is `_envoix._udp`:

- `envoix` satisfies Apple's service-name length and character rules;
- the `_udp` suffix is required for QUIC because QUIC is not TCP;
- Android publishes/subscribes to the exact same service string;
- formal IANA registration is a release task, not a reason to invent different
  Apple and Android on-air names.

Both apps declare publish and subscribe capability. For one transfer attempt,
the receiver publishes/listens and the sender subscribes/connects. This is a
transport role only; it does not change the existing product rule that either
device may display or scan a QR code in non-Aware flows.

Discovery metadata is untrusted and contains only a versioned Envoix service
marker plus the minimum data needed for role/capability filtering. Canonical
device identity is established by the secure Envoix handshake after the data
path exists, never by a display name, `PeerHandle`, or pairing alias.

## 4. Architecture decision

Provider selection follows the separate contract in
[`transport-selector.md`](transport-selector.md). Wi-Fi Aware is a peer-level
transport provider alongside iroh and a future wired provider; iroh's
direct/relay `PathPolicy` remains an internal iroh decision.

```text
Apple DeviceDiscoveryUI / Android NAN pairing
                    ↓
Apple NetworkConnection / Android Network-bound QUIC
                    ↓
       one reliable ordered native byte channel
                    ↓
 additive UniFFI foreign async transport interface
                    ↓
      Rust NativeFrameConnection adapter
                    ↓
 authentication → protocol → transfer/session/client
                    ↓
       existing durable Activity + publication
```

The platform adapter owns only discovery, OS pairing, connection establishment,
byte I/O, connection-specific key export, path metrics, and teardown. Rust keeps
all application framing and transfer semantics. Swift and Kotlin must not
reimplement `Hello`, `FileHeader`, chunks, hashes, receipts, Manifest, or the
transfer state machine.

The additive foreign interface is expected to have the following shape; exact
names are frozen only after the transport spike:

```rust
#[uniffi::export(with_foreign)]
pub trait NativeDuplexTransport: Send + Sync {
    async fn send(&self, bytes: Vec<u8>) -> Result<(), NativeTransportError>;
    async fn receive(&self, max_bytes: u32)
        -> Result<NativeTransportRead, NativeTransportError>;
    async fn export_keying_material(
        &self,
        label: String,
        length: u32,
    ) -> Result<Vec<u8>, NativeTransportError>;
    async fn close(&self) -> Result<(), NativeTransportError>;
}
```

`NativeTransportRead` distinguishes bytes from EOF. The adapter allows at most
one read and one write in flight, applies bounded buffering, propagates
cancellation, and caps each FFI read/write to a named constant. The current
64 KiB default transfer chunk is a suitable first measurement point; increasing
it requires throughput, copying, memory, and thermal evidence.

UniFFI 0.31 supports foreign async trait methods, so a polling loop or blocking
callback thread is not the default design. The Rust adapter owns frame encoding
and incremental decoding and implements the existing `FrameConnection`,
including full-duplex `send_chunk_or_recv_frame` behavior.

## 5. QUIC interoperability spike

Apple's documented APIs make `NetworkConnection<QUIC>` the preferred first
candidate, but successful interoperability with the current iroh/quinn endpoint
is not yet proven. Before transfer integration, a physical-device spike must
prove all of the following:

1. `_envoix._udp` pairing and data-path establishment works first between the
   iPhone 15 Pro Max and iPad Air 5, then between Apple and supported Android
   hardware;
2. Apple Network.framework QUIC works in both Apple device roles; after Android
   work resumes, Apple system QUIC and Android's network-bound quinn/iroh
   endpoint negotiate the same ALPN and open one bidirectional stream;
3. certificate/endpoint verification can bind the connection to the expected
   Envoix identity without disabling trust globally;
4. both sides export identical connection-specific keying material for the
   Envoix authentication transcript;
5. a hashed 64 MiB duplex stream survives normal cancellation and teardown;
6. path evidence proves the traffic used Wi-Fi Aware rather than infrastructure
   Wi-Fi, cellular, or relay.

Apple QUIC exposes `securityProtocolMetadata`, and
`sec_protocol_metadata_create_secret` provides a label-based exporter. The
Android/quinn side must use the equivalent exporter construction. If the current
iroh TLS identity cannot interoperate safely with Network.framework, the same
`NativeDuplexTransport` boundary may use a separately specified native TLS/TCP
channel. That fallback is allowed only after the QUIC spike records the precise
failure; it may not weaken authentication or copy the application protocol into
native code.

## 6. Capability and UX model

The provider reports structured availability rather than one Boolean:

```text
unsupported_os
unsupported_hardware
entitlement_missing
permission_required
permission_denied
wifi_disabled
temporarily_unavailable
pairing_required
ready
```

The normal Send sheet gains “Nearby devices” only when at least one provider is
usable. Selecting it opens a compact device picker:

- already paired and currently reachable devices appear first;
- “Add nearby device” invokes the system pairing UI;
- selecting a device creates the canonical send Activity immediately and shows
  “Connecting nearby…” while the data path forms;
- failure offers Retry and the existing QR/link route without discarding the
  selected file or creating a duplicate Activity;
- a remote trusted device is not shown as Wi-Fi Aware-reachable merely because
  it exists in the trusted-device store.

Settings may show capability diagnostics and paired-device management, but
users must not need to visit Settings to perform an ordinary transfer.

## 7. Execution slices and gates

### W0 — Capability probes

- Add read-only Apple and Android capability probes behind availability checks.
- Do not request pairing or permissions on launch.
- Verify unsupported devices retain the current UI and transfer behavior.

Gate: deterministic unit tests for every structured availability state; current
Apple and Android builds remain compatible.

Physical-device evidence, initially recorded on 2026-07-17 and refreshed on
2026-07-20:

| Endpoint | System | Capability result | Gate consequence |
| --- | --- | --- | --- |
| Xiaomi `25060RK16C` | Android 15 / API 35 | `FEATURE_WIFI_AWARE=false`; no `wifiaware` system service; Envoix probe reports `unsupported_hardware` | This device cannot be the Android endpoint for W1–W5. A different Android device that reports both Wi-Fi Aware and API-34 pairing support is required. |
| iPad Air (5th generation), `iPad13,16` | iPadOS 26.5.2 | Build `2026072001` passes the hosted physical probe in iPhone compatibility mode; `availability=pairing_required`, `pairing_supported=true`, `paired_device_count=0` | Apple W0 passes on iPad. It is available for the preliminary Apple↔Apple W1 gate. |
| iPhone 15 Pro Max, `iPhone16,2` | iOS 26.5 | Build `2026072001` passes the hosted physical probe; `availability=pairing_required`, `pairing_supported=true`, `paired_device_count=0` | Apple W0 passes on iPhone. Pairing is the next gate. |

The Android result is independent of the observed `wifi_disabled` setting:
`FEATURE_WIFI_AWARE` is a static package-manager capability, and the probe
rejects the device before permission or availability checks. It is evidence
against this target device, not evidence that cross-platform Wi-Fi Aware is
generally infeasible.

Apple Developer Program enrollment became active on 2026-07-20 for Team ID
`6638TTB2SF`. Xcode automatic signing created a development profile for
`com.envoix.app.ios` that expires on 2027-07-20 and contains
`com.apple.developer.wifi-aware = [Publish, Subscribe]`. The signed build kept
both entitlement values and declared `_envoix._udp`. The physical W0 probes on
the iPad and iPhone passed without using a Simulator. Their `.xcresult` bundles
are regenerable build evidence and are not durable repository data.

Testing-scope decision on 2026-07-20: keep the current iPhone-only
`UIDeviceFamily = [1]` build and run it in iPhone compatibility mode on the iPad
Air 5 for W0-W2. These gates measure host-device capability, system pairing,
and the Wi-Fi Aware data path, not native iPad layout quality. The signed
entitlement and declared services are unchanged by the compatibility UI mode,
while `WACapabilities` and the DeviceDiscoveryUI fallback remain the runtime
source of truth. If either framework rejects the compatibility-mode process,
the physical result invalidates this assumption. Native universal
`UIDeviceFamily = [1, 2]` support, iPad layout, multitasking, rotation,
keyboard/pointer, and iPad UI/accessibility review are deferred to product work
before W5/release acceptance.

Development-host constraint: validation on the 8 GB / 512 GB M2 MacBook Air
uses the repository's guarded stable caches, incremental builds, serialized
endpoint prebuilds, and focused physical tests. Full clean or parallel platform
matrices require a milestone-specific reason; compatibility-mode W0-W2 does
not justify a separate iPad build product.

Deterministic capability-policy tests remain the regression gate, while the two
physical results above are the W0 source of truth. Neither result advances W1:
both devices still report zero paired devices.

### Current execution policy

- Develop and validate on iPhone↔iPad first. Do not add new Android code until
  supported Android hardware is available.
- Keep pairing, raw-path, Rust-bridge, transfer, and product-UX work in separate
  reviewable commits. A later gate must not be used to hide a failure in an
  earlier one.
- Prebuild the two Apple endpoints serially with the stable guarded cache, then
  run paired physical tests without rebuilding. Do not launch a Simulator for
  W1 or W2.
- Keep `TransportProvider::WifiAware` at `implementation_pending` until W3 can
  return a peer-specific, authenticated `FrameConnection`. Hardware support or
  a paired-device count alone is never `ready`.
- Use generation tokens and explicit cancellation for pairing observation,
  browsing, listening, connecting, and transfer callbacks. A stopped attempt
  must not mutate a newer attempt.

### W1 — Apple pairing and paired-device state

Goal: establish and observe system-owned pairing only. W1 does not open a data
connection and does not change transport selection.

Implementation:

1. Introduce an Apple pairing controller that continuously observes
   `WAPairedDevice.allDevices` as an `AsyncSequence`, rather than refreshing only
   one count after a picker callback.
2. Project each `WAPairedDevice` into UI-only state containing its opaque app
   scoped ID, optional display name, pairing information, and current presence.
   Never treat this metadata as the canonical Envoix identity.
3. Keep the two system roles explicit in the developer panel:
   `DevicePairingView` makes the publisher discoverable and `DevicePicker`
   selects it from the subscriber. Show the expected action on each endpoint
   instead of two ambiguous “Add” buttons.
4. Model `idle`, `advertisingForPairing`, `choosingDevice`, `paired`,
   `cancelled`, and `failed` states. Redact pairing identifiers and system error
   payloads from logs.
5. Observe system-side removal through the paired-device sequence. The current
   public SDK exposes paired-device snapshots but no app-owned removal method;
   do not invent or persist a private removal mechanism.

Physical script:

1. Install the same build on the iPhone and iPad and confirm both report
   `pairing_required` with zero devices.
2. On the intended receiver, open **Allow device**. On the sender, open
   **Add device**, select the receiver, and complete the system prompts on both
   devices.
3. Require both apps to converge to one paired device without manual refresh.
4. Force-quit and relaunch both apps, then reboot one endpoint and confirm the
   pair is still observable.
5. Remove access using the system-supported UI, confirm both snapshots update,
   then pair again with the publisher/subscriber roles reversed.

Gate: the iPhone and iPad pass pair, observe, restart, remove, and re-pair with
no duplicate rows, stale callbacks, secret logs, router dependency, or internet
dependency. Record device/OS/build identifiers and sanitized state transitions.

### W2 — Apple raw Wi-Fi Aware data path

Goal: prove a real Wi-Fi Aware byte channel independently of Rust transfer
semantics. The existing `_envoix-probe._tcp` protocol is diagnostic only.

Implementation:

1. Replace `allPairedDevices` plus “first endpoint wins” with an explicitly
   selected `WAPairedDevice`. Feed `.selected(...)` or an equivalent precise
   predicate into the listener/browser so a connection cannot silently target
   the wrong paired device.
2. Give the publisher listener, subscriber browser, and resulting
   `NetworkConnection<TCP>` one owner and one attempt generation. Enforce one
   publish and one subscribe operation per service, bounded timeouts, idempotent
   Stop, and deterministic close ordering.
3. Preserve `.bulk` performance mode. A successful exchange must additionally
   obtain `connection.currentPath?.wifiAware`; a TCP echo without Wi-Fi Aware
   path evidence is a failure.
4. Extend the probe from the 40-byte nonce frame to bounded hash-verified 1 MiB
   and 64 MiB streams in both directions. Record setup latency, byte count,
   duration, throughput, signal strength when available, cancellation point,
   and a redacted structured failure code.
5. Add deterministic tests for frame fragmentation, truncated input, wrong
   nonce, timeout, cancel-before-connect, cancel-during-I/O, late callbacks, and
   repeat Start/Stop. Do not log payloads or pairing material.
6. After the TCP path is stable, run the `_envoix._udp` Network.framework QUIC
   spike from section 5. The TCP probe must not become the production transfer
   transport merely because it connects first.

Physical matrix:

- iPhone publisher → iPad subscriber;
- iPad publisher → iPhone subscriber;
- 20 repeated 40-byte handshakes per direction;
- 1 MiB and 64 MiB hash-verified exchange per direction;
- five cancel/retry cycles and one force-quit/relaunch cycle per role;
- Wi-Fi radio remains enabled, but infrastructure Wi-Fi, cellular data, and
  internet reachability are removed for the final path-evidence run.

Gate: every successful run reports `path=wifi_aware`, both roles complete the
same transcript, cancellation leaves no active listener/browser/connection,
and the existing LAN/QR/Room paths remain unchanged.

### W3 — Authenticated native transport bridge

Goal: adapt the proven Apple connection to the existing Rust protocol boundary
without reimplementing Envoix transfer semantics in Swift.

Implementation:

1. Freeze the additive `NativeDuplexTransport` contract only after the W2 QUIC
   spike decides the channel and exporter requirements. It must provide bounded
   send/receive, EOF, close, cancellation, and connection-specific key export.
2. Implement `NativeFrameConnection` in Rust with one read and one write in
   flight, bounded buffering, incremental frame decoding, and the existing
   full-duplex control-frame behavior.
3. Implement the Apple adapter around the owned Network.framework connection.
   Swift owns platform lifecycle and path metrics; Rust owns Hello,
   authentication, frames, hashing, receipts, resume, and Activity state.
4. Produce a peer-specific Wi-Fi Aware candidate only when the chosen paired
   device, native adapter, and authenticated channel are usable. Register that
   adapter with the existing `TransportSelector`; do not extend iroh
   `PathPolicy`.
5. Keep all current APIs source-compatible. New transport preference or native
   channel parameters are additive, and old durable records continue to decode
   as `Automatic`.

Gate: Rust memory/fault tests pass fragmentation, backpressure, EOF, concurrent
send/receive, cancellation, exporter mismatch, authentication failure, and late
callbacks. Generated Swift bindings compile on the physical Apple target. Only
then may Wi-Fi Aware move from `implementation_pending` to peer-specific
`ready`.

### W4 — Envoix single-file lifecycle over Wi-Fi Aware

Goal: carry the existing authenticated single-file protocol over the W3
connection while preserving one canonical Activity.

Implementation:

1. Create the Activity once, then attempt the selected provider. `Require`
   returns a structured Wi-Fi Aware failure; `Prefer` may fall back to iroh
   without creating a second Activity or losing the selected file.
2. Preserve send/receive progress, pause, nonzero-byte resume, cancel, receipt,
   publication, retry, and process-restart recovery exactly as on iroh.
3. Store `selected_provider=wifi_aware` and structured fallback reason in the
   Activity projection. UI and diagnostics must not infer the path from log
   text.
4. Keep discovery labels and pairing aliases untrusted until the existing
   Envoix authentication binds the channel to the expected peer identity.

Gate on iPhone↔iPad, both directions: hash-verified 8 MiB and 64 MiB transfers;
Pause→Resume with nonzero resumed bytes; explicit Cancel cleanup; receiver
publication; sender and receiver restart recovery; `Prefer` fallback and
`Require` no-fallback behavior; no duplicate Activity.

### W5 — Product picker, recovery, and Apple UX

Goal: move the proven flow out of developer settings without exposing an
unfinished provider.

Implementation:

1. Add **Nearby devices** to the normal send flow only when a peer-specific
   candidate is usable. Already paired and currently reachable devices appear
   first; **Add nearby device** invokes the system pairing UI.
2. Present `Pairing`, `Connecting nearby`, `Transferring`, and structured
   recovery states. Preserve the Activity and file selection across Retry or
   fallback to QR/link/Room.
3. Verify permission denial/recovery, Wi-Fi disabled, pairing revoked, peer
   departure, listener/browser failure, path loss, background/foreground, and
   repeated taps.
4. Complete VoiceOver labels, Dynamic Type, localization, and the iPhone
   compatibility-mode iPad review. If DeviceDiscoveryUI or the connection flow
   fails in compatibility mode, promote native iPad support before accepting
   W5 rather than adding a private workaround.

Gate: physical UI/accessibility review on both Apple devices; existing
LAN/QR/Room/direct/relay regression suites remain green; no duplicate peers,
prompts, connections, or Activities.

### W6 — Resume cross-platform validation when hardware exists

Android development resumes only after a device reports Wi-Fi Aware support,
API-34 pairing support, and the required runtime permission behavior. Re-run W0
on that hardware, then repeat W1–W4 for Apple↔Android in both directions. The
existing Android probe code is a starting point, not accepted evidence.

Release gate: Apple↔Android passes pairing persistence, raw-path evidence,
authenticated 8 MiB/64 MiB transfer, pause/resume, cancel, restart recovery,
and fallback. Manifest transfer and Android↔Android remain later composition
gates; neither may fork hashing, resume, or Activity semantics.

## 8. Release evidence

Wi-Fi Aware is advertised only after evidence records:

- exact iPad/Android models, OS/API levels, and capability results;
- entitlement, service declaration, and Android permission configuration;
- pairing and reconnect behavior after process/device restart;
- bidirectional hash-verified data with infrastructure Wi-Fi and internet
  removed;
- selected path, discovery/connect time, throughput, peak memory, energy/thermal
  observations, and failure/fallback behavior;
- Android↔Android baseline when hardware is available, plus regression of
  existing LAN/direct/relay paths.

Simulator, compile success, a discovered display name, or a successful Apple
`NetworkConnection` by itself is not sufficient evidence of Envoix Wi-Fi Aware
support.

## 9. Primary references

- [Apple: Wi-Fi Aware](https://developer.apple.com/documentation/WiFiAware)
- [Apple: Adopting Wi-Fi Aware](https://developer.apple.com/documentation/wifiaware/adopting-wi-fi-aware)
- [Apple: Building peer-to-peer apps](https://developer.apple.com/documentation/WiFiAware/Building-peer-to-peer-apps)
- [Apple: Supported capabilities (iOS)](https://developer.apple.com/help/account/reference/supported-capabilities-ios)
- [Apple: NetworkConnection](https://developer.apple.com/documentation/network/networkconnection)
- [Apple: exporting a protocol secret](https://developer.apple.com/documentation/security/sec_protocol_metadata_create_secret(_:_:_:_:))
- [Android: Wi-Fi Aware overview](https://developer.android.com/develop/connectivity/wifi/wifi-aware)
- [Android: AwarePairingConfig](https://developer.android.com/reference/android/net/wifi/aware/AwarePairingConfig)
- [Android: Network](https://developer.android.com/reference/android/net/Network)
- [UniFFI: async overview](https://mozilla.github.io/uniffi-rs/latest/internals/async-overview.html)
