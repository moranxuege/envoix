# Rendezvous broker deployment

Status: operations guide for `apps/envoix-rendezvous-server`.

This document covers the Envoix rendezvous broker only. The relay is a separate
service that Envoix does not implement; see [Relay](#relay) at the end.

## What the broker is

The broker matches an invitation creator with a joiner on the same Room
locator, assigns the two SPAKE2 roles, and then forwards opaque length-prefixed
frames between them until the Room ends. It never parses payload bodies, and
file data never traverses it at all.

An operator can therefore run a broker for peers they do not control without
being able to read their transfers. What a broker does observe is the Room
locator, both endpoint ids, peer IP addresses when a direct address is present,
and timing. With `--geoip-city` or `--geoip-asn` configured, that becomes a
city and carrier annotation in the logs. Treat a broker as an untrusted mailbox
that still sees metadata.

## Requirements

| Item | Requirement |
| --- | --- |
| Address | a stable public IPv4 |
| Inbound | UDP on the bind port, default 8445 |
| Domain | not needed |
| TLS certificate | not needed |
| CPU and memory | a few MB resident at the default caps |
| Bandwidth | negligible; control frames only, capped at 64 KiB per frame and 120 s per matched Room |
| Host | x86_64 Linux with glibc 2.31 or newer for the prebuilt artifact |

The address requirement is strict. Clients address a broker as
`<endpoint-id>@<ip:port>` and parse the right-hand side as a socket address, so
a DNS name does not work and dynamic DNS is not a substitute for a stable
address. Identity comes from the raw endpoint key rather than X.509, which is
why no domain or certificate is involved.

## Build

From a checkout:

```bash
scripts/with-build-cache-guard.sh \
  cargo build --locked --release -p envoix-rendezvous-server
```

The binary lands at `target/release/envoix-rendezvous-server`.

For a host whose glibc is older than the build machine's, dispatch the
`rendezvous server artifact` workflow instead. It builds in a Debian 11
container and publishes `envoix-rendezvous-server-linux-x86_64-glibc231`. The
workflow is `workflow_dispatch` only, so it never runs on push.

## Install

```bash
sudo install -m 0755 envoix-rendezvous-server /usr/local/bin/
sudo useradd --system --home /var/lib/envoix-rendezvous --create-home envoix
```

### The secret key is the server's identity

`--secret-key` names a file holding the persistent endpoint key, created with
owner-only permissions when missing. The endpoint id that every client has
configured is derived from it.

Losing or regenerating that file changes the endpoint id, which invalidates the
broker address on every device. Back it up, and keep it at an absolute path so
the identity does not depend on the working directory:

```bash
--secret-key /var/lib/envoix-rendezvous/rendezvous-secret.key
```

### systemd unit

`/etc/systemd/system/envoix-rendezvous.service`:

```ini
[Unit]
Description=Envoix rendezvous broker
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=envoix
ExecStart=/usr/local/bin/envoix-rendezvous-server \
  --bind 0.0.0.0:8445 \
  --secret-key /var/lib/envoix-rendezvous/rendezvous-secret.key \
  --log-format json
Restart=on-failure
RestartSec=3s
UMask=0077
StateDirectory=envoix-rendezvous
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now envoix-rendezvous
```

### Open the port

The broker is UDP. A security group or firewall that only opens TCP looks
correct and fails at pairing time.

```bash
sudo ufw allow 8445/udp
```

On a cloud host, add the same rule to the provider's security group.

## Read the broker address

At startup with `--log-format pretty` the server prints its endpoint id and a
ready-to-paste flag. Bound to `0.0.0.0` it cannot know its own public address,
so it leaves a placeholder:

```
rendezvous endpoint id: <endpoint-id>
listening on 0.0.0.0:8445
connect with: --rendezvous <endpoint-id>@<this-host-ip>:8445
```

Under `--log-format json` the same facts appear in the structured `rendezvous
server listening` line. Substitute the public IP to get the value clients need.

## Point clients at the broker

The compiled broker and relay defaults have one source:
`crates/envoix-client/src/configuration.rs`. UniFFI exports that pair through
`envoixDeploymentEndpoints()`, so Apple and Android must not duplicate it.
Changing those two Rust constants and rebuilding the clients is sufficient for
a future default deployment change.

The current deployment is:

```text
broker: 6de87065a13b786177e37cd039ad8ff2b32ac9a78fb8f248ac919a9fcbe67b92@47.237.15.48:8445
relay:  https://relay.envoix.cc:8444
```

Ad-hoc CLI runs can still override the defaults with `--rendezvous` and
`--relay`. Managed Agent settings persist their own validated route, so an
installed Agent can move without editing its service definition or rebuilding:

```bash
envoix agent configure \
  --broker '<endpoint-id>@<ip>:8445' \
  --relay '<relay-url>'
```

Use `--relay none` to disable relay use. The command writes settings atomically
and restarts the per-user service.

A remembered Relationship also contains the authenticated route used to find
that specific peer. Move it in place, without re-pairing or touching the
credential, on both peers:

```bash
envoix devices set-route '<device-id-or-exact-label>' \
  --broker '<endpoint-id>@<ip>:8445' \
  --relay '<relay-url>'
```

The Agent rejects a route change while the Relationship has an active Transfer
or pending offer. User-entered custom routes are never silently overwritten.

## Verify

UDP has no handshake, so a port scan proves little; a filtered port and an open
one can look alike. Verify by pairing two CLI endpoints against the new broker
while watching the server log for the `joined` line.

The creating side names the broker. The joining side does not, because the
broker address travels inside the invitation:

```bash
# receiver creates a directional invitation on the new broker
envoix receive --create-invite \
  --rendezvous '<endpoint-id>@<ip>:8445' \
  --output ./received

# sender pastes the invitation the receiver printed
envoix send --invite '<invite>' ./file
```

Both peers dial the broker outbound, so the broker only ever needs to accept
inbound UDP. Run the test from any host other than the server itself; passing
from a machine outside the server's own network confirms the security group as
well as the process.

## Restart and upgrade

Room state is held in memory and nothing is persisted except the secret key.
A restart drops parked Rooms and in-flight pairings; clients retry and recover
on their own. There is no database and no migration step.

Upgrading is a binary replacement plus a restart, provided the secret key file
is preserved.

Two notes for an upgrade from a v0.2.2-era build:

- The pairing wire format and rendezvous protocol version are unchanged, so
  older clients keep working without an update.
- The diagnostics endpoint is now fail-closed. If the deployment runs
  `--log-bind`, uploads require `--log-upload-token-file` and retrieval requires
  `--log-view-token-file`, or the explicit `--unsafe-open-log-view` opt-in.
  Deployments that leave `--log-bind` unset are unaffected.

## Tuning

The Room TTL, tombstone TTL, attempt budgets, rate limits, and connection caps
are all flags with the defaults documented in
[Room abuse protection](room-abuse-protection.md). The defaults suit a small
private deployment.

Note that the tombstone TTL applies to human Room codes only. A remembered
device locator is released as soon as it expires, so a remembered pair can park
again immediately rather than waiting out a tombstone.

## Optional diagnostics endpoint

`--log-bind` enables a per-room log collection endpoint. It is off by default
and should stay off unless it is needed. When enabled, supply `--tls-cert` and
`--tls-key` so it serves HTTPS, and supply the bearer token files described
above. The PEM pair is re-read periodically, so certificate renewal does not
require a restart and live Rooms survive it.

## Relay

The relay is not Envoix code. The deployment currently uses an unmodified
upstream `iroh-relay`, installed with:

```bash
cargo install --version 1.0.0 --features server iroh-relay
```

It forwards encrypted QUIC between endpoints that cannot reach each other
directly. It cannot parse the Envoix protocol, which rides inside that
encrypted connection, so hosting choices for the relay do not constrain
protocol work.

Two options:

- Use the public relay infrastructure that ships with iroh. This costs nothing
  and needs no operator, at the price of exposing volume and timing metadata to
  a third party. Envoix currently always builds a single custom relay, so
  selecting the default relay map is a code change rather than configuration.
- Self-host. Unlike the broker this needs a real domain, a publicly trusted TLS
  certificate, and real bandwidth, because a relayed transfer carries every
  payload byte. `scripts/nat-test.sh` contains a working configuration to copy.
