# Android publication evidence for receipt re-verification

**Status:** Stage 1 implemented; JVM/JNI/build verified, device validation pending.
**Scope:** Android receive publication and manual re-verification. This does not
change the JNI session transport, Room transport, or Wi-Fi Aware work.

## Problem statement

Android deliberately receives into a per-activity private directory before it
publishes the verified file through MediaStore/SAF. The core keeps a completion
receipt in that private directory after publication. This is useful for
re-delivering a lost completion acknowledgement without sending the bytes again.

Those two facts are currently conflated:

- A **completion receipt** proves that a particular transfer completed with a
  particular size and BLAKE3 hash.
- A **published-file record** must prove that the user-visible copy still exists
  and still contains those bytes.

Today `ACTION_REVERIFY` restores the session and immediately sends the `reverify`
intent. It does not validate `savedUri` first. If the user deletes or replaces the
public file but keeps the old activity, the private receipt can re-confirm the old
delivery even though Downloads no longer contains the promised artifact.

## Current behavior to preserve

1. A new receive activity owns `filesDir/incoming/<activity-id>/`; activities do
   not share partial files or receipts.
2. A fresh activity starts with `resume = false`. The transfer core ignores an
   old receipt and receives real bytes under a collision-safe name.
3. A completed staging file is published using reserve -> copy -> commit. The
   staging copy is deleted only after publication; the receipt sidecar remains.
4. Repeating a fresh file transfer reuses the exact public name only after the
   public bytes match the finalized staging file. Missing or changed public
   files are published normally under an extension-safe collision name such as
   `photo (1).jpg`.

## Required invariant

> A receipt may skip network bytes only when the public artifact associated with
> that activity still resolves and its content matches the publication evidence.

The receipt remains the protocol proof. It is never, by itself, proof that the
public copy is still available.

## Publication evidence

Persist the following optional fields beside `saved_uri` in Android platform
extras and the publication journal:

- `published_size`: byte length of the staged final.
- `published_sha256`: lowercase SHA-256 of bytes copied into the public target.
- `published_name`: existing final display name.

SHA-256 is a platform-publication checksum, not a replacement for the core's
BLAKE3 transfer hash. It can be computed while MediaStore/SAF copy reads the
verified staging file, so the normal publish path needs no additional full-file
read. Old records without evidence remain readable, but must not use the public
artifact fast path until evidence is rebuilt by a real receive/publication.

MediaStore generation/version values may later be stored as a fast invalidation
hint. They are not content proof and must not replace the checksum, especially for
SAF providers that expose no equivalent generation contract.

## Re-verification algorithm

Before `Native.sessionIntent(id, "reverify")`, run this work on an IO coroutine:

1. Require a parseable `savedUri`, `published_size`, and `published_sha256`.
2. Open the URI through `ContentResolver`; failure/null means the public file is
   missing.
3. Compare the provider-reported or streamed byte count with `published_size`.
4. Stream SHA-256 from the public URI and compare it in constant-time with
   `published_sha256`.
5. Only on a full match send `reverify` to the Rust session.
6. On missing/mismatch, invalidate `savedUri` and its evidence, persist
   `publication_invalid`, and require a new receive activity. The private receipt
   remains quarantined with the old activity until that activity is removed, but
   every future re-verification is blocked. Do not report the old delivery as
   available.

For the first implementation, a manual re-verification may read the whole public
file. This is intentionally conservative and occurs only on an exceptional
lost-ack/retry path. A later MediaStore generation fast path may avoid most reads
without weakening the fallback.

## Why size-only validation is insufficient

Checking that the URI opens and has the expected length fixes the common
"deleted file" symptom but does not establish content equality: a user or another
app can replace a file with different bytes of the same length. The UI and logs
must therefore say "exists" for an existence-only check, never "identical".

## Relationship to mature designs

- [LocalSend protocol](https://github.com/localsend/protocol) includes a nullable
  SHA-256 in file metadata and permits a receiver to answer that upload is not
  needed. The key lesson is that skipping is based on content metadata, not a
  filename or a stale UI record.
- [Syncthing Block Exchange Protocol](https://docs.syncthing.net/specs/bep-v1.html)
  describes files as hashed blocks; its
  [syncing model](https://docs.syncthing.net/users/syncing) writes temporary data
  and validates it before final placement. The reusable principle is staged
  ownership plus hash-backed identity.
- [rsync's transfer process](https://rsync.samba.org/how-rsync-works.html) separates
  its quick file-selection checks from checksum-based delta/verification and
  normally writes a temporary destination before renaming it. Cheap metadata is
  an optimization, not final evidence.
- [tus resumable upload protocol](https://tus.io/protocols/resumable-upload) asks
  the server for its authoritative committed offset before continuing. A durable
  resume URL identifies state; it does not prove that arbitrary external storage
  still owns the promised content.
- Android's [shared media storage guidance](https://developer.android.com/training/data-storage/shared/media)
  uses pending publication to hide incomplete MediaStore items. Android also
  exposes [MediaStore generation columns](https://developer.android.com/reference/android/provider/MediaStore.MediaColumns)
  as change indicators, while access should still be proven by opening the URI
  through [ContentResolver](https://developer.android.com/reference/android/content/ContentResolver).

## Implementation stages

### Stage 1: evidence and strict re-verification (implemented)

- Make MediaStore/SAF copy return `(size, sha256)`.
- Persist the evidence in the publication journal, `Transfer`, restore context,
  and typed `AndroidPlatformExtras`.
- Validate the URI and checksum before serving `reverify`.
- Keep the current per-activity staging, collision-safe publishing, and JNI
  session entry point unchanged.

### Stage 2: optional fast invalidation

- Persist MediaStore volume version/item generation when available.
- If the generation evidence is unchanged, allow the stored checksum proof;
  otherwise stream and re-hash.
- SAF and unknown providers always use the checksum fallback.

### Stage 3: pre-transfer cross-platform deduplication

Android now converges identical public artifacts after receipt, using name and
size only to select a candidate and SHA-256 as the final proof. Avoiding the
network transfer itself still requires content identity in the cross-platform
prepare/header protocol. Current fresh transfer IDs are independent, so the
platform must never serve another activity's private receipt as a shortcut.

## Verification matrix

1. **Fresh same-name, same-content transfer:** receive twice; both activities
   remain independent, but the second publication reuses the proven public file.
2. **Fresh same-name, changed-content transfer:** receive real bytes and publish
   an extension-safe collision name such as `name (1).jpg`.
3. **Lost acknowledgement, intact public file:** old card re-verifies without
   retransmitting bytes.
4. **Public file deleted:** old card must not serve its receipt or report success.
5. **Public file replaced at the same size:** checksum mismatch must prevent the
   receipt shortcut.
6. **App restart:** `savedUri` and publication evidence restore together and the
   preceding outcomes still hold.
7. **Publish crash recovery:** committed journal evidence is adopted before the
   staging file is deleted; incomplete publication keeps staging recoverable.

Instrumentation tests are required for MediaStore/SAF behavior. Pure JVM tests
should cover evidence serialization, missing-field compatibility, checksum
comparison, and the re-verification decision table.

## Verification completed

- `cargo test -p envoix-android-jni`: 16 passed.
- `:app:ktlintCheck :app:testDebugUnitTest`: passed.
- arm64 release JNI library rebuilt and staged into `jniLibs`.
- `:app:assembleDebug`: passed and packaged `lib/arm64-v8a/libenvoix_jni.so`.

The public-file deletion/replacement matrix above remains a real-device test; a
JVM test cannot reproduce ContentResolver and MediaStore ownership semantics.
