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

All four NAT cases against all five link profiles, 20 of 20 passing. Figures
are KiB/s. The cap is decimal, as `tc` reads it: 1 Mbit/s is 1,000,000 bit/s.

| link | symmetric-both-ipv4 | friendly-both-ipv4 | symmetric-one-side-ipv4 | symmetric-both-ipv6 |
| --- | --- | --- | --- | --- |
| `unshaped` | 1916.8 (relay) | 3667.3 | 3706.1 | 3731.9 |
| `lan_1gbit` | 1913.5 (relay) | 3697.0 | 3724.3 | 3739.6 |
| `home_wifi` | 1298.3 (relay) | 1301.8 | 1303.9 | 1305.1 |
| `mobile_lte` | 330.7 (relay) | 360.6 | 397.3 | 305.7 |
| `congested_edge` | 71.3 (relay) | 53.3 | 50.0 | 55.5 |

`symmetric-both-ipv4` puts a strict NAT on both sides, so it cannot hole punch
and is the one case that is expected to run over the relay. It does, on every
link. The other three complete directly, which is what they exist to prove.

Reading down a column shows the shaper working. Reading across a row shows what
the relay costs, and that turns out to depend entirely on the link:

| link | relay speed as a share of direct |
| --- | --- |
| `unshaped` | 52% |
| `lan_1gbit` | 52% |
| `home_wifi` | 100% |
| `mobile_lte` | 92% |
| `congested_edge` | 134% |

Only the unshaped pair pays the expected penalty for crossing the host twice.
Everything shaped pays little or nothing, and that is a harness artifact rather
than a fact about relays; see the receiver downlink note below. The
`congested_edge` row, where the relay came out ahead, is one sample on a link
that discards 2% of packets at random, so treat it as noise until it is
repeated rather than as a result.

These are goodput figures: the numerator counts only payload bytes that
arrived, not the headers, ACKs and retransmissions that also crossed the wire.
Goodput is always below the link rate.

For the three profiles the shaper actually constrains, throughput tracks the
cap across a 20x range and the share of the uplink stays in a narrow band, 47%
to 59% on `friendly-both-ipv4`. That band is what says the shaping is the
binding constraint rather than some other
bottleneck. Two effects hold it near half. `seconds` starts when the sender is
launched, so pairing and the handshake sit inside the measurement even though
they move no payload, which costs fast links proportionally more. The rest is
QUIC framing, encryption and virtual-NIC overhead, plus retransmission on the
two profiles that set loss.

Compare profiles against each other with these numbers. Do not quote them as
link speed.

## Known limits

### The receiver's downlink is never enforced

`netem` attached as a root qdisc shapes what leaves an interface, and the
script attaches one to each emulator's `wlan0`. On the sender that is the
payload direction, so `uplink_kbits` binds. On the receiver it is the direction
that carries acknowledgements, not the arriving file, so `downlink_kbits` never
constrains the transfer. There is no ingress shaping anywhere in the script.

Two visible consequences. The `downlink` column of every profile is currently
decorative for a one-way transfer. And a relay run is nearly free on a shaped
link, because only the sender-to-relay leg is limited while the relay-to-
receiver leg runs unshaped; that is why the relay costs 48% on `unshaped` but
nothing on `home_wifi`. Do not read that as evidence that relaying is cheap.

Enforcing it means redirecting ingress to an `ifb` device and shaping that,
which is a real addition rather than a parameter change. Until then, read every
figure as a test of the sender's uplink.

### The IPv6 case disagrees with itself on fast links

On `unshaped` and `lan_1gbit`, `symmetric-both-ipv6` finishes with the sender
reporting `direct_ipv6` and the receiver reporting `relay`. On the three slower
links both sides agree on `direct_ipv6`. The transfer passes either way and the
bytes are correct.

That it appears only on the two links where the transfer completes in about two
seconds points at a reporting race rather than a transport fault: the receiver
records the path it had when the last byte landed, and on a fast link that can
precede the upgrade to direct settling on its side. Not confirmed. It is worth
a look before anyone quotes per-side path data, and it is invisible in the
`friendly-both-ipv4` column that the earlier measurements used.

### The 1 Gbit profiles do not report a link speed

`lan_1gbit` reaches 3% of its cap and lands within 0.17% of `unshaped`, which
applies no shaping at all. Two settings a thousand times apart producing the
same number means the shaper is not what limits them. That much is solid.

What does limit them is not established. At 1 Gbit the payload needs 0.067s of
wire time against a 2.236s measurement, so 97% of that figure is setup and
transport ramp rather than the file crossing the link. Whether the real ceiling
is 50 Mbit/s or 250 Mbit/s depends on a fixed setup cost that has not been
measured; the arithmetic admits both.

So `lan_1gbit` is still worth running as a control against `unshaped`, and the
pair agreeing is itself the evidence that the shaper has stopped binding. Do
not quote either one as a throughput figure. Settling it needs a payload large
enough to make setup a rounding error, or a measurement that starts when the
first payload byte moves; the second is worth doing anyway.

The three slower profiles are unaffected. At 1 Mbit/s the payload occupies 67s
of a 144s measurement, so a setup cost of a second or two cannot distort them.

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
