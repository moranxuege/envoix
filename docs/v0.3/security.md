# Envoix v0.3 as-built security model

Status: active release review; open release blockers are listed below.

Reviewed baseline: 2026-09-04

This document describes the implementation on `refactor/v0.3`. It replaces the
2026-07-19 security-review work draft as the source for current claims. It does
not claim that an untagged development build is ready for public distribution.

## Assets and trust boundaries

The protected assets are file contents, invitation and Relationship secrets,
device trust decisions, durable Engine state, delivery evidence, diagnostic
reports, signing credentials, and release artifacts.

Envoix crosses five trust boundaries:

1. presentation to the application Engine;
2. application Engine to a platform vault and filesystem adapter;
3. desktop UI or CLI to the per-user Agent;
4. endpoint to the broker and relay;
5. source repository to CI, signing services, and distribution artifacts.

The broker and relay are not trusted with file contents or authentication
secrets. They do observe connection timing, endpoint identifiers, Room
locators, traffic volume, and direct addresses visible to their transports.
Display names are untrusted metadata and never device identity.

## Attacker model

The release considers:

- a remote client that guesses Room locators, floods joins, sends malformed
  frames, or exhausts bounded server state;
- a nearby client that advertises forged BLE, mDNS, NFC, or Wi-Fi Aware data;
- an authenticated but malicious peer that offers hostile paths, oversized
  manifests, excessive content, or corrupted blocks;
- a different local OS user attempting to control a desktop Agent;
- a server, relay, network, log, or backup operator observing metadata;
- a dependency, CI action, signing credential, or release artifact being
  replaced;
- accidental corruption, crash interruption, lost permission, and stale v0.2
  state.

A process already running as the same OS user as Envoix, a compromised kernel,
and malicious firmware or storage hardware are outside the application
boundary. Envoix minimizes same-user exposure but cannot treat that account as
an adversarial sandbox.

## Implemented controls

### Pairing and remembered Relationships

- Complete InviteV2 payloads carry a random 256-bit ticket. Human Room Codes
  use six public decimal locator digits plus eight uniformly sampled lowercase
  Base36 secret characters (about 41.4 hidden bits).
- Invitations have canonical parsing, role and capability validation, a
  five-minute expiry, context commitments, and explicit replay/legacy errors.
- Control and data authentication use separate HKDF-derived SPAKE2 inputs.
  HMAC confirmation binds roles, methods, nonces, transcript, invitation
  context, and the live QUIC TLS exporter.
- Remember is created only after mutual authenticated consent. Each generation
  derives separate locator, control, data, and presence values; one previous
  generation is retained only for bounded crash recovery.
- BLE carries only a secret-free verification locator. NFC, QR, clipboard, and
  deep-link carriers are untrusted transports for the strictly parsed
  invitation, not independent authenticators.

The detailed transcript is specified in [Pairing authentication](../auth.md).

### Broker abuse and resource control

- A creator is the only parked side. A joiner without a compatible creator is
  rejected instead of consuming a waiter slot.
- Short Rooms have a six-attempt cumulative budget, a token bucket, a fixed
  lifetime, and a tombstone. EndpointId, observed IP, subnet, per-Room, and
  global concurrency limits are independent and bounded.
- Handshake, Join, relay, idle-frame, slow-frame, and close deadlines are
  explicit. Frames are bounded before decoding and retry guidance is capped.
- Relay addresses are not mistaken for source IPs. Limit state has an entry cap
  and idle TTL. Metrics use fixed counters rather than attacker-controlled
  labels.

The tunable values and semantics are in
[Room abuse protection](../room-abuse-protection.md).

### Transfer and filesystem safety

- Manifest v2 bounds encoded size, roots, entries, path depth, component bytes,
  block size, and control frames. Arithmetic is checked and malformed input
  receives typed failure.
- Incoming offers above the automatic threshold or half of allocatable Inbox
  space require explicit approval. Payload processing does not begin before the
  authenticated offer is accepted.
- Relative paths reject traversal and unsafe components. Local publication uses
  owned staging, symlink checks, platform no-replace operations, keep-both name
  allocation, durable checkpoints, and verified BLAKE3 content digests.
- Payload completion alone is not delivery. The sender reports Delivered only
  after verifying the receiver's persistent delivery proof.
- Lifecycle cleanup never includes the Inbox. Corrupt or unsupported Engine
  state fails explicitly rather than being silently replaced.

### Secret ownership and local control

- Ordinary Engine JSON stores only bounded vault references. Secret buffers are
  non-serializable, zeroized where practical, and redact their debug output.
- The macOS helper owns the stable Keychain access group; the main app does not.
  Android uses a non-exportable Keystore wrapping key and Windows uses
  current-user DPAPI. WSL uses a documented owner-only file fallback.
- Unix Agent sockets are owner-only and verify the peer UID. Windows Named
  Pipes use an owner-SID DACL, reject remote clients, and verify each client
  process SID before command decoding.
- Local commands and responses are versioned and bounded. Unsupported Agent
  protocol versions and Engine schema versions fail closed.

### Diagnostics and privacy

- Production defaults contain no diagnostic server. Native remote upload is
  available only in Debug/developer mode, requires a process-injected bearer
  token, and starts only from an explicit user action.
- Apple and Android accept only HTTPS diagnostic URLs and never fall back to
  HTTP. The server refuses a non-loopback diagnostic listener without its own
  TLS certificate and key; loopback HTTP is for local development or a TLS
  reverse proxy only.
- Upload and report retrieval use separate bearer policies and fail closed when
  unset. Socket-source upload and view token buckets are independent, emit 429
  with `Retry-After`, and keep only bounded, expiring limiter state.
- Bodies, rooms, sides, room logs, per-room clients, and total in-memory bytes
  are bounded. Default report retention is one hour after last activity and is
  memory-only.
- Core secret types, source and destination paths, invitations, verification
  codes, credentials, and transfer capabilities have redaction tests.
  Diagnostic reports can still contain filenames, device labels, timing, and
  failure context; the explicit upload action is the consent boundary.
- `--unsafe-open-log-view` is a development-only escape hatch. A public
  deployment must not use it. Forwarding headers are not trusted; when a
  loopback reverse proxy is used, that proxy must apply client-source limits.

### Release supply chain

- Rust, Android, Apple, cargo-audit, cargo-cyclonedx, XcodeGen, and CI action
  versions are pinned. Every external GitHub Action reference is a full commit
  SHA and is checked by the release contract.
- CI rejects RustSec vulnerabilities and unsound advisories. The accepted
  warning paths and exit criteria are in
  [Dependency security](dependency-security.md).
- The desktop packaging workflow produces sorted SHA-256 checksums, a source
  revision manifest, CycloneDX 1.5 SBOMs for CLI and Agent, build provenance
  from each platform build job, and signed SBOM attestations.
- Android production signing is process-injected and all-or-none. Tag builds
  fail without the protected keystore inputs and independently pinned public
  certificate digest; APK/AAB provenance and Android plus embedded-Rust SBOM
  attestations are created only after identity and package validation.
- Bundle validation rejects missing, extra, empty, implausibly small, or
  platform-format-mismatched binaries and verifies each SBOM component and
  version before anything can enter the publishing job.

## Residual risk and release blockers

| ID | Severity | Status | Required closure |
| --- | --- | --- | --- |
| V03-SEC-01 | high for public distribution | open | Run the Android tag path with an approved production key and retain evidence; complete Developer ID/notarization for the macOS app; define the iOS/TestFlight signing evidence. Never publish a debug, ad-hoc, test-key, or unlabeled unsigned app as v0.3.0. |
| V03-SEC-02 | high for support claims | open | Complete the physical iPhone, iPad, Android, Windows, macOS, and Linux/WSL reference matrix, including revoke, reconnect, resume, and legacy-state rejection. |
| V03-SEC-03 | medium | accepted for RC, recheck at tag | The Rust `spake2` backend is not independently audited. InviteV2 adds transcript and exporter binding but does not replace a cryptographic audit. Keep the experimental statement in user-facing technical documentation. |
| V03-SEC-04 | low/medium | accepted with restriction | Human Room Codes have about 41.4 hidden bits, not the 256 bits of a complete InviteV2 ticket. The five-minute lifetime, six-attempt budget, tombstone, and source limits are mandatory; prefer QR/NFC/full invitation for unattended or high-risk use. |
| V03-SEC-05 | low/medium | accepted for WSL | The WSL fallback vault is protected by owner-only filesystem permissions, not hardware-backed storage. A multi-user or weakly administered WSL host needs an external secret store. |
| V03-SEC-06 | low | accepted, recheck at tag | `paste 1.0.15` is unmaintained and `spin 0.10.0` is yanked through upstream iroh dependencies. Neither has a current RustSec vulnerability in the reviewed lockfile. |
| V03-SEC-07 | availability/privacy | operational restriction | A relay or broker can deny service and observe metadata. End-to-end authentication protects content and peer admission, not availability or traffic-analysis resistance. |

No open high-severity item in this table is accepted for the v0.3.0 public tag.
The release process must retain evidence that V03-SEC-01 and V03-SEC-02 are
closed, and refresh dependency evidence immediately before tagging.

## Reporting and review maintenance

Security reports should identify the affected version, platform, trust
boundary, reproducible input, and whether file contents or credential material
were exposed. Do not attach a live invitation, Room Code, bearer token,
credential blob, signing file, or private diagnostic report to a public issue.

Any change to invitation grammar, authentication transcripts, vault ownership,
Agent peer validation, destination publication, diagnostic transport, server
limits, signing, or release provenance requires updating this document and a
negative test at the owning boundary.
