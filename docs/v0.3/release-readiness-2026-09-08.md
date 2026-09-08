# v0.3 release readiness — 2026-09-08

Status: active preparation record; not a release approval.

The only release destination is `moranxuege/envoix`. The stable `v0.3.0` tag remains blocked. A separately labeled experimental
preview distributes the unchanged verified rehearsal binaries.

## Candidate validation

The hybrid Wi-Fi Aware/IP migration fixture now supplies its bound loopback
backup address explicitly. It still requires both endpoints to have a live IP
backup while Wi-Fi Aware is selected, closes both custom transports, and
verifies migration and payload delivery on the same connection. It no longer
relies on asynchronous address advertisement to discover a local test socket.

Local `envoix-session --all-features` passed 77/77; the release/script contract
suite passed 75/75. General CI [34206035416](https://github.com/moranxuege/envoix/actions/runs/34206035416)
and Apple CI [34206035425](https://github.com/moranxuege/envoix/actions/runs/34206035425)
passed at `fec16855891ccf1f971b69924ad5b2eb9da007c1`; CI
[34207150156](https://github.com/moranxuege/envoix/actions/runs/34207150156)
passed at the subsequent release-preparation commit.

The full non-publishing release rehearsal
[34207922558](https://github.com/moranxuege/envoix/actions/runs/34207922558)
passed at `2cdce5f5a75c729a81477cc4476fed87195857b1`, including production-signed
Android, Windows GUI/CLI/Agent, Linux CLI/Agent/broker, both macOS CLI
architectures, manifests, SBOMs, and attestations. Independent verification of
the downloaded bundles passed all 18 checksums and 20 provenance/SBOM policy
checks with the exact source digest, signer workflow, repository, and denial
of self-hosted runners. This is CLI evidence for macOS, not app notarization.

## Signing and remaining release gates

| Platform | Prepared | Remaining |
| --- | --- | --- |
| Android | A new 4096-bit RSA production identity is retained outside Git with its password in macOS Keychain. APK/AAB production signing rejects incomplete configuration. | Signed APK/AAB and the full cloud rehearsal passed with the approved certificate. Confirm upgrade behavior from old test signatures on devices. |
| macOS | Developer ID identity and helper profile remain valid. Signing/notarization/stapling passed at `9b7e5fc7` on September 5. | Recover or recreate the notarytool Keychain credential profile, then rebuild/notarize the exact final candidate and complete clean-user Keychain/upgrade checks. Downloads contained valid provisioning profiles but no notarization credential configuration. |
| iOS/iPadOS | Apple Distribution identity and App Store profiles for the app and Share extension are installed and valid. | Archive/export the exact candidate and retain TestFlight distribution evidence plus physical-device matrix results. |
| Windows | GUI, CLI, Agent and lifecycle CI are available. | No publisher certificate or signing service is configured. The owner explicitly deferred Authenticode setup; retain this blocker, and separately test SmartScreen and foreground picker/send behavior. |
| Linux/WSL | CLI/Agent/broker release bundle, checksums, SBOM and provenance pipeline are implemented. | Full release rehearsal passed. Complete retained-state upgrade and cross-device coverage at the final candidate. |

Approved Android certificate SHA-256:
`31395b806e89f3892ff62b852455d6f7f8b57346917e40693a7724e52a7876b6`.
The private key and its password must never enter Git, release artifacts, or
logs. The owner explicitly approved upload to `moranxuege/envoix` Actions
secrets; all four signing secrets and the public certificate-digest variable
were configured. The local PKCS12 remains outside the repository under
`~/.config/envoix/signing/`, with its password in macOS Keychain. Local signed
APK/AAB verification passed for `dev.envoix.app`, version `0.3.0`, build `5`,
and both `arm64-v8a` and `x86_64`. The cloud build uses the pinned NDK 26.3;
the independent local build used the installed NDK 27.2.

The metadata job downloads only its five named desktop/broker build artifacts,
so a concurrently completed Android bundle cannot contaminate the strict desktop
manifest. Android signature verification uses the explicitly installed Build
Tools path instead of depending on `apksigner` being on `PATH`.
The broker builder now verifies the tools already contained in its digest-pinned
image instead of fetching expired Bullseye security indexes. It retains its
ELF interpreter and maximum-glibc compatibility gates.

## Public release page

The redesigned bilingual page is live at `https://envoix.cc`, served by
GitHub Pages from the personal repository's `dev:/docs`. Both apex and `www`
use DNS-only Cloudflare CNAMEs to `moranxuege.github.io`; apex flattening
resolves the Pages addresses. HTTPS was verified with normal certificate
validation, enforcement enabled, and `www` redirects to the apex. The
independent `relay.envoix.cc` DNS record was preserved.

The page uses a dark responsive layout, an illustrative replayable file
transfer, native-platform cards, and a collapsible migration guide. Desktop,
390px and 320px layouts were checked, horizontal decoration overflow fixed,
language switching and replay/guide interactions verified, and browser error
logs were empty. Reduced-motion preferences disable animation.

The page now links directly to five platform downloads in personal-repository
[preview-0.3.0-20260908](https://github.com/moranxuege/envoix/releases/tag/preview-0.3.0-20260908):
Android APK, a complete Windows GUI/CLI/Agent ZIP, Linux/WSL CLI/Agent ZIP,
and separate Apple Silicon/Intel macOS CLI ZIPs. Windows is explicitly
unsigned; macOS CLI is not a signed/notarized graphical app. iOS and the macOS
graphical app remain unavailable. Both languages use the owner-selected refined
Send Across logo in the header, footer, transfer illustration and favicon.

The ZIPs preserve the original attested binary bytes and include per-package
checksums, SBOMs, source manifests and usage instructions. ZIP containers have
separate download hashes and are not claimed to be CI-attested. Android APK/AAB
retain their original production signatures. The 12 release assets are
allowlisted packages and public metadata; no signing material is included.

## Local consolidation

The only local checkout is `/Users/moranxuege/NTU/Envoix`, on `dev`, with only
the personal `origin` remote. The pre-push hook rejects other destinations.
Three old workspaces were removed after ancestry checks and per-file backups;
40 non-generated local files were preserved and SHA-256 reverified under
`.local-retained/`. The migration manifest preserves the original paths and
hashes. Remote branches were not deleted.

After validation, regenerable Cargo/Android/Python caches were deleted,
reclaiming 18.52 GiB. The resulting checkout occupied about 946 MiB including
retained local files and signed/verified candidate artifacts. No private key
was placed in this checkout.

## Final release gate

V03-SEC-01 and V03-SEC-02 remain open. Do not create `v0.3.0` until the exact
candidate passes the release checklist in [release.md](release.md), including
platform signatures and the required real-device/upgrade evidence.

## Current-candidate recheck for preview distribution

- Runtime/build sources remain byte-identical between rehearsal source
  `2cdce5f5a75c729a81477cc4476fed87195857b1` and pre-website-update HEAD
  `bfe989719d9604c94e0ef967b6d4467e4478a47a`; intervening changes only touch docs.
- Rechecked all 18 original artifact checksums before packaging; ZIP contents
  and executable mode bits passed round-trip verification.
- `python3 scripts/release_contract.py --tag v0.3.0` passed: 0.3.0, build 5,
  41 pinned Action references. This validates metadata, not release approval.
- `scripts/cross-device-transfer-matrix.sh --validate` passed the 22-case,
  six-profile registry and runner syntax. No physical matrix execution is
  claimed: adb has no connected device; the paired iPad is unavailable.
- Current Agent protocol is **16** (`crates/envoix-client/src/product.rs`).
  September 3/4 host evidence used protocol 12 and small payloads, so it cannot
  close the current cross-platform, foreground picker, recovery or upgrade gate.
  The operations guide's stale protocol-14 instruction is corrected to 16.
- Fresh `cargo audit --json` passed with zero vulnerabilities and zero unsound
  warnings. RustSec database revision:
  `b266fb89baa88c73c6aaa53e0e87509c80bdf962` (2026-09-08).
  Warnings: `paste 1.0.15` unmaintained; `spin 0.10.0` and newly observed
  `der 0.8.0` yanked. The upstream der changelog attributes the yank to its
  minimal-versions CI check. Preview evidence keeps the tested lockfile;
  update/review der and rerun relevant validation before the stable release.

Conclusion: preview downloads are available with explicit limitations;
**v0.3.0 is not approved for stable release**. Remaining evidence is exact-
candidate macOS app notarization, iOS export/TestFlight, Windows publisher
signing (owner-deferred), and the protocol-16 physical/recovery/upgrade matrix.

Website verification: both languages passed at 320px/390px phone and 1440px
desktop viewports without horizontal overflow; all three logo images load.
All ten bilingual download links match uploaded assets. All twelve GitHub
asset SHA-256 digests match the local verified files.
