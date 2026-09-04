# ADR 0003: Keep Rooms temporary and pin deployment routes to Relationships

Status: accepted

Date: 2026-09-05

## Context

Envoix needs to let people turn a successful one-time connection into a device
that can be reached again. It also needs deterministic behavior when two
people have different default Broker or Relay settings. Treating a Room as
durable would retain a short-lived capability and mix connection lifetime with
trust lifetime. Reading global endpoint defaults for every reconnect would
instead let a settings change silently break an existing Relationship.

The current product has three related gaps:

- remember consent is attached to some send/receive forms rather than an
  explicit, mutually confirmed Room operation;
- a complete invitation carries its rendezvous route, while a short Room Code
  can only be resolved inside a deployment already selected by both peers;
- updating the route of one local Relationship does not update its peer.

## Decision

### Temporary Room, durable Relationship

A Room is always temporary. Its code, invitation secret, and live session are
never the durable object. The user-facing **Save this device** action upgrades
the authenticated peer relationship, not the Room itself.

The upgrade is a bounded, idempotent protocol identified by a random
transaction ID:

1. one peer proposes saving the device;
2. the other peer explicitly accepts or rejects;
3. both peers verify the same short authentication string;
4. each Engine host prepares a credential and durable Relationship update;
5. commit acknowledgements make the result explicit on both peers.

A retry with the same transaction ID cannot create a duplicate Relationship.
Prepared state expires and is discardable. A peer that cannot confirm the
other commit reports **Needs repair** rather than claiming the device was
saved. Local device labels are not authenticated and may differ.

Later communication opens a fresh remembered Room authenticated by the saved
Relationship. Existing transfer wire behavior remains independent from the
Room that created the Relationship.

### Deployment profiles and invitation authority

A deployment profile contains a stable local identifier, display name, Broker
address including its authenticated endpoint identity, optional Relay route,
and a monotonically increasing local revision. It contains no credentials.

The creator's complete invitation is authoritative for that one Room. A
joiner uses the embedded route without replacing its own default profile. A
short Room Code is meaningful only inside an explicitly selected common
profile. When the selected profiles differ, the UI requires a complete link or
QR invitation and must not turn the mismatch into a generic timeout.

The route that successfully establishes a durable Relationship is copied into
that Relationship. Changing the global default affects only future Rooms and
Relationships.

Broker federation is not part of v0.3. Two isolated Brokers do not discover or
forward Rooms for each other. v0.3 interoperability across different local
defaults comes from the complete invitation selecting one rendezvous route for
the Room.

### Authenticated route migration

A durable route is not changed unilaterally. One peer proposes a new route over
the authenticated Relationship. The peer validates the bounded route and
acknowledges that it can use it. Both sides then commit the same route revision
and retain the previous route for one bounded recovery window. Active
Transfers and pending offers block commit.

The proposal and acknowledgement are bound to the Relationship credential,
transaction ID, old revision, and proposed route. They contain no credential
material. Replays and revision rollback are rejected.

If no old route is reachable, automatic migration is impossible. Recovery
requires an out-of-band repair invitation authenticated with the existing
Relationship credential, or a new pairing. The UI states this limitation
directly.

### Presentation and diagnostics

Primary UI uses **Room**, **Save this device**, **Saved devices**, and **Network
configuration**. It does not expose Agent, helper, schema, or credential
terminology. Broker identity, Relay route, route revision, and migration state
appear only in Advanced settings or a redacted diagnostic report.

## Compatibility

The existing transfer Manifest and authentication protocol remain unchanged.
The Room-control capability and local Agent protocol are versioned when the
upgrade and migration messages are introduced. An older peer may still
transfer files when the preserved wire protocol permits it, but receives a
typed unsupported-capability result for saving a device or migrating a route.

Agent/application updates must reject incompatible local control commands
before mutation and guide the owner to restart the bundled matching Agent. A
released v0.3 state is migrated forward; it is not silently reset.

## Consequences

- A saved device survives Room expiry without retaining Room secrets.
- Different local defaults work through complete invitations; naked short
  codes deliberately do not claim global routing.
- Existing Relationships do not move when a default endpoint changes.
- Route migration requires both peers or an explicit repair operation.
- Perfect simultaneous commit is not claimed across two devices. Persisted
  prepared/repair states make partial failure visible and recoverable.

## Verification

The release matrix must cover:

- different local defaults joined through a complete invitation;
- short-code profile mismatch with an actionable typed result;
- save accepted, rejected, expired, replayed, interrupted, and locally failed;
- duplicate-device and already-related peers;
- negotiated route migration, rollback, and active-Transfer rejection;
- loss of the old Broker followed by repair or explicit re-pairing;
- an older peer transferring successfully but rejecting the new capability.
