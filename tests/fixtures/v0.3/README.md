# v0.3 contract fixtures

These files freeze the typed application boundary introduced during the v0.3
refactor. A contract-version change requires a new fixture; do not rewrite an
existing fixture to hide a breaking wire change.

All identifiers and user-visible values are synthetic. Invitations are inert,
verification codes are non-secret placeholders, and no credential material or
real file metadata belongs in these fixtures.

`application-contract-v1.json` is the preserved pre-rotation contract.
`application-contract-v2.json` adds remembered Relationship generation
rotation without rewriting v1. The current fixture covers every command and
event variant, and its valid event stream reconstructs the embedded snapshot,
including delivered, failed, and canceled Transfers.
