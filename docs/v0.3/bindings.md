# v0.3 typed binding contract

Status: normative for M5 and later platform migrations.

## Version negotiation

The native library exposes two independent versions:

- UniFFI API `19` identifies the complete native symbol/type surface;
- application binding `1` projects application contract `6`.

Callers must check both `envoixCoreInfo()` and
`envoixApplicationBindingInfo()` before opening a long-lived Engine handle.
An unsupported version fails closed; a frontend must not guess field or state
semantics.

## Control boundary

`FfiApplicationEngine` owns one ordered application snapshot. It accepts typed
event envelopes, returns immutable typed snapshots, and decides typed commands
against that snapshot. The returned effect is the only work a live platform
adapter may execute. Replaying a snapshot or event never executes an effect.

The binding intentionally uses sorted record arrays instead of foreign maps so
Swift and Kotlin receive identical ordering. Bulk file bytes, endpoint details,
relationship credentials, invitation material, and verification values are
absent from snapshots and events. Invitation and verification values appear
only in the immediate command/effect pair that must consume them.

Foreign transport and inbox ports use `shutdown()` for their asynchronous
protocol operation. Generated object-handle disposal remains `close()`; the two
names must stay distinct because Kotlin objects implement `AutoCloseable`.

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
Kotlin from it, and verifies that Command/Event/Snapshot types exist in both
outputs. It also rejects an asynchronous zero-argument `close()` before UniFFI
can emit an uncompilable Kotlin overload. Generated source is build output and
is not checked into the repository; Apple packaging and Android staging
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

All production Android Manifest v2 sends now restore and explicitly seal the
canonical job, open the session, observe typed progress/failure/path/timing
facts, and cancel through UniFFI. This covers remembered credentials and both
sides of a one-time InviteV2. Android publishes delivery only after the native
send future returns, rather than treating the earlier observer callback as
proof of completion. Job and cancellation handles have explicit owners and are
closed on success, failure, and service shutdown; Kotlin contract tests cover
request projection, failure policy fields, terminal-event deferral, and both
handle lifetimes.

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

Transfer-invitation deep-link routing is typed and no longer crosses the legacy
JSON JNI parser. Sender-side transfer-invitation generation and role-bound
parsing now use the same typed registry as the UniFFI send session. Receiver
generation and parsing temporarily remain on JNI with the receiver session;
they must move together with the typed platform destination/result gate so the
core cannot acknowledge delivery before SAF or MediaStore has durably saved
the payload.

All Android native entry points are compiled into `libenvoix_ffi.so`; the
`android-jni` Cargo feature adds the exceptional JNI symbols to the same
library that owns UniFFI handles and process-local credentials. The former
`libenvoix_jni.so` and its second runtime/state registry no longer exist.

The remaining hand-written JNI surface is not an accepted final M5 exception.
It currently contains live Transfer-session orchestration, discovery callbacks,
Android context initialization, log routing, and synchronous platform content
callbacks. At M5 exit, only Android-runtime integration that cannot be
expressed as a UniFFI port may remain, and every such entry must have an
explicit rationale here.
