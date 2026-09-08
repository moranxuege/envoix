# Envoix v0.3 release evidence

Status: active evidence registry

This registry records reproducible release-path checks without treating a
development or test signature as a production release approval.

## Full production-Android and desktop rehearsal — 2026-09-08

- Run [34207922558](https://github.com/moranxuege/envoix/actions/runs/34207922558)
  passed at immutable source `2cdce5f5a75c729a81477cc4476fed87195857b1`.
- All eight jobs passed: release contract, four desktop builds, compatible
  broker build, signed Android build, and desktop bundle metadata/attestations.
  Tag-only publication was skipped; no release was published.
- Desktop bundle: eight binaries, four CycloneDX SBOMs, source manifest, and
  checksums. Android bundle: APK/AAB, runtime and embedded-Rust SBOMs, signed
  certificate policy in its source manifest, and checksums.
- Independent download verification passed 18/18 checksum entries and 20/20
  provenance/SBOM checks. Every check pinned the repository, exact source
  digest and expected workflow, and denied self-hosted runners.
- APK SHA-256: `db6dcf52bb568e7ce30381b840b8e58f1ffeea99a9d5f47162c421701f2bef72`.
- AAB SHA-256: `f818e5d82f52ce7f7e425c710f82b24d9a8046fe34ae06ad1f702868621514f7`.
- Both packages use the approved production certificate
  `31395b806e89f3892ff62b852455d6f7f8b57346917e40693a7724e52a7876b6`,
  application ID `dev.envoix.app`, version `0.3.0`, build `5`.
- The preceding run failed because Bullseye security metadata expired after
  Debian 11 LTS ended. The successful run uses already-bundled tools in the
  same pinned image; the maximum glibc requirement check remains in place.

This closes the production Android signing/pipeline gap. It does not close
Windows Authenticode, latest macOS app notarization, TestFlight distribution,
or real-device/upgrade acceptance. See the
[current readiness record](release-readiness-2026-09-08.md).

## Desktop and broker bundle rehearsal 33792965105

| Field | Evidence |
| --- | --- |
| Workflow | manual `release` run [33792965105](https://github.com/moranxuege/envoix/actions/runs/33792965105) |
| Immutable source | `89b216ab13e89e932ce3da426318e585cd660852` |
| Result | contract, four platform CLI/Agent build jobs, the reusable broker build, bundle validation, build provenance, and three SBOM attestations passed; Android was intentionally excluded and tag-only publication skipped |
| Bundle | seven binaries, CLI/Agent/broker CycloneDX 1.5 SBOMs, `release-manifest.json`, and `SHA256SUMS` |
| Independent check | downloaded the single `envoix-release-0.3.0` artifact into a new temporary directory; all eleven checksum entries passed |
| Identity check | manifest repository was `moranxuege/envoix`, version was `0.3.0`, revision exactly matched the source above, and its ten input artifacts were the seven expected binaries plus three expected SBOMs |
| Provenance policy | all seven binaries passed GitHub attestation verification with the exact repository, source digest, denial of self-hosted runners, and their expected signer: `release.yml` for CLI/Agent and the reusable `rendezvous-server-artifact.yml` for broker |
| SBOM policy | all seven binaries passed a separate `https://cyclonedx.org/bom` attestation check with the exact repository, `release.yml` signer, source digest, and runner restriction |
| Workflow hygiene | every effective job had an empty warning/failure annotation set; checkout, setup, cache, upload, and download Actions used their pinned Node 24 generations |

The CLI SBOM described `envoix 0.3.0` with 464 components and 465 dependency
relationships. The Agent SBOM described `envoix-agent 0.3.0` with 462
components and 463 relationships. The broker SBOM described
`envoix-rendezvous-server 0.3.0` with 432 components and 433 relationships.
All three used deterministic UUIDv5 serials bound to the repository, revision,
and root component.

The broker job ran Rust 1.96.0 in the digest-pinned Debian 11/Bullseye image.
The builder provided glibc 2.31, while the staged x86-64 ELF requested the
expected `/lib64/ld-linux-x86-64.so.2` interpreter and required at most glibc
2.30. This independently downloaded rehearsal bundle was deleted after
verification.

This rehearsal closes the broker-integrated desktop bundle automation gap. It
does not prove production Android signing, Apple Developer ID notarization,
iOS/TestFlight distribution signing, or real-host platform behavior.

## Desktop bundle rehearsal 33782619684

| Field | Evidence |
| --- | --- |
| Workflow | manual `release` run [33782619684](https://github.com/moranxuege/envoix/actions/runs/33782619684) |
| Immutable source | `40e9099fa8d3f872b6d3c9f635986ad1f4cc6390` |
| Result | contract, four platform build jobs, bundle validation, build provenance, and SBOM attestation passed; tag-only publication skipped |
| Bundle | six desktop binaries, CLI/Agent CycloneDX 1.5 SBOMs, `release-manifest.json`, and `SHA256SUMS` |
| Independent check | downloaded the single `envoix-release-0.3.0` artifact; all nine checksum entries passed |
| Identity check | manifest repository was `moranxuege/envoix`, version was `0.3.0`, and revision exactly matched the source above |
| Provenance policy | all six binaries passed GitHub attestation verification with exact repository, `.github/workflows/release.yml` signer, source digest, and denial of self-hosted runners |
| SBOM policy | all six binaries passed a separate `https://cyclonedx.org/bom` attestation check with the same signer/source/runner restrictions |

The CLI SBOM described `envoix 0.3.0` with 464 components and 465 dependency
relationships. The Agent SBOM described `envoix-agent 0.3.0` with 462
components and 463 relationships. Both had deterministic UUIDv5 serials.

This rehearsal proves the desktop workflow behavior at its recorded commit. It
does not prove macOS Developer ID/notarization, Windows clean-host behavior,
mobile distribution signing, or the later Android tag path. It predates the
broker-integrated bundle and must not be used as evidence for that newer path.

## Supply-chain CI 33791001252

CI run [33791001252](https://github.com/moranxuege/envoix/actions/runs/33791001252)
completed successfully for immutable source
`90283fee7531646df3e6e967237db52f53cf40a7`. Its five independent jobs passed:

- Rust formatting, Clippy, all-feature workspace tests, and the pinned RustSec
  audit;
- Windows CLI/Agent lint, tests, paired build, and lifecycle test;
- Android signing-negative tests, lint, unit tests, Debug package/JNI check,
  regenerated 105-component SBOM, and license policy;
- Rust license and source policy through pinned `cargo-deny 0.20.2`;
- pushed-history secret scanning through the immutable Gitleaks Action.

Before the CI run, a redacted local Gitleaks 8.30.1 scan of the complete Git
history passed after nine exact, reviewed test/namespace fingerprints were
recorded. The Rust license/source policy also passed locally in locked offline
mode. This evidence closes the automated dependency-license/source and secret
scan implementation gap; it does not close production signing or the physical
platform matrix.

## Android test-key rehearsal

The Android production-signing interface and tag workflow were exercised
locally with a one-day, explicitly test-only certificate. The guarded build
created signed `arm64-v8a` plus `x86_64` APK/AAB packages for
`dev.envoix.app` version `0.3.0` build 5. External APK and AAB checks agreed on
one certificate; the package identity check, required ZIP/JNI entries, bundle
manifest, and all five checksum entries passed.

The Android runtime SBOM described 105 components and 106 dependency
relationships. The embedded `envoix-ffi` Rust SBOM described 495 components and
496 relationships. The test certificate, packages, downloaded desktop bundle,
and generated source-tree SBOMs were deleted after verification.

This is implementation evidence only. It must be replaced by an immutable tag
run using the approved production key and retained certificate digest before
V03-SEC-01 can close.
