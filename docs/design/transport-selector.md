# Transport provider selector

Status: **Wi-Fi Aware byte-stream adapter implemented; product selection remains peer-specific**

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
bulk/accessory channel is not an IP path at all. Both therefore remain
independent providers that converge on the existing Rust `FrameConnection`
protocol boundary.

```text
PeerSource / nearby-device identity
                 ↓
       TransportSelector
      ↙          ↓          ↘
  Wired      Wi-Fi Aware     iroh
      \          ↓          /
       provider connection adapter
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
| Wi-Fi Aware | adapter implemented | Apple Network.framework and Android `WifiAwareNetworkSpecifier` provide a raw TCP stream; Rust owns TLS 1.3, SPAKE2 authentication, Manifest v2, recovery, and delivery proof |
| wired | `implementation_pending` | Reserved for a future raw wired provider; never treated as an iroh/IP path |

Wi-Fi Aware readiness is supplied for the selected paired device, not inferred
from a global hardware probe. Apple calls the additive UniFFI native-transport
entry points while retaining the publisher/listener or subscriber/connection
scope for the complete session. Android calls the additive JNI entry point
after its Wi-Fi Aware network callback has produced a TCP socket. Existing
iroh APIs are unchanged, so `Prefer(WifiAware)` may retry through iroh without
creating another job or transfer protocol.

The native TLS server certificate is ephemeral and intentionally does not claim
Envoix identity. Immediately after TLS, mutual SPAKE2 authenticates the
invitation secret and binds it to the TLS exporter. A TLS MITM therefore cannot
authenticate either substituted connection without the invitation secret and
the matching exporter. Platform code never implements protocol frames or file
semantics.

## Validation gates

1. Fragmented in-memory streams must complete TLS, exporter binding, SPAKE2,
   Manifest v2 file delivery, receiver save, and matching delivery proof.
2. Apple simulator compilation and Android JVM/JNI checks validate the foreign
   transport contracts without consuming physical test devices.
3. Final release evidence remains hardware-gated: Apple ↔ Android must complete
   both directions on actual Wi-Fi Aware hardware, including cancellation,
   network loss, and `Prefer` fallback.
4. A future wired adapter may register another peer-specific candidate. It may
   use raw USB bulk/accessory I/O and does not require a synthetic IP interface.

The selector itself must stay pure and deterministic. Provider discovery,
permission prompts, pairing UI, connection attempts, and transfer fallback
side effects live outside it.
