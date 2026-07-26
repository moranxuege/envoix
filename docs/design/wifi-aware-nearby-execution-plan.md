# Wi-Fi Aware as a nearby-activated iroh path

Status: **approved architecture; implementation pending**

Last reviewed: 2026-07-25

## 1. Decision

Wi-Fi Aware will be integrated as an iroh custom transport path, not as a
second transfer session selected before iroh.

Nearby Discovery controls whether the Wi-Fi Aware path is made available:

```text
non-Nearby transfer
    -> ordinary iroh endpoint
    -> iroh chooses IP direct or relay

user selects a peer in Nearby Discovery
    -> resolve an exact peer-specific Wi-Fi Aware channel
    -> when ready, add it to the same iroh endpoint as a custom path
    -> also retain ordinary IP and relay paths
    -> iroh chooses and migrates paths for one QUIC connection
```

The rule is:

- only the user-initiated Nearby Discovery flow may activate a Wi-Fi Aware
  custom path;
- Manual, QR, Invite, Room, and standalone mDNS transfers continue to create
  ordinary iroh endpoints without Wi-Fi Aware;
- when Nearby cannot establish an eligible Wi-Fi Aware channel before the
  endpoint is built, the transfer proceeds through ordinary iroh; and
- once a hybrid endpoint exists, Envoix does not run a second provider-level
  fallback state machine. iroh owns path selection and migration.

Wi-Fi Aware may still be reported as `DataPath::WifiAware` when its custom path
is selected. This describes the physical path, not a separate protocol stack.

## 2. What “fully integrated into iroh” means

Both data paths use one endpoint identity, one QUIC connection, one
authentication exchange, one Manifest v2 session, and one user-visible
activity:

```text
                              +-> Apple Wi-Fi Aware connected UDP
                              |      -> iroh custom transport
peer EndpointAddr -> iroh QUIC+-> ordinary IP direct
                              +-> relay
                                      |
                                      v
                           SPAKE2 -> Manifest v2
```

The remote `EndpointAddr` contains the same `EndpointId` with all usable
transport addresses:

```text
EndpointAddr {
    id: expected_peer_id,
    addrs: [
        TransportAddr::Custom(wifi_aware_addr),
        TransportAddr::Ip(...),
        TransportAddr::Relay(...),
    ],
}
```

The existing iroh address lookup remains enabled for ordinary direct/relay
addresses. The Wi-Fi Aware custom address is added only for the selected
Nearby peer.

Swift must still use Apple's WiFiAware and Network.framework APIs to:

- present or consume the Apple system pairing context;
- create the publisher/subscriber role;
- establish the connected UDP channel; and
- retain and close the native connection lifetime.

iroh cannot discover or open Apple's opaque `WAEndpoint` by itself. The native
channel adapter makes that Apple-owned path visible to iroh. iroh then owns
QUIC packets, path validation, migration, and congestion control while applying
the configured path-selection policy.

## 3. Verified iroh capability

The repository is locked to iroh 1.0.3 with
`unstable-custom-transports`. Its local source confirms:

- `Builder::add_custom_transport` can coexist with IP and relay transports;
- one `EndpointAddr` can contain custom, IP, and relay addresses;
- the default `BiasedRttPathSelector` treats custom and IP paths as primary,
  compares them by RTT, and treats relay as backup;
- the selected path is sticky until another same-tier path is at least 5 ms
  better; and
- upstream tests cover custom-over-IP, IP-over-custom, and
  custom-over-relay selection.

The custom transport and custom path-selector APIs are explicitly unstable and
not protected by iroh semantic-versioning guarantees. The pinned iroh version
and focused compatibility tests are therefore part of the release gate.

The default selector is deliberately simple, not a complete bulk-transfer
optimizer. It considers path tier and RTT with a 5 ms switching hysteresis. It
does not directly consider application goodput, Wi-Fi Aware signal strength,
energy cost, native queue pressure, endpoint lifecycle, or the peer-specific
MTU negotiated before QUIC. `PathStats` exposes RTT, congestion window,
cumulative loss, black-hole count, and current MTU, but a production policy
must interpret those values and their deltas explicitly.

## 4. Current Envoix gap

The preserved Apple-to-Apple implementation already runs iroh QUIC over a
Wi-Fi Aware connected datagram channel, but it is isolated:

```rust
Endpoint::builder(...)
    .clear_address_lookup()
    .clear_ip_transports()
    .clear_relay_transports()
    .add_custom_transport(wifi_aware)
```

It also generates a fresh endpoint secret for that custom-only endpoint. This
proves the Wi-Fi Aware path and Manifest v2 data plane, but it prevents iroh
from seeing alternative IP or relay paths.

Full integration requires:

1. using the same session identity for custom, IP, and relay paths;
2. retaining ordinary transports and address lookup;
3. adding the Wi-Fi Aware custom address to the same remote `EndpointAddr`;
4. using one QUIC connection and observing its selected path; and
5. making custom-path lifetime changes visible without terminating the whole
   endpoint while another path remains usable.

## 5. Nearby activation boundary

Nearby Discovery decides only whether to offer iroh a Wi-Fi Aware path. It does
not select the final path.

A Wi-Fi Aware path may be added only when:

1. the local platform and signed application have the required Wi-Fi Aware
   capabilities, entitlement, and service declarations;
2. the user selected a concrete peer from the current foreground Nearby
   generation;
3. that selection resolves to the intended `WAPairedDevice` without
   display-name substring matching;
4. the remote peer has an active compatible Nearby/Wi-Fi Aware context;
5. the connected UDP channel reports Wi-Fi Aware path metadata;
6. both endpoints exchange and verify the expected iroh endpoint IDs; and
7. the negotiated datagram capacity is at least 1,200 bytes.

BLE and discovery-only mDNS identities are untrusted and ephemeral. They are
not silently treated as a cryptographic binding to a paired Apple device. If
the exact association cannot be established, the custom address is omitted and
iroh continues with its ordinary paths.

No Wi-Fi Aware pairing prompt, publisher, listener, or connection is created
for a non-Nearby entry point.

## 6. Endpoint construction

iroh 1.0.3 adds custom transports through the endpoint builder; it does not
expose a supported operation for adding a new custom transport factory to an
already-bound endpoint.

The first implementation therefore uses a per-session hybrid endpoint:

1. start ordinary iroh address preparation;
2. concurrently attempt peer-specific Wi-Fi Aware enrichment within a bounded
   setup deadline;
3. if the Apple channel becomes ready, exchange expected endpoint IDs and MTU;
4. build one endpoint with IP, relay, address lookup, and the custom transport;
5. connect with an `EndpointAddr` containing all known addresses; or
6. if enrichment is unavailable by the deadline, build the unchanged ordinary
   iroh endpoint.

A late Wi-Fi Aware result after endpoint construction is closed and ignored for
that transfer. It may be used by the next Nearby transfer. This prevents a
stale Nearby generation from mutating an active session.

A long-lived custom-transport registry capable of attaching channels to an
already-bound shared endpoint would remove the setup wait, but it introduces
multi-peer routing and lifecycle complexity. It is deferred until the
per-session hybrid endpoint is proven.

## 7. Identity and bootstrap

The current `ENVXWA02` bootstrap remains useful, but it must exchange the
endpoint IDs selected by the ordinary iroh session rather than generating a
separate Wi-Fi Aware identity.

The bootstrap must reject:

- a peer ID that differs from the selected invitation/paired-peer context;
- a reflected local endpoint ID;
- a malformed version, role, or frame length;
- an invalid datagram limit; and
- a stale response from another Nearby generation.

The custom address uses the private Envoix Wi-Fi Aware transport ID and is
associated with the same remote `EndpointId` used by IP and relay.

QUIC authenticates path migration within the same connection. SPAKE2 and
Manifest v2 run once above it; changing the selected physical path does not
restart authentication or create another transfer.

## 8. MTU rule for a hybrid connection

The proven Wi-Fi Aware endpoints reported asymmetric limits of 1,402 and 1,452
bytes. The custom-only implementation negotiates 1,402 and disables MTU
discovery to prevent oversized QUIC probes.

A hybrid QUIC connection may migrate to an IP or relay path with a different
MTU. The Wi-Fi Aware-specific 1,402-byte value must not be assumed safe for
every alternative path.

The correctness-first hybrid implementation will:

- require the Wi-Fi Aware path to support at least 1,200 bytes;
- use a fixed 1,200-byte QUIC datagram size for a hybrid connection;
- disable probes above that common safe size; and
- preserve the existing iroh MTU behavior for ordinary, non-hybrid endpoints.

This may reduce hybrid-path throughput compared with the 1,402-byte
custom-only evidence. Per-path MTU optimization is a later performance
milestone and must not precede migration correctness.

## 9. Guarded iroh path policy

The default selector is a reference implementation and test oracle, not the
unqualified production policy. iroh remains the path-selection and migration
engine, while Envoix divides its guard into two parts:

1. `GuardedNearbyPathAdmission` decides whether the peer-bound Wi-Fi Aware
   custom address may be exposed to iroh at all.
2. `GuardedNearbyPathSelector`, installed through iroh's `PathSelector`
   interface, makes deterministic choices among paths that iroh has already
   validated.

This split is required by the pinned iroh 1.0.3 API. `PathSelector::select` is
invoked on path lifecycle events, not on a periodic sampling schedule. It
therefore cannot truthfully implement a continuous goodput/no-progress
controller by itself. The admission guard owns the healthy observation window,
peer binding, negotiated MTU, native queue-pressure checks, and cooldown before
the custom address enters the endpoint.

Once admitted:

- relay remains a backup while a validated primary path exists;
- the current path remains sticky across short RTT jitter;
- selector decisions use only the current lifecycle snapshot, never pretend
  cumulative counters are interval samples;
- hybrid QUIC uses a 2-second per-path keepalive and 6-second per-path idle
  timeout, giving three failed observations before iroh abandons the path and
  invokes selection again;
- MTU is fixed at 1,200 bytes across every hybrid path; and
- every observed switch records the old path, new path, health evidence, and
  reason.

The first selector version does not claim to infer bulk throughput from RTT.
H1 and H5 establish measured thresholds as named constants; no unexplained
magic values are introduced. If the available iroh statistics cannot support a
stable decision, the selector falls back to deterministic priority with health
gating rather than pretending to estimate capacity.

Nearby activates Wi-Fi Aware eligibility. It does not guarantee that Wi-Fi
Aware wins when another healthy primary path is measurably preferable. Any
future policy that always prefers Wi-Fi Aware is a separate, evidence-backed
product decision.

Existing path policy is interpreted as follows:

| Policy | Paths exposed to iroh |
| --- | --- |
| Auto | Wi-Fi Aware when Nearby-ready, IP direct, and relay |
| DirectOnly | Wi-Fi Aware when Nearby-ready and IP direct; no relay |
| RelayOnly | relay only; do not activate Wi-Fi Aware |

The activity and UI report the actual selected path, not merely the set of
available paths. If iroh migrates during a transfer, the diagnostic timeline
records the transition.

## 10. Failure behavior

There are two distinct stages:

### Before the hybrid endpoint exists

If Wi-Fi Aware is unsupported, unpaired, unavailable, times out, or has an
invalid MTU, omit its custom address and build the ordinary iroh endpoint. This
is path enrichment failure, not a second transfer attempt.

Do not continue after:

- user cancellation;
- paired-peer or endpoint-identity mismatch;
- authentication/downgrade evidence; or
- malformed peer input.

### After one iroh connection exists

If the selected custom path disappears, iroh should validate and migrate to
another IP or relay path within the same QUIC connection. The Manifest job,
SPAKE2 session, stream, progress, and activity remain the same.

If no alternative path is usable before QUIC's connection deadline, return the
existing structured transport failure. Do not create a second connection or
restart the payload silently.

### Endpoint identity lifetime

The earlier relay failure `Another endpoint connected with the same endpoint
id` was caused by endpoint lifetime overlap: an old connection clone retained
the relay actor while a resumed endpoint reused the same identity. It was not
an RTT-ranking failure, but a hybrid endpoint would reintroduce it if identity
ownership were careless.

Each endpoint identity therefore has one explicit lease. All connection clones,
path watchers, custom-transport bridge tasks, and native channel scopes must be
dropped or joined before the endpoint closes and before that identity can be
reused. Existing policy remains unchanged: static receivers may retain their
activity-scoped identity, while dialers and Room peers use ephemeral identities.

## 11. Recovery baseline

The clean worktree is `feat/wifi-aware` at `acd36fe`. Source from the previous
detached working directory is preserved at:

```text
/private/tmp/envoix-wifi-aware-orphan-20260725-0746
```

It contains the Apple connected-UDP adapter, custom iroh transport, bootstrap
v2, negotiated MTU fix, asymmetric MTU regression, and 256 MiB physical-test
harness. Recovery must restore only relevant source changes; generated
bindings and caches are regenerated.

Persistent physical evidence is stored at:

```text
/Users/moranxuege/SJTU_JI/2026SU/ECE4410J/WiFiAwareEvidence/2026-07-24
```

It proves custom-only Wi-Fi Aware in both Apple directions. Hybrid path
selection and migration remain unproven until the gates below pass.

## 12. Execution phases

### H0 — Restore the proven custom path

- restore the orphaned Rust, UniFFI, Swift, tests, lockfile, and design changes
  surgically;
- regenerate bindings;
- reproduce the 1,402/1,452-byte regression locally; and
- create a local checkpoint before changing endpoint architecture.

Verification:

- targeted session and FFI tests;
- strict Clippy;
- Rust formatting;
- `git diff --check`; and
- no physical device use.

### H1 — Prove mixed iroh paths in memory

- build endpoints containing test custom, IP, and optional relay transports;
- use one endpoint identity and one mixed `EndpointAddr`;
- verify the default selector can choose custom or IP according to RTT;
- model admission and selection separately against jitter, cumulative-counter
  rollover, black-hole, MTU, loss, cooldown, and stale-stat sequences;
- fail the selected custom path and prove the same QUIC connection continues
  over IP;
- fail IP and prove the same connection can continue over custom;
- verify custom beats relay while a usable primary path exists; and
- verify hybrid packets never exceed the fixed 1,200-byte limit;
- repeatedly close and recreate endpoints while asserting that an identity is
  never reused before all old owners exit; and
- inject a custom-path receive/send failure while IP remains healthy and prove
  that the custom error does not terminate the whole endpoint.

Current H1 evidence:

- `mixed_endpoint_migrates_from_wifi_aware_to_ip_on_same_connection` builds one
  endpoint pair with simultaneous custom and IP paths;
- cutting both custom bridges preserves the original QUIC connection and
  bidirectional stream, then continues the second payload over IP;
- iroh's stock 5-second keepalive / 15-second path-idle policy migrated in
  15.21 seconds; and
- the Nearby hybrid 2-second / 6-second policy migrated in 6.23 seconds.

Reverse IP-to-custom migration, relay coexistence, repeated identity teardown,
and fault/counter model coverage remain open H1 gates.

This phase is the go/no-go gate for full integration. If same-connection
migration cannot be made reliable with the pinned iroh API, retain the proven
custom-only provider and revisit the architecture rather than hiding a second
connection behind “migration.”

Device use: none.

### H2 — Build the per-session hybrid endpoint

- refactor the Wi-Fi Aware binder to accept the session identity;
- remove `clear_ip_transports()` and `clear_relay_transports()` only for the
  hybrid builder;
- retain existing address lookup and relay configuration;
- merge the custom address into the expected peer `EndpointAddr`;
- keep the ordinary endpoint builder byte-for-byte behaviorally compatible
  when no Wi-Fi Aware channel is supplied; and
- preserve cancellation and bounded datagram queues;
- install the guarded selector only for Nearby hybrid endpoints; and
- make endpoint identity leases own connection clones, path watchers, bridge
  tasks, and native scopes through ordered shutdown.

Verification:

- ordinary iroh regression tests pass unchanged;
- hybrid tests report one connection and multiple paths;
- one SPAKE2/Manifest session survives a custom-to-IP migration;
- no duplicate activity or destination publication appears;
- the relay never observes overlapping use of the same endpoint ID; and
- the 1,402/1,452-byte Wi-Fi Aware mismatch cannot black-hole authentication.

Device use: none.

### H3 — Gate activation from Nearby Discovery

- make the Nearby orchestrator the only production caller allowed to supply a
  Wi-Fi Aware custom channel to the hybrid builder;
- resolve the exact selected paired device;
- reject stale discovery generations and name-only matches;
- close late or unused native channels;
- leave Manual, QR, Invite, Room, and standalone mDNS callers unchanged; and
- expose path state through existing models without editing UI styling while
  the PR62 UI-drift work is active.

Verification:

- non-Nearby tests never construct or probe a Wi-Fi Aware transport;
- Nearby without peer-specific readiness builds an ordinary endpoint;
- Nearby with readiness builds a hybrid endpoint;
- selecting peer A cannot attach peer B's custom channel;
- RelayOnly never starts Wi-Fi Aware.

Device use: none until the final H3 gate.

### H4 — No-device quality gate

Run serially through `scripts/with-build-cache-guard.sh`:

- `envoix-session` and `envoix-ffi` tests;
- mixed-path selection and migration tests;
- Nearby lifecycle/routing tests;
- strict Clippy and Rust formatting;
- one generic iOS `build-for-testing` through `scripts/apple-dev.sh`; and
- Android compile/unit compatibility only if shared Rust or FFI changed.

Do not start Simulator and do not use an Android device.

### H5 — Batched Apple physical gate

Prebuild once, install the same signed product on both devices, and use
`test-without-building`.

Required scenarios:

| Scenario | Expected result |
| --- | --- |
| Wi-Fi Aware custom-only diagnostic, both directions | selected path is Wi-Fi Aware |
| Nearby hybrid Auto, both directions | one iroh connection; selected path accurately reported |
| Wi-Fi Aware disabled before endpoint build | ordinary iroh connection |
| Wi-Fi Aware custom path lost during payload | same QUIC connection migrates to IP or relay |
| ordinary IP path lost while Wi-Fi Aware remains | same connection continues on Wi-Fi Aware |
| Manual / QR / Invite / Room while paired | no Wi-Fi Aware custom path exists |
| endpoint-ID or pairing mismatch | fail without ordinary fallback |

Payload gates:

- 8 MiB smoke in both directions;
- 256 MiB hash-verified transfer in both directions;
- one 1 GiB transfer in each direction;
- cancellation before and during payload; and
- a successful subsequent transfer without rebooting or re-pairing.

Use the iPad through wireless CoreDevice where possible so the Android device
and its USB port remain available to the other branch.

### H6 — Reliability and performance

- run 30 consecutive 8 MiB transfers per direction;
- run ten controlled 256 MiB samples per direction;
- record path availability, selection, validation, and migration timestamps;
- report p10, median, and p95 payload goodput;
- verify resident memory does not grow across repeated transfers; and
- compare fixed 1,200-byte hybrid performance with the 1,402-byte custom-only
  baseline before considering MTU optimization.

The current custom-only single-run results, approximately 192.8 Mbit/s from
iPhone to iPad and 156.8 Mbit/s in reverse, are a baseline rather than a hybrid
guarantee.

### H7 — UI and merge integration

After the separate PR62 UI-drift work is available:

- merge or rebase it before editing shared Nearby views;
- show Wi-Fi Aware as available only for the selected peer;
- show the live path actually selected by iroh;
- record custom/IP/relay path transitions without creating new activities;
- do not add a global Wi-Fi Aware toggle; and
- preserve existing non-Nearby presentation.

Finish with a generic build, focused routing/migration tests, and one short
two-direction physical regression.

## 13. API compatibility

Do not remove existing transport-provider or `DataPath` variants while
recovering the branch. Product orchestration will stop treating Wi-Fi Aware as
a separate session provider, but compatibility wrappers remain until all
callers and tests are migrated and a separate API review confirms removal is
safe.

Existing ordinary iroh APIs and behavior must remain unchanged when no custom
channel is supplied.

## 14. Commit sequence

1. `fix(wifi-aware): restore negotiated custom transport`
2. `test(iroh): prove mixed custom path selection and migration`
3. `feat(iroh): add per-session wifi-aware hybrid path`
4. `feat(nearby): activate wifi-aware path for selected peers`
5. `docs(wifi-aware): record hybrid path evidence`

Each commit passes its focused tests. No remote push or pull request is
included without a final diff review.

## 15. Completion criteria

The architecture is complete only when:

- non-Nearby transfers remain ordinary iroh and never activate Apple
  Wi-Fi Aware APIs;
- a selected Nearby peer can contribute an exact, peer-specific custom path;
- custom, IP, and relay paths belong to one endpoint identity and one QUIC
  connection;
- iroh executes selection and migration through the guarded policy, without an
  Envoix second-connection retry loop;
- custom-path loss can continue on an already validated alternate path without
  restarting SPAKE2, Manifest, or the activity;
- hybrid MTU handling cannot emit a packet larger than the custom path accepts;
- endpoint identities cannot overlap across relay actors, connection clones,
  path watchers, or custom bridge tasks;
- both Apple directions pass hash-verified large transfers and induced
  migrations;
- the UI and logs report the actual selected path;
- source, tests, design, and persistent evidence agree; and
- no Android device or Simulator is used for work that can be verified without
  them.
