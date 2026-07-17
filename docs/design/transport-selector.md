# Transport provider selector

Status: **foundation implemented; iroh is the only registered adapter**

Last reviewed: 2026-07-17

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
| Wi-Fi Aware | `implementation_pending` | Never selected; native capability diagnostics remain visible only in developer mode |
| wired | `implementation_pending` | Reserved for a future raw wired provider; never treated as an iroh/IP path |

All four client construction paths (single-file send/receive and Manifest
send/negotiated receive) now select a provider before delegating to their iroh
adapter. The default is `Automatic`, so existing callers and durable records
continue to use iroh without changing behavior. Records serialized before the
new field deserialize to `Automatic`.

## Next integration slices

1. W1/W2 physical evidence establishes Wi-Fi Aware pairing and a secure native
   byte channel.
2. Register the Wi-Fi Aware adapter only after it can produce the existing
   authenticated `FrameConnection` semantics, including channel binding,
   cancellation, full-duplex control frames, and close ordering.
3. Add peer-specific Wi-Fi Aware candidates to the same selector; do not add a
   parallel transfer state machine.
4. Add a wired adapter later by registering another candidate. It may use raw
   USB bulk/accessory I/O and does not require a synthetic IP interface.

The selector itself must stay pure and deterministic. Provider discovery,
permission prompts, pairing UI, connection attempts, and transfer fallback
side effects live outside it.
