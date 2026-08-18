# v0.3 dependency security baseline

Status: active release gate

Baseline date: 2026-08-18

Tool: `cargo-audit 0.22.2` with the current official RustSec advisory database.

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

These are targeted lockfile updates. They do not change Envoix's direct API
surface.

## Open warnings

### `paste 1.0.15` — unmaintained

Path:

```text
paste -> netlink-packet-core -> netdev/netwatch -> iroh
```

There is no reported vulnerability in the locked package. M8 must re-evaluate
the iroh/netwatch dependency line and either remove the path or record an
explicit release acceptance.

### `rustls-pemfile 2.2.0` — unmaintained

Path:

```text
rustls-pemfile -> axum-server -> envoix-rendezvous-server
```

M1/M8 evaluates an `axum-server` update or replacement. The warning cannot be
considered closed merely because PEM input is operator-controlled.

### `spin 0.10.0` — yanked

Path:

```text
spin -> futures-buffered -> n0-future -> iroh/envoix-session
```

At baseline time, `futures-buffered 0.2.13` is the current compatible upstream
release and still selects the yanked package. No RustSec vulnerability is
reported for this package. M8 must recheck upstream and remove the yanked lock
entry when a compatible release exists; introducing an Envoix-maintained fork
only to hide a yanked warning is not justified.

## Audit evidence requirements

Milestone and release evidence records:

- cargo-audit version;
- advisory database revision/date;
- exact `Cargo.lock` revision;
- vulnerability and warning counts;
- the dependency path and decision for every remaining warning.

An audit that cannot refresh its advisory database is not release evidence.
