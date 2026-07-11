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
