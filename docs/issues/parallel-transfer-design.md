## Title

Parallel Transfer Design

## Problem

Envoix currently sends file data as sequential resumable chunks.

This is the right baseline for correctness, but it limits throughput and creates future pressure for parallel transfer. Implementing parallel transfer directly now would be risky because several foundations are still evolving:

- manifest transfer;
- per-file and per-chunk integrity;
- retry and resume semantics;
- structured Activity records;
- speed limit and backpressure.

Parallel transfer should be designed before implementation.

## Existing Implementation Found

Already present:

- transfer frames include chunk index and offset;
- sequential resume uses receiver-side temp files and sidecar state;
- BLAKE3 whole-file verification exists;
- `docs/arch.md` mentions future chunk bitmap and parallel chunk transfer;
- `apps/envoix-apple/README.md` states parallel chunk transport is not implemented;
- `docs/issues/transfer-manifest-v1.md` treats parallel transfer as out of scope.

Missing:

- chunk bitmap resume;
- per-chunk integrity;
- out-of-order writes;
- flow control policy;
- concurrency limit;
- fairness with speed limit;
- Activity representation for multiple files/chunks;
- cross-platform configuration API.

## Goal

Produce a design for parallel transfer that can be implemented safely after `ManifestV1` and reliable resume semantics are stable.

The design should decide whether v1 parallelism happens at:

- file level;
- chunk level;
- stream level;
- or a staged combination.

## Recommended Direction

Do not start with arbitrary out-of-order chunk transfer.

Safer staged path:

1. support `ManifestV1`;
2. parallelize independent files in a manifest, with a small concurrency limit;
3. add `ResumeBitmapV2` for chunk-level missing-piece tracking;
4. only then evaluate chunk-level parallelism for very large single files.

This avoids making the current sequential resume sidecar carry semantics it was not designed for.

## Design Questions

### 1. Parallel unit

Decide the initial unit:

- file-level parallelism: simpler, good for many files;
- chunk-level parallelism: better for one huge file, but requires bitmap resume and random writes;
- stream-level parallelism: depends on transport behavior and flow control.

### 2. Resume model

Define how parallel progress is persisted:

```text
manifest_id
file_id
chunk_size
verified_chunk_bitmap
received_chunk_bitmap
per_chunk_hashes
```

Sequential byte-offset resume is not enough for out-of-order transfer.

### 3. Integrity model

Whole-file BLAKE3 remains useful, but chunk-level parallelism needs local per-chunk validation before marking chunks complete.

The design should specify whether per-chunk hashes live in `ManifestV1`, a follow-up manifest extension, or a separate resume metadata file.

### 4. Scheduler and backpressure

Define:

- max concurrent files;
- max concurrent chunks per file;
- memory budget;
- disk write queue behavior;
- interaction with speed limit;
- pause/cancel behavior;
- fairness across simultaneous transfers.

### 5. Activity events

Activity should be able to report:

- total transfer progress;
- per-file progress;
- current active file set;
- failed/skipped files;
- retryable chunks or files.

## System Boundary

Parallel transfer scheduling belongs in the Rust transfer/client layer.

Native clients should configure high-level policy and display progress, but they should not implement chunk schedulers independently.

## Dependencies

GitHub issue: #43

Hard dependencies:

- Transfer Manifest v1, because file-level parallelism needs a transfer set model.
- Reliable Transfer Completion, Commit, and Resume Semantics, because parallelism must not weaken verification or commit guarantees.
- Structured Transfer Events over FFI, because native clients need typed per-file/per-chunk progress.

Design dependencies:

- #40 Persistent Transfer Queue and Transfer Records, for scheduling and retry interaction.
- #42 Speed Limit and Backpressure, for fairness and bandwidth control.

## Out of Scope

- Implementing parallel transfer
- Changing default transfer behavior
- Compression
- E2E encryption implementation
- Bluetooth or USB transport
- Platform-specific UI controls beyond policy placeholders

## Acceptance Criteria

- Design document chooses the first parallelism unit.
- Design explains why other parallelism modes are deferred.
- Design specifies resume metadata for parallel transfer.
- Design specifies integrity checks for out-of-order data.
- Design specifies scheduler limits and interaction with speed limiting.
- Design explains Activity/event requirements.
- Design produces follow-up implementation issues.

## Follow-up Issues

- ResumeBitmap v2
- File-level parallel transfer for `ManifestV1`
- Chunk-level parallel transfer for large single files
- Parallel-aware Activity records
- Scheduler benchmarks on LAN and relay paths
