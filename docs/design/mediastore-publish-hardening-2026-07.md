# MediaStore publish — remaining hardening (follow-up to `dbd516c`)

**Status:** known gap, low surface. The crash is fixed; the *class* is not fully closed.
**Area:** `android/app/.../MediaStoreSaver.kt` (`reserveInDownloads`, `commit`) and its
caller `TransferService.publishOne`. The SAF path (`reserveInTree`) is unaffected — it
uniquifies and creates atomically.

## What `dbd516c` already did (real, keep)
- `commit()` returns `Result` instead of throwing, so a `UNIQUE(files._data)` at un-pend
  can no longer crash the receive service. This also removed a real asymmetry: `copyInto()`
  was already `Result`-based; `commit()` was not.
- `reserveInDownloads` best-effort uniquifies the display name via `uniqueDownloadName()`
  (queries MediaStore, mirrors the SAF path → "name (1)", …), so the common "receive the
  same name twice" case publishes cleanly.

## The residual gap (small, contained)
The uniquify is a *pre-check*, and its failure degrades to a different bug rather than a
closed class. It can miss in three narrow cases:

1. **Query-format mismatch** — the `RELATIVE_PATH = ? AND DISPLAY_NAME = ?` query must match
   MediaStore's stored (normalized) form. Believed correct (trailing-slash `RELATIVE_PATH`),
   but **never verified on a device**. The original bug came from an unverified assumption
   about MediaStore behaviour; this replaces it with another.
2. **Orphaned on-disk file** — a file at the target path with no MediaStore row: the query
   finds nothing, so no uniquify.
3. **TOCTOU** — check-then-insert is not atomic; two concurrent same-name receives can both
   pick "name (1)".

**Failure mode when the pre-check misses:** no crash (good), but `publishOne` drops the
pending target and leaves the file in staging. The next sweep hits the *same* collision and
fails identically → the file is **received-but-permanently-invisible**: the state machine
shows `Completed`, the user is told it arrived, but it never lands in Downloads. A `tl(
"failed", outcome="commit")` breadcrumb is emitted, but there is no user-facing signal.

## Proposed real hardening
Make the publish path **converge** instead of pre-checking and hoping:

- On a `commit()` `UNIQUE` failure, **bump the pending row's `DISPLAY_NAME`** (`update` the
  still-pending item — no re-copy of bytes) and **retry the un-pend**, bounded (e.g. up to
  ~100 or a `(timestamp)` fallback, matching `uniqueName`).
- The pre-query (`uniqueDownloadName`) then becomes a cheap optimization for the common case,
  not the thing correctness depends on. This closes the orphaned-file, format-mismatch, and
  TOCTOU cases with one mechanism, and guarantees forward progress (always publishes under
  *some* unique name).

## Verification owed
- On-emulator **double-receive**: send the same filename twice, confirm the second lands as
  "name (1)" in Downloads and the app does not crash. This also proves the query format in (1).
- (Optional) orphaned-file case: drop a file at the target path with no row, receive the same
  name, confirm it still publishes.

## Surface assessment
Small: one failure branch, one method, narrow trigger. Not urgent (no crash, no data
corruption — a received file's bytes are verified and retained in staging). Worth doing as a
focused follow-up commit on top of `dbd516c`, not a rush.
