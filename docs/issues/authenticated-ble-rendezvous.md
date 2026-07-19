# Design and implement authenticated BLE rendezvous security module

GitHub issue: [#52](https://github.com/ECE4410J-NUUB/envoix/issues/52)
Related: #41

## Context

The current cross-platform BLE GATT vertical slice deliberately supports only
`BleRendezvousSecurity` mode `0` (`Insecure`/`None`). It lets a user tap a nearby
card, exchange the existing `envoix://pair/` invitation, and enter the existing
SPAKE2 and Direct/Relay transfer state machines. This closes the product flow,
but it provides no peer authentication, confidentiality, anti-relay, or durable
device identity.

SPAKE2 does not repair this bootstrap: its password is carried inside the same
unauthenticated BLE invitation. The UI and privacy-safe logs therefore label the
current flow experimental and `auth=none`.

This issue owns selection, design, implementation, and review of an authenticated
replacement. The current insecure carrier remains a proof of concept and is not
the proposed security design.

## Threat model

At minimum, address:

- passive BLE observation and invitation theft;
- active device-name or presence-key impersonation;
- man-in-the-middle substitution of the invitation or handshake material;
- real-time relay/wormhole attacks between distant devices;
- replay of advertisements, frames, challenges, or prior pairing transcripts;
- downgrade from an authenticated mode to mode `0`;
- stable-identifier tracking and correlation across discovery sessions;
- malicious fragmentation, oversized fields, malformed UTF-8, and resource
  exhaustion;
- compromise, migration, loss, revocation, and reset of long-term device keys;
- a previously trusted device or account becoming hostile.

Document which attacks are prevented, detected, or explicitly out of scope.
Physical proximity or RSSI alone must not be treated as identity proof.

## Design questions

Evaluate and record the trade-offs of, at minimum:

1. Per-install long-term device identity plus signed ephemeral key agreement.
2. First-use out-of-band confirmation using QR, NFC, or compared short code.
3. TOFU/pinned public keys with explicit reset, migration, and revocation UX.
4. Platform Bluetooth LE Secure Connections and its cross-platform/OOB limits.
5. Optional account/cloud attestation for same-account or organization devices.
6. Rotating, unlinkable BLE presence identifiers derived from an approved trust
   relationship.

The design must specify whether relay resistance is a requirement and, if so,
what additional proximity proof or user ceremony provides it. Do not claim that
authenticated key exchange alone prevents relaying.

## Module boundary

Keep discovery, UI orchestration, and transfer state machines independent of the
chosen scheme. Extend the existing security boundary so a mode can:

- advertise only the minimum mode/capability metadata;
- produce and verify authenticated rendezvous envelopes;
- bind the selected discovery presence, both ephemeral keys, roles, invitation,
  protocol version, and transcript;
- return structured authentication failures without leaking secrets;
- expose a user-verifiable identity/trust state to the UI; and
- reject unknown modes and downgrade attempts.

Mode `0` must remain visibly insecure and disabled by default in release builds
once an authenticated mode is available, or be removed through an explicit
migration decision.

## Acceptance criteria

- [ ] A reviewed protocol document defines identities, keys, messages, state
      transitions, trust-on-first-use/account/OOB ceremony, and attacker model.
- [ ] The selected discovery card is cryptographically bound to the accepted
      session and both transfer roles.
- [ ] Mutual authentication, forward secrecy, transcript binding, replay
      resistance, and downgrade resistance are demonstrated or any omission is
      explicitly justified.
- [ ] Long-term secrets use Android Keystore and Apple Keychain/Secure Enclave
      where applicable; migration, reset, backup, and revocation are specified.
- [ ] Presence identifiers rotate without exposing a stable public identifier to
      arbitrary observers.
- [ ] Android and Apple share versioned golden vectors and interoperability tests.
- [ ] Negative tests cover impersonation, substitution, replay, reordering,
      truncation, oversize input, unknown modes, downgrade, and corrupted state.
- [ ] Physical Android↔iPhone tests cover first use, recognized peers, rejected
      identity changes, recovery/reset, and both invitation directions.
- [ ] UI copy accurately distinguishes unverified, first-use, verified, changed,
      revoked, and failed states; logs contain no invitation, key, code, address,
      or stable identifier.
- [ ] A security review records residual risks, especially relay resistance and
      account/cloud dependencies, before release enablement.

## Out of scope

- Replacing SPAKE2, Direct/Relay transport selection, or the file-transfer state
  machine unless the chosen binding requires a narrowly documented change.
- Treating a fixed user-visible device code or editable device name as an
  authentication credential.
- Claiming the current mode-0 proof of concept is safe for production.
