## Title

Transfer Manifest v1: Multi-File, Directory, and Safe Receive Semantics

## Problem

Envoix currently behaves like a single-file transfer system.

That model is not enough for multi-file transfer, directory transfer, per-file progress, conflict reporting, safe receive paths, or future parallel transfer. Without a manifest, each new feature has to infer transfer structure from local UI state or ad-hoc protocol behavior.

The protocol needs an explicit description of what is being transferred before bytes are written.

## Goal

Introduce `ManifestV1` as the protocol-level description of a transfer set.

The manifest should define:

- which files are included;
- where each file should be placed relative to the selected receive directory;
- each file's size and integrity metadata;
- how name conflicts are handled;
- how progress and final results are reported.

The design direction is that a single-file transfer can eventually be represented as a manifest with one file entry. However, implementation should initially keep the existing single-file protocol as a compatibility path until `ManifestV1` is stable.

## Implementation Status

The additive contract, wire-codec, and core-engine slices are implemented as of
2026-07-14 in `envoix-protocol` and `envoix-transfer`:

- exact `envoix/1` and `envoix/manifest/1` ALPN constants and transfer-shape
  selection distinguish `single_file_v1` from `manifest_v1` without fallback;
- versioned manifest, entry, kind, hash-algorithm, and identifier types are
  serializable without changing the existing single-file frame family;
- structural validation enforces the named encoded-size, entry-count, UTF-8
  path, component, and depth limits with checked aggregate-byte arithmetic;
- validation rejects unsafe/duplicate paths, unstable entry IDs, missing or
  file-valued parents, invalid file/directory metadata, and mismatched declared
  counts before a future receiver performs writes;
- the independent Manifest frame family freezes IDs 16 through 26 for hello,
  offer/accept, sequential entry transfer/resume/completion, aggregate
  completion, and typed errors. Its codec round-trips all frame variants,
  revalidates hostile decoded manifests, rejects cross-family frames, and has a
  borrowed chunk writer that avoids an extra payload copy;
- the existing public single-file `Frame` enum and `FrameConnection` trait are
  unchanged, and their established frame IDs 1 through 9 are fixture-tested;
- the independent sequential engine preflights every source file, negotiates a
  complete receiver-owned conflict map, preserves top-level directory roots by
  renaming them as a unit, skips identical files without payload, stages active
  entries under a symlink-safe private directory, resumes verified prefixes,
  and exclusively commits hash-verified files without overwriting;
- the receiver persists its accepted path map by manifest ID, so a resumed
  directory transfer reuses the same claimed root instead of creating a new
  suffix on every attempt. Already committed identical entries become skips;
- existing single-file session entry points still advertise only `envoix/1`;
  additive negotiated receive entry points advertise both ALPNs, retain the
  selected protocol on the iroh connection, authenticate with the unchanged
  `Frame::Auth` handshake, and only then route to the matching transfer engine;
- Manifest manual/direct send entry points request only
  `envoix/manifest/1`. A legacy single-file receiver rejects that ALPN before
  authentication or payload writes and the sender reports the stable
  `manifest.unsupported_peer` diagnostic instead of falling back;
- the existing mDNS discovery loop now has additive Manifest send and
  negotiated receive entry points. Its advertisement accepts both ALPNs while
  the legacy mDNS functions retain their signatures and single-file behavior;
- the existing rendezvous room pairing is shared by additive Manifest send and
  negotiated receive entry points; broker matching, sealed descriptor exchange,
  derived data-plane token authentication, and legacy single-file APIs are
  unchanged;
- real iroh tests prove Manifest directory/multi-file routing, old single-file
  compatibility on the same dual endpoint, pre-engine authentication failure,
  legacy-peer ALPN rejection, real mDNS discovery into Manifest, and an old
  single-file mDNS sender reaching the new negotiated receiver. A loopback
  rendezvous test additionally proves room pairing followed by a Manifest
  directory/file transfer;
- the additive client facade exposes `Client::send_manifest`, negotiated
  `Client::receive_transfer`, a `TransferSet` handle, aggregate/per-entry
  lifecycle events, and typed single-file-or-Manifest summaries without
  changing `TransferRequest`, `Client::send`, `Client::receive`, `Client::run`,
  or `Transfer::wait`.

The engine is exercised both over an in-memory full-duplex connection and the
additive manual/direct, mDNS, and room iroh session paths. The Rust client facade
now selects all three paths and has a real multi-file/directory loopback test.
As of 2026-07-15, the Apple app also has durable Manifest Activity/FFI
projection, multi-file and folder selection, Manifest preparation cancellation,
multi-root receive publication, and Manifest-aware Activity details. One
regular file deliberately remains on the compatible single-file path. Generic
iOS build/build-for-testing and macOS hosted tests pass. A physical iPhone 15
Pro Max has also sent one folder containing a regular file and an empty
directory plus one loose file to the production macOS `AppModel`: both
canonical Activities completed over Direct IPv6, and the receiver verified 2
roots, 2 files, 2 directories, 63 bytes, the final tree, exact payload bytes,
and both SHA-256 values. A paired physical reverse gate now sends one folder
containing a file and an empty directory plus one loose file from the production
macOS `AppModel` through Invite/Relay to the production iPhone `AppModel`. The
iPhone completes app-private staging and multi-root publication with 2 roots,
2 files, 2 directories, 75 exact bytes, the final tree, and both SHA-256 values.
These gates do not prove final manual UI, physical Share Extension multi-item,
or Apple↔Android acceptance. A separate compatible single-file
macOS `AppModel`→iPhone `AppModel` gate now passes through Invite/Relay with 37
exact bytes and SHA-256
`7168fd00a9cc516cb7502c53760d5740f38c0671edc338f32ab6ce606fb32165`.
That gate also proves Manifest invite delivery to the existing native observer
without changing the FFI surface. Multi-item Share intake is implemented and
hosted-tested. Separately, two synthetic Photos providers have now passed the
physical main-app `PhotoDraftImporter` → v2 draft → production Manifest sender
→ production macOS receiver path with 2 roots, 2 files, and 136 exact bytes.
The dedicated iOS Folder path has also passed on a physical iPhone: the real
system picker selected its current directory through Apple's **Open/打开**
action, the production Send UI started the Manifest transfer, and the
production macOS receiver verified 1 root, 1 file, 1 directory, 36 exact bytes,
and SHA-256 over a selected Direct path. This proves the app-owned fixture path,
not every iCloud or third-party File Provider. The system Share Extension
multi-item host path and Files provider path have not yet passed the
physical-device gate.

## Compatibility Boundary

`ManifestV1` is additive. The existing `envoix/1` ALPN, `Hello`, `FileHeader`,
single-file engine, public client methods, and native bindings remain unchanged.
The first implementation introduces a separate `envoix/manifest/1` ALPN and a
manifest-specific frame family. Legacy single-file receivers continue to
advertise only `envoix/1`. The additive negotiated receiver advertises both
ALPNs, while the sender selects the one required by the transfer shape before
authentication. The receiver records the negotiated ALPN, completes the
existing authentication handshake, and only then invokes either engine.

This separation is intentional:

- a single file continues to use `single_file_v1` by default during migration;
- multiple files or a directory require `manifest_v1` and never fall back to a
  zip, repeated unrelated single-file activities, or `single_file_v1`;
- an older peer rejects the manifest ALPN before manifest or payload bytes are
  sent, and the initiating client surfaces `manifest.unsupported_peer`;
- the existing frame header version is not bumped merely to add Manifest;
- Rust, UniFFI, Swift, and Kotlin APIs for Manifest are additive until the old
  single-file receive path can be retired in a separately approved version.

## Implementable V1 Contract

### Transfer set and entry model

One `ManifestV1` describes one durable transfer Activity and has:

- `manifest_id`: the stable transfer-set identifier;
- `entries`: entries in canonical parent-before-child path order;
- `file_count`, `directory_count`, and `root_count`;
- `total_bytes`: the checked sum of regular-file sizes;
- `hash_algorithm`: exactly `blake3_256` in v1.

Each entry has:

- `entry_id`: a zero-based identifier stable for the life of the manifest;
- `relative_path`: a `/`-separated UTF-8 path relative to the selected receive
  directory;
- `kind`: `regular_file` or `directory`;
- `size`: required for files and exactly zero for directories;
- `hash`: a 32-byte BLAKE3 digest for files and absent for directories;
- `modified_at_unix_ms`: optional, informational metadata that a receiver may
  preserve only when its platform supports doing so safely.

An empty directory therefore has an explicit directory entry. A selected
directory has its own top-level directory entry, and all descendants use that
name as their first path component. Multiple picker selections are represented
as multiple top-level roots in the same manifest. Symbolic links, aliases that
resolve outside a selected root, sockets, device nodes, and other special files
are rejected by the sender rather than followed or serialized.

The sender hashes regular files while constructing the offer. It revalidates
size and hash while streaming so a source that changes after preflight fails the
entry and cannot be reported as the offered object.

### Hard protocol limits

The first implementation uses named constants and rejects the whole offer
before creating receive files when any limit is exceeded:

| Limit | V1 value |
|---|---:|
| Encoded manifest bytes | 4 MiB |
| Entries, including directories | 10,000 |
| UTF-8 bytes in one relative path | 4,096 |
| UTF-8 bytes in one component | 255 |
| Path depth | 64 components |

There is no smaller arbitrary protocol cap on a file or transfer set beyond the
checked `u64` size fields. The receiver applies its own available-space,
platform, and configured-quota policy during preflight and may reject the offer
before payload. Count, path-length, and byte-total arithmetic must be checked;
overflow is a protocol error.

### Portable path validation

Validation operates on manifest strings, not on a joined host path. The whole
manifest is rejected before any payload write when a path:

- is empty, absolute, begins or ends with `/`, or contains an empty component;
- contains `.`, `..`, NUL, a control character, or `\` in any component;
- exceeds the component, depth, or full-path limits;
- duplicates another manifest path;
- places an entry below a parent declared as a regular file;
- omits the explicit parent directory entry for a nested entry; or
- cannot be represented safely by the receiving platform.

Before creating a destination, the receiver opens or inspects every existing
ancestor without following symbolic links and guarantees the resolved write
remains below the selected receive root. Platform adapters must also treat
case-folding or Unicode-normalization aliases as conflicts on filesystems where
those names address the same object. They must never allow two entries to write
the same target.

### Conflict planning

Conflict decisions are receiver-owned, deterministic, and non-interactive in
v1. Planning produces an explicit source-path to final-relative-path mapping
that is persisted with the durable Activity.

- A missing target is accepted at its offered path.
- An existing regular file with the same BLAKE3 hash is not transferred when
  the receiver can safely preflight it, or is discarded during publication
  when the final platform destination is only available then. The result is
  `skipped_identical` in either case.
- An existing regular file with a different hash keeps both. The incoming leaf
  uses the first available `name (n).ext` form.
- An existing directory may be reused only for nested entries that were not
  selected as a top-level directory root.
- A colliding selected top-level directory is renamed as a unit, for example
  `Photos (1)`, and every descendant follows that mapped root. It is not silently
  merged into an existing `Photos` directory.
- A symbolic link or non-directory object in an ancestor position is never
  traversed. The incoming root/component is renamed or the offer is rejected if
  no safe mapping can be produced.

The final exclusive create/rename repeats collision detection because another
process may create a target after preflight. A later collision advances the
suffix and updates the result mapping; it never changes the policy to overwrite.

### Sequential wire lifecycle

The manifest ALPN uses the following logical sequence. The protocol
implementation freezes numeric frame IDs 16 through 26, and codec fixtures
cover both those IDs and the existing single-file IDs 1 through 9.

```text
ManifestHello/Offer
  -> ManifestAccept(entry dispositions and safe target mapping)
  -> [EntryStart -> ResumeStatus -> Chunk* -> Complete -> CompleteAck]*
  -> ManifestComplete
  -> ManifestCompleteAck(final per-entry results)
```

Directories and entries accepted as `skipped_identical` do not have payload
frames. File payloads are sequential in v1; chunks remain sequential within an
entry. Every payload/control frame identifies both `manifest_id` and `entry_id`,
and each streamed file also has a stable transfer identifier for the existing
resume and receipt machinery.

The active file may reuse the existing verified-prefix resume behavior. On a
retry, already committed files are recognized by their manifest hash and do not
need to be retransmitted. Parallel file scheduling and independently pausing
individual entries are not part of v1.

### Commit, publication, and final result

Every regular file first lands in transfer-owned staging, is size/hash verified,
and is atomically committed before its `CompleteAck`. On Apple platforms,
network completion may still be followed by the existing native `Publishing`
phase for a Files/FileProvider destination. The overall Activity is not
`Completed` until all accepted entries are committed and required native
publication has succeeded.

V1 does not promise an all-or-nothing filesystem transaction across the whole
set. A failure or explicit cancellation preserves already published files,
cleans only uncommitted staging owned by that Activity, and records a partial
result. Retry reuses committed entry results and resumes or restarts the active
entry without overwriting them.

Each final entry result is one of:

- `completed` with its final relative path;
- `skipped_identical` with the existing relative path;
- `renamed` with offered and final relative paths;
- `failed` with a structured failure code;
- `cancelled` if it had not committed when cancellation won.

The durable record and additive FFI projection expose transfer mode, root/file/
directory counts, aggregate bytes, completed files, current entry, and these
per-entry results. UI display names are projections and are never used as
staging, receipt, or resume identity.

## Required Changes

### 1. Manifest data model

Add a `ManifestV1` structure that can describe one transfer containing one or more entries.

Each file entry should include at least:

- relative path;
- file size;
- file hash;
- file type, initially regular file and directory;
- optional modified time, if supported consistently.

The manifest must not use archive formats such as zip as the primary protocol description. Archive support, compression, or tar streaming can be evaluated separately.

### 2. Multi-file and directory transfer

Sender should be able to offer:

- one file;
- multiple selected files;
- one directory;
- mixed file and directory selections, if the platform picker supports it.

Receiver should create the destination structure from the manifest and report progress at both transfer-level and file-level granularity.

### 3. Safe receive path rules

All manifest paths must be relative paths.

The receiver must reject entries that contain:

- absolute paths;
- `..` path traversal;
- platform-specific path escape behavior;
- names that cannot be safely created on the target platform.

The receiver must guarantee that manifest extraction cannot write outside the selected receive directory.

### 4. Conflict handling

`ManifestV1` must not overwrite existing files by default.

Recommended v1 policy:

- if the target path does not exist, write normally;
- if the target path exists and the hash is identical, skip the incoming file and record `skipped_identical`;
- if the target path exists and the hash differs, keep both by renaming the incoming file, for example `photo (1).jpg`;
- record all skipped and renamed files in the final transfer result.

Interactive replace can be added later, but overwrite must not be the default behavior for v1.

### 5. Protocol negotiation

Peers must explicitly negotiate the transfer mode before payload bytes are sent.

Expected modes:

```text
single_file_v1
manifest_v1
```

During the migration period:

- single regular file transfer may continue to use `single_file_v1` by default;
- multi-file and directory transfer must require `manifest_v1`;
- a peer that does not support `manifest_v1` must fail clearly before transfer starts;
- once a transfer mode is selected, the implementation must not silently fall back to another mode within the same transfer.

After `ManifestV1` is stable, single-file transfer can be migrated to `manifest_v1` by default while keeping `single_file_v1` receive support for older clients.

### 6. Activity and result reporting

Activity UI should not depend on whether the transfer used `single_file_v1` or `manifest_v1`.

The shared transfer record should expose:

- transfer mode;
- item count;
- total bytes;
- completed bytes;
- completed item count;
- current file, when available;
- skipped files;
- renamed files;
- failed files;
- final result.

## Out of Scope

- Parallel transfer
- Speed limiting
- Compression
- Tar streaming
- Zip packaging
- Independently pausing, prioritizing, or scheduling individual files
- File-level E2E encryption
- Trusted-device auto receive
- Interactive conflict resolution UI
- Metadata privacy

## Acceptance Criteria

- `ManifestV1` is specified with enough fields to represent multiple files and directories.
- Manifest limits and checked aggregate arithmetic reject oversized or malformed offers before writes.
- Receiver path validation rejects unsafe paths before writing files.
- Default conflict behavior never overwrites existing files.
- Identical existing files can be skipped by hash.
- Different existing files are preserved by keeping both files.
- A colliding top-level directory is renamed as a unit rather than silently merged.
- Protocol negotiation distinguishes `single_file_v1` from `manifest_v1`.
- Multi-file and directory transfer require `manifest_v1`.
- Existing single-file transfer remains compatible during the migration period.
- Activity/result reporting can represent skipped, renamed, completed, and failed files.
- A partial failure cannot be reported as overall completion and does not delete already published files.

## Follow-up Issues

- Run the physical Photos/Files multi-select acceptance gate for the implemented
  multi-item Share Extension → Apple Manifest send path. This is distinct from
  the completed main-app multi-Photos provider payload gate in `e1b6c0e`.
- Extend physical Manifest/multi-root coverage to Apple↔Android transfers,
  retaining final path, size/hash, result mapping, and publication evidence.
  The compatible single-file reverse direction is covered by
  `EnvoixMacOSHostedTests.testSendMacOSToIosAppInvite` paired with
  `EnvoixIOSLoopbackTests.testCrossDeviceReceiveMacOSToIosAppInvite`; reverse
  Manifest publication is covered by
  `EnvoixMacOSHostedTests.testSendMacOSToIosAppManifestInvite` paired with
  `EnvoixIOSLoopbackTests.testCrossDeviceReceiveMacOSToIosAppManifestInvite`;
  the iPhone→macOS Manifest direction is covered by
  `EnvoixIOSLoopbackTests.testCrossDeviceSendIosToMacOSManifestRoom` paired with
  `EnvoixMacOSHostedTests.testReceiveIosToMacOSAppManifestRoom`.
- Add full Apple UI visual/accessibility coverage for long Manifest names,
  partial results, and large top-level selections.
- Add per-file resume after manifest transfer is stable.
- Evaluate tar streaming for partial readability under poor network conditions.
- Evaluate compression after measuring CPU cost and network bottlenecks.
