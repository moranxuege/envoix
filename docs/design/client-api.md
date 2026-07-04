# Client API redesign

Status: draft for team review. Nothing here is implemented; this is the shape we
agree on before writing code.

Scope: `envoix-client` and its boundary with the CLI (and future mobile/desktop
frontends). Server-side (rendezvous/relay) logging and hardening are tracked
separately.

---

## 1. Problems in the current design

The client crate is not an API; it is six hand-wired workflows behind one
struct, with the real dispatch logic living in the CLI. Evidence, by problem:

### 1.1 O(modes x concerns) API growth

Two conceptual operations (send, receive) are exposed as 12 client methods
(`send_file`, `send`, `send_file_via_room`, `receive`,
`receive_file_with_bound_peer`, `receive_file_via_room`, each doubled with a
`_with_cancel` twin) plus 6 overlapping request structs. `envoix-session`
mirrors this with 16 public entry points. Adding the `--direct-only` flag
touched 5 files across 4 layers to thread one boolean. That diff shape is the
disease: every new mode or flag multiplies against every existing concern.

### 1.2 Mode dispatch lives in the CLI

`run()` in `apps/envoix-cli/src/main.rs` is a ~250-line if/else ladder that
picks the client method - and knows things only the API should know. Worst
case: `client_for_room` fabricates a fake pairing token
(`"envoix-room-unused-placeholder"`) because `ClientConfig` demands a
`PairingConfig` even in the mode that derives it from SPAKE2. A mobile app
would have to re-implement this ladder, placeholder hack included.

### 1.3 Cancellation as an afterthought parameter

The `_with_cancel` twins exist because cancellation was bolted on as an extra
argument instead of being part of the operation contract. It doubles the
surface at two layers, and it still failed its purpose: Ctrl-C during room
pairing hung because the token must be manually threaded through every phase
and one phase forgot. A cross-cutting concern enforced by convention will be
dropped; one owned by the operation runner cannot be.

### 1.4 Three-and-a-half event channels, none complete

- `EventSink`/`TransferEvent`: transfer progress only.
- `ClientEventSink`/`ClientEvent`: lifecycle - but 4 of its 7 variants
  (`DialStarted`, `Authenticated`, `ConnectionFailed`, `TooManyAuthFailures`)
  are never emitted anywhere. Dead vocabulary.
- `on_bound: FnOnce(PeerDescriptor)`: a third ad-hoc channel just for "here is
  your address". It is a generic parameter, so the method is not object-safe -
  this one signature breaks UniFFI/JNI binding.
- tracing as a data channel: the `data path: direct/relay` line - the product's
  most important diagnostic - is only observable via a tracing subscriber. A
  GUI cannot render it. Room pairing progress is not evented at all (the CLI
  prints "pairing in room via ..." before calling the client).

A UI needs one stream that tells the story of a transfer. None of these does.

### 1.5 Errors are strings wearing category hats

`PublicError = CoreError`, eight variants each wrapping a bare `String`. No
phase (pairing vs dial vs transfer vs finalize), no retriability, no code.
"transport error: connection lost" during pairing was undiagnosable by design.
Mobile needs error codes for localization and retry policy.

### 1.6 Configuration smeared across two planes

`ClientConfig` holds chunk/pairing/identity; relay, relay_only, direct_only and
listen_addrs live per-request, duplicated field-for-field on both room request
structs; `session_config()` hardcodes `relay: None` and each room method
mutates it back in. No single place states "the effective configuration of
this transfer".

### 1.7 No transfer handle

Every operation is one await-to-completion future. No transfer id until
completion, no state queries, no way to list active transfers (mobile needs
this), cancellation via external token, progress via callbacks. The CLI's
`run_interruptible` select-gymnastics exist only because the future is the
whole interface.

### Root cause

Accretion under a locally-correct rule. Every feature (mdns, invite, room,
cancel, relay, relay-only, direct-only) was added as the smallest surgical
diff: a parallel method, a twin, one more threaded parameter. Each diff was
individually reasonable; the sum is combinatorial. Nothing forced
consolidation because the API had exactly one co-owned consumer. Mobile is the
first real client, which is why the pain surfaces now.

---

## 2. The domain model

> A transfer = direction + file + peer-source + transport policy, observed as
> an event stream, controlled by a handle.

Three axes that the current design conflates, which are actually independent:

1. **Rendezvous** - how the two peers find each other and authenticate:
   manual descriptor + token, QR invite, mDNS, room code via broker.
2. **Connection establishment** - who dials whom, over which candidate paths.
3. **Data direction** - who sends file bytes (the operation the user asked
   for).

### 2.1 Dial direction is not send direction (symmetry analysis)

Today the sender always dials and the receiver always listens; in room mode
the sender literally uploads a placeholder address
(`room.rs`: "the sender only dials"). This coupling is incidental, not
essential. Decoupling matters:

- **Reachability**: if only the *sender* side is directly reachable (receiver
  behind hard NAT, no relay), reversed establishment succeeds where the
  current fixed roles fail. We have already observed direction-dependent
  hole-punch success (CN->US direct worked; US->CN failed) - a reverse-dial
  fallback converts those from failures into transfers.
- **Workflows**: mobile share-sheet is sender-initiated end to end; "fetch
  from my desktop" is receiver-initiated. Both are natural once establishment
  is decoupled from direction.

What is **genuinely asymmetric** (must NOT be papered over):

| Concern | Asymmetric to | Notes |
|---|---|---|
| Wire protocol roles (Hello/FileHeader/chunks -> CompleteAck) | data direction | fine: negotiated after connect, independent of who dialed |
| Resume state (partial file + sidecar) | data direction | lives with the receiver of the bytes |
| Filesystem (read path vs writable output dir, overwrite policy, disk space) | data direction | unavoidable |
| Close ordering (receiver sends last frame, sender closes) | data direction | already direction-based, not dialer-based |
| Who must display an address | rendezvous method | manual mode only; room/mdns/invite already exchange addresses |
| SPAKE2 pairing roles (initiator/responder) | arrival order | the broker already assigns these independent of direction - proof the crypto does not care |

**The control/consent question** ("do they need to negotiate each other's
allowed connections?"): connection-level authorization is *already symmetric*
- both sides prove knowledge of the code/token via SPAKE2; nobody accepts a
connection they did not opt into. What symmetric establishment adds is an
*application-level* consent gap: today the receiver implicitly accepts
whatever single file the authenticated sender pushes. The fix is an explicit
**Offer/Accept handshake** after connect+auth: the sending side issues
`Offer{file name, size, hash?}`, the other side responds Accept/Reject.

- CLI one-shot mode auto-accepts (UX unchanged).
- Mobile renders it as the confirmation dialog it will need anyway.
- It also becomes the natural extension point for multi-file sessions and
  pull-mode ("receiver requests, sender offers").
- Wire change - gated behind `protocol_version` (Hello already carries one).

**Recommendation**: keep "sender dials" as the default; exchange *both*
endpoint addresses through the broker (stop sending the placeholder); add
reverse-dial as a fallback when the primary dial fails. Do NOT race both
directions initially - iroh does not deduplicate two simultaneous connections
between the same pair, so racing needs an app-level tiebreak; fallback gives
most of the value at a fraction of the complexity.

Caveat: reverse-dial requires a *bidirectional* address exchange, which only
Room provides. Manual and Invite carry the listener's address one way by
nature (a printed descriptor / QR has no back-channel), so they stay
single-direction dial. That is a property of the rendezvous method, not a
defect; the design must simply not make establishment *depend* on the
exchange being bidirectional.

### 2.2 Either side can produce the invite (QR / printed descriptor)

Today the invite and manual flows force the **receiver** to go first: the
listener is the QR/descriptor producer, and the listener is hardwired to be
the receiver. Only the second half of that is incidental. What a QR or pasted
descriptor fundamentally identifies is *one listening endpoint plus a secret*:

> Whoever **shows** it listens. Whoever **scans** it dials.
> Which of them **sends the file** is a separate negotiation.

This is not a new feature - it is the section 2.1 decoupling (dial direction
vs data direction) surfacing in the UX. One protocol capability, two payoffs:
reverse-dial fallback (automatic) and either-side-QR (user-chosen).

| Producer (shows QR, listens) | Consumer (scans, dials) | Status | UX |
|---|---|---|---|
| receiver | sender pushes | today | "prepare to receive" must happen first |
| sender | receiver pulls | new | share-sheet flow: pick file -> show QR -> peer scans - the natural mobile flow |

The sender-produced QR is the AirDrop-shaped flow mobile will want; today's
invite forces it backwards.

What has to change (all shared with reverse-dial - implement once):

1. **Dialer speaks first, declaring its data role.** Today the protocol
   assumes dialer = file sender (`expect_sender_hello` is the receiver's
   first act). New rule: the dialer opens the stream and sends
   `Hello{role: <its data role>}`; the acceptor runs the complementary state
   machine. Gated by `protocol_version`.
2. **Auth transcript role binding.** SPAKE2 currently binds
   `SENDER_IDENTITY`/`RECEIVER_IDENTITY` - data-direction identities - into
   the transcript (`envoix-auth`: `start_a` as sender). With flexible roles,
   bind SPAKE2 initiator/responder to **connection roles** (dialer/acceptor),
   which are unambiguous before any application frame flows.
3. **Invite payload carries the producer's data role**, so a scan can detect
   mismatch (two senders / two receivers) at scan time - and mobile can
   auto-open the right mode from any envoix QR. The payload's reserved
   `flags: u32` (currently must be 0, already version-checked) is the
   extension point.
4. **Optional relay candidates in the invite payload.** A phone on
   cellular/CGNAT showing a QR may be un-dialable directly; embedding relay
   URLs as *additional candidates* keeps the flow working - still broker-free,
   still I1/6.3-compliant (candidates, never a requirement).

Consent is unchanged in strength: scanning is consent to the flow the QR
declares, the token still gates the connection, and Offer/Accept still runs
before bytes move.

Persisting physical asymmetry (do not design it away): the producer must be
*reachable* - it is the listener. On an airgapped LAN that is automatic; a
cellular sender showing a QR needs relay candidates (item 4) or a shared LAN.
That is physics, not API shape.

---

## 3. Target API

```rust
// WHO to talk to - one enum, replaces 4 modes x 2 directions of methods.
// Every variant is valid for BOTH send() and receive() (section 2.2):
// consumer variants dial, producer variants listen; data direction is
// negotiated after connect (Offer/Accept), independent of who dialed.
pub enum PeerSource {
    // consumer side: I have the peer's address - I dial.
    Manual { peer: PeerDescriptor, token: String },   // pasted descriptor
    Invite { invite: String },                        // scanned QR
    // producer side: I advertise - the peer dials me.
    ShowManual { token: Option<String> },             // print descriptor (+token)
    ShowInvite { ttl_secs: u64 },                     // generate + show QR
    Mdns { token: Option<String> },                   // LAN advertise/discover
    // brokered: both sides meet at the rendezvous server.
    Room { code: String, broker: String },
}

// HOW to connect - one struct, replaces the smeared flags. See section 5.
pub struct TransportPolicy {
    pub relays: Vec<String>,                 // 0..n relay URLs (see 4)
    pub paths: PathPolicy,                   // allow-set + preference (see 5)
}

impl EnvoixClient {
    pub fn send(&self, file: PathBuf, to: PeerSource, opts: TransferOptions)
        -> Result<Transfer, TransferError>;
    pub fn receive(&self, into: PathBuf, from: PeerSource, opts: TransferOptions)
        -> Result<Transfer, TransferError>;
}

pub struct Transfer;                          // the handle
impl Transfer {
    pub fn id(&self) -> TransferId;
    pub fn events(&self) -> EventReceiver;    // channel; adapter for FFI callbacks
    pub fn cancel(&self);                     // no more token threading
    pub async fn wait(self) -> Result<TransferSummary, TransferError>;
}
```

- **One event enum, the full story**: `Binding -> Advertised{peer, invite?} ->
  Pairing{step} -> Connecting -> Connected{path} -> PathChanged{path} ->
  OfferReceived{meta} -> Progress -> Verifying -> Completed | Failed`.
  Absorbs `EventSink` + `ClientEventSink` + `on_bound` + the data-path tracing
  hack (the live path poller feeds `PathChanged`; the CLI log line becomes a
  renderer of the event).
- **Structured error**: `TransferError { phase: Phase, kind: ErrorKind,
  message: String }`. The runner that owns the operation knows the phase, so
  attaching it there is cheap; attaching it later is impossible.
- **Cancellation owned by the handle**: the runner creates the token
  internally; every phase it drives observes it. `run_interruptible` shrinks
  to "on Ctrl-C, `transfer.cancel()`".
- **`TransferOptions` is `#[non_exhaustive]` + `Default`**: new capabilities
  are new defaulted fields, never sibling methods.
- **Layering becomes real**: apps depend only on `envoix-client`; the client
  stops re-exporting session types; session's 16 entry points collapse behind
  it and go internal.

Future-proofing (deliberately NOT built now, but the shapes must not preclude
them):

- **Session object**: multi-file/folder transfer means N transfers over one
  connection. `Transfer` stays; a future `Session` owns the connection and
  hands out `Transfer` children. Event stream already carries `transfer_id`
  everywhere, so this is additive.
- **Reattach**: mobile backgrounding needs "reconstruct UI state from events +
  ids". Structured events with ids make transfer state replayable.
- **Trusted devices**: persistent identity exists; a future TOFU
  pin/allowlist of peer EndpointIds is a single `PeerAuthz` policy hook at the
  accept gate, same place for CLI and mobile.

### 3.1 UniFFI binding constraints (binding checklist)

Every public `envoix-client` signature must satisfy, from day one:

- No generics, no `impl Trait`, no closures, no lifetimes in public signatures.
- Callbacks only as boxed object-safe traits with concrete argument types
  (UniFFI "callback interface"); prefer the channel + optional
  `set_listener(Box<dyn TransferListener>)` adapter pattern.
- Handles (`EnvoixClient`, `Transfer`) as `Arc`-based objects; `Send + Sync`.
- Enums with named-field variants are fine; keep field types primitive or
  API-owned (no iroh types in the surface - `PeerDescriptor` string forms,
  paths as `String` at the boundary if needed).
- Errors as fielded enums (fits `TransferError{phase, kind, message}`).
- Async: UniFFI supports async fns, but keep the event stream as the primary
  progress channel; `wait()` is the only long await.

The current `receive<F: FnOnce(PeerDescriptor)>` is the canonical violation.

---

## 4. Multiple relays

Today: `relay: Option<String>` -> `RelayMode::Custom(RelayMap::from(url))` -
exactly zero or one. Wanted: official relay + self-hosted relay(s)
simultaneously.

- iroh's `RelayMap` natively holds several relays; the endpoint probes and
  picks a home relay (latency-based). Change is mechanical:
  `relays: Vec<String>` -> `RelayMap` from all of them.
- A peer does NOT need our relay in its own config to reach us: our advertised
  `EndpointAddr` carries our relay home URL and the peer dials it directly.
  So mixed configs interoperate; multiple relays mostly buy better home
  selection + redundancy.
- CN2-style tuning wants to *pin* the home relay rather than trust the latency
  probe (lowest RTT is not best throughput). Express pinning through the
  preference grid (section 5) - `relay:<url>` cells are ordered - instead of a
  separate "home relay" knob.
- VERIFY before design freeze: exact iroh 1.0 semantics of multi-relay
  `RelayMap` (home selection policy, whether non-home relays are used for
  punching), and whether home can be pinned explicitly.

---

## 5. Path policy: allow-set + preference grid

### 5.1 What exists today

There is **no preference setting**. The three existing flags are all *hard
filters* that shrink the candidate set:

- `--relay-only`: bind no IP transport (direct impossible).
- `--direct-only`: strip the relay from the data endpoint (relay impossible).
- `--ip-version v4|v6`: receiver binds/advertises one family only.

Actual selection among surviving candidates is iroh's: prefer a punched direct
path, pick by latency. **Latency-based selection is bandwidth-blind** - it
cannot see that a low-ping direct path is thin and the relay is fat. That is
the gap the user story describes.

### 5.2 The grid

Candidate paths form a product: `{direct} U {relay_i}` x `{v4, v6}`. A cell is
one combination ("direct-v4", "relay:cn2.example.com-v6", ...). Two
independent axis toggles **cannot** express real orderings - e.g. the CN2
case `direct-v4 > relay-v6 > direct-v6 > relay-v4` is not expressible by any
(path-major or family-major) combination of "prefer direct/relay" +
"prefer v4/v6". Preference must be a total order over cells. So:

```toml
# envoix.toml - power-user tier, never a CLI flag
[transport]
path_preference = ["direct-v4", "relay-v6", "direct-v6", "relay-v4"]
# entries may wildcard: "direct", "relay", "relay:cn2.example.com-*"
```

```rust
pub struct PathPolicy {
    pub allow: CellSet,          // default: all - subsumes relay_only /
                                 // direct_only / ip_version as one concept
    pub prefer: Vec<CellPattern>, // default: empty = iroh auto
}
```

Note an honest subtlety: for a relay cell, the family (`-v4`/`-v6`) describes
*our leg to the relay*; the far peer's relay leg is their choice, invisible to
us. Document this so grid entries are not oversold.

### 5.3 Enforcement, staged by what is actually implementable

- **Stage 1 - allow-set (trivial now)**: every cell filter composes from
  primitives we already ship (`clear_ip_transports`, relay-less data endpoint,
  v4/v6 bind addrs). `relay_only`/`direct_only`/`ip_version` become sugar for
  allow-sets; one concept replaces three flags.
- **Stage 2 - ordered preference via resume-based failover**: we cannot make
  iroh prefer a slower-RTT path, but we CAN run the transfer constrained to
  the top preference cells, measure achieved throughput (BBR gives a live
  bandwidth estimate; or simple bytes/sec over a probe window), and on breach
  of a `min_throughput` option abort + reconnect constrained to the next
  cells. **Idempotent resume makes this cheap and safe** - the retry continues
  from the delivered offset. This is the "agent prefers relay when direct is
  thin" behavior, without touching iroh internals.
- **Stage 3 - native multipath preference (research)**: iroh 1.0 has
  multipath; VERIFY whether it exposes per-path priorities/weights. If yes,
  Stage 2's reconnect loop collapses into a config knob.

### 5.4 Tiered exposure ("configurable but not usually exposed")

The answer to "exposing the grid is annoying for users" is progressive
disclosure - three tiers with strict precedence (flag > env > file > default;
the `from_runtime_sources` machinery for exactly this already exists):

| Tier | Surface | Audience | Contents |
|---|---|---|---|
| 0 | nothing | everyone | `auto` (iroh default behavior) |
| 1 | CLI flags / one mobile setting | occasional intervention | `--prefer direct\|relay\|ipv4\|ipv6` - four presets, each expanding to a canonical grid order; plus the existing hard filters |
| 2 | config file + env only | power users / us testing | full `path_preference` grid, `min_throughput`, relay list order |
| 3 | compiled-in | maintainers | default table |

Mobile mirrors this: a single "Connection preference: Auto / Prefer direct /
Prefer relay" picker (tier 1), and a hidden developer screen or config import
for tier 2. The grid never appears in primary UI.

---

## 6. Server-independence invariant (offline-first)

Manual, Invite, and mDNS transfers must keep working when the broker - or the
entire server fleet - is unreachable. Today this holds *accidentally*: each
mode is a separate hand-wired code path, so manual mode cannot call the broker
because its code literally contains no broker logic. The unified runner
removes that accidental protection: shared stages (endpoint construction,
advertising, reconnect) could silently couple every mode to server-related
behavior. So the guarantee must be stated and enforced, not assumed.

### 6.1 Dependency matrix (what each mode may require)

| PeerSource | Broker | Relay | DNS | Internet | Works on airgapped LAN |
|---|---|---|---|---|---|
| Manual | never | optional candidates only | no (raw IPs) | no (LAN addrs) | yes |
| Invite | never | never today; future: optional embedded relay candidates (2.2 item 4) - candidates only, never required | no | no | yes |
| Mdns | never | never by default (see 6.3) | no | no | yes |
| Room | required for pairing | optional (broker reachability + data fallback) | only for relay URLs | yes | no |

Also verified in the current code and to be preserved: the iroh `presets::N0`
defaults are overridden (`relay_mode()` replaces the relay map;
`clear_address_lookup()` removes public discovery), so there is **no hidden
dependency on n0's public infrastructure**. Any future use of a preset must
re-verify this.

### 6.2 The invariants

- **I1 - serverless modes are serverless**: `Manual`, `Invite`, `Mdns`
  complete transfers with zero configured or reachable servers (no broker, no
  relay, no DNS, no internet), given a viable direct path.
- **I2 - graceful degradation**: a *configured but unreachable* server never
  fails, and never meaningfully delays, a transfer that has a viable
  serverless path. Concrete hazard: `ready_endpoint_addr` waits up to 5s
  (100 x 50ms) for the relay home before advertising - correct for Room,
  but if that stage were shared naively, every LAN transfer with a relay in
  the config file would gain 5 seconds. Server-related waits must be scoped
  to the modes that need them and always bounded + skippable.
- **I3 - reconnects never re-enter rendezvous**: after first establishment,
  the session holds the peer's `EndpointAddr` and derived credentials, so
  resume/failover reconnects (path-preference Stage 2) dial directly. A
  broker that dies mid-transfer must not break the retry loop.
- **I4 - failures carry their phase**: broker-unreachable surfaces as
  `Phase::Rendezvous`, relay problems as their own kind - so a UI can say
  "rendezvous server unreachable" instead of "connection lost".

### 6.3 Per-PeerSource transport defaults

Global config (e.g. `relays` in `envoix.toml`) must not silently change the
character of serverless modes:

- `Mdns`: no relay by default. Advertising a relay home in a LAN-discovery
  mode would extend reachability beyond the LAN behind the user's back
  (token auth still gates, but the exposure change should be opt-in).
- `Manual` / `Invite`: relays, if configured, join as *additional candidates*
  only - never awaited, never required.
- `Room`: relays as configured (broker reachability + data fallback).

### 6.4 Enforcement

A CI integration test per serverless mode that runs a loopback/LAN transfer
with the broker and relay configured to a **black-holed address** (e.g.
`203.0.113.1:1` - unroutable TEST-NET) and asserts (a) the transfer succeeds
and (b) wall-clock time is within a small factor of the no-server baseline.
This converts I1/I2 from documentation into a regression gate.

---

## 7. Migration plan (staged, never red)

1. New surface beside the old in `envoix-client` (new types + `Transfer`
   handle), implemented by delegating to existing session functions; adapters
   turn old sink callbacks into the unified event stream.
   Verify: unit tests mapping options -> session calls.
2. Port the CLI mode-by-mode (manual first, room last).
   Verify: loopback transfer per mode + existing arg-parse tests.
3. Fill event gaps: emit `Pairing`, `Connected`, `PathChanged` (path poller
   feeds events); delete the 4 dead `ClientEvent` variants and the
   unimplemented `detect_network_environment`.
4. Delete old client methods; collapse session's `_with_cancel` twins; session
   goes internal.
5. `TransferError` at the boundary (map `CoreError` -> phase/kind in the
   runner).

Then, as separate efforts once the surface is stable: Offer/Accept handshake +
reverse-dial fallback (wire version bump), multiple relays, PathPolicy stages.

Sequencing: own branch off `dev` after `feat/speed-and-logging` merges.
Orthogonal to the rendezvous/server logging work - can proceed in parallel.
Steps 1-2 are the 20% that buys 80% (fixes the binary<->client mess, unblocks
mobile); 3-5 can trail. Time-box the rest.

---

## 8. Prevention rules (proposed for CLAUDE.md / docs/arch.md)

1. **One entry point per user operation.** A new capability is a new defaulted
   field on a `#[non_exhaustive]` options struct - never a sibling method. If
   you are naming something `foo_with_x`, `x` belongs in the contract.
2. **Apps depend only on `envoix-client`.** Enforced mechanically: CI check
   that `apps/*/Cargo.toml` never names `envoix-session`/`envoix-transfer`;
   client does not re-export internals.
3. **Everything user-visible is an event.** If a UI might display it (address,
   invite, pairing step, chosen path), it must be on the event stream.
   `println!`/`tracing` are renderers and debuggers, never the transport of
   product information.
4. **No bare-string errors at the public boundary** - phase + kind required.
5. **The N-layer-thread test**: if adding one flag touches more than two
   layers, stop and file a design note instead of threading it.
6. **FFI checklist on any `envoix-client` API change**: no generics, no
   closures, object-safe, serializable - "could Swift express this
   signature?".
7. **Serverless modes stay serverless** (section 6): any change to shared
   runner stages must keep the black-hole CI tests green; a new server-side
   dependency in Manual/Invite/Mdns is a design regression, not a tradeoff.

---

## 9. Open items to verify before design freeze

- iroh multi-relay `RelayMap` semantics: home-selection policy, pinning,
  non-home relay use during punching.
- iroh multipath: any exposed per-path preference/priority API (Stage 3).
- iroh behavior on simultaneous bidirectional connect between the same pair
  (needed if reverse-dial fallback ever becomes a race).
- Offer/Accept + role negotiation: exact `protocol_version` gating story and
  backward compatibility window; includes the dialer-speaks-first change and
  rebinding SPAKE2 roles from data-direction identities to connection roles
  (2.2 items 1-2) - a transcript-compatibility break, so it must ride the
  same version bump.
- iroh behavior when a configured relay is unreachable: confirm endpoint
  bring-up and direct connects proceed without added blocking (needed to make
  I2's "additional candidates only" defaults safe).
- Campaign finding (2026-07-04): stripping the relay from the data endpoint
  also strips hole-punch signaling + QAD, so a NAT'd peer cannot reach a
  direct-only receiver at all - and stateful IPv6 firewalls make this true on
  v6 too (no NAT does not mean no filter; punching survived the death of NAT).
  Redefine DirectOnly as selected-path policy: keep the relay configured for
  signaling/QAD, fail unless the selected path becomes direct within a grace
  window (post-connect check on the paths() API).
- Address observation should not be relay-coupled: (a) relay QAD works today;
  (b) preferred: a tiny broker "observe" ALPN - the DATA endpoint dials it and
  the broker echoes the observed transport address; correct by construction
  because all connections of an iroh endpoint share one socket, so the
  observed mapping is the data socket's. ~60 lines total, no iroh changes.
  Notes: broker learns the data endpoint id (privacy trade-off), symmetric
  NAT unhelped (inherent to any third-party observation). Observation alone
  does NOT give connectivity (2026-07-04 runs 9/10: single-sided dials die at
  stateful gateways even with correct addresses) - it must pair with
  simultaneous mutual dial triggered by pairing completion; together they
  replace the relay for cone-NAT / stateful-v6 cases. (c) literal STUN is out:
  iroh owns the data socket and exposes no foreign-datagram API; a separate
  socket observes the wrong mapping.
- Path-migration reasons are invisible: selection is liveness-driven inside
  iroh; BBR is active but its bandwidth estimate is not read out anywhere.
  Enrich PathChanged with the old path's last-known stats if paths() exposes
  them; campaign cells can capture RUST_LOG=iroh=debug for ground truth.
