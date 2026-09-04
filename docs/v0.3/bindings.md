# v0.3 typed binding contract

Status: normative for M5 and later platform migrations.

## Version negotiation

The native library exposes two independent versions:

- UniFFI API `25` identifies the complete native symbol/type surface;
- application binding `1` projects application contract `6`.

Callers must check both `envoixCoreInfo()` and
`envoixApplicationBindingInfo()` before opening a long-lived Engine handle.
An unsupported version fails closed; a frontend must not guess field or state
semantics.

API 22 introduced dedicated trusted boundaries for credential delivery and
durable credential storage; API 23 added the desktop Agent host/control
projection, API 24 added Agent-owned pairing, and API 25 adds shared deployment
defaults plus authenticated Relationship route migration.
`FfiRememberedCredentialVault` is the only Room/Transfer-session callback that
receives a newly paired or rotated opaque credential. `FfiRoomControlSnapshot`
and the general `TransferObserver` contain no credential bytes. Room pairing
invokes `storePairingCredential(vault:)`; authenticated transfer entry points
receive the vault separately from their progress observer. A platform vault
adapter must store the value immediately and must not project, retain, or log
it.

`FfiApplicationEngine.openPersistent(stateDirectory:vault:)` acquires the
single durable Engine owner for a host state directory. Relationship labels,
endpoints, generations, revocation state, and bounded vault references live in
Engine schema v2. `FfiApplicationVault` stores only opaque credential bytes by
those non-secret references. Credentials may return only from the explicit
trusted `loadRelationship` call used to enter an authenticated operation; they
never appear in application snapshots, events, commands, effects, or
diagnostics. A second owner, legacy state, unavailable vault, interaction
requirement, permission denial, and corrupt vault data are distinct typed
errors; cancellation remains distinct from invalid input.

## Control boundary

`FfiAgentHost` is the sole desktop owner of the durable Engine, injected
`FfiApplicationVault`, and owner-only local control endpoint. Its lifecycle is
typed as starting, ready, stopping, stopped, or failed; callers must await
readiness and await explicit `shutdown()` before assuming the Engine lock or
endpoint has been released. Host failures have stable categories, including
single-owner, persistent-state, and vault failures. Linux, macOS, and Windows
may start a host; mobile targets expose the same binding surface but fail with
`UnsupportedPlatform`.

`FfiAgentControlClient` projects every Agent command and response as typed
UniFFI enums and records and rejects an incompatible Agent protocol version.
The bounded JSON envelope remains an implementation detail of the owner-only
Rust IPC transport. Snapshots, events, status, diagnostics, and lifecycle
records contain no credential or invitation material. Pairing has two explicit
exceptions: the creator response carries ephemeral room and verification codes
for display, while the Agent-owned join request carries a bounded invitation,
label, and one-time code into the durable owner. Both redact authentication
factors from Rust debug output and must not be persisted or logged. The join
response returns only a non-secret device summary; credential bytes never cross
the control boundary.

`envoixDeploymentEndpoints()` is the only platform binding source for the
compiled broker and relay defaults. Agent protocol v12 projects
`UpdateDeviceRoute` so an existing Relationship can move to a new broker and
relay without exposing, replacing, or re-pairing its credential. The Agent
rejects route changes while that Relationship owns active transfer work.

`FfiApplicationEngine` owns one ordered application snapshot. Its no-argument
constructor is limited to contract tests and transient previews; product hosts
open the persistent constructor. It accepts typed event envelopes, returns
immutable typed snapshots, and decides typed commands against that snapshot.
The returned effect is the only work a live platform adapter may execute.
Replaying a snapshot or event never executes an effect.

The binding intentionally uses sorted record arrays instead of foreign maps so
Swift and Kotlin receive identical ordering. Bulk file bytes, endpoint details,
relationship credentials, invitation material, and verification values are
absent from snapshots and events. Invitation and verification values appear
only in the immediate command/effect pair that must consume them.

Foreign transport and inbox ports use `shutdown()` for their asynchronous
protocol operation. Generated object-handle disposal remains `close()`; the two
names must stay distinct because Kotlin objects implement `AutoCloseable`.

Foreign callbacks are not assumed to run on a UI thread. Apple observers hop
to `MainActor` before touching observable application state and reject events
from stale operation identities. Android callback targets are thread-safe and
may be invoked concurrently; they never mutate Compose state directly. Tests
exercise both contracts so a generated-binding runtime change cannot silently
introduce UI-thread violations. A vault callback performs storage only and must
not re-enter the application Engine that invoked it.

`ManifestV2PlatformDestination` is the typed exception to Rust-owned local
filesystem output. It freezes public root names before Accept and asynchronously
commits platform-owned roots before receiver results or delivery proof. The
port exchanges bounded records only; invitation material and file bytes never
cross it. A successful platform completion returns final root names and URIs,
while private verified staging paths remain internal.

## State and recovery ownership

- `envoix-client` alone decides valid transitions, retryability, recovery
  action, cancellation outcome, and terminal state.
- Swift and Kotlin adapters may translate typed values for presentation but
  must not parse diagnostic text or maintain fallback tables.
- Authenticated Room operations expose `Rejected`, `NetworkLost`, `Canceled`,
  and `Failed` errors. Adapters use the variant, never `reason`, to decide
  whether a Room remains usable.
- A duplicate event is idempotent. A sequence gap is a typed `EventGap` error
  and requires a fresh Engine snapshot; callers must not skip the missing fact.
- An invalid identifier or command value is rejected before it reaches the
  reducer.

## Generated binding gate

Run:

```bash
scripts/check-generated-bindings.sh
```

The script builds one metadata-bearing native library, generates Swift and
Kotlin from it, and verifies that Command/Event/Snapshot types and the
persistent Engine/Relationship/vault surface exist in both outputs. It also
rejects an asynchronous zero-argument `close()` before UniFFI can emit an
uncompilable Kotlin overload. The gate rejects credential fields on application
or Room snapshots and on Transfer observers. Generated source is build output
and is not checked into the repository; Apple packaging and Android staging
generate from the same crate.

## Android migration ledger

Android application orchestration uses the generated UniFFI API by default.
Room invitation parsing, connection, authenticated commands, event delivery,
and cancellation use typed UniFFI records and errors; the former Room JSON JNI
bridge has been removed. Command calls remain serialized by the Android
adapter, and an offer response returns only after Rust has written it.

Manifest v2 job creation, provider-source preparation, source decisions,
reauthorization, cancellation, and sealing also use typed UniFFI objects. The
platform adapter retains ContentResolver access and reports structured provider
issues without rebuilding a Rust snapshot from JSON. Each short-lived job
handle is closed after the operation, including failure paths, and sealing is
idempotent so a caller can safely retry after losing the first response. The
seven superseded job-preparation JNI calls have been removed.

Android Room and Transfer adapters register protected remembered credentials
through the typed UniFFI function. Only the opaque process reference leaves
that trusted call; the duplicate byte-array JNI registration entry has been
removed.

New and rotated credentials travel through a separate
`FfiRememberedCredentialVault` adapter implemented by the Android Keystore
owner. They are absent from general session observers and Room snapshots, so a
presentation or progress adapter cannot accidentally acquire credential
material.

Android remembered Relationships now use one persistent
`FfiApplicationEngine` handle. The former Kotlin-owned
`relationships-v1.json` metadata and generation-indexed credential files are
not read by the v0.3 runtime. Engine schema v2 owns the non-secret record, while
an `FfiApplicationVault` adapter AES-GCM wraps the credential with a
non-exportable Android Keystore key under `noBackupFilesDir`. Rotation replaces
one referenced credential atomically, and the Engine coordinates state-write
rollback for rotation and revoke failures. Missing or modified ciphertext
fails closed without removing the Relationship. Existing Android v1 files are
retained but not imported, so upgraded test installations must pair again;
received files are untouched.

All production Android Manifest v2 sends now restore and explicitly seal the
canonical job, open the session, observe typed progress/failure/path/timing
facts, and cancel through UniFFI. This covers remembered credentials and both
sides of a one-time InviteV2. Android publishes delivery only after the native
send future returns, rather than treating the earlier observer callback as
proof of completion. Job and cancellation handles have explicit owners and are
closed on success, failure, and service shutdown; Kotlin contract tests cover
request projection, failure policy fields, terminal-event deferral, and both
handle lifetimes.

All production Android Manifest v2 receives now open an authenticated typed
offer, project its bounded inventory directly from UniFFI, and wait on a typed
destination decision. The Android destination adapter freezes public names,
copies verified roots through SAF or MediaStore, and returns committed names
and URIs before Rust can publish receiver results or delivery proof. Pending
offer, cancellation, and destination-decision lifetimes are explicit and are
closed on success, failure, cancellation, service shutdown, and the race where
an offer arrives after its Activity attempt was removed. Kotlin contract tests
cover request roles and remembered generations, bounded offer projection,
integer overflow, deferred completion, and handle ownership.

The destination adapter keeps the generated request and reply records typed
throughout the live call. JSON is used only for its versioned crash-recovery
journal; there is no string request API or typed-to-JSON-to-typed orchestration
loop in the Android process.

The persistent Room outbox deliberately negotiates a fresh one-time InviteV2
for each accepted data-plane Transfer, so that main product path still uses the
invitation session rather than a remembered credential. Its sender-side invite
production and consumption now share the typed UniFFI registry. Activity cards
store only the shared six-digit transfer locator returned by Rust; a complete
InviteV2 never becomes repository or diagnostic identity. This migration does
not weaken the control-plane/data-plane credential separation.

The Swift concurrency adapter projects the same Room error variants. A rejected
authenticated command leaves the current Room usable, while network loss,
cancellation, and native failure follow terminal paths without inspecting the
diagnostic message.

Apple uses the same split callback contract. Only the dedicated vault adapters
may call `RememberPersistenceContext`; Swift Room snapshots and transfer
observers remain secret-free. This keeps Keychain access tied to pairing or
credential rotation rather than presentation updates or reconnect polling.

Transfer-invitation generation, deep-link routing, and role-bound parsing are
typed for both sender and receiver and no longer cross the legacy JSON JNI
parser. Both directions use Rust's secret-free six-digit transfer locator for
Activity identity; complete InviteV2 material remains only in the immediate
connection flow.

All Android native entry points are compiled into `libenvoix_ffi.so`; the
`android-jni` Cargo feature adds the exceptional context-bootstrap symbol to
the same library that owns UniFFI handles and process-local credentials. The
former `libenvoix_jni.so` and its second runtime/state registry no longer exist.

The direct physical-test driver now exercises the same typed UniFFI send and
receive gateways as the product instead of a parallel JSON JNI session API.
The uncalled Wi-Fi Aware diagnostic transfer mode and its legacy
session/list/continue/cancel symbols have been deleted. The Android Wi-Fi Aware
socket adapter implements the typed `FfiNativeDuplexTransport` port; the
capability probe remains Android-owned, and later product integration must use
the typed native-transport session functions rather than restore a second
orchestration surface.

Android and Apple Nearby invitation discovery now share
`FfiNearbyInviteInbox`, including typed endpoint routes, incoming offers,
delivery completion, shutdown, and UniFFI handle ownership. Android retains
mDNS/TXT advertising and lifecycle coordination in Kotlin, but the duplicate
JNI session registry, request correlation, and JSON callbacks have been
deleted.

Core trace and structured timeline routing now use the typed `FfiLogSink`
callback. Runtime filtering is the typed `setLogLevel` function, and
`typed_log_sink_v1` advertises the capability. The prior `GetMethodID` callback,
Java `Long` cast, and two logging JNI symbols have been removed without changing
the envelope grammar or per-transfer routing policy.

Android context initialization is the sole hand-written JNI exception. It is a
required platform bootstrap: `ndk-context` must receive the process `JavaVM`
and application object before Rust networking touches Android DNS, interfaces,
or trust-store services. It is called once at process startup, exposes no
application command or secret, and cannot be represented by a UniFFI value.
