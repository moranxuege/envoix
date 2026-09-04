# v0.3 dependency security baseline

Status: active release gate

Baseline date: 2026-09-04

Tools:

- `cargo-audit 0.22.2` with the current official RustSec advisory database;
- `cargo-deny 0.20.2` for Rust license and dependency-source policy;
- Gitleaks Action `v3.0.0` pinned by commit, with a local full-history
  cross-check using Gitleaks `8.30.1`.

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

## License and dependency-source policy

[`deny.toml`](../../deny.toml) is the executable Rust policy. It checks the
all-feature workspace graph, permits only reviewed SPDX identifiers present in
the current lock graph, denies unknown registries, and denies every Git source.
The only permitted registry is the crates.io index. Workspace and the patched
`vendor/noq-udp` path remain visible in the graph rather than being pruned.

License alternatives are evaluated as choices. A dual-licensed dependency is
accepted only when at least one of its choices is in the policy; this does not
globally approve an unlisted copyleft license. The policy passed against the
locked 508-package graph with no license or source finding on 2026-09-04.

Android has a separate JVM/Android dependency graph. CI generates the direct
CycloneDX 1.6 SBOM and runs
[`check_sbom_licenses.py`](../../scripts/check_sbom_licenses.py). Every
component must declare at least one audited `Apache-2.0` or `BSD-3-Clause`
choice. A missing identifier, a license outside the policy with no approved
alternative, an unknown component shape, duplicate component identity, or an
unaudited SPDX expression is a hard failure. The refreshed baseline contains
105 components; JNA is accepted under its Apache-2.0 choice, not by approving
LGPL globally.

## Secret scanning policy

CI uses an immutable `gitleaks/gitleaks-action` commit and a full Git checkout
so push ranges can be evaluated without shallow-history gaps. Findings are not
uploaded as artifacts and are not copied into workflow comments because those
surfaces can amplify an exposed value. GitHub secret scanning and push
protection are also enabled on `moranxuege/envoix`; the two controls are
defense in depth, not substitutes for credential rotation.

A local redacted Gitleaks 8.30.1 scan of the complete repository history found
nine deterministic test/protocol strings and public Keychain namespace values,
then passed after each was reviewed and recorded by its exact fingerprint in
`.gitleaksignore`. The policy does not ignore a file tree, filename pattern, or
detector rule. A new finding must be treated as a credential incident until its
value and use are reviewed; merely adding another fingerprint is not closure.

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
