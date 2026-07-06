# Observability

Envoix exposes three planes of observability. Keep them distinct: an
occurrence maps to whichever plane(s) fit, not all three by reflex.

| Plane | What | For | Where |
|---|---|---|---|
| **Events** | the transfer's story (`TransferEvent`) | users, UIs, campaigns | client `--json` / human render |
| **Logs** | diagnostic detail (`tracing`, levelled) | developers, operators | client stderr, server stdout |
| **Metrics** | aggregate counters/gauges | operators | *not yet built* |

## Correlation ids

Two ids stitch a single transfer across the broker and both peers. They appear
as span fields on every log line and as event fields on the stream, so the same
key greps across all three logs:

- **`room`** — the numeric rendezvous id (the part before the first `-` in a
  room code; the remainder is the SPAKE2 password and never reaches a log).
  Links **broker ↔ both clients**. Room transfers only.
- **`transfer_id`** — derived when the transfer starts; identical on both peers.
  Links **sender ↔ receiver**. All modes.

## Logs

**Levels** (client and server share one policy): `error` = the operation failed
and needs attention · `warn` = recoverable/anomalous (a pairing retry, a relay
home that never registered, a rejected join) · `info` = lifecycle milestones
(pairing, connected, completed; broker `matched`/`expired`; the per-transfer
summary) · `debug` = developer detail (data-path changes, dropped candidates) ·
`trace` = iroh internals.

**Client verbosity:** default shows `info`; `-v` adds envoix `debug`; `-vv`
adds iroh internals. `RUST_LOG` overrides. Logs go to stderr so stdout stays
clean for `--json`.

**Server format:** `--log-format pretty` (human) or `--log-format json` (one
JSON object per line, span fields included, for aggregators). Default filter is
`envoix_rendezvous=info,envoix_rendezvous_iroh=info,warn`; override with
`RUST_LOG`.

**Spans.** A client transfer runs in a `transfer{direction, mode, room,
transfer_id}` span. A broker connection runs in a `conn{room, peer, geo}` span
(`peer`/`geo` fill in a few seconds after connect, once the peer's direct path
settles — a NATed peer reaches even a public broker over the relay first). The
broker also emits a `peer located` line and each client a `transfer finished`
summary line (`bytes`, `file`, `outcome`) at `info`.

## Stitching one transfer end to end

A room transfer touches three logs: the broker and the two clients. Given a
room id `888`:

```
# Broker: both peers join, their addresses/geo, and the match.
grep 'room=888' broker.log
#   conn{room=888 peer=117.135.95.10:15801 geo=China Mobile (AS24400)}: peer located
#   conn{room=888 peer=73.47.70.209:...    geo=Comcast (AS7922)}:       peer located
#   conn{room=888 ...}: matched two peers

# Either client: same room id, plus the shared transfer id.
grep 'room=888' sender.stderr        # -> transfer_id=transfer-72dd...
grep 'room=888' receiver.stderr
```

Then pivot on the shared `transfer_id` to line up the two client sides (works
for non-room modes too, where there is no `room`):

```
grep 'transfer_id=transfer-72dd...' sender.stderr receiver.stderr
#   sender:   transfer finished bytes=8192 file=f.bin outcome="completed"
#   receiver: transfer finished bytes=8192 file=f.bin outcome="completed"
```

For machine parsing, run the clients with `--json` (the event stream carries
`ts_ms`, `transfer_id`, and typed `path`) and the server with
`--log-format json` (span fields, including `room`/`peer`/`geo`, become object
keys). One transfer is then: broker JSON filtered by `room`, joined to the two
client streams by `transfer_id`.

## GeoIP (optional, offline)

The broker annotates peer addresses with a location + ISP when given MaxMind-DB
files. Nothing is committed (licensing + size) and no external service is ever
queried:

```
envoix-rendezvous-server --geoip-city GeoLite2-City.mmdb --geoip-asn GeoLite2-ASN.mmdb ...
#   conn{... geo=Shanghai, CN China Mobile (AS9808)}: peer located
```

The operator supplies the `.mmdb` files — MaxMind **GeoLite2** (free, account
required) or **DB-IP Lite** (CC-BY, no account); both are the same format. The
City database gives `city, country`; the ASN database gives the carrier. Either
is optional; with neither, peer lines carry the address only.

## Privacy

- The broker never sees the SPAKE2 password: `room` is the numeric prefix only.
- Peer addresses/geo are logged only on the operator's own broker (an
  access-log-style record), at `info`; the client keeps peer addresses in the
  event stream's `path` field and at `debug` in logs.
- Candidate addresses advertised to a peer can be scoped with the `[candidates]`
  CIDR allow/deny config (see `docs/design/client-api.md` §5.5 C2).
