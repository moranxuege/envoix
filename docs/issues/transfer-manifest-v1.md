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
- Advanced resume across individual files
- File-level E2E encryption
- Trusted-device auto receive
- Interactive conflict resolution UI
- Metadata privacy

## Acceptance Criteria

- `ManifestV1` is specified with enough fields to represent multiple files and directories.
- Receiver path validation rejects unsafe paths before writing files.
- Default conflict behavior never overwrites existing files.
- Identical existing files can be skipped by hash.
- Different existing files are preserved by keeping both files.
- Protocol negotiation distinguishes `single_file_v1` from `manifest_v1`.
- Multi-file and directory transfer require `manifest_v1`.
- Existing single-file transfer remains compatible during the migration period.
- Activity/result reporting can represent skipped, renamed, completed, and failed files.

## Follow-up Issues

- Implement `ManifestV1` protocol frames.
- Add multi-file selection in Apple sender UI.
- Add directory transfer support on desktop.
- Add manifest-aware receive path validation tests.
- Add manifest-aware Activity records.
- Add per-file resume after manifest transfer is stable.
- Evaluate tar streaming for partial readability under poor network conditions.
- Evaluate compression after measuring CPU cost and network bottlenecks.
