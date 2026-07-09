# Transfer state machine (design)

Roadmap item #4 of `architecture-review-2026-07.md`. Status: DESIGN — reviewed
with the user before implementation.

## Why (recap)

Every status bug of the July arc came from the same shape: `Transfer.status` is
a last-writer-wins race between user actions and a core event fold in
`TransferService`, patched by accumulating guards (terminal-ignore, paused-keep,
stale-bytes checks, substring classification). Each fix was correct; the shape
guarantees more bugs. The machine replaces the shape.

## Design principles (settled during the arc)

1. **User intent is a first-class input**, not a racing write. A user-initiated
   state can only be left by another user action or by the outcome that action
   requested.
2. **Core events are observations.** They move the machine along legal edges;
   an observation with no legal edge is dropped (and logged), never applied.
3. **A card is a SESSION spanning multiple attempts** (run → pause → resume →
   run …). Every attempt gets a monotonically increasing `attempt` number, and
   **every input is tagged with the attempt it belongs to; inputs from a stale
   attempt are ignored structurally.** This single rule deletes the whole
   late-event bug class (Connecting reviving a Cancelled card, stale bytes
   faking Unconfirmed) without any guards.
4. **Typed first, facts as fallback.** Classification uses `FailureCode`; when
   absent (best-effort delivery), durable facts decide (partial exists, all
   bytes written, was actively Transferring).
5. **The state is serializable.** The upcoming durable `TransferRecord` (#5) is
   exactly `(SessionParams, State)` written to disk — design for it now.

## Where it lives

`envoix-client`, two parts:

- **`machine` (pure):** `reduce(State, Input) -> (State, Vec<Effect>)`. No I/O,
  no clocks, no platform types. Exhaustively table-tested; fuzzable (property:
  no input sequence produces an illegal transition or panics).
- **`TransferSession` (driver):** owns the reducer + the current attempt
  (`Client::run` handle + cancel token) + the mailbox poller. Applies effects,
  feeds inputs, emits a **state snapshot stream** — the FFI surface. Kotlin
  (and later Swift) stop interpreting events: they call intents, render
  snapshots, and keep only platform side-effects (notifications, MediaStore
  publish, multicast lock), which they key off snapshot *transitions*.

The raw event stream stays available for per-transfer logs; it is no longer
the source of truth for status.

## States

```
Waiting        advertising an invite / parked in the room, no peer yet
Connecting     pairing + connecting (peer known or joining)
Verifying      hashing (resume prefix / final verification), no bytes moving
Transferring   bytes moving
Confirming     SEND only: all bytes + Complete frame sent, awaiting the
               receiver's CompleteAck over the live connection (the
               Two-Generals round-trip — real, failure-prone, previously
               hidden inside "Transferring 100%"). Bounded by a confirm
               timer (~15-20s): on expiry the driver cancels the attempt and
               escalates to Unconfirmed (out-of-band proof) proactively.
Paused(origin) resumable stop; origin ∈ {Local, Peer, Lost}
Unconfirmed    send delivered every byte, ack unknown; mailbox poll active
Completed      done (receiver may re-enter Connecting to serve a re-verify)
Failed         genuine failure (typed reason retained)
Cancelled      user abandoned this transfer
```

Notes:
- `Waiting` replaces the `Connecting && qrPayload != null` UI hack with a real
  state (entered on `Advertised`, or on room-park before a partner joins).
- `Paused(origin)` is ONE state with a label detail, not three states — the
  affordance (Resume) is identical; only the subtitle differs.
- `Verifying` is visible (it is real, multi-second work on big files; the
  events already exist; the CLI already renders it).
- `Confirming` (user-requested during design review) requires ONE new core
  event: the engine emits `Confirming` right after sending the `Complete`
  frame (additive, local-only, no wire change). Its payoff: the Unconfirmed
  classification stops being a fact-bundle proxy (`Transferring ∧ Send ∧
  bytes=total ∧ connection_lost`) and becomes a single edge — `connection_lost`
  while Confirming → Unconfirmed. In-band proof (Confirming) and out-of-band
  proof (Unconfirmed + mailbox) are two rungs of one explicit escalation
  ladder. The receiver needs no mirror state: its finalize is milliseconds
  (incremental hash + rename).
- Terminality is explicit per state: `Failed`/`Cancelled` are terminal for the
  session (Retry starts a NEW attempt from them); `Completed` is terminal but
  re-enterable (receiver re-verify); `Unconfirmed` is pseudo-terminal (mailbox
  can still complete it).

## Inputs

```
User:      Start(params) · Pause · Cancel · Resume · Remove
Attempt n: Advertised · Pairing(step) · Connecting · Connected(path) ·
           PathChanged(path) · Started{tid, name, total, resumed} ·
           Progress(bytes) · Verifying · Verified · Confirming ·
           Completed(bytes) · Failed{reason_code, reason} · RunEnded(result)
Driver:    ConfirmTimeout (the confirm timer expired)
External:  ReceiptVerified(tid) · ReceiptMismatch(tid)
```

`RunEnded` is the attempt future returning — the belt behind the "every failed
run ends its stream with a typed Failed" contract (7153682); if a terminal
event was somehow missed, `RunEnded` classifies from the result.

## Transition table

Legend: `—` = input ignored (logged at debug). Attempt-stale inputs are dropped
before the table applies. `n+1` marks edges that launch a new attempt
(`Effect::StartAttempt{resume: true|fresh}`).

| State \ Input | Pause | Cancel | Resume | Advertised | Pairing/Connecting | Started | Progress | Verifying/Verified | Completed | Failed(code) | ReceiptVerified |
|---|---|---|---|---|---|---|---|---|---|---|---|
| **Waiting** | Paused(L) ¹ | Cancelled ¹ | — | — | Connecting | Transferring ² | — | Verifying | — | classify³ | — |
| **Connecting** | Paused(L) ¹ | Cancelled ¹ | — | Waiting | — | Transferring ² | — | Verifying | Completed⁴ | classify³ | — |
| **Verifying** | Paused(L) ¹ | Cancelled ¹ | — | — | — | Transferring ² | — | (Verified→ last phase) | Completed | classify³ | — |
| **Transferring** | Paused(L) ¹ | Cancelled ¹ | — | — | — | — | update bytes | Verifying | Completed | classify³ | — |
| **Confirming** ⁸ | Paused(L) ¹ | Cancelled ¹ | — | — | — | — | — | — | Completed | classify³ | — |
| **Paused(any)** | — | Cancelled | Connecting *n+1, resume* | — | — | — | — | — | — | — ⁵ | — |
| **Unconfirmed** | — | Cancelled | Connecting *n+1, resume* | — | — | — | — | — | — | — ⁵ | **Completed** |
| **Completed** | — | — | Connecting *n+1, resume* ⁶ | — | — | — | — | — | — | — ⁵ | — |
| **Failed** | — | — | Connecting *n+1, resume* | — | — | — | — | — | — | — ⁵ | — |
| **Cancelled** | — | — | Connecting *n+1, FRESH* ⁷ | — | — | — | — | — | — | — ⁵ | — |

¹ Effect: `PauseToken` / `CancelToken` on the current attempt. The state
changes IMMEDIATELY (user intent is authoritative); the attempt's subsequent
`Failed` echo is attempt-current but has no edge out of Paused/Cancelled — the
race is gone by construction, not by guard.

² `Started` RESETS `bytes := bytes_resumed`, sets `tid/name/total`. Stale
bytes from a previous attempt cannot leak into this one.

³ **classify(Failed{code}, facts)** — the one classification table, in Rust:

| condition (first match wins) | → state |
|---|---|
| code = `paused` / `cancelled` (echo of local intent — normally unreachable, see ¹) | keep state |
| code = `peer_paused` | Paused(Peer) |
| code = `peer_cancelled` | *decision D1 below* |
| state = Confirming ∧ code = `connection_lost` | Unconfirmed (effect: `StartMailboxPoll`) |
| code ∈ {`peer_cancelled`, `connection_lost`} ∧ bytes > 0 | Paused(Lost) |
| otherwise | Failed |

Prose fallbacks for code-less events live HERE (one place), not in frontends.

⁴ Receiver `receive_existing_final` / `receive_from_receipt` complete without a
`Started` — hence Completed legal from Connecting.

⁵ A late `Failed` from the CURRENT attempt reaching a resting state can only
happen after Resume relaunched (attempt moved on) — then it is attempt-stale
and already dropped. Defensive `—` regardless.

⁶ Receive direction only (serve a peer's re-verify). For Send, Resume from
Completed is illegal (nothing to re-join — the f010749 lesson).

⁷ Pending decision D1; if Cancel discards partials, restart must be fresh.

⁸ Entered from Transferring on the new `Confirming` event. Entry effect:
`StartConfirmTimer`. `ConfirmTimeout` while Confirming ⇒ effect `CancelToken`
(silently stop waiting on the dying path) and → Unconfirmed +
`StartMailboxPoll` — escalation is proactive, not a 30s QUIC-idle hang. The
timer is cancelled on exit. (Receivers that finalized but lost the ack path
still post their receipt: finalize is local, so the mailbox rung is sound.)

## Effects (returned by the reducer, executed by the driver)

```
StartAttempt{resume: bool}   spawn Client::run, attempt += 1
PauseToken / CancelToken     on the current attempt's cancel handle
StartConfirmTimer / StopConfirmTimer
StartMailboxPoll / StopMailboxPoll
PostReceipt                  (receive completed; driver seals + posts, retries)
```

Platform actions (publish to MediaStore, notifications, multicast lock) are
NOT effects: the app derives them from snapshot transitions it observes
(e.g. `* → Completed` on a receive ⇒ publish). The machine stays portable.

## Snapshot (the FFI surface)

One JSON object per state change (plus throttled Progress updates):

```json
{ "seq": 41, "attempt": 2, "state": "transferring",
  "origin": null, "direction": "receive",
  "file_name": "a.zip", "transfer_id": "transfer-…",
  "bytes": 1048576, "total": 35651584, "bytes_resumed": 16777216,
  "path": "direct 1.2.3.4:5", "speed_bps": 5200000.0, "avg_bps": 4800000.0,
  "reason_code": null, "reason": null }
```

- `seq` is monotonic — frontends drop out-of-order snapshots (Binder/channel
  reordering immunity).
- Speed/avg come from the driver (finally consuming `TransferStats` instead of
  every frontend re-deriving them); the UI keeps only presentation smoothing.
- The snapshot struct is the serialization unit `TransferRecord` (#5) persists.

## JNI surface (replaces the 12-arg runTransfer eventually; additive first)

```
createSession(paramsJson) -> sessionId
sessionIntent(sessionId, "start"|"pause"|"resume"|"cancel")
   snapshots delivered via the existing callback (JSON, tagged "snapshot")
destroySession(sessionId)   (Remove; deletes partials per D1/D2 semantics)
```

## Testing

- **Table tests**: every (state × input) cell above, including all classify
  rows — data-driven, one assertion per cell.
- **Stale-attempt property**: for any legal history, replaying any prefix of a
  previous attempt's events changes nothing.
- **Fuzz**: random interleavings of user intents and event sequences never
  panic, never reach an illegal transition, and always end in a resting state.
- **Loopback integration**: pause/resume/cancel/unconfirmed flows over
  `memory_connection_pair`, asserting snapshot sequences.

## Migration (three PR-sized steps)

- **A.** `machine` + `TransferSession` in `envoix-client`, fully tested. CLI
  untouched; nothing wired.
- **B.** JNI session API; `TransferService` swaps its fold for snapshot
  rendering behind the SAME `Transfer`/UI model (Status maps 1:1 from
  snapshot.state). Old event path kept for per-transfer logs only.
- **C.** Delete the Kotlin fold, the guards, and the classification; remove the
  Spec map (the driver owns attempts). CLI adoption optional later.

## Decisions (RESOLVED 2026-07-09, design review with the user)

All four decided as recommended; D1 with the lost-message ruling; plus one
addition from review: **mailbox retention** — receipts (and future kinds) are
NEVER deleted on read; expiry is by TTL only. Delete-on-read would
reintroduce the one-shot fragility the mailbox exists to cure (a lost GET
response would destroy the only proof). See `peer-mailbox.md` rule 2.

The async channel itself is now formalized in `docs/design/peer-mailbox.md`
(slot keys namespaced by kind, AAD binds scheme+kind, trust and containment
rules, kind registry: `receipt` v1, `cancel` v2).


- **D1 — Cancel semantics** (user-proposed, reverses the earlier
  "cancel keeps partial" decision): Cancel tells the peer `peer_cancelled` and
  the RECEIVER DISCARDS the partial + resume state; Cancelled cards restart
  fresh. Pause remains the resumable stop. → makes Pause/Cancel genuinely
  different. **Recommended: yes.**
  **Ruling (design review): the discard fires ONLY on an explicit typed
  `peer_cancelled`.** If the cancel message is lost (best-effort), the receiver
  sees a bare `connection_lost` and lands in `Paused(Lost)` — ambiguity always
  resolves to the side whose wrong guess is recoverable: discarding a partial
  that was really a pause destroys progress irreversibly; keeping a partial
  that was really a cancel costs disk + a stale card, cleaned by Remove (D2).
  Optional v2 extension (not now): a sealed cancel TOMBSTONE in the rdz
  mailbox under the same transfer-id key, letting a Paused(Lost) receiver
  discover the cancel out-of-band — same infra as receipts.
- **D2 — Remove semantics**: swipe-Remove deletes the local partial + resume
  state + (receiver) receipt. Today it only drops the card. **Recommended:
  yes** (completes "the one true abandon").
- **D3 — Failed + Retry**: retry from Failed relaunches with `resume: true`
  (receiver-side facts decide whether anything is reusable). **Recommended:
  yes** (harmless, and the receiver's state machine already guards content
  mismatch).
- **D4 — mDNS mid-run Failed events**: the session's mDNS loop emits
  per-attempt Failed events while `run()` is still falling back; under the
  machine these arrive attempt-CURRENT and would classify. Treat session-level
  retry reports as non-terminal (new event kind or suppression in the driver
  until `RunEnded`). **Recommended: suppress in driver; only the run-terminal
  Failed (7153682) reaches the machine.**
