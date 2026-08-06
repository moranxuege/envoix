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

The relay is deliberately held at a distance. A deployed relay sits tens of
milliseconds away, but this one runs on the test host, where reaching it costs
nothing and a direct connection is never the faster option. Left alone the app
has no reason to leave the relay, and no profile can exercise a direct path.
So the relay answers on its own address, `198.18.0.10`, and each router delays
traffic to that address by `--relay-delay-ms` (50 ms by default, 0 to disable).

Only the relay is slowed. The broker keeps its real timing, so pairing is not
inflated, and phone-to-phone traffic is untouched, so the delay decides which
path gets chosen without changing what the transfer then measures. Verified:
adding it moved `home_wifi` from 1299.2 to 1299.6 KiB/s, a 0.03% difference.

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

Percentages compare payload delivered against the uplink the profile sets. The
cap is decimal, as `tc` reads it: 1 Mbit/s is 1,000,000 bit/s.

| profile | uplink cap | measured | as bit/s | share of cap |
| --- | --- | --- | --- | --- |
| `congested_edge` | 1 Mbit/s | 57.1 KiB/s | 0.47 Mbit/s | 47% |
| `mobile_lte` | 5 Mbit/s | 361.1 KiB/s | 2.96 Mbit/s | 59% |
| `home_wifi` | 20 Mbit/s | 1299.6 KiB/s | 10.65 Mbit/s | 53% |
| `lan_1gbit` | 1 Gbit/s | 3663.8 KiB/s | 30.01 Mbit/s | 3% |
| `unshaped` | none | 3657.6 KiB/s | 29.96 Mbit/s | n/a |

All five now complete over a direct connection rather than the relay.

These are goodput figures: the numerator counts only payload bytes that
arrived, not the headers, ACKs and retransmissions that also crossed the wire.
Goodput is always below the link rate.

For the three profiles the shaper actually constrains, throughput tracks the
cap across a 20x range and the share stays in a narrow band, 47% to 59%. That
band is what says the shaping is the binding constraint rather than some other
bottleneck. Two effects hold it near half. `seconds` starts when the sender is
launched, so pairing and the handshake sit inside the measurement even though
they move no payload, which costs fast links proportionally more. The rest is
QUIC framing, encryption and virtual-NIC overhead, plus retransmission on the
two profiles that set loss.

Compare profiles against each other with these numbers. Do not quote them as
link speed.

## Known limits

### The 1 Gbit profiles measure the harness, not the link

`lan_1gbit` reaches 3% of its cap, and it lands within 0.17% of `unshaped`,
which applies no shaping at all. Two settings that differ by a factor of a
thousand producing the same number means the shaper is not what limits them.
The emulator's virtual network and the host CPU are, at roughly 30 Mbit/s.

So treat 30 Mbit/s as this harness's ceiling. `lan_1gbit` is a useful
upper-bound case and a control against `unshaped`, but it does not tell you how
the app behaves on a real gigabit LAN. Any profile set above about 30 Mbit/s
will report the same figure. The three slower profiles are the ones whose
numbers describe the link they name.

### Why the relay is delayed at all

Before the relay delay existed, `unshaped` and `lan_1gbit` completed through
the relay rather than a direct path, and failed `friendly-both-ipv4`. Both set
`rtt_ms = 1`, and the relay ran on the test host next to both emulators, so the
relay path was indistinguishable from the direct path and there was no reason
to migrate off it.

Isolated by elimination at the time:

- Not payload size. A 128 MiB transfer on `unshaped` still relayed, which ruled
  out the transfer finishing before hole punching completed.
- Not bandwidth. A diagnostic profile at 1 Gbit/s with `rtt_ms = 10` connected
  directly and passed.

Setting `--relay-delay-ms 0` restores the old behaviour, which is worth knowing
if a future change makes a case relay unexpectedly: run it once with the delay
disabled to tell a path-selection problem apart from a transfer problem.

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
