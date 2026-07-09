# Architecture review & refactor plan (2026-07)

Outcome of a holistic review (workspace + client seam + Android layer), cross-checked
against the 12 bugs found during the July pause/resume/logging debugging arc. This doc
records the findings, the decisions made with their refinements, and the agreed roadmap.

## Verdict

The Rust core below `envoix-client` is sound: clean crate DAG, iroh contained to
`session`/`rendezvous-iroh`/`qr`+apps, transport-agnostic `transfer`/`auth` over
`FrameConnection`, binding-friendly client API. **Not one of the 12 bugs was in the
transfer engine.**

The consistent weakness: **the product layer above `envoix-client` doesn't exist as a
shared thing.** Status semantics, outcome classification, source precedence, speed math,
resume specs, error humanization — all re-implemented (divergently) per frontend, and
that layer is where every bug lived. `Transfer::phase()` and `Transfer::stats()` exist
in the client API and are used by *neither* consumer.

Two boundary sins explain most bugs:
1. **Intent destroyed at boundaries.** Pause/cancel collapse into one `cancel()` bit at
   the FFI; the wire carries only `connection lost`. Apps reconstruct intent with
   heuristics (partial-exists → resumable; bytes≥total → unconfirmed; English substring
   matching on error strings).
2. **Happy-path awaits.** The broker's parked waiter selects on partner-join/TTL but not
   its own connection dying (dead-slot bug). Same shape as the CompleteAck edge.
   Rule: every await on a peer must also select on that peer's death.

## Decisions (with refinements)

### 1. Typed outcomes + cancel-reason — keystone
- `cancel(id, reason: Paused | Aborted)` through the FFI; core events carry a typed
  `reason_code` (e.g. `PeerInterrupt | ConnectionLost | ...`) instead of prose strings.
- **Refinement (decided): the reason reaching the peer is BEST-EFFORT only.** The peer's
  connection may already be too degraded to deliver a close-reason (observed in practice:
  receiver saw raw `connection lost`, not `interrupted by peer`). Design accordingly:
  - Local intent is authoritative for the local card (you always know your own pause).
  - Remote reason is opportunistic: use it when delivered.
  - The partial-exists → resumable heuristic REMAINS as the fallback classification.
  - Net: "typed reason if delivered, durable-facts heuristic otherwise" — never a
    protocol that *requires* the reason to arrive.

### 2. Transfer state machine in `envoix-client` — agreed 100%, careful redesign
- Extend the client's existing `Phase` into the full product state machine
  (incl. Paused / Unconfirmed / Cancelled), as a pure reducer where **user intent is a
  first-class input** alongside core events — not a racing write.
- Kotlin's 12-write-site `when(ev)` fold in `TransferService` collapses to rendering
  the state the core hands over. iOS reuses it wholesale.
- Needs a deliberate design pass (states, events, legal transitions, terminal rules)
  before code. Do AFTER typed reasons (#1) so the machine is built on enums, not strings.

### 3. Durable client-side transfer record
- **Clarification (asked): this is NOT the current Kotlin storage.** Today the relaunch
  `Spec` is an in-memory `ConcurrentHashMap` in `TransferService`'s companion — dies with
  the process. The receiver's `.part` + resume `.json` (Rust `envoix-storage`) are durable,
  but the client-level relaunch parameters are not persisted anywhere.
- Plan: a `TransferRecord` store in `envoix-client` (natural home: the `storage` crate),
  persisting the transfer's identity + relaunch parameters + outcome.
- Gives: resume across process death, and transfer history for free.
- **Semantics (decided): Cancel KEEPS the record (history metadata + partial, resumable).
  Swipe-left Remove REALLY removes it — record and partial both deleted (the one true
  abandon).**

### 4. Broker dead-slot fix
- Parked waiter must also select on its own connection closing and evict itself from
  `waiting` immediately. Registry entries must be invalidated by the resource they
  represent, not by timers alone. Test on :8446 first, then prod (:8445).

### 4b. Completion receipts (agreed 2026-07-08, after the Unconfirmed dead-end)
Observed failure: CompleteAck lost → sender "Unconfirmed"; receiver is Done and never
rejoins → sender's Retry joins an empty room (dead end). Forcing both back in re-sends
the whole file, because finalize deletes resume state AND the app deletes the core's
output copy after publishing (TransferService.kt src.delete()).

Design (two layers that compose; user-proposed mailbox evaluated and adopted):
1. **Local completion receipts (foundation).** On finalize the receiver writes a small
   receipt (file name, size, final blake3) instead of just deleting resume state — the
   durable "I finished this" proof that survives the file being moved/published. On a
   re-offer matching a receipt, the receiver replies "at 100%" → sender sends only the
   Complete frame → hash compared against the receipt → CompleteAck in seconds, zero
   bytes re-sent. No server changes. This is the first slice of #4/TransferRecord.
2. **rdz receipt mailbox (async layer — the magic-wormhole mailbox pattern).** The rdz
   is the party that's always online: the receiver POSTs the receipt there (with
   retry — the one-shot ack dies on a dying connection, but the post can retry for
   hours; same network domain, different TIME domain), and the sender polls on
   retry/launch → Unconfirmed flips to Done with NO receiver presence or action.
   - **Privacy: the receipt must be sealed** (rdz stays blind — no file names/hashes
     in the clear; a plaintext hash also enables confirmation attacks). Use the
     envoix-pairing seal primitives. **OPEN QUESTION (needs a design pass): key
     derivation** — the SPAKE2 session key dies with the connection; a pairing-code-
     derived key is the natural choice but its entropy vs. the rdz operator needs a
     deliberate look.
   - Server: small store keyed by room/transfer id, TTL a few days, GC. This makes
     the rdz officially a third thing (matchmaker + dev log collector + receipt
     mailbox) — a deliberate architectural step.
   - Cross-restart confirmation on the sender needs the durable TransferRecord (#4);
     within one app session it works without it.

### 5. Keep hand-written JNI; no speculative UniFFI
- The client API is already binding-friendly (no generics/closures/lifetimes) — that IS
  the iOS prep. Wire UniFFI only when iOS work actually starts. Items 1–3 are the real
  portability work and pay off on Android today.

### 6. Formalize logging & error reporting (new)
- Current system was built by accretion during debugging: `LogStore` (ring + rotation),
  `OpLog` (breadcrumbs), `LogSink` (regex room="…" routing of formatted lines),
  per-transfer log lists in `Transfer`, rdz upload, `crash-latest.log`, tail-cap
  constants scattered across screens. It works but has no design.
- Formalize as explicit log domains, one owner each:
  | Domain | Content | Today |
  |---|---|---|
  | core trace | tracing output, whole app | LogStore ring + core-N.log |
  | per-transfer | one transfer's story | regex-routed lines in Transfer.log |
  | operations | user-action breadcrumbs | OpLog |
  | crash | uncaught + native | crash-latest.log, tombstones |
- Key fixes: per-transfer logs should be built from **typed events / a structured tracing
  layer**, not regex-parsing formatted strings (the `room="…"` regex + `substringBefore('-')`
  convention is fragile and repeated in ~5 places — introduce a `Room` value type);
  a single `DiagnosticsReport` assembler (build id + op tail + transfer log + core tail,
  size-capped once) instead of per-screen tail-capping; upload paths unified.

## Smaller hygiene (one batch)
- `Room` value type (kills the `substringBefore('-')` magic split ×5).
- Dedupe two `humanBytes`; move `smoothedBps` out of `HomeScreen`; split `HomeScreen.kt`
  (708 LOC) into card/waiting/drawer/chart files; move DetailDrawer's upload side-effect
  out of the composable.
- `envoix-qr`: drop the full-iroh dep (used only to validate an EndpointId).
- `PathPolicy` enum plumbed as enum into session (not two bools); JNI relay `Some("")` vs
  CLI `None` divergence; stale `SettingsStore.renderConfig` doc; duplicated `cfg(unix)`
  key-file code.
- Leave the four Role/direction enums alone (wire-coupled, documented — litigated in Stage 0).
- Watch `envoix-session` (1869 LOC, widest iroh fan-in) — split by discovery method when
  it next grows.

## Roadmap (agreed order)
1. Typed outcome/cancel-reason (core + client + JNI + Kotlin) — best-effort on wire.
   ✅ DONE 2026-07-08 (a246b87).
2. Broker dead-slot fix (small; :8446 then prod).
   ✅ DONE 2026-07-08 (7586a8c); field-verified on :8446, promoted to prod :8445 2026-07-09.
3. Completion receipts: local (foundation) then rdz mailbox (async layer) — see §4b.
   ✅ Local foundation DONE 2026-07-09 (receipt on finalize + receive_from_receipt
   short-circuit + Done-card re-join in the app). rdz mailbox still pending.
4. Transfer state machine in `envoix-client` (design doc first, then implement).
5. Durable `TransferRecord` (cancel keeps, remove deletes; receipts are its first slice).
6. Logging/diagnostics formalization. Requirement learned 2026-07-08: at TRACE the
   4×8 MB core-log ring churns in minutes (a needed room's story rotated away mid-
   debug) — retention must be per-room/per-transfer, not a byte ring. Also surface
   the PRIMARY source's error in client::run multi-source fallback, not the last
   fallback's noise (mDNS "no peers in 5s" buried the real Room failure).
7. Hygiene batch.
