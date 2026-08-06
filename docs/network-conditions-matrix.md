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

Omit `--run` to take all four NAT cases against that link on one pair of
emulator boots, which is how the matrix below was produced:

```bash
for net in unshaped lan_1gbit home_wifi mobile_lte congested_edge; do
    scripts/nat-test.sh --verbose --network "$net" \
        AVD_Phone AVD_Phone2 /tmp/matrix-payload.bin
done
```

That is 20 transfers and takes roughly 40 minutes, most of it `congested_edge`.

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

Two AVDs on one Linux host, 8 MiB payload, 2026-08-06. Every completed run
delivered a byte-identical file (SHA-256 of the received copy matched the
source). Each figure is the mean of three repetitions, in KiB/s, with the
spread between the fastest and slowest run. The cap is decimal, as `tc` reads
it: 1 Mbit/s is 1,000,000 bit/s.

| link | symmetric-both-ipv4 | friendly-both-ipv4 | symmetric-one-side-ipv4 | symmetric-both-ipv6 |
| --- | --- | --- | --- | --- |
| `lan_1gbit` | 1922.8 +-0.6% | 3718.0 +-1.3% | 3735.8 +-0.5% | 3722.6 +-0.7% |
| `home_wifi` | 1087.7 +-29.3% | 1296.5 +-1.2% | 1302.7 +-0.2% | 1299.6 +-0.2% |
| `mobile_lte` | 377.9 +-29.1% | 342.7 +-16.3% | 353.3 +-18.4% | 342.1 +-16.0% |
| `congested_edge` | 79.0 +-9.8% | 58.9 +-9.2% | 58.7 +-13.0% | 54.1 +-10.9% |

`unshaped` is absent. All three of its repetitions failed the same way: the
`symmetric-both-ipv4` case timed out, after which the sender emulator lost its
Wi-Fi address and took the rest of that invocation with it. The same case had
passed earlier the same day, so this is unexplained rather than understood. It
was not investigated further; the host was low on disk at the time, which is a
candidate but not a finding.

`symmetric-both-ipv4` puts a strict NAT on both sides, so it cannot hole punch
and is the one case expected to run over the relay. It does, on every link. The
other three complete directly, which is what they exist to prove.

Repetition changes the reading. Direct transfers are stable, within 1.3% across
runs, and the relay column is not: it swings up to 29%. Any single relay
measurement is close to meaningless, and the relay-versus-direct comparison
below only became trustworthy once averaged.

| link | relay speed as a share of direct |
| --- | --- |
| `lan_1gbit` | 52% |
| `home_wifi` | 84% |
| `mobile_lte` | 110% |
| `congested_edge` | 134% |

Only `lan_1gbit` pays the expected penalty for crossing the host twice. The
penalty shrinks as the link slows and inverts on the two lossy profiles, where
relaying beat connecting directly across all three repetitions. That is no
longer dismissible as sampling noise, and it has no explanation here. Part of
it is certainly the harness: the relay-to-receiver leg runs unshaped, for the
reason in the receiver downlink note below. Whether that accounts for all of it
is untested.

The `home_wifi` relay figure is bimodal rather than noisy: 981.5, 981.5, and
1300.2. The outlying run matches that link's direct speed almost exactly. Three
samples cannot say whether that is a second code path or a coincidence.

### Payload size dominates everything else

Same link and same NAT case, varying only the file. Three repetitions each.

| size | mean time | of which on the wire | throughput | share of the 20 Mbit/s cap |
| --- | --- | --- | --- | --- |
| 1 MiB | 2.21 s | 0.42 s | 3.80 Mbit/s | 19% |
| 8 MiB | 6.32 s | 3.36 s | 10.61 Mbit/s | 53% |
| 64 MiB | 31.00 s | 26.84 s | 17.32 Mbit/s | 87% |

Every other measurement in this document used 8 MiB, and that choice cost the
app a factor of 1.6. At 64 MiB it reaches 87% of the shaped link, which is a
reasonable figure for an encrypted transport. The 53% reported at 8 MiB, and
the "roughly half the cap" reading built on it, are artifacts of a payload too
small to amortise startup.

Two effects produce the ramp. Time spent off the wire is 1.79s, 2.97s and 4.16s
for the three sizes, so a fixed setup cost of roughly 1.6 to 2 seconds is
present but does not grow with the payload. The remainder is the transport
starting slow and accelerating, which is why the effective rate keeps climbing
rather than flattening once setup is amortised.

Prefer 64 MiB for anything meant to characterise throughput. Keep 8 MiB only
where run time matters more than accuracy, and do not compare figures taken at
different sizes.

A fourth size, 64 KiB, is missing from the table because all three of its runs
failed an assertion rather than a transfer; see the note on small transfers
below.

These are goodput figures: the numerator counts only payload bytes that
arrived, not the headers, ACKs and retransmissions that also crossed the wire.
Goodput is always below the link rate.

For the three profiles the shaper actually constrains, throughput tracks the
cap across a 20x range and the share of the uplink stays in a narrow band, 47%
to 59% on `friendly-both-ipv4`. That band is what says the shaping is the
binding constraint rather than some other bottleneck. Two effects hold it near
half. `seconds` starts when the sender is
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
receiver leg runs unshaped; that is why relaying costs 48% on `lan_1gbit` but
only 16% on `home_wifi` and nothing at all on the two slower profiles. Do not
read that as evidence that relaying is cheap.

Enforcing it means redirecting ingress to an `ifb` device and shaping that,
which is a real addition rather than a parameter change. Until then, read every
figure as a test of the sender's uplink.

### Short transfers finish before the direct path is ready

A transfer that ends quickly enough is delivered over the relay whatever the
NAT allows, and the two sides then disagree about which path was used: the
sender reports `direct_ipv4` while the receiver reports `relay`.

The size sweep isolates this. On `home_wifi` with `friendly-both-ipv4`, a
relaxed NAT on both sides and nothing else varying, all three 64 KiB runs ended
in that disagreement and failed the direct-path assertion, while every run at
1 MiB and above passed with both sides reporting a direct connection. The
transfers themselves succeeded; the payload arrived intact each time.

This is the same disagreement seen earlier on `symmetric-both-ipv6`, which
appeared only on the two links where the transfer completed in about two
seconds and never on the three slower ones. Both observations fit one
explanation: hole punching has not finished when a short transfer is already
over, so the data really does travel by relay, and only the sender's record
catches the subsequent upgrade.

Two consequences. A direct-path assertion is only meaningful once the transfer
lasts longer than pairing, so a case that asserts one needs a payload no
smaller than about 1 MiB. And for the product, small files do not benefit from
hole punching at all; whether that is worth changing is a question for the
transport, not for this harness.

### The 1 Gbit profiles do not report a link speed

`lan_1gbit` reaches 3% of its cap and lands within 0.17% of `unshaped`, which
applies no shaping at all. Two settings a thousand times apart producing the
same number means the shaper is not what limits them. That much is solid.

What does limit them is not established. At 1 Gbit the payload needs 0.067s of
wire time against a 2.2s measurement, so 97% of that figure is setup and
transport ramp rather than the file crossing the link.

The size sweep narrows this without closing it. Setup costs roughly 1.6 to 2
seconds, which is most of a 2.2 second measurement, so the 1 Gbit figures are
very largely startup. It does not give the ceiling, because the sweep was run
on `home_wifi` where the shaper still binds; the transport had no reason to go
faster than 20 Mbit/s there. Repeating the sweep at 64 MiB against `lan_1gbit`
would settle it and is the obvious next measurement.

Until then, do not quote either 1 Gbit profile as a throughput figure. They
remain useful as a control pair: the two agreeing is itself the evidence that
the shaper has stopped binding.

The three slower profiles are unaffected at 8 MiB and above. At 1 Mbit/s the
payload occupies 67s of a 144s measurement, so a setup cost of a second or two
cannot distort them.

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
