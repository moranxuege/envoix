# Envoix v0.3 release process

Status: active release-candidate procedure; v0.3.0 tagging is blocked by the
open items below.

## Immutable release contract

`scripts/release_contract.py` is the release entry gate. It requires one
version across the Cargo workspace, Android `versionName`, every Apple
`MARKETING_VERSION`, and the macOS archive name. Android `versionCode` and all
Apple `CURRENT_PROJECT_VERSION` values must also agree. A tag must be exactly
`v<version>`, and every external GitHub Action must use a lowercase 40-character
commit SHA.

The release workflow always checks out the event's immutable revision. It has
no independent `source_ref` override, because building one revision while the
attestation names another is not acceptable.

## Desktop bundle pipeline

A manual `release` workflow run is a non-publishing rehearsal. A `v*` tag runs
the same gates and then creates the GitHub Release.

1. Validate versions, build numbers, tag, and pinned actions.
2. Build CLI/Agent on Linux and Windows plus the standalone CLI on macOS arm64
   and x86_64.
3. In each platform build job, sign GitHub/Sigstore build provenance over the
   staged binaries before upload.
4. Download all binaries into one metadata job and generate reproducible
   CycloneDX 1.5 CLI and Agent SBOMs with pinned `cargo-cyclonedx 0.5.9`.
5. Reject missing, extra, empty, undersized, wrong-format, wrong-component, or
   wrong-version artifacts.
6. Write `release-manifest.json` with the exact repository revision and sorted
   artifact digests, then write `SHA256SUMS` over every binary, SBOM, and the
   manifest.
7. Sign SBOM attestations for the matching CLI and Agent binaries.
8. Upload exactly one verified bundle. The tag-only publish job downloads only
   that named bundle and never reconstructs checksums.

The attestation jobs receive only `contents: read`, `id-token: write`,
`attestations: write`, and `artifact-metadata: write`. Only the final tag-only
job receives `contents: write`.

## Artifact policy

| Artifact | Owner | Required signature/evidence | Installation/update policy | v0.3.0 status |
| --- | --- | --- | --- | --- |
| Linux/WSL CLI + Agent | desktop owner | SHA-256, source manifest, CycloneDX, GitHub provenance/SBOM attestations | per-user systemd install; paired atomic update | automated; real WSL evidence passed |
| Windows CLI + Agent | desktop owner | SHA-256, source manifest, CycloneDX, GitHub provenance/SBOM attestations; document any SmartScreen limitation | per-user Task Scheduler install; paired atomic update | automated CI; real Windows host evidence still required |
| macOS application + helper | Apple owner | Developer ID, hardened runtime, stable Team/access groups, notarization, staple, SHA-256 | signed application replacement retaining helper-owned state | path implemented; notarization evidence open |
| iOS/iPadOS application | Apple owner | App Store/TestFlight distribution signing and archive validation | TestFlight/App Store update retaining Engine schema 2 | signing evidence open |
| Android application | Android owner | production keystore signing, `apksigner` verification, version check, artifact digest/SBOM/provenance | package-manager update with stable application id/key | Gradle injection is fail-closed; key custody, tag workflow, and signed evidence remain open |
| Broker | service owner | pinned source revision, checksum/SBOM/provenance or locally recorded equivalent | preserve endpoint key across binary rollback/update | deployment works; release artifact integration open |
| Relay | service owner | pinned upstream iroh-relay version and verified package origin | preserve TLS/ACME configuration | operated separately from Envoix release |

An artifact listed as open is not converted into a release artifact by renaming
a Debug, simulator, unsigned, or ad-hoc build. Development evidence must keep
that label.

## macOS Developer ID path

The repository's fail-closed path is:

```bash
export ENVOIX_MACOS_DEVELOPER_ID='Developer ID Application: <name> (6638TTB2SF)'
export ENVOIX_MACOS_NOTARY_PROFILE='<notarytool-keychain-profile>'
export ENVOIX_MACOS_RELEASE_DIR='<new-absolute-output-directory>'
scripts/apple-dev.sh macos-release
```

It archives a universal app, verifies nested helper identity and entitlements,
submits to notarytool, staples, validates with stapler and Gatekeeper, verifies
again, and creates `Envoix-0.3.0-macos-notarized.zip`. The output directory must
not already exist. A signed Debug helper build is not notarization evidence.

## Candidate verification

Download the single `envoix-release-0.3.0` workflow artifact into an empty
directory, then verify checksums and attestations:

```bash
sha256sum -c SHA256SUMS
gh attestation verify envoix-cli-linux-x86_64 --repo moranxuege/envoix
gh attestation verify envoix-agent-linux-x86_64 --repo moranxuege/envoix
```

Repeat attestation verification for every desktop binary. Inspect
`release-manifest.json` and require its revision to equal the intended commit.
Parse both SBOM JSON files and archive the current cargo-audit result and
RustSec database revision alongside the test evidence.

Platform app verification is additional, not replaced by GitHub attestations:

- macOS: `codesign --verify --deep --strict`, designated requirements, Team ID,
  entitlements, `stapler validate`, and `spctl --assess`;
- Android: `apksigner verify --verbose --print-certs` plus stable certificate
  digest and package/version inspection;
- iOS/iPadOS: archive/export validation, distribution profile, Team ID,
  entitlements, TestFlight install, and upgrade test;
- Windows: malware scan, clean-host install/lifecycle run, and any selected
  Authenticode/SmartScreen policy.

## Android production-key input

Android Release builds remain unsigned by default so a source checkout cannot
accidentally impersonate an official build. A production build supplies all
four process-only environment variables:

```bash
export ENVOIX_ANDROID_KEYSTORE_PATH='<absolute-keystore-path>'
export ENVOIX_ANDROID_KEYSTORE_PASSWORD='<store-password>'
export ENVOIX_ANDROID_KEY_ALIAS='<key-alias>'
export ENVOIX_ANDROID_KEY_PASSWORD='<key-password>'
android/gradlew -p android :app:bundleRelease \
  -Penvoix.requireProductionSigning=true --no-daemon
```

The build rejects a partial set, an invalid keystore path, or a required signed
build with no set. Passwords must come from a protected CI secret or an
operator's environment, never a Gradle property, command line, checked-in
file, workflow artifact, or log. This input boundary alone is not release
evidence: the tag workflow must still verify the resulting signing-certificate
digest, APK/AAB identity and version, provenance, and SBOM before publication.

## Tag checklist

Do not create `v0.3.0` until all items are true:

- V03-SEC-01 and V03-SEC-02 in the security model are closed with evidence;
- Rust, Android, Apple, Windows, Linux/WSL, and cross-device gates are green at
  the exact candidate revision;
- current dependency, license, and secret-scan reports have no unaccepted
  release-blocking finding;
- clean install, retained-state update, explicit legacy rejection, recovery,
  revoke, and uninstall data-policy tests pass;
- platform owners approve their signing identities and no credential exists in
  repository, workflow input, log, or artifact;
- release notes describe supported and unavailable features without promoting
  an unverified host;
- a manual release rehearsal from the same revision produced and verified the
  complete bundle.

After approval, create one annotated `v0.3.0` tag at the reviewed commit and
push only that tag. If any tag workflow gate fails, do not mutate or reuse the
tag: fix on a new commit and select a new version/tag according to the release
policy.

## Rollback and revocation

A bad application build is withdrawn and replaced by a higher version/build;
clients must never install different bytes under the same version. Preserve
compatible Engine state and received files. A breaking state issue follows the
explicit backup/reset/re-pair policy rather than a silent downgrade importer.

For a compromised signing key, token, workflow credential, or artifact:

1. stop publication and mark the affected release compromised;
2. revoke/rotate the credential at its authority;
3. retain forensic workflow, digest, and access evidence without copying the
   secret;
4. rebuild from a reviewed commit with new credentials and a new version;
5. publish the affected digest/certificate identifiers and user remediation.
