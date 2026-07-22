# Manifest v2 Goal 0 contract

- Status: frozen for Goal 1–9 implementation
- Frozen on: 2026-07-23
- Branch base: `feat/unified-transfer` from `origin/dev@4bf42a74e49b8582bd1926a88e8a21b345edcb20`
- Runtime status: no Manifest v2 transfer engine is enabled by Goal 0

This document is the implementation contract produced by Goal 0. It replaces
Manifest v1 assumptions for the new pipeline; it does not provide a v1
compatibility or migration contract.

## 1. Authority and Issue ownership

GitHub Issue #55 owns the canonical job, Manifest v2 structural/data-plane
contract, sequential entry transfer, destination save proof, and removal of the
old single-file/Manifest split. Goal 0 records this by an Issue comment instead
of rewriting issue history.

Issue boundaries are strict:

| Issue | Owns | Explicitly not absorbed into #55 |
|---|---|---|
| #38 | Shared structured failure taxonomy and presentation | Manifest defines only the phase/entry envelope and uses #38 codes |
| #40 | Obsolete queue proposal | It is not an architectural source for #55; status is left unchanged and the supersession is recorded by comment |
| #43 | Parallel file/chunk scheduling | Manifest v2 starts sequentially and does not pre-implement parallel scheduling |
| #56 | Retry budgets, backoff, TTL, quota, and cleanup policy | #55 supplies mechanical checkpoints and durable effects only |
| #57 | Invite, Room, roles, and authenticated peer context | No payload or Manifest implementation |
| #59 | Discovery, candidate gathering, and path ranking | No payload, compression, or resume implementation |

No presentation/skeletal build is in scope. No old public wrapper, Manifest v1
wire, durable migration, downgrade, or old-client compatibility requirement is
carried forward.

## 2. Existing implementation disposition

The following is a replacement disposition, not permission to delete everything
in Goal 0. Physical deletion occurs only after the replacement is integrated
and verified.

| Area | Files/symbols | Disposition | Reason |
|---|---|---|---|
| Authentication and authenticated session context | `envoix-auth`, session channel binding, Room/discovery inputs | Keep | Manifest begins after authentication and consumes the established peer/role context |
| Manifest v1 wire | `envoix-protocol/src/manifest.rs`, `MANIFEST_V1_ALPN`, V1 frames and fixtures | Replace, then delete in Goal 9 | String IDs, joined paths, pre-hash gate, and V1 negotiation conflict with this contract |
| Single-file wire | `SINGLE_FILE_V1_ALPN`, `Frame::FileHeader` data path | Replace, then delete in Goal 9 | A one-file job must use the same Manifest v2 engine as every other shape |
| Transfer engines | `TransferEngine` and `ManifestTransferEngine` | Replace with one engine | They duplicate completion, pause, retry, and storage behavior |
| Client drivers | `api/driver.rs` and `api/manifest_driver.rs` | Replace with one reducer/driver | The Manifest driver currently drops confirmation timer/mailbox/post-receipt effects and has a different post-commit boundary |
| Session state | current `Preparing/Waiting/.../AwaitingPublication` reducer | Replace projection and reducer | It conflates connection, transfer, and save; “publication” is not product language |
| FFI | separate manifest module and action predicates | Replace with canonical job queries/actions | Native code must not choose engines or maintain a second reducer |
| Apple send/receive | source-shape engine selection and `ReceivePublication.swift` | Adapt to providers | Current custom Files flow stages in app storage and clone/copies later; that is `CopyAfterVerify`, not default `DirectSave` |
| Android receive | `MediaStoreSaver`, `PublishJournal`, SAF branch | Adapt to providers | MediaStore pending is usable; generic SAF creates a user-visible document before copy and cannot be assumed atomic |
| Local storage finalization | `LocalFileStorage::finalize_temp_file` | Replace in destination Goal 3 | Its FAT fallback is check-then-rename and is explicitly racy |
| Durable ledgers/effect discipline | receipt, resume, activity stores, confirmation mailbox concepts | Reuse concepts, replace schemas | Commit-before-effect and replay are retained; V1 identities and facts are not |
| Legacy tests | V1 codec, dual-protocol, wrapper tests | Delete with old code | They must not become compatibility requirements |

## 3. Canonical durable model

`CanonicalTransferJobV1` is version 1 of the new local durable schema. The `V1`
suffix does not mean Manifest v1 and grants no legacy compatibility.

Sender-owned record:

```text
schema_version: u16 = 1
job_id: [u8; 16]                 // non-zero random identity
selection_revision: u64
compression_policy: Never | Always | Smart
preparation: Preparing | ReadyToSeal | Sealed
roots: bounded source-root records
entries: bounded canonical inventory records
completeness: Complete | UserApprovedPartial(omitted_entry_count)
sealed_offer: optional canonical bytes + BLAKE3-256 digest
generation: u32
active_attempt: optional local AttemptId
terminal: optional Delivered | Failed | Canceled
local_source_handles: redacted provider-owned references
job_owned_artifacts: durable leases, never wire values
```

Receiver-owned active/completed record:

```text
schema_version: u16 = 1
job_id + generation
authenticated_peer_binding
sealed_manifest_bytes + structural_digest
destination_plan + decision_evidence
root_name_plan + plan_revision
proof_capability (secret, redacted)
entry arbiter/checkpoints/results
standing save effects
aggregate result bytes + proof
terminal Delivered | Failed | Canceled
```

Owners are exclusive: sender record is sender authority; receiver ledger is
receiver data-plane authority; Swift/Kotlin/UI records are projections only.
Absolute paths, URI strings, bookmarks, provider tokens, capability bytes, and
OS errors never enter wire frames or normal logs.

### Identities

| Identity | Width/scope | Rule |
|---|---|---|
| `activity_id` | local implementation | UI correlation only, never proof |
| `job_id` | 16 random bytes | Stable for the prepared job; all-zero is invalid |
| `source_item_id` | local provider value | Stable only before Seal; never sent raw |
| `root_id` | canonical `u32` | Dense from zero in root order; independent of name |
| `entry_id` | canonical `u32` | Dense from zero in deterministic parent-before-child order |
| `attempt_id` | local/session context | One physical connection attempt; not repeated in every frame |
| `generation` | `u32` | Invalidates stale events/effects; proof-bound |
| `proof_capability` | 32 random secret bytes | Receiver-created and stable across reconnects for one job/generation |

Generation starts at one. Reconnect/resume keeps it unchanged. It increments
only when the same sealed Manifest deliberately abandons a nonterminal receiver
acceptance epoch and creates a new capability epoch; a changed selection creates
a new job instead. Locally delivered effect results also carry `attempt_id`, so
an old connection cannot advance the current generation.

## 4. Structural Manifest v2 schema

The implemented reference codec is
`crates/envoix-protocol/src/manifest_v2.rs`. It uses:

```text
magic "ENV2"
version u16 = 2
frame_type u16
payload_length u32
fixed-width big-endian integers
u32 count/length-prefixed arrays and UTF-8 bytes
explicit one-byte optional/enum tags
no trailing bytes and no unknown critical tag
```

`ManifestOfferV2` is `BLAKE3-256(canonical Manifest body) || Manifest body`.
The digest is verified before decoded content is used.

The Manifest body contains:

```text
job_id, generation, selection_revision, compression_policy
roots[]: root_id, root_entry_id, requested_name, completeness
entries[]: entry_id, root_id, relative component array, kind,
           plaintext_size, Known(digest) | Deferred
declared file/directory/byte totals
```

Wire paths are exactly `root_id + count-prefixed UTF-8 component array`. They
are never absolute, platform-native, or joined path strings. The root entry has
an empty relative component array; its requested top-level name lives in the
root record. The decoder reconstructs parent relationships only from previously
seen canonical component arrays.

Forest validation requires:

- dense root and entry IDs, parent before child, and one root entry per root;
- unique canonical `(root_id, component array)` paths;
- a directory parent for every non-root entry and no child of a regular file;
- explicit empty directories;
- nonempty UTF-8 components other than `.`/`..`, with no separator or control
  character;
- exact totals with checked arithmetic;
- directory size zero and no directory digest;
- a nonzero omitted count for `UserApprovedPartial`.

Root IDs follow durable user-add order after duplicate/overlap normalization;
renaming a root does not change its ID. Within each root, entry IDs use
depth-first preorder with siblings sorted by byte-exact UTF-8 component value.
The validator rejects a different order, so provider enumeration order cannot
silently change the sealed digest.

Unicode comparison on the wire is byte-exact UTF-8. Platform-specific Unicode,
case, and forbidden-name equivalence is a destination concern; it must not
mutate or collapse sender entries.

## 5. Frozen frame family

The ALPN is only `envoix/manifest/2`. No V1 ALPN is negotiated by the completed
implementation. Frame tags are reserved in code:

| Tag | Frame | Direction and canonical payload |
|---:|---|---|
| 1 | `Offer` | S→R; structural digest + Manifest body |
| 2 | `Accept` | R→S; job/generation, Manifest digest, Accept nonce, first-send proof capability, plan revision, root name plan, per-entry Receive/Reuse plan |
| 3 | `EntryStart` | S→R; entry, effective Identity/Zstd encoding, plaintext block size |
| 4 | `EntryContentDigest` | Bidirectional; proposed set-or-equal BLAKE3 digest, then ContinuePayload/ReuseExisting decision |
| 5 | `EntryBlock` | S→R; entry, block index, plaintext offset/length, encoded length and bytes |
| 6 | `EntryComplete` | S→R; entry, final size/digest, PayloadComplete/ReuseChosen |
| 7 | `EntryResult` | R→S; entry, Saved/ReuseExisting result, final size/digest and optional final-component override |
| 8 | `JobComplete` | S→R; canonical sender entry completion-set digest |
| 9 | `DeliveryProof` | R→S; Manifest/result digests, receiver nonce, aggregate MAC |
| 10 | `ResumeRequest` | S→R first frame; immutable Offer, sender checkpoint and fresh challenge |
| 11 | `ResumeStatus` | R→S; durable entry boundaries plus the challenge MAC |
| 12 | `Cancel` | Either direction; typed scope/reason without raw OS text |
| 13 | `Error` | Either direction; #38 failure code, phase and optional entry ID |

Every frame after Offer repeats `job_id` and `generation`. `attempt_id` is bound
to the authenticated connection/session context instead of being duplicated in
every frame. Receivers reject a frame whose job/generation does not match the
durably accepted offer.

### Canonical payload layouts

The table above reserves semantic roles; the layouts below freeze their bytes.
`bytes32` and `bytes16` are fixed-width, `string` is `u32 byte_length || UTF-8`,
`array<T>` is `u32 count || T...`, and `optional<T>` is `u8(0)` or `u8(1) || T`.
All arrays use canonical ID order and all optional/enum tags reject unknown
values. `P` below is the common `job_id: bytes16 || generation: u32` prefix.

```text
Offer                 = structural_digest: bytes32 || ManifestBody
Accept                = P || manifest_digest: bytes32 || accept_nonce: bytes32
                        || proof_capability: bytes32 || plan_revision: u32
                        || array<RootPlan> || array<EntryPlan>
RootPlan              = root_id: u32 || planned_name: string
EntryPlan             = entry_id: u32 || disposition: u8
                        || next_plaintext_block: u64

EntryStart            = P || entry_id: u32 || encoding: u8
                        || plaintext_block_bytes: u32
EntryContentDigest    = P || entry_id: u32 || digest: bytes32
                        || decision: u8
EntryBlock            = P || entry_id: u32 || block_index: u64
                        || plaintext_offset: u64 || plaintext_length: u32
                        || encoded_length: u32 || encoded_bytes
EntryComplete         = P || entry_id: u32 || final_size: u64
                        || final_digest: bytes32 || completion_choice: u8
EntryResult           = P || entry_id: u32 || result: u8 || final_size: u64
                        || optional<final_digest: bytes32>
                        || optional<final_component_override: string>
JobComplete           = P || sender_completion_set_digest: bytes32

DeliveryProof         = P || manifest_digest: bytes32
                        || result_set_digest: bytes32 || proof_nonce: bytes32
                        || proof_mac: bytes32

ResumeRequest         = P || encoded_offer_length: u32 || encoded Offer
                        || accept_body_digest: bytes32
                        || sender_checkpoint_digest: bytes32
                        || challenge_nonce: bytes32
ResumeStatus          = P || accept_body_digest: bytes32
                        || plan_revision: u32 || array<ResumeEntry>
                        || challenge_nonce: bytes32 || challenge_mac: bytes32
ResumeEntry           = entry_id: u32 || arbiter: u8
                        || next_plaintext_block: u64
                        || optional<content_digest: bytes32>
                        || optional<canonical EntryResult body>

Cancel                = P || scope: u8 || optional<entry_id: u32>
                        || failure_code: u32
Error                 = P || failure_code: u32 || phase: u8
                        || optional<entry_id: u32>
```

Frozen enum tags:

| Type | Values |
|---|---|
| `EntryPlan.disposition` | `ReceivePayload=0`, `ReuseExisting=1` |
| `EntryStart.encoding` | `Identity=0`, `Zstd=1` |
| `EntryContentDigest.decision` | `Proposed=0`, `ContinuePayload=1`, `ReuseExisting=2` |
| `EntryComplete.completion_choice` | `PayloadComplete=0`, `ReuseChosen=1` |
| `EntryResult.result` | `Saved=0`, `ReusedExisting=1` |
| `ResumeEntry.arbiter` | `PayloadOpen=0`, `ReuseChosen=1`, `PayloadCompleteChosen=2` |
| `Cancel.scope` | `Job=0`, `Entry=1` |
| `Error.phase` | `Offer=0`, `Destination=1`, `Payload=2`, `Verify=3`, `Save=4`, `Proof=5` |

Directory results require `final_size=0` and no final digest; regular-file
results require a digest. `ReuseExisting` is invalid unless the receiver's
provider matrix and stable opened identity permit it. `encoded_length` must
equal the remaining frame payload bytes and stay within the encoded-block
limit; `plaintext_offset` must equal `block_index * plaintext_block_bytes`
except the last block may be shorter. Ordinary control frames are limited to
4 MiB; `ResumeRequest` is bounded to one maximum-size Offer plus 120 bytes, and
`EntryBlock` uses the encoded-block limit plus its fixed metadata.

A new connection begins with `Offer` and receives `Accept`; it never sends a
zero-filled resume sentinel. A reconnect begins with `ResumeRequest`, which
carries the immutable Offer so the receiver can validate and display it before
loading durable state. The sender persists Accept before its first payload
frame. That first durably recorded payload frame is the implicit commitment:
after it exists, the receiver rejects a plain Offer and requires an authenticated
ResumeRequest. The fresh challenge and its MAC are folded into
`ResumeRequest/ResumeStatus`.

The receiver persists DeliveryProof before sending it and replays the same proof
when a connection is lost. The sender enters Delivered only after validating and
persisting that proof; no additional proof-Ack frame is required. The sender and
receiver hash canonical Accept, checkpoint, completion, result and proof bodies
exactly as encoded, so replay equality is byte equality rather than reconstructed
object equality.

`proof_capability` is sent only inside the encrypted Accept, which may be
repeated only until the receiver has durably observed the first payload frame.
Key material
uses BLAKE3 `derive_key` with distinct exact contexts:

```text
envoix/manifest/v2/accept-challenge-key
envoix/manifest/v2/delivery-proof-key
```

The challenge and delivery transcripts also start with their distinct context
string. A capability or derived key has a redacted `Debug`/log representation.

## 6. Reducer and user decisions

Preparation and connection are orthogonal durable facts. UI state is a
projection, not the source of truth:

```text
Preparing -> ReadyToSend -> SendRequested
SendRequested -> OrganizingFiles and/or Connecting
sealed + authenticated -> Offering -> Receiving -> Verifying
-> FinalizingSave -> WaitingForReceiverSave -> Delivered
```

The sender never reports Delivered before the receiver's final user destination
has been saved and the aggregate proof is verified. The visible waiting copy is
“等待接收方保存”, not “传输中”. Receiver product copy is limited to “接收中、校验中、保存中、已接收”.

All reducer transitions follow:

1. validate job, generation, attempt, current arbiter and terminal monotonicity;
2. reduce input into a new record plus idempotent standing effects;
3. atomically commit the record before starting an effect or sending a frame;
4. execute effects outside the transaction;
5. feed typed results back through the same reducer;
6. on restart, enumerate committed standing effects and replay them.

### U1: inaccessible source decision

An inaccessible root/subtree does not block preparation of unrelated entries.
Before Send can Seal, the sender must persist exactly one decision:

- `Reauthorize` and retry the same source identity;
- `RemoveSelection` and increment `selection_revision`;
- `ApprovePartial`, recording `UserApprovedPartial(omitted_entry_count > 0)`;
- `CancelJob`.

The receiver sees an authenticated partial fact but does not reconfirm it.

### U2: destination decision

Before Accept and before payload, the receiver persists one of:

- `UseCopyAfterVerify` for this destination plan;
- `ChooseAnotherDestination` and reprobe it;
- `CancelReceive`.

No provider silently changes a DirectSave plan into an extra full copy.

## 7. T1 safety constants and space model

### Hard codec bounds

| Constant | Frozen value |
|---|---:|
| encoded Offer | 4 MiB |
| roots | 1,024 |
| entries | 10,000 |
| component UTF-8 bytes | 255 |
| relative component depth | 64 |
| logical relative path bytes | 4,096 |
| default plaintext block | 4 MiB |
| maximum plaintext block | 16 MiB |
| maximum encoded block | 16 MiB + 64 KiB |
| inventory default/max page | 128 / 512 entries |
| inventory response budget | 1 MiB |

The executable maximum-shape test builds 10,000 entries with 255-byte
components and Known digests. Its canonical Offer is 3,129,823 bytes, remains
under 4 MiB, decodes without unbounded allocation, and completed with the six
Manifest v2 tests in 0.24 seconds on the Goal 0 macOS host. This is a codec
bound check, not a claim about directory enumeration or native UI speed.

### Receiver admission

Provider-reported allocatable capacity is authoritative. Apple uses capacity
for important user-requested storage; Android uses `getAllocatableBytes`; Windows
uses bytes available to the caller. Envoix adds 64 MiB of checked operational
headroom for ledger/metadata/finalization but does not invent a percentage-based
disk reserve on top of the OS budget.

```text
DirectSave required(domain) = remaining plaintext allocation + 64 MiB

CopyAfterVerify required(staging domain) = remaining plaintext + 64 MiB
CopyAfterVerify required(target domain)  = final plaintext + 64 MiB

if staging domain == target domain:
    required = remaining plaintext + final plaintext + 64 MiB
```

Every sum is checked `u64`. Unknown capacity cannot auto-accept a Copy plan.
Compression never reduces the plaintext destination reservation.

An otherwise valid offer requires explicit exceptional-transfer confirmation
when plaintext exceeds 64 GiB or consumes more than half of the currently
reported allocatable destination bytes. This decision is made before payload;
it does not change the hard protocol limits.

## 8. T2 destination provider allowlist and evidence

`DirectSave` means verified plaintext is written into a hidden/pending object in
the final storage domain, followed by a no-overwrite metadata finalization. A
normal success path performs no second full payload copy/read. `CopyAfterVerify`
means a user-approved extra copy from verified staging. `Unsupported` means
neither guarantee is available.

| Provider | Goal 0 classification | Required evidence/gate |
|---|---|---|
| macOS path-backed local volume | DirectSave when same-volume identity and exclusive rename capability are present | Official volume capability plus local collision/success probe passed |
| Apple security-scoped local URL | Conditional DirectSave | Same volume, durable access, replacement workspace, exclusive rename capability; real iOS/macOS provider tests required in Goal 3 |
| Apple File Provider/cloud URL | CopyAfterVerify or Unsupported | No DirectSave without a provider-specific hidden/pending and no-overwrite proof |
| Android MediaStore API 29+ | DirectSave through provider-native pending object | `IS_PENDING` is owner-only until commit; physical Android test required in Goal 3 |
| Android generic SAF tree/document provider | CopyAfterVerify | Provider flags vary; create is visible and rename may return a new identity; no generic atomic/hidden guarantee |
| Windows local NTFS | Conditional DirectSave | Stable volume identity, same-volume workspace, `FileRenameInfoEx` with replace disabled, collision and crash tests on Windows |
| Windows FAT/exFAT/removable | Runtime-probed DirectSave or CopyAfterVerify | Never infer from drive letter or filesystem name; require same volume and exclusive no-overwrite rename probe |
| Linux/local CLI | Conditional DirectSave | Same `st_dev`, `renameat2(RENAME_NOREPLACE)` support and crash tests |
| Opaque provider with neither guarantee | Unsupported | User must choose another destination |

Goal 0 local probe evidence on macOS 26.5 / Darwin 25.5.0:

```text
volume_id_present=true
exclusive_rename_capability=true
collision: renamex_np(RENAME_EXCL) -> -1/EEXIST,
           existing destination unchanged, staging retained
success:   renamex_np(RENAME_EXCL) -> 0,
           destination contains staging bytes, source name removed
```

No Android device or external volume was attached during Goal 0, so MediaStore,
SAF, FAT/exFAT and Windows are not marked physically verified. They are explicit
Goal 3/4 device gates, not assumed support.

Evidence sources:

- Apple documents same-volume replacement workspaces and
  [`volumeSupportsExclusiveRenaming`](https://developer.apple.com/documentation/foundation/urlresourcevalues/volumesupportsexclusiverenaming).
- Apple documents capacity for user-requested storage in
  [Checking Volume Storage Capacity](https://developer.apple.com/documentation/foundation/checking-volume-storage-capacity).
- Android documents owner-only pending media through
  [`MediaStore.MediaColumns.IS_PENDING`](https://developer.android.com/reference/android/provider/MediaStore.MediaColumns#IS_PENDING).
- Android documents variable provider flags and SAF create/rename behavior in
  [Access documents and other files](https://developer.android.com/training/data-storage/shared/documents-files) and
  [`DocumentsContract`](https://developer.android.com/reference/android/provider/DocumentsContract).
- Android defines the preflight budget through
  [`StorageManager.getAllocatableBytes`](https://developer.android.com/reference/android/os/storage/StorageManager#getAllocatableBytes(java.util.UUID)).
- Windows documents same-volume, fail-if-exists rename through
  [`FILE_RENAME_INFORMATION`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information), and caller-visible capacity through
  [`GetDiskFreeSpaceEx`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getdiskfreespaceexa).
- Microsoft documents that FAT/exFAT do not provide NTFS hard links/journaling
  in its [filesystem comparison](https://learn.microsoft.com/en-us/windows/win32/fileio/filesystem-functionality-comparison).

## 9. T3 late digest, reuse, checkpoints, and crash ordering

Each file entry has one durable arbiter:

```text
PayloadOpen -> ReuseChosen | PayloadCompleteChosen
```

Rules:

1. `Known` digest may choose `ReuseExisting` before payload only when the
   destination holds a stable opened object identity.
2. `Deferred` starts payload immediately. The sender proposes the first
   `EntryContentDigest`; the receiver commits it set-or-equal and replies with
   the same frame carrying `ContinuePayload` or `ReuseExisting`. Exact duplicate
   proposals replay the committed decision; a different value fails.
3. Receiver comparison and payload may race. Reuse wins only while the arbiter
   is `PayloadOpen`; sender stops at the next complete compression-block boundary.
4. A complete block is decode-verified, BLAKE3-fed, flushed according to the
   provider contract, then its checkpoint is committed. Partial block bytes are
   never resumable.
5. `ReuseChosen` retires all payload checkpoints for that entry. Payload frames
   after the committed choice are rejected as stale; duplicate digest/complete
   frames with identical bytes replay the prior response.
6. `PayloadCompleteChosen` cannot later become reuse. A final size/digest
   mismatch retires that entry's payload checkpoints before failure.
7. Reconnect repeats `EntryStart` with the same effective encoding and block
   size. `ResumeStatus` returns only the next complete block and durable arbiter.
8. Ack loss replays the identical committed result/proof. A changed capability,
   digest, root name plan, result set, or generation fails closed.
9. Network EOF does not cancel `FinalizingSave`; save is a receiver standing
   effect. An unknown provider result is probed for exact identity/adoption only
   under Issue #56 policy.

Encoding is `Identity=0` or independent `Zstd=1` frames. `Never`, `Always`, and
`Smart` are sealed user policy; the authenticated `EntryStart` freezes the
effective encoding. Network readiness never waits for whole-job precompression.

## 10. Inventory and destination naming

FFI exposes only bounded queries:

```text
list_roots(job_id)
list_children(job_id, parent_item_id, cursor, limit)
get_item(job_id, item_id)
inventory_summary(job_id)
```

Multiple folders, mixed files/folders, repeated Add Folder, Photos and Share
items all become roots of one job. Overlap normalization uses source identity;
content hashes never collapse logically distinct paths.

The receiver serially allocates all final top-level names once before Accept.
Same-name roots become `Photos`, `Photos (1)`, and so on. Directories are never
merged. Internal final-component overrides are recorded lazily only for actual
provider illegality/equivalence; there is no whole-tree sparse name mapping.
Late external collision retains staging and returns typed destination contention
instead of moving to one final name and renaming again.

Content actions remain unavailable before Delivered. Afterwards the app may
offer read-only internal preview plus platform edit/share/move/open actions.

## 11. Acceptance-to-evidence map and handoff

| Contract | Goal 0 evidence | Later physical gate |
|---|---|---|
| One canonical structural Manifest | Public V2 types, bounded codec and golden fixture | Goal 1 source adapters; Goal 2 engine |
| No joined/absolute wire path | Component-array codec and negative validation | Fuzz corpus in Goal 2 |
| Bounded decode | Count/length checks before allocation; maximum-shape test | Fuzz/sanitizer runs in Goal 2 |
| Partial source decision | Durable U1 schema and reducer decision point | UI/source-provider tests in Goal 1/6 |
| Explicit CopyAfterVerify | Durable U2 schema and provider matrix | Native UI/device tests in Goal 3/6 |
| Receiver actually saves | Saved/Delivered facts and proof order | Crash/Ack-loss tests in Goal 4/5 |
| No overwrite | macOS exclusive rename collision probe | Android/Windows/removable gates in Goal 3/4 |
| No v1 fallback | Separate ALPN/tag family; deletion disposition | Physical deletion and audit in Goal 9 |
| Issue boundaries | Ownership table and #55/#40 comments | Enforced during every later goal review |

Goal 1 may implement local preparation, durable `CanonicalTransferJobV1`, source
providers, pagination and Seal against this document. It must not implement the
network data plane, destination finalization, delivery proof, native integration,
compression, or Issue #56 retry policy.

Any later evidence that changes user-visible waiting, automatic acceptance,
extra storage, security guarantees, or platform support requires a separately
approved Goal 0 amendment before implementation continues.
