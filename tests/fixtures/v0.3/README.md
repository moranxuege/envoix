# v0.3 contract fixtures

These files freeze the typed application boundary introduced during the v0.3
refactor. A contract-version change requires a new fixture; do not rewrite an
existing fixture to hide a breaking wire change.

All identifiers and user-visible values are synthetic. Invitations are inert,
verification codes are non-secret placeholders, and no credential material or
real file metadata belongs in these fixtures.

`application-contract-v1.json` is the preserved pre-rotation contract.
`application-contract-v2.json` adds remembered Relationship generation
rotation without rewriting v1. `application-contract-v3.json` separates peer
admission from authentication and records atomic Room replacement without
rewriting v1 or v2. `application-contract-v4.json` adds explicit incoming
Transfer offer, acceptance, and typed rejection without rewriting v1-v3.
`application-contract-v5.json` makes recovery and removal explicit and splits
payload completion from verified delivery proof without rewriting v1-v4.
`application-contract-v6.json` makes the shared failure projection's stable,
fine-grained failure codes canonical without rewriting v1-v5. The current
fixture covers every current command and event variant; its valid event stream
reconstructs the embedded snapshot, and its removed Transfer is intentionally
absent from that snapshot.

`engine-state-v1.json` freezes the retired migration-bearing Engine envelope.
`engine-state-v2.json` is the current empty, non-secret envelope and removes
legacy migration metadata without rewriting v1.

`agent-control-v4.json` freezes the first explicitly versioned Agent request
and response envelopes, including the immutable Engine/Inbox snapshot.
`agent-control-v5.json` adds a snapshot event cursor, bounded event polling,
and an explicit `snapshot_required` recovery response without rewriting v4.
`agent-control-v6.json` adds durable Transfer creation and inspection plus a
secret-free diagnostic response without rewriting v4 or v5.
`agent-control-v7.json` adds bounded, secret-free pending incoming-offer state,
single-use approve/reject commands, and pending-offer events without rewriting
v4-v6.
`agent-control-v8.json` adds bounded active-path snapshots, list responses, and
selected/cleared path events without rewriting v4-v7.
`agent-control-v9.json` binds diagnostics to Engine schema v2 and is the
current paired CLI/Agent contract without rewriting v8.
