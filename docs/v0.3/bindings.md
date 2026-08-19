# v0.3 typed binding contract

Status: normative for M5 and later platform migrations.

## Version negotiation

The native library exposes two independent versions:

- UniFFI API `18` identifies the complete native symbol/type surface;
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
Invitation deep-link routing is typed and no longer crosses the legacy JSON
JNI parser. Invitation generation and role-bound parsing remain temporarily on
the legacy bridge because their opaque references still use its session
registry; moving only the producer would create references that the active
transfer entry point cannot resolve. The producer and consumer now share one
binary and can be migrated together in the next typed-session slice.

All Android native entry points are compiled into `libenvoix_ffi.so`; the
`android-jni` Cargo feature adds the exceptional JNI symbols to the same
library that owns UniFFI handles and process-local credentials. The former
`libenvoix_jni.so` and its second runtime/state registry no longer exist.

The remaining hand-written JNI surface is not an accepted final M5 exception.
It currently contains Room/Transfer JSON orchestration, discovery callbacks,
Android context initialization, log routing, and synchronous platform content
callbacks. At M5 exit, only Android-runtime integration that cannot be
expressed as a UniFFI port may remain, and every such entry must have an
explicit rationale here.
