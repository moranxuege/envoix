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
suite passed 75/75. Exact cloud evidence is recorded after completion.

## Signing and remaining release gates

| Platform | Prepared | Remaining |
| --- | --- | --- |
| Android | A new 4096-bit RSA production identity is retained outside Git with its password in macOS Keychain. APK/AAB production signing rejects incomplete configuration. | Verify the signed packages and immutable workflow rehearsal with the approved certificate; confirm upgrade behavior from old test signatures. |
| macOS | Developer ID identity and helper profile remain valid. Signing/notarization/stapling passed at `9b7e5fc7` on September 5. | Rebuild/notarize the exact final candidate and complete clean-user Keychain/upgrade checks. |
| iOS/iPadOS | Apple Distribution identity and App Store profiles for the app and Share extension are installed and valid. | Archive/export the exact candidate and retain TestFlight distribution evidence plus physical-device matrix results. |
| Windows | GUI, CLI, Agent and lifecycle CI are available. | No publisher certificate or signing service is configured. The owner explicitly deferred Authenticode setup; retain this blocker, and separately test SmartScreen and foreground picker/send behavior. |
| Linux/WSL | CLI/Agent/broker release bundle, checksums, SBOM and provenance pipeline are implemented. | Rehearse at the final candidate and complete retained-state upgrade and cross-device coverage. |

Approved Android certificate SHA-256:
`31395b806e89f3892ff62b852455d6f7f8b57346917e40693a7724e52a7876b6`.
The private key and its password must never enter Git, release artifacts, or
logs. GitHub Actions credential upload requires the owner's explicit approval.

The metadata job downloads only its five named desktop/broker build artifacts,
so a concurrently completed Android bundle cannot contaminate the strict desktop
manifest. Android signature verification uses the explicitly installed Build
Tools path instead of depending on `apksigner` being on `PATH`.

## Public release page

The bilingual page links only to the personal repository and labels v0.3 as
not yet published. It must not advertise unavailable binaries or treat previous
revision evidence as proof for the final candidate. Custom-domain DNS and HTTPS
must be verified before the domain is declared live.

## Final release gate

V03-SEC-01 and V03-SEC-02 remain open. Do not create `v0.3.0` until the exact
candidate passes the release checklist in [release.md](release.md), including
platform signatures and the required real-device/upgrade evidence.
