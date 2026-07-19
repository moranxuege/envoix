# Single-source-of-truth audit (2026-07)

Status: AUDIT — findings + fix ownership. Triggered by a scattered chunk-size
setting (`envoix-ffi` hardcoded `1024*1024` while the core default is `64*1024`).
A repo-wide sweep (endpoints/ports, sizes/timeouts/caps, cross-language
constants) found more of the same class: one conceptual value defined in several
places, kept in sync by hand, silently drift-prone.

## Ownership key (see the team model)

- **ours** — user + QizhenSun maintain (Rust core + Android). Fixable here.
- **flag** — lives in moranxuege's Apple/FFI (`crates/envoix-ffi`,
  `apps/envoix-apple`); do NOT edit, raise it for him.
- **fixed** — resolved in the 2026-07 window-setting change.
- **inert** — real inconsistency, but no input reaches the divergent branch today.

---

## 0. Headline — chunk-size divergence is a live interop bug (flag)

Not a smell, not UI copy: an actual "Apple can't transfer with Android/CLI" bug.

- Canonical `DEFAULT_CHUNK_SIZE = 64*1024` — `crates/envoix-transfer/src/lib.rs:25`.
  The client (`api.rs:135`), Android (empty setting → core default), and CLI all
  funnel to it.
- `crates/envoix-ffi/src/lib.rs:35` `GUI_CHUNK_SIZE = 1024*1024`, applied at
  `:304` (`client.chunk_size = GUI_CHUNK_SIZE`). The Apple app uses 1 MiB.
- The receiver **hard-rejects a mismatch**: `validate_header`
  (`crates/envoix-transfer/src/lib.rs:1523`) returns an error at `:1530` when
  `header.chunk_size != receiver_chunk_size`.

Consequence: Apple(1 MiB) → Android/CLI(64 KiB) and the reverse both fail at
header validation. **The Apple app can only transfer with other Apple apps.**

Fix (moranxuege's call): align `GUI_CHUNK_SIZE` to `DEFAULT_CHUNK_SIZE` (one
line), or — larger design — have the receiver adopt the sender's chunk size from
the header instead of requiring equality. Until then, Apple↔{Android,CLI} is
broken. **Owner: moranxuege (FFI).**

---

## 1. Data-stream window — was a hidden/mislabeled knob (fixed)

Two coupled problems, both fixed in the 2026-07 window change:

- The advanced setting labeled `"Chunk size · e.g. 16MB"`
  (`SettingsScreen.kt`) dangled the *window's* value next to the chunk knob.
  Setting chunk = 16 MiB is the degenerate case (`chunk == window` → no
  pipelining depth), so the hint was actively harmful. → hint corrected to
  `"Chunk size · e.g. 64KB (16KB–16MB)"`.
- The throughput-relevant knob — the per-stream QUIC flow-control window
  (`endpoint.rs`, was a hardcoded `16 MiB`) — was exposed nowhere. → now a
  first-class **per-session frozen** setting `data_stream_window`, threaded
  `ClientContext → Client → SessionConfig → data_transport_config(window)`. Not
  a global (avoids concurrent-session last-writer-wins), never in the wire /
  resume / hash (resume-safe). Rejected out of `[1 MiB, 128 MiB]` (no clamp).

---

## 2. Real robustness holes (ours — worth fixing regardless of SSoT)

### 2a. State-string silent drop — HIGH blast radius

The 11 session states are hand-copied in three places:
`enum State` (`crates/envoix-client/src/api/machine.rs:37-65`, serde snake_case)
vs two Kotlin literal `when` blocks (`TransferService.kt:603-615` and `:696-713`)
plus the `enum Status` (`Transfer.kt:5`, listed in a *different order*). The
mapper's fall-through is `else -> return` (`TransferService.kt:615`): a renamed
or new Rust state makes Kotlin **silently discard the whole snapshot** → the card
freezes, no error, no log. Nothing pins all 11 strings on either side.
**Fix:** exhaustive Rust test (every `State` → its exact string) mirrored by a
Kotlin test; make the `else` branch loud in debug instead of `return`.

### 2b. `MAX_FRAME_SIZE` invariant is unenforced

`crates/envoix-protocol/src/lib.rs:41` `MAX_FRAME_SIZE = 16*1024*1024 + 64*1024`
is exactly `MAX_CHUNK_SIZE` (`transfer/lib.rs:29`) + `MAX_FRAME_BODY` (64 KiB),
but nothing ties them (`envoix-protocol` sits below `envoix-transfer`, so it
hardcodes the sum). Raise `MAX_CHUNK_SIZE` and frames silently under-cap → valid
chunks rejected. **Fix:** a `const_assert`/test pinning
`MAX_FRAME_SIZE >= MAX_CHUNK_SIZE + frame overhead`.

### 2c. Room-id length cap 64 vs 128 — inert

`MAX_ROOM_KEY = 64` (`apps/envoix-rendezvous-server/src/logs.rs:30`) vs
`MAX_ROOM_ID_LEN = 128` (`crates/envoix-rendezvous/src/broker.rs:27`), same
concept at two values. **Verified inert:** real room keys are the ~6-digit code
prefix, a 32-char mDNS hex token (`generate_token`, 16 bytes), or `app-`/`crash-`
report keys — all well under 64. Align defensively; not a live bug.

---

## 3. True duplicates (agree today, will drift)

| Value | Sites | Owner | Fix |
|---|---|---|---|
| Broker `…@67.230.187.238:8445` | `envoix-ffi:30-31` · `TransferRepository.kt:104` · `Support.swift:18` (byte-identical incl. pubkey); port also `rendezvous-server/main.rs:29` | ours+flag | one source exported over FFI / shared consts; Kotlin needs a JNI getter or shared file |
| Relay `https://envoix.chkxwlyh.us:8444` | `envoix-ffi:33` · `TransferRepository.kt:105` · `Support.swift:19` | ours+flag | same |
| Invite TTL `300s` | `envoix-client/api.rs:114` (`DEFAULT_INVITE_TTL_SECS`) · `envoix-ffi:28` (`INVITE_TTL_SECS`) | flag | FFI should reference the client const |
| Snapshot/notice JSON keys (~20) | Rust serde (`driver.rs`, `machine.rs`, `types`) vs Kotlin `optString` literals in `TransferService.kt` | ours | type the JNI boundary (serde) or a golden-JSON fixture test — kills the silent-empty-default class |
| Timeline envelope | schema `1` (`android-jni/lib.rs` + `TransferTimeline.kt`), target `"envoix::timeline"` (const + ~20 bare literals in `driver.rs`/`transfer/lib.rs`), escaping octets + column order — all duplicated Rust↔Kotlin | ours | shared golden line (Rust emits, Kotlin parses) + Kotlin column test |
| `MAX_FRAME_BODY = 64KB` | `envoix-pairing/src/wire.rs:14` · `envoix-rendezvous/src/io.rs:14` | ours | soft — independent protocols; hoist only if unifying the framing |
| HTTP courier timeout `8000ms` (×6) | `android/.../LogUpload.kt` lines 24,25,45,46,61,62 | ours | one `HTTP_TIMEOUT_MS` const |

Also: the design doc `docs/design/diagnostics.md:254` states the timeline column
order with `source_seq` in position 3, but the code prepends `source_seq` as the
leading column (`TransferLogs.appendTimeline`) — the prose has already drifted
from both builders. Fix the doc when doing the timeline-envelope test.

---

## 4. Minor / mechanical (ours)

- JNI method names + callback signatures (17 pairs) duplicated Rust↔Kotlin — but
  fail **loudly** (`UnsatisfiedLinkError` / bad-signature at first call). Low
  priority; a load-and-invoke smoke test would pin them.
- Server bind port `8445` (`main.rs:29`) is decoupled from the port embedded in
  the three broker literals — changing one silently needs the others.
- `process_run_id`: Rust `std::process::id()` vs Kotlin `Process.myPid()` —
  correct *by construction* (shared JVM process). Keep the comment.
- Stale comment "keeping the last 60" (`TransferService.kt:407`) vs
  `LOG_CAP = 200` — comment drifted, the const is single-sourced.

---

## 5. Checked — NOT violations (so the sweep is on record as discriminating)

- `PROTOCOL_VERSION` (`envoix-types`), `WIRE_VERSION` (`envoix-protocol`), both
  ALPNs (`envoix/1` in `envoix-session`, `envoix-rendezvous/1` in
  `envoix-rendezvous-iroh`), and the frame-type tags are **single-source in
  Rust**; both peers share that Rust, no Kotlin re-implementation.
- Coincidentally-equal-but-distinct numbers: the 16 MiB QUIC window vs
  `MAX_CHUNK_SIZE` (different concepts — but see 2b for the one real coupling);
  three unrelated 256 KiB caps; the server memory caps (`MAX_ROOMS`,
  `MAX_CLIENT_BYTES`, …); the Android log-file caps. Same value, different
  purpose — not duplicates.
