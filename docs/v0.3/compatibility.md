# v0.3 compatibility and upgrade policy

Status: accepted breaking-upgrade policy for the v0.3 test cycle.

v0.3 uses the new Engine architecture as its only product runtime. It does not
import or execute v0.2 product state. This deliberately trades test-build
upgrade continuity for one state owner, one credential path, and one durable
schema before the stable release.

Received files are the exception: they are user data and are never removed by
Engine startup, legacy-state rejection, update, or lifecycle state cleanup.

## 1. Compatibility classes

| Surface | Policy | Rationale |
| --- | --- | --- |
| Manifest v2 transfer wire | preserve unless a security defect requires a versioned change | proven core behavior and cross-device interoperability |
| SPAKE2 and channel binding | preserve security properties | authentication boundary |
| Room code and current Room capability | preserve where compatible; reject unsupported versions explicitly | supports controlled endpoint rollout |
| legacy word/direct invite product workflow | remove from app-facing APIs | superseded product model |
| Rust internal/public re-exports | breaking changes allowed | enables the Engine application boundary |
| Swift/Kotlin binding API | replace with versioned Command/Event/Snapshot contracts | removes parallel product orchestration |
| Agent local control API | current protocol only; older versions receive a typed error | prevents two local semantics under one version |
| Engine/ProductStore state | no v0.2 or Engine schema v1 import | avoids retaining a second state and credential model |
| remembered Relationship and credential | reset and re-pair | old identity/credential meaning is not carried into the new Engine |
| transient Room/session/outbox state | reset | temporary state must not constrain the architecture |
| received files | always preserve | user data is not Agent-owned migration scratch space |
| settings | retain only while their current version validates | invalid settings fail closed |

## 2. Version separation

The application release, Manifest/network protocol, Engine schema,
application binding contract, and Agent IPC protocol have independent version
numbers. Every serialized boundary carries or implies its version and rejects
unsupported versions explicitly.

The first migration-bearing Engine envelope is frozen as schema v1. The
breaking test-cycle cleanup removes its v0.2 migration metadata and introduces
schema v2 rather than silently changing v1. Agent diagnostics expose that
schema change, so the paired CLI/Agent control contract advances to protocol
v9. A v9 process does not execute v3-v8 commands.

## 3. Breaking state boundary

On startup, the Engine follows this order:

1. Load and validate `engine-state-v2.json`, recovering its v2 previous
   snapshot when allowed.
2. If valid v2 state exists, ignore residual v0.2 ProductStore and Engine v1
   files; they are not read or merged.
3. If no v2 state exists but an Engine v1 snapshot or
   `product/product-state-v1.json` exists, return `UnsupportedLegacyState`.
4. Only a directory with neither current nor recognized legacy state may start
   as a fresh Engine.

The explicit rejection prevents a binary update from looking successful while
silently discarding Relationships. It does not modify the old state, vault, or
Inbox. There is no automatic importer, fallback ProductStore, or
`re_pair_required` shadow model.

The Android pre-Engine Relationship store used a platform-specific location
and is outside the Engine directory. The v0.3 Android host records a diagnostic
when that v1 metadata exists, retains both its metadata and encrypted
credentials, and opens a fresh Engine v2 state without importing either one.
The remembered-device list is therefore empty until the devices pair again.
This boundary never traverses or removes received files.

## 4. Test-build upgrade procedure

During the v0.3 test cycle, upgrade by intentionally resetting Agent-owned
state and then re-pairing:

```text
stop/uninstall the old Agent with confirmed state cleanup
  -> install the paired v0.3 CLI and Agent
  -> verify protocol v11 and Engine schema v2
  -> pair supported devices again
```

Lifecycle cleanup is allowlisted. It may remove old and current Engine
snapshots, vault entries, ProductStore data, migration remnants, transfer
checkpoints, and settings. It must not traverse or remove `inbox/`, an external
configured Inbox, or unknown files. Copy any other test-only history that is
wanted before confirming the reset.

## 5. Cross-version network behavior

Manifest and authentication compatibility is independent from local state and
IPC compatibility. Where the preserved protocol is sufficient, a v0.2 and
v0.3 endpoint may still complete that protocol. No v0.3 product feature may
depend on this without a dedicated compatibility test.

When a v0.3-only Room, Relationship, or capability is required, the older
endpoint receives a version/capability error. It must not be represented as a
generic timeout, wrong code, or connection failure.

## 6. Fixtures and removal rule

Current Engine v2 and Agent v9 fixtures round-trip every supported field.
Engine v1 and Agent v3-v8 fixtures remain frozen only to prove explicit
rejection or document the superseded contract. Retired ProductStore migration
fixtures and the importer are removed; Git history is their archive.

A legacy decoder may remain only when it is the smallest safe way to identify
an obsolete version and return a typed error. It must not construct domain
state, read old credentials, or execute a command.
