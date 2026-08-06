# Emulator speed matrix under controlled network conditions

Measures transfer throughput between two Android emulators over a link whose
bandwidth, latency and loss we set ourselves. Nothing in the loop is external:
the broker, the relay and both routers run on the test host, so a run is
reproducible and does not depend on anyone's Wi-Fi.

`scripts/nat-test.sh` already built the isolated two-emulator topology for NAT
testing. This adds link shaping, a recorded throughput number, and the registry
rows that name the conditions.

## Topology

Two emulators sit behind separate routers on isolated networks. Each router
NATs toward a host bridge. The broker and an `iroh-relay` instance run on the
host with a locally generated CA, so pairing and relay fallback work with no
internet access.

Shaping is applied with `tc netem` on each emulator's `wlan0`, which carries
rate, delay and loss in a single qdisc. Half the profile's round trip is
applied at each end, so the registry figure is what a packet actually sees.

## Profiles

Defined in `tests/e2e/matrix/cases.v1.json` under `network_profiles`.

| profile | downlink | uplink | RTT | loss |
| --- | --- | --- | --- | --- |
| `unshaped` | 1 Gbit/s | 1 Gbit/s | 1 ms | 0% |
| `lan_1gbit` | 1 Gbit/s | 1 Gbit/s | 1 ms | 0% |
| `home_wifi` | 50 Mbit/s | 20 Mbit/s | 10 ms | 0% |
| `mobile_lte` | 10 Mbit/s | 5 Mbit/s | 60 ms | 0.5% |
| `congested_edge` | 2 Mbit/s | 1 Mbit/s | 200 ms | 2% |

`unshaped` skips shaping entirely rather than applying a wide-open qdisc, so it
is the control case.

List them without starting anything:

```bash
scripts/nat-test.sh --list-networks
```

## Running it

Needs the Android SDK, two x86_64 AVDs, and root for network namespaces,
bridges, iptables and dnsmasq. The first run builds the CA, the JNI libraries
and the APK, so allow several minutes; later runs reuse them.

```bash
head -c 8388608 /dev/urandom > /tmp/matrix-payload.bin
scripts/nat-test.sh --verbose --network home_wifi --run friendly-both-ipv4 \
    AVD_Phone AVD_Phone2 /tmp/matrix-payload.bin
```

Each completed transfer prints one machine-readable line:

```text
[friendly-both-ipv4] THROUGHPUT link=home_wifi bytes=8388608 seconds=6.305 kib_per_second=1299.2
```

Feed a measurement into the contract with:

```bash
python3 scripts/matrix_contract.py record-result ... \
    --throughput-bytes 8388608 --throughput-seconds 6.305
```

The script kills `netsimd` and reconfigures host networking as root. Do not run
it on a machine whose networking you care about.

## Measured results

Two AVDs on one Linux host, 8 MiB payload, 2026-08-06. Every run delivered a
byte-identical file (SHA-256 of the received copy matched the source).

| profile | uplink cap | measured | goodput vs cap |
| --- | --- | --- | --- |
| `congested_edge` | 1 Mbit/s | 57.1 KiB/s | 45% |
| `mobile_lte` | 5 Mbit/s | 361.1 KiB/s | 56% |
| `home_wifi` | 20 Mbit/s | 1299.2 KiB/s | 51% |
| `unshaped`, `lan_1gbit` | 1 Gbit/s | not measurable, see below | |

Throughput tracks the caps and stays monotone across a 20x range, so the
shaping is the binding constraint rather than some other bottleneck.

Two things stop these numbers from being raw link capacity. `seconds` starts
when the sender is launched, so pairing and the handshake are inside the
measurement. The rest is QUIC, encryption and virtual-NIC overhead. The ratio
is stable near half the shaped rate across every profile, so the figures
compare profiles against each other correctly; do not read them as link speed.

## Known limits

### The 1 ms profiles never leave the relay

`unshaped` and `lan_1gbit` complete through the relay instead of a direct path,
so they fail `friendly-both-ipv4`, which asserts a direct connection. This is a
property of the harness, not of the app.

The relay runs on the test host, next to both emulators. At `rtt_ms = 1` the
relay path is indistinguishable from the direct path, so there is no reason to
migrate off it. Every profile that adds latency makes the relay's two hops
measurably worse than one direct hop, and the upgrade happens.

Isolated by elimination:

- Not payload size. A 128 MiB transfer on `unshaped` still relayed, which rules
  out the transfer finishing before hole punching completes.
- Not bandwidth. A one-off diagnostic profile at 1 Gbit/s with `rtt_ms = 10`
  connected directly and passed at 3710.7 KiB/s.

Fixing it means making the relay realistically distant, roughly the 20-50 ms a
deployed relay would sit at. `netem` is currently on the device's `wlan0` root
qdisc and therefore hits all traffic, so penalising only relay traffic needs a
classful qdisc with a filter, or shaping on the host side instead. Until then,
the direct-path assertion is only meaningful for profiles with latency.

### Timeouts follow the link

A fixed timeout is meaningless once the link is shaped. 8 MiB needs 67 s of
wire time at `congested_edge`'s 1 Mbit/s uplink before loss and RTT are
counted, which overran the old 120 s limit and reported a failure for a
transfer that was progressing normally.

The per-transfer timeout is now derived from the payload size and the profile's
uplink, and is raised only when the link is the slower constraint. With an 8 MiB
payload that moves `congested_edge` to 198 s and leaves every other profile at
the configured default.

## Fixes this work required

Both were pre-existing and independent of the shaping.

`friendly-both-ipv4` asserted that the peer type equals `direct`, but the app
reports `direct_ipv4` or `direct_ipv6`; the unqualified value is legacy only.
The case could not pass under any network condition. The check now accepts the
whole direct family.

The transfer timeout was the fixed 120 s described above.
