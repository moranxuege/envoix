# Protocol liveness rules

Status: adopted 2026-07-11, after the unbounded-auth incident.

## The rule

**Every await whose progress depends on the peer must be bounded by both the
cancel token and a deadline.** The peer controls its own sends, so an
unbounded read is a lever the peer holds: an accepted connection that goes
silent pins the session, user pause/cancel only takes effect on transport
failure, and failure counters never fire because a stalled exchange never
*fails*. Deadline expiry is a normal protocol failure — never a hang.

`envoix_session::auth_bounded` is the pattern: `tokio::select!` over the
handshake future under `tokio::time::timeout`, and `cancel.cancelled()`.

## Why it is written down

The invariant used to live as habit. Every older peer-dependent wait (accept,
mDNS discovery, mDNS connect) had a bound because its author happened to add
one; the auth handshake arrived later through a different crate and got none —
nobody re-derived the rule. Recording it makes the review question ("where is
this await's bound?") point at a sentence instead of somebody's memory.

## Inventory of peer-dependent awaits (envoix-session)

| Await | Bound |
| --- | --- |
| room pairing | `pair_in_room` select on cancel + broker room TTL |
| endpoint accept | `accept_or_cancel` (cancel) |
| auth handshake (all 4 sites) | `auth_bounded` (30s + cancel) |
| mDNS discovery | `MDNS_DISCOVERY_TIMEOUT` |
| mDNS connect | `MDNS_CONNECT_TIMEOUT` |
| transfer frames | engine `recv_frame_or_cancel` (cancel) + QUIC idle timeout |

A new handshake step, frame exchange, or discovery mechanism must appear in
this table with its bound before it ships.
