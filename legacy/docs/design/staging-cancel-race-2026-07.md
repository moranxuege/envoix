# Design: make concurrent staging unrepresentable (staging cancel/retry)

**Status:** design **rev 2** — reworked after the Codex review; NOT to implement until signed off.
**Addresses:** PR #49 review finding #1. Touches `envoix-client/src/api/machine.rs` + `driver.rs` (co-owned with Sun) and `TransferService`.
**Rev-1 verdict (accepted):** the first design *moved* the race — `cancelAndJoin` ordering + a `staged` origin flag + imperative cancel from the action path — instead of eliminating it. This rev makes two staging generations, and a source that isn't ready, structurally unrepresentable.

## Core invariant
> A committed **`Preparing(generation)`** snapshot is the *sole* authority for a staging worker. Only a **`StageComplete` stamped with the current generation** marks the source ready. Any staging input from another generation is dropped in the reducer.

Everything below serves that invariant. Generation = the machine's existing **`attempt`** (mirrors `Input::Event { attempt, .. }`), incremented before *every* retry, including a retry back into `Preparing`.

## Design

### 1. Generation-stamped staging inputs — the structural fix (was P0)
Today `StageProgress`/`StageComplete`/`StageFailed` carry only the card id, so a stale `StageComplete` from a cancelled copy can land on a fresh `Preparing` and launch `StartAttempt` while the new copy is writing. Fix: stamp them and reject stale ones exactly like attempt events.
- `Input::StageProgress { generation, bytes }`, `StageComplete { generation }`, `StageFailed { generation, reason }`.
- Reducer, first line of each arm: `if generation != self.attempt { return Vec::new() }` (before the `state == Preparing` guard).
- JNI/driver thread the generation: `Native.stageComplete(id, generation)` → `Input::StageComplete { generation }`. The Kotlin worker stamps callbacks with the `attempt` it was authorized by (read from its `Preparing` snapshot).

### 2. `source_ready` durable fact — readiness, not origin (was P1 #2)
Replace the rev-1 `staged` (true forever, re-copies a complete source, and leaves the `Failed→Resume` bypass) with a readiness fact:
- **Direct send** (created in `Connecting`): `source_ready = true`.
- **`start_staging`** (created in `Preparing`): `false`.
- **Accepted `StageComplete`**: `true`.
- **Cancel/`StageFailed` during staging**: stays `false` (never set true).
- **`on_resume` (any retry)** decides on `source_ready`, not on `state`:
  - `source_ready == false` → `State::Preparing`, `attempt += 1`, **no `StartAttempt`** (Kotlin re-stages under the new generation; `StageComplete{new}` then launches attempt 1 as today).
  - `source_ready == true` → `State::Connecting` + `StartAttempt` (bump `attempt`), unchanged.

This closes the `Preparing → StageFailed → Failed → Resume → StartAttempt` bypass (Failed with `source_ready == false` re-stages), and **preserves a complete staged source** when cancel happens during `Connecting`/`Transferring` (`source_ready == true` → no re-copy). D1 "fresh" governs the *wire* attempt and peer partials, not re-copying an immutable, already-complete sender source.

### 3. Snapshot-derived worker lifecycle — no cross-barrier cancel (was P1 #3)
Staging start *and* stop are rendered from committed snapshots in both directions (matching the existing "platform effects derive from the observed snapshot" rule):
- A committed `Preparing(generation)` snapshot ⇒ ensure exactly that worker exists (start it; retire any worker of a different generation).
- Any committed **non-`Preparing`** snapshot ⇒ retire the worker.
- `ACTION_CANCEL` sends **only** `Native.sessionIntent(id, "cancel")`. It does not touch the job. The worker retires when the `Cancelled` snapshot commits — so a cancel that fails to commit never retires platform work, and restore/storage-failure/future transitions get worker teardown for free.

### 4. Owner-checked `StageWork`, stream-closing cancellation (was P2 #5)
- One entry per id: **`StageWork(generation, job, streams)`**, replacing the separate `stagingJobs` + `stagingStarted`. All start/retire/guard decisions go through it, **owner-checked** so an old generation's teardown can never remove or clear a newer generation's entry.
- The worker **owns its open input/output streams**; retiring **closes them first** (this is what actually unblocks a provider-backed blocking `read`/`write` — `cancelAndJoin` alone can wait arbitrarily long), then cancels + joins. `ensureActive()` per chunk stays as a secondary check and to bound bytes.

### 5. Explicit partial cleanup — GC is only a post-*removal* backstop (was P2 #6)
Cancel keeps the durable record, and `gcStaging()` preserves send dirs whose ids are still in the record set — so a cancelled staged partial is **not** an orphan and startup GC will **not** reap it (the rev-1 claim was wrong). So: when an unready worker retires (cancel or re-stage), **explicitly delete/replace the partial `spec.path`** after the old worker has closed its streams and joined. On restore of a cancelled, unready staged send, delete/replace the stale partial **before** any retry. GC remains a backstop only after record removal (Remove).

### 6. Record migration for `source_ready` (was P1 #4)
`source_ready` is a durable machine fact → a record-schema change. A bare `#[serde(default)]` (⇒ `false`) would misclassify every old direct/completed record as not-ready and wrongly re-stage/fail. Version-gate it (`RECORD_VERSION` already exists; old records load as `version = 0`):
- On load of a pre-`source_ready` record, derive it from the persisted state:
  - state `Preparing` → `false` (must re-stage).
  - any state past staging (direct sends, and staged sends already in Connecting/Transferring/Paused/Unconfirmed/Completed/Failed with a proven complete source) → `true`.
  - **`Cancelled` staged** is ambiguous (we can't tell mid-staging cancel from post-staging cancel in an old record) → conservatively `false` (re-stage; if `source_recoverable == false`, retry surfaces "source needs re-picking"). Never silently assume ready.
- Bump `RECORD_VERSION`; new saves write `source_ready` explicitly.

## Recommended-shape mapping (from the review)
1. durable `source_ready` — §2. 2. bump generation before every retry incl. into `Preparing` — §1/§2. 3. stamp Stage{Progress,Complete,Failed} — §1. 4. reject stale in the reducer — §1. 5. worker start/stop from committed snapshots only — §3. 6. one owner-checked `StageWork(id, generation, job, streams)` — §4. 7. close streams → join → then delete/replace partial — §4/§5. 8. preserve a complete staged source after active-transfer cancel — §2. 9. specify+test migration before implementation — §6.

## Deterministic tests (must pass before merge)
Machine (`machine.rs`, pure):
- Old `StageComplete{gen=1}` after cancel+retry (now `attempt=2`) is ignored.
- Old `StageFailed{gen=1}` cannot fail generation 2.
- `Preparing → StageFailed → Failed → Resume` returns to `Preparing` with `source_ready == false` (no `StartAttempt`).
- `Cancel` during `Connecting`/`Transferring` then `Resume`: `source_ready == true` → `Connecting` + `StartAttempt`, no re-copy.
- Migration: legacy `Preparing` → `false`; legacy direct/past-staging → `true`; legacy `Cancelled` staged → `false`.

Platform (instrumented / harness where unit tests can't reach):
- A failed `Cancel` record commit does not cancel platform staging.
- The next generation's worker cannot open `spec.path` until the previous worker has closed streams + joined.
- Cancellation closes a deliberately-blocked source stream (bounded stop time, not just bounded bytes).
- Restore of a cancelled, unready staged send deletes/replaces the stale partial before retry.
- On-emulator smoke: large source, cancel mid-stage, immediate retry → received SHA equals source (never truncated).

## Open questions
- **Generation = `attempt`** vs a dedicated `staging_generation`. `attempt` is minimal and mirrors `Input::Event`; a dedicated counter is cleaner if we later want staging generations independent of wire attempts. Recommend `attempt`.
- **`RECORD_VERSION` bump** — confirm no other in-flight schema change collides.

## Scope / non-goals
Only the staging cancel/retry correctness + its record migration. Not the broader publication/journal work. The `machine.rs`/`driver.rs` changes are shared — this doc (rev 2) is the review gate; implementation waits for sign-off.
