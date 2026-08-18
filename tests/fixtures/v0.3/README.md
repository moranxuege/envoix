# v0.3 contract fixtures

These files freeze the typed application boundary introduced during the v0.3
refactor. A contract-version change requires a new fixture; do not rewrite an
existing fixture to hide a breaking wire change.

All identifiers and user-visible values are synthetic. Invitations are inert,
verification codes are non-secret placeholders, and no credential material or
real file metadata belongs in these fixtures.

`application-contract-v1.json` covers every v1 command and event variant. Its
event stream is valid and reconstructs the embedded snapshot, including
delivered, failed, and canceled Transfers.
