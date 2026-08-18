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
