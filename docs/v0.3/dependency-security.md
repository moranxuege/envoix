# v0.3 dependency security baseline

Status: active release gate

Baseline date: 2026-09-04

Tool: `cargo-audit 0.22.2` with the current official RustSec advisory database.

Reference audit: advisory database revision
`5a0ebedfe8bdd2e295b171f4162f8c977bcad9a5` (2026-09-02), 508 locked
dependencies, zero vulnerabilities, zero unsound advisories, and two accepted
warnings described below.

## Policy

Continuous integration rejects:

- every RustSec vulnerability affecting `Cargo.lock`;
- every RustSec `unsound` warning.

Unmaintained and yanked transitive dependencies are reported but do not fail
ordinary development CI. They must be resolved, replaced, or explicitly
accepted in the release security review before v0.3.0 is tagged. A warning is
not silently ignored in configuration because its dependency path and exit
condition are part of the review evidence.

CI command:

```bash
cargo audit --deny unsound
```

## Remediated baseline findings

| Package | Previous version | Remediated version | Reason |
| --- | --- | --- | --- |
| `crossbeam-epoch` | 0.9.18 | 0.9.20 | RUSTSEC-2026-0204 |
| `quick-xml` | 0.39.4 | 0.41.0 through `plist` 1.10.0 | RUSTSEC-2026-0194 and RUSTSEC-2026-0195 |
| `lru` | 0.18.0 | 0.18.2 | RUSTSEC-2026-0253 unsoundness |
| `h2` | 0.4.15 | 0.4.16 | RUSTSEC-2026-0258 |
| `chacha20` | 0.10.1 | 0.10.2 | Removed a yanked transitive release selected through `rand` and iroh. |
| `rustls-pemfile` | 2.2.0 | removed through `axum-server` 0.8.0 | Removed the unmaintained PEM parser from the rendezvous-server TLS path. |

These are targeted lockfile updates. They do not change Envoix's direct API
surface.

## Open warnings

### `paste 1.0.15` — unmaintained

Path:

```text
paste -> netlink-packet-core 0.8 -> netdev 0.45 -> netwatch 0.19.3 -> iroh
```

There is no reported vulnerability in the locked package. Updating from
`netwatch` 0.19.1 to 0.19.3 introduced the newer `netdev` line where supported,
but its Linux graph still retains `netdev` 0.45 and `paste`. The compatible
iroh 1.0 dependency line does not currently remove it. Release acceptance is
limited to this unmaintained proc-macro warning and must be rechecked before
the tag; an Envoix-maintained fork only to hide the warning is not justified.

### `spin 0.10.0` — yanked

Path:

```text
spin -> futures-buffered -> n0-future -> iroh/envoix-session
```

At the 2026-09-04 refresh, `futures-buffered 0.2.13` remains the current
upstream release and still selects the yanked package. No RustSec vulnerability
is reported for this package. Release acceptance is limited to this yanked
transitive release and must be rechecked before the tag; introducing an
Envoix-maintained fork only to hide a yanked warning is not justified.

## Audit evidence requirements

Milestone and release evidence records:

- cargo-audit version;
- advisory database revision/date;
- exact `Cargo.lock` revision;
- vulnerability and warning counts;
- the dependency path and decision for every remaining warning.

An audit that cannot refresh its advisory database is not release evidence.
