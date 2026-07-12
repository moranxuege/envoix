# App diagnostics (design)

Roadmap #6 of `architecture-review-2026-07.md`. Status: DESIGN — for review
before implementation. Builds on `docs/observability.md` (planes, correlation
ids, level policy); this doc covers the ANDROID app's diagnostics system,
which grew by accretion during the July debugging arc and now gets a shape.

## What exists today (accreted) and its failures

| Piece | Problem |
|---|---|
| `LogStore` ring (4×8 MB files) | at TRACE it churns in MINUTES — a needed room's story rotated away mid-debug (field lesson, 2026-07-09) |
| `LogSink` regex `room="…"` on formatted lines | fragile string parsing of structured data; the `room.substringBefore('-')` split is repeated in ~5 places |
| `Transfer.log` (200-line cap) | the card's log is the UPLOAD source too — long transfers upload a truncated story |
| Tail caps in `LogScreen` | per-screen constants; three upload buttons each assemble differently |
| `crash-latest.log` | written, then nothing — no offer to report on next launch |

## Domains (one owner each — the settled taxonomy)

| Domain | Content | Owner | Durability |
|---|---|---|---|
| **core trace** | full tracing output, whole app | `LogStore` ring (unchanged) | 4×8 MB ring — the "verbose logcat", allowed to churn |
| **per-transfer** | one card's complete story | NEW: `logs/transfers/transfer-<id>.log` | file per card id, GC'd by COUNT (keep 20) — survives ring churn |
| **operations** | user-action breadcrumbs | `OpLog` (unchanged) | 128 KB tail |
| **crash** | uncaught + native crash | `crash-latest.log` + NEW ack/offer flow | until acknowledged |

The per-transfer file is keyed by the SAME durable id as `TransferRecord`
(`record-<id>.json` ↔ `transfer-<id>.log`): one identity across state and
diagnostics. Remove (D2) deletes both. The card's in-memory 200-line log
remains purely a UI view; it is no longer any upload's source.

## Typed routing (the regex dies)

Today: Kotlin regex-parses `room="…"` out of FORMATTED lines. The room is a
SPAN FIELD (observability.md) — extract it where the structure lives:

- The JNI tracing subscriber gains a `Layer` that walks the span scope for
  `room` (the pattern `apps/envoix-rendezvous-server/src/logs.rs` already
  uses server-side) and delivers it as a separate argument:
  `LogCallback.log(room: String?, line: String)` (FFI change; fleet moves
  together).
- Kotlin routes by the typed room: append to the matching card's UI log AND
  to its `transfer-<id>.log` file. No regex, no re-stamping games.
- A `Room` value class (Kotlin) with `code` and `id` (the numeric prefix)
  replaces the 5 `substringBefore('-')` sites. (Rust already has
  `split_code`.)

## The DiagnosticsReport assembler (the UX centerpiece)

ONE function builds every report; nothing else assembles or caps:

```
DiagnosticsReport.build(kind):
  header    build id (vX.Y (sha)) · device/emulator · settings summary
  ops       op.log tail                                  (≤ 32 KB)
  transfer  the card's FULL transfer-<id>.log            (≤ 256 KB)   [transfer kind]
  crash     crash-latest.log                             (≤ 64 KB)    [crash kind]
  core      core.log tail                                (fills the remaining budget)
  TOTAL ≤ 480 KB (the rdz body cap), sections trimmed tail-first by priority:
  header > crash > transfer > ops > core
```

Kinds: `transfer(id)` (card Upload/Copy), `app` (Logs screen "Report
problem"), `crash` (the offer flow). Upload keys stay as today
(`/logs/<room>?side=send|receive`, `app-<ts>`, new `crash-<ts>`).

## Crash offer flow (closing the loop)

On launch: if `crash-latest.log` exists and is newer than the last-acked
marker → the Logs screen shows a one-line banner (dev mode not required —
crashes matter to everyone): *"Previous session crashed — Upload report?"*
→ builds `DiagnosticsReport(crash)` → uploads → writes the ack marker.
Dismiss = ack without upload. No modal, no nagging: one banner, one tap.

## UX inventory after the change

| Surface | Before | After |
|---|---|---|
| Card detail: Copy / Upload | 200-line UI log | full `DiagnosticsReport(transfer)` — complete story, one cap policy |
| Logs screen | live view + dev history dialog | + "Report problem" button (app report); + crash banner when applicable |
| Dev history dialog | per-session core files + ops row | unchanged (raw access stays for deep debugging) |
| Everything else | — | unchanged; caps live in ONE `Diagnostics` object |

## Deliberately NOT in scope

- rdz server changes (the log endpoint is fine).
- Restructuring `LogStore`/`OpLog` internals (they work; they get owners, not
  rewrites).
- Metrics (observability.md marks them planned; nothing here blocks them).
- Streaming/remote log tailing.

## Implementation order

1. FFI: `LogCallback.log(room, line)` + JNI span-field Layer (kills the regex).
2. `Room` value class + replace the 5 split sites.
3. Per-transfer files: write path in the service (typed routing), count-GC,
   Remove/D2 deletion, record-id alignment.
4. `Diagnostics` object: caps + `DiagnosticsReport.build` + rewire the three
   upload/copy surfaces.
5. Crash banner + ack marker.
6. Tests: report budget trimming (unit); routing (instrumented manual);
   crash-offer state machine (unit on the marker logic).

## Open decisions (for review)

- **D-A: transfer-file GC count.** Keep 20 transfer logs (≈ a few MB total at
  normal levels; TRACE sessions can make individual files large — files also
  size-capped at 4 MB each, oldest-half truncated). Reasonable?
- **D-B: crash banner for everyone or dev-mode only?** Proposed: everyone
  (crashes are the one thing casual users should report). Dismiss = never
  nags again for that crash.
- **D-C: keep the rdz upload as the only report transport?** Share-as-file
  (FileProvider) was floated earlier for full untruncated logs — proposed:
  add it later if the 480 KB budget proves too small in practice.

## Redesign (2026-07-12): the diagnostic contract

Field lesson: the logging system was formalized around the transport + frame
layer (iroh endpoints, `envoix_transfer` events). When the state machine
became the authority (`envoix-client` machine/driver), its transitions were
never wired into logging — so the single most important view, the machine's
own state path, appeared in NO upload. A completed-via-mailbox transfer's log
ended at the engine attempt's failure with no trace of the recovery. D-C's
"480 KB is probably enough" was also wrong: the core trace was trimmed
tail-first to ~256 KB, discarding the beginning where the transfer starts.

### The contract

Every **authority-level event** emits a structured `tracing` event inside a
room-tagged span, so it routes through `RoomTag` into the per-transfer log and
thus every upload. The per-transfer log is the authoritative timeline. Feature
work does not invent its own logging — it emits contract events:

| Source | Event |
| --- | --- |
| driver `apply()` | `transition` (from → to, attempt, transfer_id) — DONE |
| driver receipt path | `mailbox receipt verified` / `… failed verification` — DONE |
| driver commit barrier | persist failure / retry / `record store unwritable` — DONE (routed) |
| driver effects | `StartAttempt` / `PostReceipt` / `DiscardPartial` execution — TODO |
| Preparing (planned) | staging reserve / copy progress / stage-complete / stage-failed |
| publish journal (planned) | reserve target / commit / staging-deleted |

Mechanism: the actor runs under `info_span!("session", room = …)`
(`session_room()` derives the room id from the sources). Before this, the
actor ran outside any room span and its events were dropped from the
per-transfer log.

### Truncation policy

- **Debug**: NO trimming. `Diagnostics.build` uploads the full report;
  `MAX_BODY` on the server is 64 MB. Server space is not a concern pre-release,
  and a clipped diagnostic is unusable.
- **Release (TODO)**: a real retention/rotation policy — prioritize the
  timeline and frame events over the raw iroh core trace (the core trace is the
  LEAST diagnostic-dense and currently eats the budget), and consider
  share-as-file (FileProvider) for full logs on demand.

### Builds

Debug and release APKs are produced every update. The emulator and full-log
field testing use the DEBUG variant (`android:debuggable`, so `adb run-as`
can read the sandboxed per-transfer logs, and `BuildConfig.DEBUG` enables full
uploads). Release stays minified for real-device installs.

## Redesign v2 (2026-07-12): the transfer timeline

A full audit (Rust emission + routing, the five Kotlin sinks, the rdz merge
model) showed the v1 contract above was necessary but far from sufficient. The
DEEP root cause: logging was built as a *byproduct of transport tracing*, so it
inherited tracing's model — free-text lines, level-filtered, span-routed to a
global console sink — instead of a *transfer-event* model. Every gap is a
symptom of that one framing:

- the **engine** reports its lifecycle through a *different mechanism*
  (`EventSink::on_event`, not `tracing`) → Progress/Confirming/Completed/
  **CompleteAck** never enter the log pipeline at all;
- the **app** reports through *UI drawers* (`addLog` → in-memory `Transfer.log`,
  capacity 200, never read by `Diagnostics.build`) → its own state narration and
  every staging / publish-journal step never reach the upload;
- the **courier** is half-dark: verify/mismatch log (`driver.rs:582/586`) but
  poll-fire, empty-slot, `PostReceipt`-fire, `ReceiptPosted` are silent, and the
  re-verify spawn (`driver.rs:607`) is not `.instrument()`-ed so it loses the
  room and drops from the per-transfer log;
- **nothing carries a normalized timestamp or a transfer_id** → two sides can't
  be interleaved and the log can't be joined to the mailbox
  (`blake3(transfer_id)`); the rdz stores opaque per-side blobs and `view`
  *concatenates* `rdz / send / receive`, never interleaves.

**Thesis:** stop treating the log as *captured tracing*; treat it as *a durable,
structured record of transfer-authority events*. Tracing is ONE transport into
that record — the record, not the console format, is the design.

### The model — one transfer timeline

One structured, timestamped, per-transfer event stream, emitted at each
authority choke point, written to the uploaded per-transfer file separate from
the raw trace, keyed by card-id and carrying transfer_id, time-mergeable across
both sides at the rdz. Six principles:

- **P1 — events, not lines.** An entry is a structured *envelope* (fields below)
  rendered to one delimited line. A *stable boundary vocabulary* (below), not
  ad-hoc strings and not a mirror of the state enum.
- **P2 — emit where the fact is known, not at the transport.** The **driver** is
  where most engine `EventSink` inputs and the machine's state meet — emit there.
  But facts the driver never receives (a receiver's CompleteAck failure lives
  only in `envoix-transfer`) are emitted at *their* authority point, under the
  session span. The app's **TransferService** is the app-side choke point. Never
  claim a layer observed a fact it was never given.
- **P3 — the target classifies; a layer serializes.** Authority events carry
  `target: "envoix::timeline"`; a custom timeline layer visits the event/span
  fields and builds the envelope. The target is a *classifier*, not the
  serializer — the raw `fmt` formatter is for the raw-trace tier only.
- **P4 — route by durable session_id, carried on the wire.** The durable
  card/record id (`session_id`) rides on the session span AND the structured JNI
  callback envelope; Kotlin routes the timeline file **directly by session_id**.
  The room→newest-card lookup is DELETED — room is reusable transport
  correlation, not ownership (two live cards can share a room). `room_id` and
  `transfer_id` are correlation *fields* only; `transfer_id` is what joins the
  log to the mailbox (`blake3(transfer_id)`).
- **P5 — per-source lanes are canonical; the merged view is secondary.** Sender,
  receiver, and broker clocks are not synchronized, so `epoch_ms` sorting can
  fabricate false causality. The canonical rdz view preserves one ordered lane
  per source using a monotonic `source_seq`, wall times shown. A best-effort
  time-merged view is offered as an explicitly **clock-skew-labelled secondary**,
  never as truth. `source_seq` is owned by the **single per-device writer** (§seq
  ownership), not per-process — two producers share one lane per device.
- **P6 — timeline never trimmed; the raw trace is what gets trimmed.** The
  timeline is bounded (tens of events/transfer) and survives any budget; the
  iroh trace is the appendix that yields space, preserving both its **beginning
  and its tail** (the start is where the transfer begins; the tail is where it
  fails) — not tail-only.
- **P7 — the timeline is independent of raw-trace verbosity.** The reloadable
  `EnvFilter` (hot-swapped by `setLogLevel`) gates the raw tier ONLY. The
  timeline layer has its own always-on filter, so turning trace down to `warn`
  at runtime never drops authority events.
- **P8 — diagnostics never drive behaviour.** The timeline is the canonical
  *diagnostic* history; state restore and effect re-derivation use
  `TransferRecord` only. And it never describes uncommitted state as committed.
  One `apply()` reduces several inputs but commits **once**, at the end — the
  intermediate edges were never durably committed. So each edge is logged
  `machine.transition{outcome=decided, batch=<apply#>}`; on `try_commit` success
  a **single** `record.committed{state=<final>, batch}` is emitted; a persist
  failure surfaces as `record.commit_failed{batch}`. No intermediate edge is ever
  labelled committed. `batch` reuses the existing per-`apply` counter.

### The wire envelope + encoding

Structured envelope, delimited encoding (NOT JSON — we debug by eyeballing raw
logs via `adb run-as … read` and `curl rdz/logs/{room}`; a delimited line stays
greppable AND parseable; the rdz merge needs only the leading `epoch_ms` +
`source_seq` to order lanes, never a full JSON parse):

```
schema ⇥ epoch_ms ⇥ source_seq ⇥ process_run_id ⇥ session_id ⇥ attempt ⇥ side ⇥ layer ⇥ event ⇥ outcome ⇥ k=v ⇥ k=v …
```

- Fixed leading columns are positional and delimited; variable data trails as
  named `k=v` cells, so fields can be ADDED without breaking older parsers.
- `schema` version + `process_run_id` handle format evolution and restart
  detection. `room_id` / `transfer_id` appear in the `k=v` tail when known.

**Escaping grammar (formal — the price of not using JSON).** The delimiter is a
literal **TAB** (`⇥`). Fixed leading columns are safe by construction (digits, a
u64 id, or a controlled enum — never a TAB, `=`, or newline). In each trailing
`key=value` cell, the key is a controlled identifier; the **value** percent-
encodes **exactly three** octets and nothing else: `%`→`%25`, TAB→`%09`,
LF→`%0A`. So URIs, spaces, `=`, `&`, `:`, `/` all pass through literally (stays
greppable). Parse = split the line on TAB → fixed columns positional → split each
tail cell on the **first** `=` → percent-decode the value. That is the whole
grammar; there is no quoting mode.

**`source_seq` ownership.** A single device has two timeline producers — the Rust
core (via the JNI callback) and Kotlin `TransferTimeline`. They funnel through
**one** per-transfer Kotlin writer (`TransferLogs`), which stamps `source_seq`
from a single atomic counter as it appends. Rust does **not** assign it. The rdz
is its own lane and stamps its own. Per-lane = one writer = no collisions; seq
order is the true serialization order at that source, independent of the clock.

### The event vocabulary — stable boundaries, specifics as fields

Boundary events (so the diagnostic schema does NOT have to evolve in lockstep
with the state enum); state names, causes, outcomes, attempt, and ids are
*fields*:

```
session.created / session.restored / session.removed
machine.input          (kind, attempt, accepted|ignored)   ← NON-progress inputs only
machine.transition     (from, to, outcome=decided|committed, batch)
machine.fact_changed   (fact, old, new)          ← same-state fact deltas
effect.started / effect.completed / effect.failed (name, …)
protocol.complete_ack  (sent|failed, cause?)      ← a QUIC frame, emitted in envoix-transfer
platform.stage.*       (start|progress*|complete|failed, cause?)
platform.publish.*     (reserve→uri | commit→uri | adopt | staging_deleted | failed→cause)
platform.courier.*     (poll_start|poll_empty|poll_hit|verified|mismatch|posted|post_failed|reverify_served)
record.committed / record.commit_retry / record.commit_failed
```

(`*` = milestone-throttled, not per-byte. **`machine.input` is emitted for
NON-progress inputs only** — Progress and StageProgress never produce a
`machine.input`; they surface only via the throttled `platform.stage.progress` /
transfer-progress milestones, so the timeline stays O(tens/transfer) and P6's
"never trimmed" holds even for a multi-GB file. `machine.input` DOES log
*ignored/stale* inputs — that is how the spurious self-cancel-on-resume, room
596855, would finally show up. `complete_ack` is `protocol.*`, not
`platform.courier.*`: it is a wire frame, not the HTTP receipt mailbox.)

### Redaction (applied where a field enters the envelope)

Uploaded timelines must not leak secrets or PII. Redact at emission — the raw
value never reaches a sink:

- `content://` / SAF URIs → **scheme + display-name only**; drop the
  document-tree / provider path (it embeds a device-scoped tree identifier).
- source file paths → **basename only**, never the full filesystem path.
- pairing code → the `room_id` **prefix** only, never the secret segment.
- `transfer_id` may stay in full — it is an opaque non-reversible hash.

A small `redact()` helper at each boundary, not a PII-classification framework.

### Changes per layer

- **Rust driver** — a `timeline!(event, k=v…)` helper (`info!(target:
  "envoix::timeline", …)`). Instead of the single entry→final `transition` line,
  observe **each `reduce(input)`**: emit `machine.input` (kind, attempt,
  accepted/ignored), `machine.transition` (exact before→after, per step, with
  the `decided`/`committed` outcome of P8), `machine.fact_changed`, and
  `effect.*` for every effect run. Add `platform.courier.*` where the driver
  handles the courier (poll fire/empty/hit, posted). `ReceiptPostFailed` is NOT
  a machine input — emit `platform.courier.post_failed` as a platform
  observation. Fix the re-verify spawn to `.instrument(Span::current())`.
- **`envoix-transfer`** — emit `protocol.complete_ack` (sent/failed) where the
  fact is actually known (P2); the client's `transfer` span must carry
  `session_id` so those events route by card-id like the rest.
- **JNI subscriber** (`android-jni/src/lib.rs`) — a custom timeline layer
  (target `envoix::timeline`) with its OWN always-on filter (P7) that builds the
  delimited envelope and hands `(session_id, line)` across JNI; everything else
  formats to the raw-trace tier under the reloadable `EnvFilter`. The low-volume
  timeline lane stays synchronous and reliable. The raw tier keeps its current
  synchronous per-line path for now; batching it through a bounded background
  writer (observer effect) is DEFERRED (§sequence item 4), not part of this work.
- **Kotlin** (`TransferService`, new `TransferTimeline`) — route the durable file
  by `session_id` (delete the room→card lookup). App-side authority events
  (staging, publish, restored-source fail) write the same delimited envelope into
  `TransferLogs`. Surface causes: the boundaries we instrument
  (`MediaStoreSaver`, staging, journal, `LogUpload`) return *typed results*
  (code, stage, cause) instead of `Boolean`/`null`, so `platform.*.failed` can
  carry a real cause. `stateLogLine` stops being drawer-only; the drawer becomes
  a *view derived from the timeline*.
- **Diagnostics.build** — the `transfer` section becomes `timeline` (full, never
  trimmed) + `raw trace` (head+tail under budget).
- **rdz** (`logs.rs`) — capture `epoch_ms` + `source_seq`; `view` renders
  per-source lanes as canonical, plus a labelled skew-sensitive time-merge (P5);
  keep `transfer_id` as a field for mailbox correlation. **Separately** (security,
  not part of the timeline): `GET /logs/{room}` is unauthenticated over a
  low-entropy room id — track operator auth as its own item.

### Decisions (settled 2026-07-12, after codex cross-review)

- File keyed by durable **session_id** (card/record id) carried on span + JNI
  envelope; room→card lookup deleted; room/transfer_id are correlation fields.
- **Delimited structured line**, not JSON (greppability + parse both matter).
- Per-source **lanes canonical**, time-merge a **labelled secondary** view.
- Timeline has an **always-on** filter, independent of the raw-trace knob.
- Diagnostics **never drive behaviour**; each edge logged `decided`, one
  `record.committed` per `apply` batch — intermediate edges never marked committed.
- `ReceiptPostFailed` and other platform observations do **not** add machine
  inputs.
- Typed error results scoped to the **instrumented** boundaries only.
- The UI drawer is a *view* over the timeline, not a separate sink.
- Delimited encoding has a **formal escaping grammar** (TAB + 3-octet percent-
  encode); no informal `k=v`.
- `machine.input` excludes Progress/StageProgress; those are milestone events only.
- `source_seq` is stamped by the **single per-device Kotlin writer**, not Rust.
- `complete_ack` is `protocol.*`, not `platform.courier.*`.
- **Redaction** (URIs/paths/pairing-secret) lands IN the timeline commits;
  retrieval **auth** is separate but **gates broad rollout** of the richer reports
  (unauth is fine against our own rdz during development).

### Implementation sequence

Commit (a) is split into three unit commits (it was too large for our
unit-commit standard); a1 is the spine, a2/a3 are independent after it.

1. **a1 — envelope + routing** (the spine): `session_id` on the **session**
   span; the custom always-on timeline layer building the delimited envelope
   (with the escaping grammar); `(session_id, line)` JNI callback; Kotlin routes
   `TransferLogs.appendTimeline` by `session_id` and stamps `source_seq`;
   `session.created` proves the path. (The `transfer` span gains `session_id` in
   a4, where the protocol events that need it live. The legacy room→card lookup
   for RAW core lines stays until commit b's `Diagnostics` cutover — a1 is purely
   additive, no regression.) Test: escaping/envelope unit tests; on-device, two
   cards sharing one room land in distinct files.
2. **a2 — machine instrumentation**: per-`reduce` `machine.input` (non-progress,
   incl. ignored) / `machine.transition` (decided) / `machine.fact_changed` /
   `effect.*`, and the `record.committed{batch}` / `commit_failed` outcome pair.
   Rust tests: multi-input `apply`, ignored/stale input, synchronous StartAttempt
   failure.
3. **a3 — driver courier**: `platform.courier.*` under the session span —
   poll_start / poll_empty / poll_hit / verified / mismatch / posted (the exact
   mDNS-Unconfirmed diagnostic: did the receiver post? did the sender poll an
   empty slot?); re-verify spawn `.instrument(Span::current())` fix + a
   `reverify_served` event. (`post_failed` is a Kotlin HTTP observation → it
   lands in commit b with `TransferTimeline`.)
4. **a4 — protocol**: `session_id` on the `transfer` span, then
   `protocol.complete_ack` (sent/failed) in `envoix-transfer`, routed by card-id
   like the rest. (The complete-ack-undeliverable warning already reaches the
   raw per-transfer log via room-routing; a4 elevates it to the timeline.)
5. **b — app authority + typed results**: `TransferTimeline`, typed boundary
   results (+ `redact()`), staging/publish instrumentation, drawer-as-view, and
   `Diagnostics.build` onto the timeline (raw trace = head+tail appendix).
6. **c — rdz lanes + merge**: `source_seq`/`epoch_ms` capture, per-source lane
   rendering (canonical) + labelled skew-sensitive time-merge.
7. **Deferred, separate**: raw-trace batching + overload/dropped-record
   hardening; log-retrieval auth (**must precede broad rollout** of the richer
   reports); removal of legacy `Transfer.log`/`OpLog` duplication after migration.
