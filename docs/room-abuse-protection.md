# Room abuse protection

The iroh rendezvous service owns Room lifecycle, attempt accounting, resource
ceilings, rate limits, and retry guidance. Clients consume typed outcomes and
do not reproduce the broker's counters.

## Lifecycle

- Only an invitation creator can create and wait in a Room. A joiner is never
  parked: without a compatible live creator it receives `room_not_found`.
- One attempt is charged atomically when a compatible joiner is handed to the
  creator. Every later result has already consumed that attempt.
- Malformed joins, incompatible joins, and pre-match disconnects consume source
  rate capacity, but not Room attempts.
- Creator reconnects retain the Room's original expiry, attempts, and rate
  state. An exhausted Room returns `room_under_attack`; users create a fresh
  Room Code.
- Attempt budgets apply only to six-digit human Room locators. High-entropy
  remembered-device locators still receive source, concurrency, frame, and
  global resource enforcement.

`Reply::Rejected` carries a stable `BrokerOutcome` plus an optional,
server-capped whole-second `retry_after`. Client retries require both a
retryable outcome and guidance no larger than the client's configured cap.

## Source enforcement

Every connection is debited against its cryptographically authenticated iroh
EndpointId. EndpointIds are treated as best-effort source identities, not
durable device identities.

When iroh exposes a direct `TransportAddr::Ip`, the connection is additionally
debited against the individual IP and an IPv4 `/24` or IPv6 `/64`. Joining is
not delayed while waiting for a direct path. Relay addresses are never treated
as client addresses, so relay-only clients receive EndpointId, Room, and global
enforcement only.

Source limiter records have both a TTL and a global entry cap. Metrics use only
fixed counters; Room locators, EndpointIds, IPs, and prefixes are never metric
labels.

## Server configuration

All policy values are CLI-configurable. Defaults are initial test values and
should be tuned using direct, shared-NAT, and relay load tests.

| Policy | CLI option | Default |
| --- | --- | ---: |
| Room lifetime | `--room-ttl` | 300 s |
| Expired/exhausted tombstone | `--room-tombstone-ttl` | 300 s |
| Cumulative short-Room attempts | `--room-attempt-limit` | 6 |
| Room rate | `--room-rate-events/period/burst` | 6 / 300 s / 2 |
| EndpointId rate | `--endpoint-rate-events/period/burst` | 10 / 60 s / 20 |
| IP rate | `--ip-rate-events/period/burst` | 30 / 60 s / 60 |
| `/24` or `/64` rate | `--subnet-rate-events/period/burst` | 120 / 60 s / 240 |
| Global connections | `--max-connections` | 256 |
| Connections per EndpointId | `--max-connections-per-endpoint` | 8 |
| Connections per Room | `--max-connections-per-room` | 2 |
| Room-state / creator-waiter caps | `--max-room-states`, `--max-waiting-creators` | 8192 / 4096 |
| Source records / idle TTL | `--max-source-states`, `--source-state-ttl` | 8192 / 600 s |
| Handshake / Join deadlines | `--handshake-timeout`, `--join-timeout` | 10 s / 10 s |
| Relay lifetime / idle deadline | `--relay-ttl`, `--relay-idle-timeout` | 120 s / 30 s |
| Slow-frame deadline / frame body | `--slow-frame-timeout`, `--max-frame-body` | 10 s / 64 KiB |
| Close grace | `--close-grace` | 10 s |
| Retry guidance cap / unavailable delay | `--max-retry-after`, `--unavailable-retry-after` | 300 s / 1 s |

Client runtime TOML may set `rendezvous_pairing_attempts`,
`rendezvous_server_retries`, and `rendezvous_max_retry_after_seconds`.

`RoomRegistry::metrics_snapshot()` provides bounded internal counters and
gauges for active connections, active Rooms, creator waiters, Room connections, tracked sources,
matches, exhaustion, expiry, rejection classes, timeouts, malformed joins, and
oversized frames. No metrics HTTP endpoint is exposed.
