# Issue documentation routing

GitHub issues are the canonical source for active implementation work. Do not
read every file in this directory during repository orientation. Start from
this index, then open only the document required by the current task.

Last reconciled with repository implementation: 2026-07-14.

Cross-issue sequencing, accepted product decisions, workstream ownership, and
validation gates live in
[`../design/apple-client-execution-plan.md`](../design/apple-client-execution-plan.md).
The files below remain subject-specific contracts and must not grow independent
roadmaps that contradict that execution plan.

## Canonical GitHub issues

- [#14 Client Return to Handshake on Timeout](https://github.com/ECE4410J-NUUB/envoix/issues/14)
- [#31 CLI All Paths Test](https://github.com/ECE4410J-NUUB/envoix/issues/31)
- [#38 Structured Error Model and Diagnostics Pipeline](https://github.com/ECE4410J-NUUB/envoix/issues/38)
- [#39 Cancellation and Retry UX](https://github.com/ECE4410J-NUUB/envoix/issues/39)
- [#40 Persistent Transfer Queue and Transfer Records](https://github.com/ECE4410J-NUUB/envoix/issues/40)
- [#41 Cross-Platform Nearby Discovery v1](https://github.com/ECE4410J-NUUB/envoix/issues/41)
- [#42 Speed Limit and Backpressure](https://github.com/ECE4410J-NUUB/envoix/issues/42)
- [#43 Parallel Transfer Design](https://github.com/ECE4410J-NUUB/envoix/issues/43)
- [#44 Polish Receive Destination UX on Apple Platforms](https://github.com/ECE4410J-NUUB/envoix/issues/44)
- [#45 Apple receiver role and developer mode toggles feel unresponsive](https://github.com/ECE4410J-NUUB/envoix/issues/45)
- [#47 Transfer identity, staging collisions, SAF data loss, and receipt proofs](https://github.com/ECE4410J-NUUB/envoix/issues/47)

The former local drafts for #38–#44 were removed after publication. The former
Apple-only Activity draft is superseded by cross-platform issue #40. The former
structured-FFI-event draft described a missing foundation that now exists as
`FfiTransferEvent`; remaining integration belongs to #38 and #40.

## Local-only design drafts

Read these only when the task directly concerns their subject:

- `reliable-transfer-completion-resume.md` — current P0 completion, commit,
  receipt, and resume semantics. Several GitHub issues refer to this design,
  but no dedicated cloud issue exists yet.
- `transfer-manifest-v1.md` — active multi-file/directory contract; protocol,
  engine, session, and Rust client facade are implemented, while durable
  Activity, FFI, and product UI remain.
- `trusted-device-store.md` — future device identity and trust policy.
- `sender-initiated-transfer-flows.md` — future sender-first product flow.
- `design-file-level-e2e-encryption.md` — future payload-encryption design.

## Reconciliation warnings

- #39 currently says receiver partial state is retained by default. The policy
  needs refinement: transient network/system interruption may retain bounded
  resume data, while explicit user Cancel should remove temporary data by
  default. Retention also needs TTL, quota, and disk-pressure eviction rules.
- #44 calls Apple receive destination work only polish. Current evidence shows
  a deeper boundary: verified, committed, published, and available are distinct
  states, and FileProvider/iCloud destinations need an explicit staging policy.
- #47 is written from the Android failure case, but its principle is shared:
  transfer identity must key staging and receipt proofs; display names are only
  metadata.
- #45 may be related to high-frequency SwiftUI invalidation. Do not close it
  until the current throttling changes are verified on a physical iOS device.

The repository-external project Wiki is the current architecture overview:
`../ECE4410J-NUUB-WIKI.md` relative to the repository's parent directory.
