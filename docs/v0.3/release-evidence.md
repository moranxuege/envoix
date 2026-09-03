# Envoix v0.3 release evidence

Status: active evidence registry

This registry records reproducible release-path checks without treating a
development or test signature as a production release approval.

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
