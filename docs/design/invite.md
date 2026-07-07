# Invite — one advertisement, many transports

Status: design (agreed 2026-07-07). Implements the "receiver advertises a bundle
of reachability methods; the sender consumes it" model, replacing the current
fragmented per-mode flows. Scope of the first cut: the shared `Invite` type +
payload format, populated with the **room** method only; direct / mDNS are
additive later. Auth unifies on the pairing code (SPAKE2) across all transports.

## 1. Why

Today the CLI has three *separate* pairing flows — manual/token, mDNS/invite,
room — each its own `PeerSource`, each with its own auth. The app only exposes
room. There is no single "here is how to reach me" object, so there is nothing
coherent to put in a QR, and adding a transport means touching every layer.

An **Invite** is the receiver's single reachability advertisement. It bundles
every method the receiver has enabled; the sender consumes it and lets the best
path win. This maps almost 1:1 onto iroh's `EndpointAddr` (id + relay + direct
addrs), which is already a multi-path bundle — we extend it with a rendezvous
hint and the pairing code.

## 2. The concept

```
Invite {
    code:   String,              // the pairing code, e.g. 144055-cobalt-flint
                                 //   - the SPAKE2 password (authenticates ANY transport)
                                 //   - its digit prefix is the broker room id (public)
    room:   Option<RoomHint>,    // { broker }         rendezvous via a broker
    relay:  Option<String>,      // relay URL          WAN/NAT reachability (iroh)
    direct: Vec<SocketAddr>,     // (future) dial these directly
    mdns:   Option<String>,      // (future) LAN service name to discover
    node:   Option<EndpointId>,  // (future) iroh id for direct/mDNS dialing
}
```

First cut populates `code` + `room` + `relay`. `direct` / `mdns` / `node` are
present in the type and the payload grammar from day one (so adding them later is
additive, never a reparse), but left empty until their transports land.

## 3. Auth — one code, any transport (decision #1)

Pairing already happens at the app layer over the *established* connection
(`authenticate_{sender,receiver}` run on a `FrameConnection`), so it is
transport-agnostic. We therefore standardise on **SPAKE2 with `code` as the
password**, regardless of which transport connected. The bearer-token model
(manual/mDNS today) is retired for the invite flow.

Consequence: the QR/code is a single-use pairing secret shown to the intended
peer (wormhole model). A wrong code fails SPAKE2 — no data flows, no wrong pair.
Room mode already works this way, so **the first cut needs no auth change**; the
unification only bites when direct/mDNS are added (they move from token → SPAKE2).

## 4. Payload format (decision #3: an `envoix://` URL)

Human-inspectable, deep-linkable on phones, and trivially forward-compatible
(unknown query params are ignored). One method → one param:

```
envoix://pair/<code>?broker=<id@ip:port>&relay=<url>&direct=<addr>&direct=<addr>&mdns=<name>&node=<id>
```

- `<code>` — the pairing code; the broker room id is `code.split('-').next()`.
- `broker`, `relay` — present when the room method is enabled (first cut).
- `direct` (repeatable), `mdns`, `node` — reserved now, populated later.

Two rendering targets from one payload:
- **QR** encodes the whole URL (all enabled methods). ~300–400 B for a fat
  bundle → a comfortable mid-density QR.
- **Typed code** is just `<code>` (the only typeable part). The typed path has no
  broker in it, so it falls back to the peer's configured/default broker+relay —
  exactly today's behaviour.

## 5. API surface (core: `envoix-client::api::invite`)

```rust
impl Invite {
    /// Receiver side: build a room invite, generating a fresh code.
    pub fn room(broker: String, relay: Option<String>) -> Self;
    // future, additive: fn with_direct(..), fn with_mdns(..), fn with_endpoint(..)

    pub fn code(&self) -> &str;              // typeable code (for display / entry)
    pub fn payload(&self) -> String;         // envoix:// URL (for the QR)
    pub fn parse(input: &str) -> Result<Self, TransferError>; // typed code OR envoix:// URL

    /// The `PeerSource` this invite drives (both sides use Room for now).
    pub fn peer_source(&self) -> PeerSource;
}
```

- **Receiver:** `let inv = Invite::room(broker, relay); show(inv.code(), inv.payload()); client.receive(dir, inv.peer_source(), opts)`.
- **Sender:** `let inv = Invite::parse(scanned_or_typed)?; client.send(file, inv.peer_source(), opts)`.

`parse` accepts either a bare code (`144055-cobalt-flint`) or a full
`envoix://…` URL, so "typed" and "scanned" converge on one code path.

## 6. Layering — one home, thin platforms

```
core (envoix-client::api::invite)   Invite: build · payload() · code() · parse()   ← the ONLY home
        │  used by
   ┌────┴─────┐
  CLI         app (JNI)
  print code  Generate button → show code + render QR(payload)
  + ASCII QR  scanner → Invite::parse(scanned)
```

QR **rendering** and camera **scanning** are per-platform UI (an ASCII QR crate
for the CLI; a QR bitmap + ML Kit/ZXing on Android). They only ever touch the
payload *string* and `parse` — never the bundle internals. Generation and the
payload grammar exist in exactly one place.

Settings select which methods `Invite::room/with_*` includes; nothing else in the
stack knows about "modes".

## 7. Phased plan

- **P1 — shared type (this cut).** `Invite` + `envoix://` encode/parse (room
  method), code generation moved behind `envoix-client::api` (CLI stops reaching
  into `envoix-rendezvous-iroh`). CLI: `--room` with no value → `Invite::room` →
  print code (+ ASCII QR). Unit tests: round-trip payload, parse(code) ==
  parse(url) for the room subset, reject malformed.
- **P2 — app UX.** JNI `generateInvite()` / `parseInvite()`; New-transfer sheet
  gets a **Generate** (show code + QR) and a **Scan** path. No engine change.
- **P3 — direct + relay in the bundle (additive).** Populate `direct`/`node`
  from the receiver's bound endpoint; sender tries them before the broker; auth
  becomes SPAKE2 on whichever connects. Payload/type unchanged in shape.
- **P4 — mDNS hint (additive).** Same shape, one more param.

## 8. Non-goals / open questions

- **Not** re-touching the manual `--token`/`--peer` flow now; it stays until P3
  folds address-based dialing into the invite.
- Payload signing/expiry: out of scope; the code is single-use and SPAKE2 gates
  everything. Revisit if invites get persisted/shared out-of-band.
- Multi-file / multi-use invites: out of scope (one invite = one pairing).
