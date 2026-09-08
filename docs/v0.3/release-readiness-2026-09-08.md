# v0.3 release readiness — 2026-09-08

Status: active preparation record; not a release approval.

The only release destination is `moranxuege/envoix`. No v0.3.0 tag or public
binary release was created by this preparation.

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

The page links only to the personal repository and labels v0.3 as not yet
published. Unavailable binaries are not advertised as downloads.

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
