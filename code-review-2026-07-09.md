# Code review — feat/android-app (2026-07-09)

Scope: the Android-app branch surface — `envoix-client` state machine + driver +
record/receipt, the JNI bridge, and the Kotlin app — with spot checks of
`envoix-transfer`, `envoix-storage`, `envoix-session`, and the rendezvous
receipts endpoint. All Rust tests pass locally (envoix-client 62, storage 25,
transfer suites green).

Reviewed at local `feat/android-app` (48d857b), which is **200 commits behind
`origin/feat/android-app`**. Two local bugs are already fixed on origin:

- `c40ab9f` — restored sessions rebuilt a default `Client`, losing
  chunk-size/candidate-CIDR settings (now persisted as `SessionContext`).
- `8c5581c` — `Completed` without a prior `Started` (existing-final / receipt
  paths) left `transfer_id`/`file_name` unset, so the receiver's `PostReceipt`
  bailed and the confirmation duty was never discharged.

Everything below was verified to still exist at `origin/feat/android-app`.

---

## A. Correctness

### A1. Restore-then-intent race in `TransferService` (medium)
`TransferService.kt:166` (also `REVERIFY` at 183). `restoreAllRecords()` only
*queues* collector coroutines; `Native.restoreSession` (which registers the
session in the JNI map, after a `block_on(load_all())`) runs inside the
coroutine. The very next line, `Native.sessionIntent(id, "resume")`, runs on
the main thread and will usually win the race, hit "session not found", and
drop the intent. Masked today because `MainActivity.onCreate` restores long
before a user can tap. Fix: register synchronously, or queue intents for a
not-yet-registered id on the Rust side.

### A2. Synchronous attempt-launch failure is not persisted (medium)
`driver.rs:543-556`. The error arm of `launch_attempt` reduces `RunEnded`
inline, emits a snapshot, but skips `persist()` — the on-screen Failed state
diverges from the record; after restart the card resurrects as `Paused(Lost)`
with a stale reason. The `debug_assert!(effects.is_empty())` is also not
strictly true (`classify` can return `DiscardPartial` on a peer-cancelled
message), and release builds would silently drop the effect. Route through
`apply()` instead of duplicating half of it.

### A3. Restore coerces a dead `Confirming` to `Paused(Lost)` (medium)
`driver.rs:129-139`. A send killed in Confirming already delivered every byte
plus the `Complete` frame; the faithful restore state is `Unconfirmed` +
mailbox poll (which restore already knows how to re-arm for `Unconfirmed`).
As written the card says "interrupted" and waits for a manual Resume that
re-dials the peer for nothing the receipt couldn't prove.

### A4. Actor blocks on a full-file hash (medium)
`driver.rs:345` awaits `verify_receipt_against_file` — a full BLAKE3 hash of
the source file — inside the actor's `select!` loop. The design doc's
concurrency audit claims receipt I/O is "bounded, milliseconds"; on a phone a
multi-GB file is tens of seconds during which pause/cancel/snapshots stall.
Spawn the verify and feed the result back as a command.

### A5. Unconfirmed dead-ends silently (medium-low)
`driver.rs:350-354`: an authenticated receipt mismatch clears the polls
(correct) but leaves the card Unconfirmed with no surfaced reason —
indistinguishable from "still waiting". Likewise the bounded 4-poll schedule
(`driver.rs:35`, ~62 s) exhausts silently; nothing polls again until an app
restart. The mismatch deserves a terminal Failed ("peer finalized different
content"); exhaustion deserves a hint or a slow keep-alive poll.

## B. Security / hardening

### B1. Path traversal via content-provider display name (medium)
`TransferService.kt:302-306`. `stageAndStart` does
`File(File(cacheDir, "send"), displayName(uri))` with the name straight from
another app's ContentProvider; a hostile provider can return
`../../shared_prefs/foo.xml` and make the staging copy write anywhere in the
sandbox. The Rust side got this right (`is_plain_file_name`); mirror it:
`File(name).name`, reject empty/dot names. Low exploitability today (system
picker), one `ACTION_SEND` intent-filter away from real.

### B2. Secrets in cloud backup (medium-low)
`AndroidManifest.xml` has `allowBackup="true"` while `records/*.json` persist
full room codes (the segment after the first `-` is the SPAKE2 password) and
file paths; log files persist transfer metadata. Add backup exclusion rules
or disable backup.

### B3. Blanket cleartext + receipts tied to the log-server setting (medium-low)
`usesCleartextTraffic="true"` app-wide; the receipt courier
(`TransferService.kt:502-516`) reuses `settings.logServer`
(`http://67.230.187.238:8460`). Log uploads are plaintext HTTP carrying file
names/addresses, and clearing the "log server" setting silently disables
receipt confirmation — a correctness feature dying with a diagnostics toggle.
Give receipts their own endpoint derived from broker config (HTTPS behind the
Cloudflare name) and scope cleartext with a network-security-config.

## C. Design / robustness

### C1. `useRoom` frozen at creation (low)
`TransferService.kt:133` bakes `hasInternet()` into the Spec; the sources list
is then persisted in the record. A transfer created during a connectivity blip
loses the Room path permanently, across every resume and restart.

### C2. Card-id allocation relies on an ordering convention (low)
`TransferRepository.kt:19-31`. New ids start at 1 and only avoid colliding
with persisted record ids because `MainActivity.onCreate` restores first. Any
future entry point that creates a transfer before restore (share intent, tile)
mints id 1 and `RecordStore.save` overwrites `record-1.json`. Seed `nextId`
from the max persisted record id.

### C3. Receiver-initiated pause reaches the sender as `ConnectionLost` (low)
The receiver sends the typed pause Error frame, but the sender never reads
frames during its chunk loop (`envoix-transfer/src/lib.rs:393-420`); it only
observes the close and lands in `Paused(Lost)` instead of `Paused(Peer)`.
Both resumable; a last-gasp drain of a pending Error frame on send failure
would recover the label.

### C4. Synthetic failure snapshots hardcode `seq: 1` (low)
`envoix-android-jni/src/lib.rs:439-444`. Any card that already applied a real
snapshot drops a later synthetic one on the Kotlin `seq <= prev` guard. Only
the poisoned-mutex arms hit it today; a trap for the next error path.

### C5. Doc/impl divergence in the machine (low)
Design table allows `Transferring × Verifying → Verifying`; `machine.rs:333`
only allows `Waiting|Connecting`. Currently unreachable (finalize hashes
incrementally) — fix the table or widen the guard.

## D. Nits / perf

- JNI `emit`/`log_line` call `attach_current_thread()` per event/log line; the
  guard detaches on drop — constant JVM attach/detach churn at `envoix=debug`
  volume. Attach pump threads permanently.
- `restoreAllRecords` → `listRecords` + per-id `restoreSession` each re-run
  `load_all()` — O(n²) file reads via `block_on` on the main thread at startup.
- `sweepStaging` races (start-of-session sweep on IO vs completion sweep on the
  collector) can double-publish a final as "name (1)"; completion sweep does
  blocking I/O + MediaStore inserts on `Dispatchers.Default`.
- `Cmd::Discard` on a send card calls `delete_receipt(params.path, …)` treating
  the source *file* as a directory — harmless failed delete, reads as a bug.
- `Session.direction` serializes PascalCase (`"Send"`) in an otherwise
  snake_case FFI contract; Kotlin special-cases it (`TransferService.kt:233`).

---

# Assessment of the second review set

Verdict per item ("agree" = mechanism verified in code).

## Most-likely-real problems

**R1. Receive hash verification loses facts before Started — AGREE (mechanism),
impact narrower than stated.** `driver.rs:425-426` maps `Verifying`/`Verified`
factless although the events carry `transfer_id`/`file_name`. But the bad
window is only attempt 1 of a *fresh session* dying pre-`Started` (a resumed
attempt inherits `file_name`/`transfer_id`/`bytes` from the previous attempt,
so classify still lands in `Paused(Lost)` and discard works). The fresh-session
case needs a pre-existing partial from an *older, removed-without-cleanup*
session — then yes: classified Failed (bytes=0) instead of Paused(Lost), and
`DiscardPartial` no-ops for lack of ids. Resumability itself survives via D3
(Retry from Failed uses `resume: true`). Cheap fix, same shape as origin's
`8c5581c`: carry the ids on `Verifying` (and arguably on `Started`'s absence
paths generally).

**R2. Restored mDNS sessions miss the multicast lock — AGREE, good catch.**
Lock acquire/release lives only in `startSession`'s collector
(`TransferService.kt:373/383`); the restore collector (`:274-282`) never
touches it. A restored mDNS transfer that the user resumes runs discovery
without the lock — mDNS responses are likely dropped. Room usually rescues it,
so mdns-only setups are the visible casualty.

**R3. Shared receive staging causes cross-card publish/attribution — AGREE**
(matches my sweep-race nit, and they're right it's worse than cosmetic): card
A's completion sweep can publish B's just-finalized file; B's own sweep then
finds nothing and B never gets `savedUri` (dead Open button). With same-named
files the `it.fileName == src.name` guard can also *mis*attribute. Per-transfer
staging subdirectories fix this cleanly.

**R4. Pre-start receive sweep is not ordered before start — AGREE, low.**
`TransferService.kt:368-370` launches the sweep on IO and immediately starts
the Rust session. Pairing latency (seconds) vs sweep (ms) makes the race
essentially always won, but "before" is a comment, not a guarantee — await the
sweep, then start.

**R5. Remove during send staging starts a hidden transfer — AGREE, and it's
worse than stated.** `stageAndStart`'s coroutine is untracked; REMOVE deletes
card/spec/record, then staging completes, re-adds `specs[id]`, and calls
`startSession` — a live send with no card. Because `createSession` passes
`record_for(id)`, the actor also **re-creates `record-<id>.json`**, so the
removed transfer resurrects as a card on the next restore. Track the staging
Job in `jobs` (cancel on REMOVE) or re-check card existence before
`startSession`.

**R6. Remove loses D2 cleanup when the native session is already gone —
AGREE.** `jobs` is per-service-instance; `specs` is static. If the service was
destroyed (its `onDestroy` → `awaitClose` → `destroySession(discard=false)`)
while the process lives, a later REMOVE hits an empty JNI map and the discard
early-returns (`envoix-android-jni/src/lib.rs:742-748`) — partial, resume
state, receipt, and the *record* all survive, so the card resurrects on next
restore. Note REMOVE, unlike RESUME/REVERIFY, does not call
`restoreAllRecords()` first; doing so (or adding a session-less
`discardRecord(id)` JNI entry) fixes it.

**R7. Receipt POST retry survives remove — AGREE, low/benign.**
`onPostReceipt`'s retry loop (`TransferService.kt:537-547`) lives in the
service scope, not `jobs[id]`; after REMOVE it keeps retrying (~35 s) and then
pings a dead session id (warn, no-op). Arguably even desirable — the receipt
still reaches the sender. Worth tying to the job only for tidiness.

**R8. Restored Android-only UI facts missing (qrPayload, savedUri) — AGREE,
UX.** Restore passes `qrPayload = null` (`TransferService.kt:267`) and never
recovers `savedUri`, so restored Completed receives lose their Open button.
qrPayload matters less (restore coerces active→Paused(Lost), so no Waiting
card exists to show a QR). `savedUri` belongs in the record's planned
platform-extras — the design addendum already names this follow-up.

## Future hotspots

**H1. Android shadow Spec rebuilt from Rust JSON — AGREE.** The design doc
itself lists "full Spec deletion + record platform-extras" as follow-up;
origin's `c40ab9f` (SessionContext in the record) is a first step. R8 and the
`useRoom`-freeze (C1) are both instances of this drift.

**H2. JNI session replacement without a generation token — AGREE (fragile,
not currently a bug).** `map.insert(id, session)` silently replaces a live
session; the old pump keeps emitting on its own callback. Current Kotlin id
discipline avoids it; a `(id, generation)` key or an insert-guard would make
it structural.

**H3. Driver tags events with the current attempt at consume time — AGREE
(safe today by construction).** Attempt bump and `current` replacement happen
in the same `on_cmd` call with no await between reduce and `StartAttempt`, so
no window exists; but the safety is an artifact of actor scheduling, not of
the data. Tagging events at the source (stamping the attempt into the channel)
would survive future refactors.

## Combined priority order

1. R5 (hidden transfer + record resurrection) and A1 (restore-then-intent race)
2. R6 (Remove loses cleanup), A2 (unpersisted launch failure), B1 (display-name sanitize)
3. R2 (multicast lock on restore), A3 (Confirming restore coercion), R3/sweep isolation
4. A4/A5 (actor stall, Unconfirmed dead-ends), R1 (Verifying facts), B2/B3 (backup/cleartext)
5. Everything else as hygiene alongside other work

---

# Root-cause analysis (added after review of both finding sets)

The findings are not independent. Two deep causes generate ~80% of them, and
both are the July lesson — "each fix was correct; the shape guarantees more
bugs" — recurring one layer out from where the machine fixed it.

## Deep cause I: lifecycle authority never moved into the durable layer

The July refactor made the Rust machine authoritative for *status*. But
authority over *existence, identity, and launch context* — does this transfer
exist, is it live, what does it need to relaunch, what must Remove clean —
still lives in volatile runtime state: the JNI `SESSIONS` map, the
per-service-instance `jobs` map, the static `specs` map, and an in-RAM id
counter starting at 1. The durable record is *written* as a side effect of
state changes but almost never *read* as the authority. Every finding below is
a consequence:

- **Intents address transient handles, not durable identity.** Resume/Remove/
  Reverify go through the JNI live-session map, with ad-hoc "restore first if
  missing" guards — present for RESUME/REVERIFY (and racy: A1), absent for
  REMOVE (R6). If intents were addressed to record ids, with the Rust side
  rehydrating or queueing internally, "is the session live right now?" would
  not be the caller's problem. → A1, R6.
- **Remove (D2) is scatter-cleanup, not a record-ordered transaction.** Partial
  + resume state + receipt + record die in the Rust actor (only if a session is
  live); staged copy, logs, card, maps die in Kotlin (only if `specs`/`jobs`
  entries exist at that instant); in-flight work (staging coroutine, receipt
  retry) is never fenced. Nothing orders these against the record, so both
  resurrection bugs exist: a late `startSession` re-creates the deleted record
  (R5), and a dead-session REMOVE leaves the record behind (R6). If Remove were
  "tombstone the record first, run cleanup *from* the record, delete the record
  last" — and create/restore honored tombstones — resurrection would be
  unrepresentable. → R5, R6, R7.
- **Ids are allocated in the most volatile layer.** `TransferRepository.nextId
  = 1` in RAM, colliding with durable record ids unless MainActivity's
  restore-first ordering holds forever; the JNI map accepts silent replacement
  of a live session under the same id; `record_for(id)` happily recreates a
  record for an id that was just removed. Allocate ids from the RecordStore and
  key replaceable things by (id, generation). → C2, H2, R5's resurrection.
- **Launch context is shadowed.** The Kotlin `Spec` duplicates what the record
  holds, and every field that lives only in Spec is a restore bug by
  construction — config loss (fixed in c40ab9f by moving it INTO the record:
  the fix confirms the diagnosis), qrPayload/savedUri (R8), the frozen
  `useRoom` decision (C1). The design docs already name the fix ("full Spec
  deletion + record platform-extras"). → H1, R8, C1.
- **Platform effects are keyed off the launch path, violating the project's
  own rule.** The design doc says platform actions (MediaStore publish,
  notifications, **multicast lock**) are "derived from snapshot transitions."
  The multicast lock is instead acquired in `startSession`'s collector — one
  particular launch path — so the restore path silently lacks it (R2). The
  staging sweeps are similarly imperative calls at particular moments rather
  than a rule over snapshots + a per-transfer staging namespace, hence the
  ordering race (R4) and cross-card interference (R3 — finals lose their
  transfer identity the moment the core renames them into a *shared* dir).
  → R2, R3, R4.

The accumulated Kotlin guards — `if (!jobs.containsKey(id))`,
`jobs.remove(id)?.cancel()`, the sweep's dot-file filter, the
`fileName == src.name` attribution guard — are the same accumulating-guards
shape the design doc diagnosed for status. Lifecycle is waiting for its
machine.

## Deep cause II: the machine's fact pipeline has informal edges

The recorded principle is "state derives from durable facts, never the
reverse." It is enforced for status transitions but not for how facts *enter,
persist through, and exit* the machine:

- **The event mapping is lossy about identity facts.** `Verifying`/`Verified`
  carry `transfer_id`/`file_name` on the public stream; the driver's mapping
  drops them (R1). The identical bug for `Completed` already shipped and was
  fixed once (8c5581c) — proof the mapping is *generative* of this class.
  The attempt tag is likewise stamped at consume time, safe only by actor
  scheduling (H3). Fix at the source: stamp attempt + identity into events
  when emitted, and make identity monotone set-once facts.
- **`apply()` is a convention, not a pipeline.** `launch_attempt`'s
  synchronous-failure arm re-implements half of apply (reduce + snapshot, no
  persist, a wrong `debug_assert`) instead of feeding an Input through the one
  path (A2). One enforced input→reduce→effects→persist→snapshot pipeline
  removes the bypass class.
- **Effects have no blocking classification.** The driver executes all effects
  inline in the actor loop, including a full-file BLAKE3 hash (A4). The
  parallel-proofs design already demonstrates the right pattern (spawned work
  reporting back as an Input); receipt verification needs the same
  classification: token ops inline, I/O spawned.
- **Driver-held knowledge has no Input vocabulary.** The design's input
  alphabet includes `ReceiptMismatch(tid)`; the implementation never added it
  (verified absent locally AND at origin). So when the driver learns something
  terminal — authenticated receipt mismatch, poll-schedule exhaustion — it can
  only mutate its own scheduling state (`polls.clear()`), and the machine (and
  therefore the user) never hears about it. A5 is not a missing guard; it is a
  designed edge that was dropped during implementation.
- **Restore coerces by state name, not by facts.** Restored `Confirming`
  becomes `Paused(Lost)` although the durable truth ("every byte + Complete
  frame sent") derives `Unconfirmed` + mailbox poll (A3). The design addendum
  anticipates exactly this mechanism ("a Facts struct enters Session when a
  real edge needs it") — restore-of-Confirming is that edge; a monotone
  `complete_sent` fact makes the coercion table unnecessary.
- C5 (doc table vs guard on Transferring×Verifying) is the documentation-level
  symptom of the same informality.

## Secondary causes

- **The trust model stops at the wire (B1, B2, B3).** Wire inputs are
  validated (`is_plain_file_name`), payloads sealed, the rdz blind — but
  platform inputs (ContentProvider display names), platform transports
  (auto-backup of records containing SPAKE2 password halves; app-wide
  cleartext), and infrastructure config (the receipt courier endpoint riding
  the dev log-server setting) are implicitly trusted. Treat the Android
  platform boundary as an input boundary of the same rank as the wire.
- **The "dumb HTTP courier" split is load-bearing for three findings.** Putting
  the mailbox HTTP in Kotlin created the endpoint-from-settings coupling (B3),
  the unfenced retry job (R7), and JNI surface (fetch/post notices +
  receiptResponse). The core already reaches platform TLS/DNS through
  ndk_context; moving the courier into the driver would delete that boundary.
  Tradeoff: Kotlin HTTP was the cheap, debuggable choice — but note it now
  costs correctness, not just elegance.

## Leverage map (one structural change → findings deleted)

| Structural change | Findings it removes |
|---|---|
| Record-authoritative lifecycle: ids from RecordStore, intent-by-record-id (rehydrate/queue in Rust), tombstone-first Remove | A1, C2, R5, R6, H2 (+ fences R7) |
| Platform effects derived from snapshot transitions (per the existing design rule) + per-transfer staging dirs | R2, R3, R4 |
| Record platform-extras + delete the Kotlin Spec (already planned) | R8, C1, H1 |
| Source-stamped events + monotone identity/`complete_sent` facts + single `apply()` + implement `ReceiptMismatch` | A2, A3, A5, R1, H3 (and the 8c5581c class) |
| Effect blocking classification (inline vs spawned-report-back) | A4 |
| Platform boundary treated as wire-rank input boundary | B1, B2, B3 |

---

# Batch 3 assessment (other agent's second set)

## Confirmed issues

**N1. Same-named send staging overwrites active transfers — AGREE, and
escalate.** `TransferService.kt:306` stages every pick as
`cacheDir/send/<displayName>`. Two concurrent sends named `photo.jpg` share
one path — and the escalation: the sender hashes *as it reads*
(`lib.rs:400-403`, `hasher.update` over each chunk read), so if the file is
replaced mid-send with same-size content, the mixed bytes hash to exactly what
was sent, the receiver's verification PASSES, and a silently corrupt file
finalizes as verified. Different sizes fail loudly (`offset != total_bytes`);
equal sizes corrupt silently. Highest priority of the batch.

Phrasing refinement (agreed in cross-review): this is a **source identity /
snapshot integrity** failure, not broken transfer cryptography. The wire
crypto and stream hash are intact — the receiver correctly verifies the bytes
that crossed the wire; the broken promise is app-level: "card A sends the file
the user picked for card A." The proof basis is "whatever the mutable path
contains during send" instead of a committed snapshot — a Basis-audit hit as
much as a Key-audit hit.

Also: Remove of card A deletes the staged file card B is actively sending
(the `startsWith(staged)` deletion). Nuance (agreed): with the fd already
open, Linux unlink semantics let B's in-flight read continue — the immediate
send survives. But the path is gone for everything path-addressed afterwards:
resume's prefix re-hash, restore relaunch, and receipt verification (which
re-opens by path today, fails, and is mistaken for an authenticated mismatch
→ polls cleared → stuck Unconfirmed). The committed-hash fact heals the
receipt-verification leg of this too.

**N2. Shared receive staging — AGREE** (same finding as R3; their framing via
identity is sharper).

**N3. SAF publish deletes same-named user files — AGREE, user data loss.**
`MediaStoreSaver.kt:38` does `tree.findFile(displayName)?.delete()` before
creating — destroys a previous receive or an unrelated user file in the picked
folder. Note the policy inconsistency: the Downloads path never deletes
(MediaStore uniquifies to "name (1)"); the SAF path destructively overwrites.
Uniquify instead of delete.

**N4. Restored Confirming loses the receipt-poll duty — AGREE** (identical to
A3, already in plan Phase 1 task 6).

**N5. Receipt fetch responses are not provenance-stamped — AGREE, good
catch.** `Cmd::ReceiptResponse(Option<Vec<u8>>)` carries no key/attempt. Every
attempt mints a fresh random `transfer_id`, so: sender polls key K1 →
user resumes → new attempt, new tid, new schedule K2 → the in-flight K1
response arrives, fails to open under the current tid (different AEAD key),
and is treated as an authenticated mismatch → `polls.clear()` kills K2's
legitimate schedule. In `Unconfirmed` this composes with A5 into a permanent
silent dead-end; in `Confirming` it self-heals (ConfirmTimeout re-derives the
key and restarts polls). Fix is the machine's own philosophy applied to the
courier: stamp responses with the key (or attempt) they answer, drop stale
ones. I flagged `ReceiptVerified` as un-tagged during review and dismissed it
as monotone — correct for the *verified* side, wrong for the *mismatch* side.
Their scenario is the counterexample.

## Medium risks

**Unversioned records — AGREE.** `TransferRecord` is a serde dump of live
structs with no schema version field; the params→context migration was
hand-rolled compatibility. Add a `version` field now, while there is only one
version in the wild.

**Receipt verification re-hashes the mutable source — AGREE, and it is the
elegant fix for two other findings.** `verify_receipt_against_file` hashes the
file at fetch time; if the file changed after send (N1's staging overwrite is
one way!), a VALID receipt is rejected as a mismatch → polls cleared → stuck.
The sender already computed the definitive hash during send (the `Complete`
frame's `file_hash`) — it is just never persisted. Committing that hash as a
durable fact in the record and comparing `receipt.file_hash` against it:
- removes the mutable-source false-rejection,
- removes the full-file hash from the actor entirely (A4 disappears — nothing
  left to spawn),
- makes receipt verification O(1) and restart-safe.

**Inline full-file hash in the actor — AGREE** (= A4; superseded by the
committed-hash fact above).

## Their pushbacks — all three accepted

- Core wire-name validation: agree, never disputed — my B1 is a *different
  boundary* (ContentProvider display name → Kotlin staging write), not
  sender→core traversal. B1 stands.
- Receipt-preempted-by-partial: agree, verified the resume-state gate and its
  field-bug comment during review.
- Completed-without-Started: agree, fixed in 8c5581c.

---

# Unified root cause and a diagnostic method

## Their "identity collapse" is real — it is the third deep cause

Display name / `file_name` is overloaded as UI label, staging path key,
final-file key, receipt-sidecar key (`receipt_path(dir, file_name)` — evidence
they missed), SAF overwrite key, and attribution key — while the system
already owns proper unique keys (record id, `transfer_id`) and drops them at
exactly those boundaries. N1, N2, N3, the receipt-sidecar collision, and the
savedUri attribution races are all this one decision.

## The three causes are facets of one failure

1. **Authority** (cause I): who owns the identity's lifecycle → volatile maps
   instead of the record.
2. **Fact flow** (cause II): how the identity's facts propagate → lossy
   mappings, missing inputs, uncommitted facts (the send hash), unstamped
   responses (N5).
3. **Key space** (cause III, theirs): what strings subsystems use to refer to
   the identity → file name instead of transfer identity.

The common origin: **the core's contract still assumes the CLI's world** —
one transfer at a time, a caller-owned real output directory, process
lifetime = transfer lifetime, file name unique within the dir. The app broke
every assumption (durable, concurrent, restartable, invisible staging) without
renegotiating the contract. The code knows: the `sweepStaging` comment says
verbatim "correct for the CLI (a real output dir), wrong for the app." Each
un-renegotiated assumption is where a bug cluster lives.

## The diagnostic method (how to find this class mechanically)

Five audits, each one question asked exhaustively over an enumerable surface.
Between them they would have caught essentially every finding in all three
batches:

1. **Key audit** — for every filesystem path, map key, and routing key:
   *unique per transfer? mutable? user- or peer-influenced?* Any key derived
   from `file_name`/`displayName` is a defect candidate; any key reused across
   replacement without a generation is one too.
   → N1, N2, N3, receipt-sidecar collision, C2, H2.
2. **Provenance audit** — for every input crossing an async boundary back into
   the actor/machine: *is it stamped with the attempt/key/generation it
   belongs to, and is the stamp checked?* Events: yes. ConfirmTimeout: yes.
   ReceiptResponse: no. Kotlin snapshots: seq but no generation.
   → N5, C4, H2, H3.
3. **Basis audit** — for every verification or proof: *computed against a
   committed immutable fact, or re-derived from mutable state?*
   → the receipt re-hash risk, and the committed-hash fix.
4. **Lifetime audit** — for every side effect and background work item: *what
   owns it, what cancels it, what re-establishes it after restart?* Owner must
   be the durable transfer, not a launch path or a service scope.
   → R2, R5, R6, R7, A1, multicast, sweeps.
5. **Alphabet audit** — diff the design doc's input/state/effect vocabulary
   against the implemented enums; every designed-but-missing symbol is a
   silent dead-end somewhere.
   → ReceiptMismatch (A5), C5.

These are checklist-able (grep-level enumeration + one question each) and
test-able (the stale-attempt property test is audit 2 turned into a test;
audit 1 becomes "two transfers, same name, all flows").

---

# Relationship between the three review batches

Batch 1 = my full review (A/B/C/D findings). Batch 2 = other agent's first
set (R/H). Batch 3 = other agent's second set (N/M). ~30 substantive findings
total.

## Overlap: 3 duplicates out of ~30 — the batches are largely independent

| Duplicate pair | Finding |
|---|---|
| N2 = R3 | shared receive staging cross-card publish/attribution |
| N4 = A3 | restored Confirming loses the receipt-poll duty |
| M3 = A4 | full-file hash inline in the actor |

Plus one partial: batch 1's sweep-race nit overlaps R3/R4. Everything else is
unique to its batch. The duplicates are convergence evidence: independently
re-derived findings are the ones to trust most (all three duplicated items
were verified real).

## Why so little overlap: each batch was implicitly ONE audit lens

Retro-classifying the batches against the five audits explains the
complementarity almost cleanly:

| Batch | Dominant lens | Its unique findings |
|---|---|---|
| 1 (mine) | Alphabet + trust boundary + pipeline discipline | A1, A2, A5/ReceiptMismatch, B1-B3, C1-C5 |
| 2 (theirs, first) | Lifetime/ownership | R2, R5, R6, R7, R8, H1-H3 |
| 3 (theirs, second) | Key space + basis | N1, N3, N5, M1, M2 |

No single pass carried all lenses — each reviewer swept the surface visible
through their implicit lens and saturated it. That is the strongest argument
for keeping the five audits as the explicit review framework: run all five
deliberately per review instead of hoping successive reviewers happen to bring
different implicit ones. It also predicts where residual risk lives: any
surface no lens has swept yet.

## Surfaces not yet swept (residual risk map)

- **Key audit over session/rendezvous crates**: room-id as log-routing key
  (`TransferRepository.appendCoreLog` routes to "newest transfer with that
  room id" — a name-collapse instance if a code is ever reused), mDNS token
  collisions, relay slot keys. Low expected yield (fleet-validated), not zero.
- **Provenance audit over the session layer**: path-watcher events after
  connection replacement; mDNS multi-peer loop attribution (D4 suppression
  covers the known case).
- **Alphabet audit over the other design docs**: peer-mailbox.md's kind
  registry (`cancel` v2 designed, not implemented — known deliberate
  deferral), diagnostics.md vs implementation.
- **Basis audit over auth/pairing**: invite expiry vs wall clock, pairing
  transcript binding. Not reviewed this arc.

## Cross-review refinements adopted (batch 3 feedback round)

- N1 is a **source-identity/snapshot failure, not broken transfer crypto** —
  the receiver correctly verifies the wrong stream. Phrasing corrected above.
- Unlink nuance on the Remove-deletes-shared-staging leg: an open fd keeps
  the in-flight read alive; the loss is everything path-addressed afterwards
  (resume re-hash, restore, receipt verification).
- Priority order for the fix batch (agreed): send staging + name sanitize →
  SAF uniquify → committed hash facts → response stamping → receive staging.
  Reflected in the plan's "Suggested Immediate Order" as Batch 0.
