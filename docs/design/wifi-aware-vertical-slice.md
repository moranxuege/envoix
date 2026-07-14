# Cross-platform Wi-Fi Aware vertical slice

Status: **research contract — not implemented or advertised as supported**

Last reviewed: 2026-07-14

This document defines the smallest honest path to an Envoix Wi-Fi Aware data
path between iPhone and Android. It does not replace the remote rendezvous,
mailbox, or relay paths, and it does not claim that an Apple `WAEndpoint` can be
passed directly to the existing Rust iroh endpoint.

## 1. Product outcome

For a nearby, explicitly selected device, Envoix should be able to:

1. discover and pair without a router, internet connection, or cloud service;
2. create an authenticated Wi-Fi Aware data path;
3. run the existing Envoix authentication, framing, hashing, pause/resume,
   receipt, publication, and canonical Activity semantics over that path; and
4. fall back visibly to the existing QR/link/room/direct/relay flows when Wi-Fi
   Aware is unsupported, unavailable, or the peer is remote.

The first accepted matrix is iPhone↔Android in both transfer directions plus an
Android↔Android baseline. macOS is not included because Apple does not list it
among devices that support the public Wi-Fi Aware framework.

## 2. Verified platform boundary

### Apple

- The public framework requires iOS/iPadOS 26 and supported hardware. Apple's
  current list starts at iPhone 12.
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

1. `_envoix._udp` pairing and data-path establishment works between the target
   iPhone and Android hardware;
2. Apple system QUIC and Android's network-bound quinn/iroh endpoint negotiate
   the same ALPN and open one bidirectional stream;
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

### W1 — Cross-platform pairing

- Apple DevicePicker/DevicePairingView and Android API-34 NAN pairing.
- Pair, list, remove, restart, and verify the same physical devices.
- Treat pre-pairing metadata as unauthenticated.

Gate: one iPhone↔Android pair and re-verification after both apps restart; no
router or internet is present.

### W2 — Raw data path and QUIC spike

- Create `_envoix._udp` publisher/subscriber sessions in `.bulk` mode.
- Prove the QUIC, trust, exporter, duplex hash, cancellation, and path checks in
  section 5 before changing Envoix transfer routing.

Gate: reproducible logs contain capability, pairing, data-path, negotiated ALPN,
and sanitized path evidence; no secret, pairing credential, or payload is
logged.

### W3 — Rust transport injection

- Add the versioned foreign async transport interface and
  `NativeFrameConnection` with memory/fault-injection tests.
- Keep all existing public transfer entry points and iroh paths unchanged.
- Add new start/restore functions or request fields additively.

Gate: frame fragmentation, backpressure, EOF, concurrent send/receive,
cancellation, exporter mismatch, and late callback tests pass; generated Swift
and Kotlin bindings compile against the current apps.

### W4 — Envoix single-file transfer

- Run the existing authenticated single-file lifecycle over Wi-Fi Aware.
- Preserve pause/resume, cancel, receipt, publication, and Activity sequence.
- Report `wifi_aware` as the selected data path without parsing log text.

Gate: iPhone→Android and Android→iPhone each pass 8 MiB and 64 MiB hash-verified
transfers, Pause→Resume with nonzero resumed bytes, explicit Cancel cleanup, and
app restart recovery. Android→Android passes at least the same 8 MiB baseline.

### W5 — Product picker and fallback

- Add the nearby-device picker and recovery copy described above.
- Verify permission denial/recovery, Wi-Fi disabled, peer departure, path loss,
  and fallback without duplicate Activities.

Gate: physical UI/accessibility review on supported iPhone plus Android smoke;
remote QR/room/relay tests remain green.

Manifest transfer over Wi-Fi Aware is a later composition gate. Neither feature
may fork its own hashing, resume, or Activity model.

## 8. Release evidence

Wi-Fi Aware is advertised only after evidence records:

- exact iPhone/Android models, OS/API levels, and capability results;
- entitlement, service declaration, and Android permission configuration;
- pairing and reconnect behavior after process/device restart;
- bidirectional hash-verified data with infrastructure Wi-Fi and internet
  removed;
- selected path, discovery/connect time, throughput, peak memory, energy/thermal
  observations, and failure/fallback behavior;
- Android↔Android baseline and regression of existing LAN/direct/relay paths.

Simulator, compile success, a discovered display name, or a successful Apple
`NetworkConnection` by itself is not sufficient evidence of Envoix Wi-Fi Aware
support.

## 9. Primary references

- [Apple: Wi-Fi Aware](https://developer.apple.com/documentation/WiFiAware)
- [Apple: Adopting Wi-Fi Aware](https://developer.apple.com/documentation/wifiaware/adopting-wi-fi-aware)
- [Apple: Building peer-to-peer apps](https://developer.apple.com/documentation/WiFiAware/Building-peer-to-peer-apps)
- [Apple: NetworkConnection](https://developer.apple.com/documentation/network/networkconnection)
- [Apple: exporting a protocol secret](https://developer.apple.com/documentation/security/sec_protocol_metadata_create_secret(_:_:_:_:))
- [Android: Wi-Fi Aware overview](https://developer.android.com/develop/connectivity/wifi/wifi-aware)
- [Android: AwarePairingConfig](https://developer.android.com/reference/android/net/wifi/aware/AwarePairingConfig)
- [Android: Network](https://developer.android.com/reference/android/net/Network)
- [UniFFI: async overview](https://mozilla.github.io/uniffi-rs/latest/internals/async-overview.html)
