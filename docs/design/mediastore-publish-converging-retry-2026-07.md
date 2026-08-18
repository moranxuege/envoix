# Design: converging + durable MediaStore publish (close the collision liveness gap)

**Status:** design (rev 2, review incorporated), not implemented.
**Follows:** `dbd516c` (crash fix) and [`mediastore-publish-hardening-2026-07.md`](./mediastore-publish-hardening-2026-07.md).
**Class:** **P1 correctness/liveness — not data corruption.** The received bytes stay safe in private staging and are SHA-verified, but the Rust machine is already `Completed` while the user never sees the public file, and a restart hits the *same* collision, so it cannot self-converge.
**Area:** `MediaStoreSaver.kt`, `TransferService.{publishOne,adopt,writePublishJournal,onSnapshot}`, `Transfer` card, and the Rust `AndroidPlatformExtras` DTO.

## Scope (deliberately narrow)
This closes the **name-collision** publish failure and gives it **forward progress + a user-visible outcome**. It does **not** claim to make the whole publish path crash-safe: `writePublishJournal` swallows its write error and `syncExtras` doesn't prove the record durably committed before staging is deleted — the pre-existing **publication barrier** (batch-1 review). That barrier is out of scope except where this fix directly touches it (recording the published name durably, and not deleting staging on an unproven write). Do not advertise "publication is now crash-safe."

## Two mechanisms
1. **Converging retry at `commit`** — resolves a collision *in-line* (bump the pending name, re-commit) within one `publishOne` call. Handles the common case.
2. **Durable publication duty** — for the *residual* cases (genuine transient errors, exhaustion) it provides re-drive + a terminal, user-visible state, instead of silently leaving the file in staging.

---

## 1. `commit` — bounded converging retry (fixes points 3, 5b, 5c)
`Reserved` carries the name so `commit` can bump it; `commit` returns the **final** name:
```
data class Reserved(val uri: Uri, val mediaStorePending: Boolean, val displayName: String)
fun commit(context, target): Result<PublishOutcome>   // PublishOutcome(uri, finalName)
```
Loop over `nameSequence(target.displayName)`:
- From the 2nd candidate on, **rename the pending row**: `update(uri, DISPLAY_NAME=candidate)` — **and require it affected exactly 1 row** (point 3: a `0` return means the row is gone → fail, do not treat as success).
- **Un-pend**: `update(uri, IS_PENDING=0)` — **again require rows-affected == 1.**
- Classify the outcome:
  - rows == 1 → success, return `(uri, candidate)`.
  - threw, and the cause chain contains a **UNIQUE** constraint violation → collision, advance to the next candidate. *(point 5b: `SQLiteConstraintException` is often wrapped by the provider/Binder — walk `.cause` and match specifically `SQLITE_CONSTRAINT_UNIQUE` / a `UNIQUE`-worded message, not any constraint.)*
  - rows == 0, or any non-UNIQUE error → **fail immediately** (don't loop on IO / provider-dead / NOT NULL).
- `nameSequence(base)` = `base`, `base (1)` … `base (99)`, then **random-suffix** candidates (point 5c: a timestamp isn't unique under clock rollback/concurrency — keep generating, stay commit-driven, so uniqueness is proven by a *successful* commit, never assumed). It's effectively unbounded but each step is gated by a real collision, so it terminates the instant one lands.

**Drop `uniqueDownloadName`** and the pre-query entirely — proven to miss (it can't see pending/orphaned rows), and the retry subsumes it at zero cost in the no-collision case.

## 2. `adopt` — split identity name from published name (fixes point 1)
Today `adopt` no-ops the `savedUri` update unless `fileName == name`; a bumped `data (1).bin` vs a card `data.bin` silently drops the URI. Split the two concepts:
```
adopt(id, expectedSourceName = src.name, publishedName = finalName, uri = outcome.uri)
```
- `fileName` = **transfer identity / original name** — the match/guard key; never overwritten by the published name.
- `publishedName` = **platform display name** — a new `Transfer` field, set alongside `savedUri`.
- The guard stays `fileName == null || fileName == expectedSourceName`, but on match it sets `savedUri` **and** `publishedName = finalName`.

## 3. Journal + durability (fixes points 1, 4)
- Record the **published name** too, so crash-recovery knows which file was adopted:
  `{ "target": …, "pending": …, "published_uri": …, "published_name": "data (1).bin" }`
- **Do not swallow the journal write.** If the pre-delete journal/extras write fails, **do not delete staging** — leave it and mark the publish duty pending (below), so recovery/retry still has the bytes. (Full barrier hardening remains the separate batch-1 item.)
- Recovery in `publishOne` adopts using the journal's `published_name`, not `src.name`.

## 4. Durable publication duty — real forward progress (fixes point 2)
Leaving the file in staging and returning is **not** forward progress: nothing re-drives it, and `onSnapshot`'s `completed` block only re-fires on restart. Add a narrow duty, **in `platform_extras`, not the Rust reducer, no outbox**:
- Extend the typed **`AndroidPlatformExtras`** DTO (`crates/envoix-ffi/src/android_jni/manifest_v2.rs`, `deny_unknown_fields`) with `published_name: Option<String>` and `publish: Option<String>` (`"failed"` after give-up; absent = not-yet/пending). Without this, `setSessionExtras` rejects the new keys.
- **Pending signal** = the staging file still exists (publishOne deletes it only on success) and `publish != "failed"`.
- **Driver:** the `completed && Receive` branch of `onSnapshot` already calls `sweepStaging`; keep it, plus a **small bounded in-session re-attempt** (a delayed coroutine, e.g. a few tries with backoff) so a transient failure doesn't wait for a restart. On restart the same branch re-fires and retries — eventual convergence.
- **Terminal state:** after the bounded attempts fail on a *non-collision* error, set durable `publish = "failed"` and surface it in the UI (card reads "Received — couldn't save to Downloads · Retry"), so it's never silently invisible. A user "Retry" clears the flag and re-drives.
- Success path sets `published_name` + `saved_uri` in extras and deletes staging.

## 5. SAF path (fixes point 5a)
`reserveInTree` already uniquifies via `uniqueName()`, so the actual name is `doc.name`, not the requested one. `Reserved.displayName` for the SAF branch must be **`doc.name`** (the created document's real name), and SAF `commit` is a no-op that returns `PublishOutcome(uri, doc.name)` — so the record shows the true SAF name.

---

## Edge cases
- `update` returns 0 (row deleted mid-flight) → failure, staging kept.
- Rename (`DISPLAY_NAME`) itself throws UNIQUE → treat as this candidate failing, advance.
- Non-UNIQUE / wrapped-non-constraint error → fail fast → publish duty (retry later + user-visible).
- Exhaustion → practically impossible (random-suffix tail), but falls through to the duty, never a silent drop.
- SAF unaffected (no pending/commit).

## Testability & verification
- **Pure:** `nameSequence` and the cause-chain UNIQUE matcher are pure → unit tests (base with/without extension; wrapped-vs-bare-vs-non-UNIQUE exceptions).
- **On-emulator (main risk = MediaStore rename-pending-then-unpend):** receive `nat-test-input` 3× → `nat-test-input`, `nat-test-input (1)`, `nat-test-input (2)` all land in Downloads, no crash, each card gets `savedUri` + `publishedName`. If MediaStore rejects renaming a pending row, fall back to delete-pending + re-reserve under the new name (costs a re-copy — the less-preferred path); the verification decides which.
- **Restart convergence:** force a transient failure, kill the app → on relaunch the `completed` render retries and lands (note: harder to automate).

## Change list (implementable)
| File | Change |
|---|---|
| `MediaStoreSaver.kt` | `Reserved.displayName` (SAF = `doc.name`); drop `uniqueDownloadName`; `commit` → converging retry, rows-affected checks, cause-chain UNIQUE match, random-suffix tail, returns `PublishOutcome(uri, finalName)` |
| `Transfer.kt` | + `publishedName: String?` |
| `TransferService.kt` | `adopt` split (source name vs published name); journal records `published_name` + no swallow / no-delete-on-failed-write; `publishOne` uses `finalName`; `onSnapshot` drives the publish duty + bounded in-session retry; surface `publish=failed` |
| `crates/envoix-ffi/src/android_jni/manifest_v2.rs` | `AndroidPlatformExtras` += `published_name`, `publish` (keeps `deny_unknown_fields`) |
| tests | `nameSequence` + UNIQUE-matcher unit tests; on-emulator thrice-receive |

## Non-goals
Full publish-barrier crash-safety (swallowed journal writes, proving extras durability before staging deletion) — the batch-1 publication barrier — stays a separate, larger change.
