# Wi-Fi Aware as a nearby-activated iroh path

Status: **Nearby hybrid path passes scoped no-device gates; Apple physical gate awaiting approval**

Last reviewed: 2026-07-26

## 1. Decision

Wi-Fi Aware will be integrated as an iroh custom transport path, not as a
second transfer session selected before iroh.

Nearby Discovery controls whether the Wi-Fi Aware path is made available:

```text
non-Nearby transfer
    -> ordinary iroh endpoint
    -> iroh chooses IP direct or relay

user selects a peer in Nearby Discovery
    -> admit one unambiguous Wi-Fi Aware paired-device candidate
    -> when ready, add it to the same iroh endpoint as a custom path
    -> cryptographically bind it to the Room peer before payload
    -> iroh validates an ordinary IP backup on that connection
    -> iroh chooses and migrates paths for one QUIC connection
```

The rule is:

- only the user-initiated Nearby Discovery flow may activate a Wi-Fi Aware
  custom path;
- Manual, QR, Invite, standalone Room, and standalone mDNS transfers continue
  to create ordinary iroh endpoints without Wi-Fi Aware;
- when Nearby has no unambiguous paired Wi-Fi Aware device, the transfer
  proceeds through ordinary iroh; and
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

Room pairing supplies the authenticated peer `EndpointAddr` with ordinary
addresses. The sender verifies that its `EndpointId` equals the Wi-Fi Aware
bootstrap ID, then deliberately starts the QUIC connection with only the
custom address:

```text
EndpointAddr {
    id: expected_peer_id,
    addrs: [
        TransportAddr::Custom(wifi_aware_addr),
    ],
}
```

This prevents an ordinary path from winning the initial handshake before the
custom path is validated. Both endpoints still bind their ordinary IP
transports; once the custom connection exists, iroh's NAT traversal exchanges
the IP candidates and opens the backup on the same connection. The endpoint's
relay transport remains configured, but iroh 1.0.3 cannot safely inject the
Room relay address after this custom-first connection is bound. A prevalidated
relay backup is therefore not claimed by this milestone.

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

The Nearby hybrid endpoint does not use that default selector. Its construction
and small deterministic policy are described in section 9: the first hop is
custom-only, so an ordinary path cannot win the initial handshake race.

## 4. Implemented Envoix integration

The preserved custom-only diagnostic still runs iroh QUIC over an isolated
Wi-Fi Aware connected datagram channel:

```rust
Endpoint::builder(...)
    .clear_address_lookup()
    .clear_ip_transports()
    .clear_relay_transports()
    .add_custom_transport(wifi_aware)
```

The Nearby production path now differs intentionally:

1. `ENVXWA02` exchanges the endpoint IDs and datagram limits over the
   Apple-owned connected UDP channel;
2. one fresh endpoint secret owns Wi-Fi Aware custom, IP direct, and optional
   relay transports;
3. the receiver advertises its authenticated endpoint address through Room
   pairing;
4. the sender requires the Room peer ID to equal the Wi-Fi Aware bootstrap ID;
5. the sender dials the custom address first and iroh NAT traversal opens the
   ordinary IP backup;
6. one QUIC connection runs one SPAKE2 and Manifest v2 session; and
7. the custom bridge remains alive until the sender finishes or the receiver's
   pending offer completes its destination save.

The sender initially supplies only the custom address, so Wi-Fi Aware can be
selected immediately. iroh's NAT traversal then validates the ordinary IP
candidate on that same connection and retains it as backup.

Ordinary endpoint builders and non-Nearby callers remain unchanged.

## 5. Nearby activation boundary

Nearby Discovery decides only whether to offer iroh a Wi-Fi Aware path. It does
not select the final path.

A Wi-Fi Aware path may be added only when:

1. the local platform and signed application have the required Wi-Fi Aware
   capabilities, entitlement, and service declarations;
2. the user selected a concrete BLE peer from the current foreground Nearby
   generation;
3. the current Apple snapshot contains exactly one Wi-Fi Aware paired device;
4. the remote peer has an active compatible Nearby/Wi-Fi Aware context;
5. the connected UDP channel reports Wi-Fi Aware path metadata;
6. both endpoints exchange and verify the expected iroh endpoint IDs; and
7. the negotiated datagram capacity is at least 1,200 bytes.

BLE and discovery-only mDNS identities are untrusted and ephemeral. The
one-day integration therefore never guesses between paired devices and never
matches display names: zero or multiple paired Wi-Fi Aware devices omit the
custom path. With exactly one candidate, Room authentication still must bind
its endpoint ID to the bootstrap ID before dialing. A wrong candidate fails
closed rather than reaching payload.

This is deliberately narrower than a future peer-registry association. It is
safe for the current two-device test topology and does not claim that BLE and
Apple paired-device identifiers are intrinsically equivalent.

No Wi-Fi Aware pairing prompt, publisher, listener, or connection is created
for a non-Nearby entry point.

## 6. Endpoint construction

iroh 1.0.3 adds custom transports through the endpoint builder; it does not
expose a supported operation for adding a new custom transport factory to an
already-bound endpoint.

The first implementation therefore uses a per-session hybrid endpoint:

1. Nearby captures the only eligible Apple paired-device ID before its
   discovery generation stops;
2. Swift establishes and validates the connected Wi-Fi Aware UDP channel;
3. Rust exchanges endpoint IDs and MTU through `ENVXWA02`;
4. Rust builds one endpoint with IP, configured relay transport, and the custom
   transport;
5. Room pairing exchanges the receiver address and authenticates its endpoint
   ID;
6. the sender verifies Room/bootstrap identity equality and initially dials
   only the custom address; and
7. iroh NAT traversal validates the ordinary IP backup on that connection.

When the paired-device snapshot is absent or ambiguous, no Apple channel is
opened and the existing ordinary Room path runs. Once a unique candidate has
entered native setup, setup errors are currently explicit failures rather than
an uncoordinated one-sided fallback. A coordinated pre-admission timeout is a
follow-up reliability item; it must not retry after authentication or payload.

A long-lived custom-transport registry capable of attaching channels to an
already-bound shared endpoint would remove the setup wait, but it introduces
multi-peer routing and lifecycle complexity. It is deferred until the
per-session hybrid endpoint is proven.

## 7. Identity and bootstrap

`ENVXWA02` exchanges the fresh endpoint IDs and datagram limits before the
hybrid endpoints are bound. Those same secrets then own the custom, IP, and
optional relay transports.

The bootstrap rejects:

- a reflected local endpoint ID;
- a malformed version, role, or frame length;
- an invalid datagram limit; and
- frames that do not carry the expected bootstrap magic and role.

After Room pairing, the sender additionally requires the authenticated Room
peer ID to equal the bootstrapped Wi-Fi Aware peer ID. BLE display names are
never used as an identity check.

The custom address uses the private Envoix Wi-Fi Aware transport ID and is
associated with the same remote `EndpointId` later used by the validated IP
backup.

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

iroh remains the path-validation and migration engine. Envoix installs one
small `PreferWifiAwarePath` policy only on Nearby hybrid endpoints:

1. the authenticated sender initially dials only the already-ready Wi-Fi Aware
   custom address;
2. whenever a validated Wi-Fi Aware path is open, select it;
3. iroh NAT traversal independently validates an ordinary IP candidate on that
   same connection; and
4. if Wi-Fi Aware disappears, prefer an open IP or other direct path over a
   relay, then choose the lowest RTT within that class.

The custom-only first hop is necessary because an IP-first experiment confirmed
that iroh 1.0.3 cannot attach a still-unvalidated custom path after the QUIC
handshake has finished. The production construction removes that race instead
of relying on address order. The deterministic loopback regression starts
custom-only, observes selected Wi-Fi Aware plus the IP backup opened by NAT
traversal, cuts both custom bridges, and proves the same QUIC connection and
bidirectional stream continue over IP.

The policy does not claim to estimate bulk throughput, signal quality, energy,
or native queue pressure. Those inputs remain future optimization work. The
current correctness controls are:

- fixed 1,200-byte hybrid MTU;
- 2-second path keepalive and 6-second path idle timeout;
- actual path changes reported through the existing transfer event stream; and
- no provider-level second connection or payload retry.

Existing path policy is interpreted as follows:

| Policy | Paths exposed to iroh |
| --- | --- |
| Auto | Wi-Fi Aware first; IP direct validated by NAT traversal; relay transport configured but no prevalidated remote relay path |
| DirectOnly | Wi-Fi Aware first and IP direct via NAT traversal; no relay |
| RelayOnly | relay only; do not activate Wi-Fi Aware |

The activity and UI report the actual selected path, not merely the set of
available paths. If iroh migrates during a transfer, the diagnostic timeline
records the transition.

## 10. Failure behavior

There are two distinct stages:

### Before the hybrid endpoint exists

If the Nearby snapshot contains zero or multiple Wi-Fi Aware paired devices,
omit the custom path and run the existing ordinary Room transfer.

After one unique device has been staged and native setup begins, an unavailable
device, setup timeout, invalid MTU, or identity mismatch is currently an
explicit failure. The two peers do not independently guess whether to retry an
ordinary session; a coordinated pre-admission fallback remains follow-up work.

Do not continue after:

- user cancellation;
- paired-peer or endpoint-identity mismatch;
- authentication/downgrade evidence; or
- malformed peer input.

### After one iroh connection exists

If the selected custom path disappears, iroh should migrate to the already
validated IP path within the same QUIC connection. The Manifest job,
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

The clean worktree is `feat/wifi-aware`; the restored checkpoint is
`286018c`. Source from the previous
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

It proves custom-only Wi-Fi Aware in both Apple directions. Hybrid selection
and Wi-Fi-Aware-to-IP migration are proven in memory; Apple physical hybrid
selection and migration remain gated on explicit approval.

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

- build endpoints containing test custom and IP transports;
- use one endpoint identity, a custom-only first hop, and iroh NAT traversal
  for the IP backup;
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
  endpoint pair with deterministic custom and IPv4 loopback paths;
- Wi-Fi Aware is selected immediately, after which iroh NAT traversal opens and
  retains IP as backup;
- cutting both custom bridges preserves the original QUIC connection and
  bidirectional stream, then continues the second payload over IP;
- the Nearby hybrid 2-second / 6-second policy migrates in approximately
  6.2 seconds; and
- the deterministic regression passed five consecutive runs, followed by the
  complete session test suite.

Reverse IP-to-custom recovery, physical relay coexistence, and longer repeated
teardown remain reliability gates. They are not represented as completed
features.

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
- verify the Room peer ID, then use the custom address as the initial dial
  address so IP cannot win the first handshake race;
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

Status: complete locally. Ordinary and hybrid builders share the builder
implementation without changing ordinary transport configuration. Candidate
filtering preserves non-IP addresses, Room rejects endpoint identity mismatch,
and the hybrid connection learns its IP backup through iroh NAT traversal.

### H3 — Gate activation from Nearby Discovery

- make the Nearby orchestrator the only production caller allowed to supply a
  Wi-Fi Aware custom channel to the hybrid builder;
- never guess among multiple paired devices or use display-name matching;
- reject stale discovery generations and name-only matches;
- close late or unused native channels;
- leave Manual, QR, Invite, Room, and standalone mDNS callers unchanged; and
- expose path state through existing models without editing UI styling while
  the PR62 UI-drift work is active.

Verification:

- non-Nearby tests never construct or probe a Wi-Fi Aware transport;
- Nearby without peer-specific readiness builds an ordinary endpoint;
- Nearby with readiness builds a hybrid endpoint;
- a candidate whose bootstrap ID differs from the Room peer fails before
  payload;
- RelayOnly never starts Wi-Fi Aware.

Device use: none until the final H3 gate.

Status: complete locally for the two-device topology. Only the Nearby BLE flow
can stage a Wi-Fi Aware device ID. Zero or multiple Apple paired devices use
ordinary iroh; non-Nearby entry points never stage the route.

### H4 — No-device quality gate

Run serially through `scripts/with-build-cache-guard.sh`:

- `envoix-session` and `envoix-ffi` tests;
- mixed-path selection and migration tests;
- Nearby lifecycle/routing tests;
- strict Clippy and Rust formatting;
- one generic iOS `build-for-testing` through `scripts/apple-dev.sh`; and
- Android compile/unit compatibility only if shared Rust or FFI changed.

Do not start Simulator and do not use an Android device.

Status: passed on 2026-07-26:

- `envoix-session`: 27/27 tests;
- `envoix-ffi`: 3/3 tests;
- strict Clippy with `-D warnings`;
- Rust formatting and `git diff --check`;
- generic arm64 iOS app build; and
- generic arm64 iOS `build-for-testing`, including the hybrid physical-test
  entry point.

The test build emits one pre-existing Swift 6 actor-isolation warning in
`WifiAwareCapabilityTests`; the new Nearby hybrid sources compile without a
new warning.

No Simulator or Android device was started. The Android compatibility run was
deferred to avoid occupying the device and build branch already in use by the
separate Android work.

### H5 — Batched Apple physical gate

Prebuild once, install the same signed product on both devices, and use
`test-without-building`.

Required scenarios:

| Scenario | Expected result |
| --- | --- |
| Wi-Fi Aware custom-only diagnostic, both directions | selected path is Wi-Fi Aware |
| Nearby hybrid Auto, both directions | one iroh connection; selected path accurately reported |
| no unambiguous Wi-Fi Aware device in Nearby snapshot | ordinary iroh connection |
| Wi-Fi Aware custom path lost during payload | same QUIC connection migrates to validated IP |
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

Status: not started. The user requested that both-Apple testing wait for
explicit approval.

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

The additive UniFFI surface is advertised as API version 6 with capability
`wifi_aware_nearby_hybrid_v1`. Existing transport functions remain exported;
the new entry points are `send_transfer_job_v2_nearby_hybrid` and
`receive_transfer_offer_v2_nearby_hybrid`.

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
