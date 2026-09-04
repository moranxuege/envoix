# Envoix v0.3 document status

Status: active documentation boundary

The files directly listed by [the v0.3 index](README.md) are authoritative for
v0.3 product semantics. The following supporting documents remain current for
their narrow implementation or operations scope:

- `docs/auth.md` — current InviteV2 and remembered authentication transcript;
- `docs/rendezvous-deployment.md` — current broker/relay deployment runbook;
- `docs/room-abuse-protection.md` — current broker limits and lifecycle;
- `docs/observability.md` and `docs/design/diagnostics.md` — diagnostic schema
  details where they do not conflict with the v0.3 privacy policy;
- focused design records referenced by source or tests for Manifest v2, nearby
  transports, and platform publication.

The following are historical evidence and not current release guarantees:

- `docs/releases/v0.2.2.md` and the v0.2 download application;
- `docs/arch.md` and architecture-review/SSOT audit documents written before
  the Engine application boundary;
- `docs/security-review-2026-07-19.md`, which is a work draft from before the
  as-built v0.3 controls;
- issue plans, debate transcripts, milestone proposals, and superseded vertical
  slice plans;
- old Agent, invitation, Room, binding, or persistence descriptions whenever
  they conflict with the current v0.3 index or compatibility policy.

Historical files stay in Git for traceability and may explain why a decision
was made. They must not be copied into user documentation as statements of
current behavior without re-verification. Source code and passing tests remain
the final arbiter when an ostensibly current supporting document drifts.

Every release-facing document change must use one of these states in its first
lines: active, accepted, historical, superseded, work draft, or archived. A new
v0.3 claim belongs in the authoritative set or links to a narrowly current
supporting document; it must not rely only on an old issue or debate record.
