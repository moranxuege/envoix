# v0.2 compatibility fixtures

These files freeze the externally visible formats found when the v0.3
refactor began. Migration tests must work on copies; the fixtures themselves
remain immutable evidence.

All identities, paths, endpoints, room codes, verification codes, timestamps,
and transfer invitations are synthetic. Timestamps are expired, `.invalid`
endpoints cannot resolve, and credential-shaped values are deliberately absent
or invalid. Never replace them with captured production data.

`product-state-v1.json` references a credential file that is intentionally not
present. It characterizes the safe “metadata retained, re-pair required” case.
The corrupt, truncated, unknown-version, and partial-migration files must all
fail closed without changing their source copy.
