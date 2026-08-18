# v0.3 compatibility and migration policy

Status: accepted policy; desktop ProductStore import is implemented in M4.

v0.3 deliberately breaks accidental internal interfaces. It does not use an
architecture refactor as permission to lose received files, silently weaken
authentication, or produce ambiguous cross-version failures.

## 1. Compatibility classes

| Surface | Policy | Rationale |
| --- | --- | --- |
| Manifest v2 transfer wire | preserve unless a security defect requires a versioned change | proven core behavior and cross-device interoperability |
| SPAKE2 and channel binding | preserve security properties | authentication boundary |
| Room code and current Room capability | preserve where compatible; keep version parsing explicit | supports staged client updates |
| legacy word/direct invite product workflow | remove from app-facing APIs after reachability audit | superseded product model |
| Rust internal/public re-exports | breaking changes allowed | currently prevent a real application boundary |
| Swift/Kotlin binding API | replace with versioned Command/Event/Snapshot contract | current parallel orchestration is the primary debt |
| Agent local control API | introduce a versioned v0.3 protocol | GUI and CLI require stable local control |
| received files | always preserve | user data, never migration scratch space |
| device identity | migrate when cryptographically and operationally safe | avoids unnecessary identity churn |
| remembered Relationship | attempt one bounded migration; otherwise require explicit re-pair | no indefinite legacy credential path |
| transient Room/session state | reset allowed | temporary state must not constrain the architecture |
| pending v0.2 outbox/UI drafts | reset allowed after backup and user-visible notice | formats are duplicated and not reliable contracts |
| completed Transfer history | migrate when trustworthy; otherwise preserve raw backup | useful metadata but not equal to received files |
| settings | migrate supported semantic values, discard obsolete switches | prevents old transport modes from surviving as product concepts |

## 2. Version separation

The following versions are independent:

- application release version (`0.3.0`);
- network protocol/manifest version;
- Engine schema version;
- binding/control contract version;
- Agent local IPC version.

A source release bump does not automatically require a wire version bump.
Every serialized boundary carries or implies its own version and rejects an
unsupported version with a typed error.

## 3. Migration invariants

1. Migration never traverses, deletes, or moves received user files.
2. The old state remains intact until the new state is fully written and
   validated.
3. Migration is restartable or records a terminal, recoverable failure.
4. A secret is never copied into an ordinary JSON/database field during
   migration.
5. Unknown credential formats do not fall back to plaintext.
6. Re-pairing is explicit and explains which Relationship could not be
   imported.
7. A failed import cannot make a valid v0.2 install unusable without retaining
   a recoverable backup.
8. Compatibility code has a removal milestone and cannot become a second
   permanent runtime path.

## 4. Migration transaction

The target sequence is:

```text
discover old state
  -> validate and inventory
  -> create immutable backup/reference
  -> build new state in a temporary location
  -> validate schema and vault references
  -> atomically activate new state
  -> retain bounded migration evidence
```

If validation fails, v0.3 starts in a recovery state with these choices:

- retry migration after correcting the reported condition;
- continue with a fresh local product state while preserving the backup and
  received files;
- re-pair relationships that cannot be imported.

There is no silent fallback that runs v0.2 and v0.3 product engines side by
side.

## 5. Relationship migration

A remembered Relationship is imported only if all of these are true:

- the peer identity and local identity are unambiguous;
- credential generation and relation identifiers validate;
- secret material can be moved or referenced without exposing it;
- the new Engine can distinguish imported, rotated, and revoked state;
- both endpoints can produce a clear authentication failure if their
  generations no longer agree.

Otherwise, metadata may be retained for explanation, but the Relationship is
marked `re_pair_required`. A user must never see it as trusted-but-offline when
its credential is unusable.

## 6. Cross-version behavior

Where Manifest v2 and authentication remain compatible, v0.2 and v0.3 may
complete a basic Transfer during staged rollout. No product feature may depend
on this unless it has an explicit compatibility test.

When a v0.3-only Room, Relationship, or control behavior is required, the
older endpoint receives a version/capability error. It must not be represented
as a generic timeout, wrong code, or connection failure.

The broker and relay remain backward-compatible only for the bounded rollout
window documented by their deployment milestone. The repository does not keep
unused public server APIs indefinitely for hypothetical clients.

## 7. Fixtures and tests

M1 records sanitized, secret-free fixtures for:

- current Room codes and Room control envelopes;
- Agent command/event envelopes;
- remembered Relationship metadata with fake credentials;
- Transfer records in active, paused, delivered, and failed states;
- corrupt, truncated, unknown-version, and partially migrated state.

M4 migration tests run against copies of those fixtures. Fixtures never contain
real device identities, production endpoints that reveal private data, or
usable credentials.

The desktop Agent opens the Engine store at the state-directory root. On first
open it discovers the v0.2 `product/` store, validates the complete candidate,
installs an immutable source backup, copies only supported opaque credentials
into the Agent vault, and atomically activates Engine schema v1. Missing or
unsupported credentials are recorded as `re_pair_required`; the legacy store
and received files remain untouched.

## 8. Removal rule

A legacy API or schema adapter may be removed when:

1. repository reachability shows no supported consumer;
2. the replacement has characterization and contract tests;
3. every affected supported host builds and passes its relevant tests;
4. staged rollout behavior is documented;
5. any required one-time migration remains available without retaining the old
   runtime implementation.

Git history is the archive. Dead production code is not documentation.
