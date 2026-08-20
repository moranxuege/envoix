# v0.2 rejection fixtures

These files freeze the externally visible formats found when the v0.3
refactor began. They exist only to prove that current decoders reject obsolete
contracts with an explicit version error; they are not migration inputs.

All identities, paths, endpoints, room codes, verification codes, timestamps,
and transfer invitations are synthetic. Timestamps are expired, `.invalid`
endpoints cannot resolve, and credential-shaped values are deliberately absent
or invalid. Never replace them with captured production data.

Persisted v0.2 ProductStore fixtures were removed with the importer. A small
synthetic marker is created inside the rejection test instead; Git history is
the archive for the retired schema.
