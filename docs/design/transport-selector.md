# Transport provider selector

Status: **Apple Wi-Fi Aware UDP/QUIC adapter physically validated; product selection remains peer-specific**

Last reviewed: 2026-07-24

## Decision

Envoix has three independent routing axes:

1. `PeerSource` decides how peers find and authenticate each other (manual,
   invite, mDNS, or Room).
2. `TransportSelector` decides which provider establishes the data channel
   (wired, Wi-Fi Aware, or iroh).
3. `PathPolicy` constrains direct/relay behavior *inside iroh* after iroh is
   selected.

The selector must never encode Wi-Fi Aware as “iroh direct.” An Apple
`WAEndpoint` is an opaque Network.framework endpoint, and a future raw USB
bulk/accessory channel is not an IP path at all. They remain independent
providers even though the Apple adapter now reuses iroh's QUIC engine over a
custom, platform-owned datagram transport.

```text
PeerSource / nearby-device identity
                 ↓
       TransportSelector
      ↙          ↓          ↘
  Wired      Wi-Fi Aware      iroh
     ↓             ↓            ↓
 raw stream   platform UDP   IP/relay
     ↓             ↓            ↓
 native frame   iroh custom transport
     connection       \        /
                       QUIC
                        ↓
                FrameConnection
                 ↓
 auth → protocol → transfer → Activity

iroh only: PathPolicy → Auto / RelayOnly / DirectOnly
```

## Selection contract

The client API exposes three policies:

- `Automatic`: choose the first ready provider in the stable priority order
  wired → Wi-Fi Aware → iroh;
- `Prefer(provider)`: choose it when ready, otherwise use the first ready
  fallback and retain a structured fallback reason;
- `Require(provider)`: fail during setup when that provider is not ready; it
  never silently falls back.

Candidate input is validated before selection. Duplicate provider entries are
an error rather than an order-dependent result. Availability is structured,
including OS/hardware, entitlement, permission, disabled, temporary,
pairing-required, and implementation-pending states.

`Ready` is deliberately peer-specific. A platform capability probe reporting
supported hardware is necessary but not sufficient: the provider adapter must
be compiled, and the selected peer must have a usable candidate. This prevents
the W0 Apple/Android probes from advertising a data path before the physical
pairing and channel gates pass.

## Current adapter matrix

| Provider | Compiled status | Current behavior |
| --- | --- | --- |
| iroh | `ready` | Existing Manual/Invite/mDNS/Room send and receive functions; existing `PathPolicy` preserved |
| Wi-Fi Aware | adapter implemented | Apple Network.framework provides connected UDP; Rust runs iroh QUIC, SPAKE2, Manifest v2, recovery, and delivery proof over a custom transport. Android currently provides a raw TCP stream with Rust-owned TLS 1.3 and the same upper layers |
| wired | `implementation_pending` | Reserved for a future raw wired provider; never treated as an iroh/IP path |

Wi-Fi Aware readiness is supplied for the selected paired device, not inferred
from a global hardware probe. Apple calls the additive UniFFI datagram entry
points while retaining the publisher/listener or subscriber/connection scope
for the complete session. The Rust custom transport disables ordinary IP and
relay transports, exchanges ephemeral iroh endpoint IDs over a bounded,
retrying bootstrap, and reports the selected path as `wifi_aware`. Android
calls the additive JNI stream entry point after its Wi-Fi Aware network
callback has produced a TCP socket. Existing iroh APIs are unchanged, so
`Prefer(WifiAware)` may retry through ordinary iroh without creating another
job or transfer protocol.

On Apple, iroh's ephemeral QUIC endpoint identity protects the transport and
mutual SPAKE2 authenticates the invitation secret over that connection. On the
Android stream adapter, the native TLS server certificate is ephemeral and
intentionally does not claim Envoix identity; SPAKE2 binds the invitation
secret to the TLS exporter. In both cases, platform code never implements
protocol frames or file semantics.

## Validation gates

1. In-memory datagram and fragmented-stream adapters must complete their
   transport handshake, SPAKE2, Manifest v2 file delivery, receiver save, and
   matching delivery proof. Datagram bootstrap must survive a lost first
   packet without starting concurrent foreign receives.
2. Apple simulator compilation and Android JVM/JNI checks validate the foreign
   transport contracts without consuming physical test devices.
3. Apple ↔ Apple has passed both directions on actual hardware. Final
   cross-platform release evidence remains hardware-gated: Apple ↔ Android
   must complete both directions, including cancellation, network loss, and
   `Prefer` fallback.
4. Register the production Wi-Fi Aware service names with IANA before release;
   the current names satisfy Apple's syntax and 15-character label limit.
5. A future wired adapter may register another peer-specific candidate. It may
   use raw USB bulk/accessory I/O and does not require a synthetic IP interface.

The selector itself must stay pure and deterministic. Provider discovery,
permission prompts, pairing UI, connection attempts, and transfer fallback
side effects live outside it.

### Apple physical validation status

Apple-to-Apple discovery, pairing, raw UDP, and canonical transfer passed on
2026-07-24 between an iPhone 15 Pro Max and an iPad Air 5, both running
26.5.2 (23F84). The final hardware gate used Apple's documented `bulk` mode
with the default `bestEffort` service class:

- connected UDP reached `ready` in both roles with
  `NWPath.wifiAware` present;
- the current iPhone sender path reported `maximumDatagramSize == 1402` while
  the receiver reported 1452. Bootstrap v2 exchanges both limits, selects the
  smaller value, and pins iroh's QUIC MTU to it with discovery disabled so an
  oversized probe cannot black-hole the authentication stream;
- the receiver path used the private `nan0` interface, while the sender could
  initially report ordinary Wi-Fi during `preparing` and then expose valid
  Wi-Fi Aware connection metadata at `ready`;
- iroh negotiated `envoix/manifest/2`, mutual SPAKE2 authenticated the shared
  token, and Manifest v2 completed verification, save, and delivery proof;
- iPhone → iPad and iPad → iPhone each transferred and saved 8,388,608 bytes
  with zero XCTest failures; and
- after the MTU negotiation fix, the same iOS/iPadOS 26.5.2 (`23F84`) pair
  transferred and verified 268,435,456 bytes in each direction. Payload
  goodput was 192.8 Mbit/s from iPhone to iPad and 156.8 Mbit/s from iPad to
  iPhone; and
- an observed lost first bootstrap datagram is covered by retransmission and a
  regression test that models both a cancelled foreign receive lingering
  across UniFFI and asymmetric 1402/1452-byte platform limits.

The same devices still reproduce Darwin `ENOBUFS` (55) on the first TCP frame
after the Wi-Fi Aware connection becomes ready. Full device reboot, re-pairing,
and the 26.5.2 update did not change that result. Apple also prints
`reclassify can't find client` immediately before successful UDP transfers in
both directions, so that line alone is not a transfer failure.

`NWPath.availableInterfaces` and the private `nan0` name remain diagnostic
details rather than the path contract. Production Apple transfers therefore
use connected UDP without an interface-type constraint, require
`NWPath.wifiAware`, require a QUIC-capable datagram size, and keep TCP only as a
diagnostic reproducer. Milestone evidence was captured by
`WifiAwarePhysicalTransferTests.testManifestV2TransferServicePath` in all four
direction/role combinations; its `.xcresult` bundles are regenerable test
artifacts rather than durable project data.

The physical test accepts `ENVOIX_WIFI_AWARE_PAYLOAD_MIB` in the range
1–1024 (default 8). `ENVOIX_WIFI_AWARE_TIMEOUT_SECONDS` may override the
size-scaled timeout in the range 30–7200 seconds. Payload generation and
verification use bounded 1 MiB buffers so benchmark size does not become an
artificial memory-pressure test. Each connection records Wi-Fi Aware
throughput ceiling, current capacity, capacity ratio, signal strength, and
maximum datagram size at ready and completion. Transfer progress is sampled at
25 percent intervals and reports payload goodput separately from discovery,
handshake, verification, and save time.
