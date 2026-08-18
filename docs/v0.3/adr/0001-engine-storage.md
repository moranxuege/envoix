# ADR 0001: Use a bounded atomic-file Engine store

Status: accepted

Date: 2026-08-18

## Context

v0.3 needs one Engine-owned schema for durable Relationship, Transfer, Inbox,
and migration metadata. The same implementation must work in embedded mobile
applications and desktop Agents. Received file contents and vault secrets are
outside this store.

The architecture already requires one durable Engine owner per process and one
writer per state directory. Product state is bounded: Inbox history is capped,
completed history has an explicit retention policy, and payload bytes are held
by content/destination ports. The store therefore does not need concurrent SQL
writers or arbitrary queries.

## Decision

Use a versioned, bounded JSON snapshot replaced atomically in one private state
directory. Do not add SQLite in v0.3.

The implementation must provide:

- one schema envelope containing the application snapshot, vault references,
  Inbox metadata, and migration metadata;
- semantic validation after decoding and before every activation;
- a same-directory temporary file, file flush, atomic replacement, and parent
  directory flush where the platform exposes that operation;
- one last-known-good snapshot for recovery from corrupt or interrupted state;
- an exclusive lifetime lock acquired before migration or Engine startup;
- a hard encoded-size limit and bounded collections before allocation;
- durable checkpoints at product transitions, with progress coalesced instead
  of flushing every byte callback;
- vault references only. A secret value must never enter the JSON envelope;
- a one-time, restartable v0.2 import that creates an immutable backup before
  activating v0.3 state;
- Windows replacement and locking implemented with native file semantics, and
  Unix locking plus owner-only modes.

The initial layout is:

```text
state-directory/
  engine.lock
  engine-state-v1.json
  engine-state-v1.previous.json
  migration/
    v0.2-product-state-v1.backup.json
    import-v0.2-v1.json
  vault/                         # desktop protected-credential adapter
  inbox/                         # received user files; never migration-owned
```

The Windows adapter stores only versioned, user-scoped DPAPI ciphertext in the
`vault/` directory and binds every blob to its opaque reference with optional
entropy. A domain-separated digest inside the protected envelope rejects a
rare corrupted result even if DPAPI itself returns success. Linux/WSL uses the
documented owner-only file fallback there. Mobile and signed Apple hosts use
their native vault ports.

## Alternatives considered

### SQLite

SQLite supplies transactions, checksums through page validation, and efficient
queries. It also adds a native dependency, schema tooling, mobile packaging,
backup/WAL policy, and a second concurrency model. Those costs do not buy a
v0.3 requirement because concurrent writers are prohibited and the state is
small and bounded. Reconsider SQLite if later clipboard history or searchable
transfer history makes bounded snapshot replacement measurably unsuitable.

### Append-only event log

An event log matches the Engine contract but needs compaction, partial-record
recovery, and retention policy before it can replace a snapshot. v0.3 keeps
ordered events as the in-memory/control contract and persists the validated
snapshot at durable checkpoints.

### Keep the v0.2 ProductStore

The current store persists only remembered-device metadata and completed Inbox
items. It cannot make active Transfers or shared application state durable and
would leave two product schemas. It remains only as a bounded import adapter
until the v0.2 migration removal gate is met.

## Consequences

- Packaging and fixture tests stay portable and deterministic.
- One owner lock, size bounds, validation, backup, and recovery are required
  correctness mechanisms rather than optional hardening.
- Write amplification must be controlled at the Engine checkpoint boundary.
- The store is not an authorization boundary: local state remains protected by
  operating-system ownership, while credentials remain in a vault.
- A future SQLite migration is possible behind the same Engine store contract;
  it is not implemented speculatively in v0.3.
