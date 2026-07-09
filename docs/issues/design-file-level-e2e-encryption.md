## Title

Design File-Level End-to-End Encryption for Transfer Payloads

## Problem

Envoix currently has several security building blocks:

- QUIC transport encryption;
- SPAKE2 shared-token pairing before file metadata is sent;
- BLAKE3 whole-file integrity verification after transfer.

However, this is not yet file-level end-to-end encryption.

The transfer payload is not encrypted by an application-level content key, and the encryption boundary for future features such as multi-file manifests, resume state, relay fallback, and trusted devices is not yet defined.

Before implementing encryption, Envoix needs a clear design for what is encrypted, how keys are derived, how encrypted chunks interact with resume, and what security claims the project is allowed to make.

## Goal

Produce a concrete design for file-level payload encryption.

The design should be detailed enough to guide a later implementation issue, but this issue does not implement encryption.

## Current Security Model

Current security properties:

- QUIC encrypts the transport channel.
- SPAKE2 authenticates the session using a shared token before transfer frames.
- The token is not sent over the network.
- BLAKE3 verifies the completed plaintext file.

Current limitations:

- file payloads are not encrypted by an application-level content key;
- file metadata encryption is not defined;
- encrypted resume state is not defined;
- chunk-level authentication is not defined;
- trusted-device based key agreement is not defined;
- current SPAKE2 backend is prototype security and must not be described as production-grade assurance.

## Design Questions

### 1. Key Source and Derivation

Decide how content encryption keys are derived.

Options to evaluate:

- derive a transfer content key from the existing SPAKE2-authenticated session;
- use a separate ephemeral key exchange after authentication;
- later use trusted-device identity keys for authenticated ECDH.

The design should define:

- root secret;
- key derivation function;
- domain separation labels;
- per-transfer key;
- per-chunk nonce strategy.

### 2. Encryption Granularity

Decide the encryption unit.

Likely v1 target:

```text
chunk plaintext -> AEAD encrypt -> encrypted chunk frame
```

Each chunk should have independent authentication so failed or tampered chunks can be detected locally.

The design should explain why whole-file encryption or stream-only encryption is not chosen for v1 if chunk encryption is selected.

### 3. Resume Compatibility

The current transfer model supports sequential resume using receiver-side partial files and sidecar state.

The encryption design must specify:

- whether resume state tracks plaintext offsets, ciphertext offsets, or chunk indexes;
- how chunk nonces are reproduced on retry;
- how the receiver validates already-written plaintext;
- whether the sender can resume without re-encrypting from the beginning;
- what data is stored in the sidecar.

### 4. Integrity Model

Decide how AEAD authentication and BLAKE3 whole-file verification interact.

Expected direction:

- AEAD authenticates each encrypted chunk;
- BLAKE3 continues to verify the completed plaintext file;
- future per-chunk hashes may be part of a manifest, but are out of scope for v1.

### 5. Metadata Encryption Boundary

Decide what remains plaintext in v1.

Candidate v1 boundary:

- encrypt file bytes;
- keep file name, file size, chunk size, and transfer id plaintext;
- document that metadata privacy is not provided yet.

The design should explicitly state whether metadata encryption is deferred to a future manifest design.

### 6. Protocol Versioning and Compatibility

The design must define how peers negotiate encryption support.

Questions:

- Is encryption mandatory once implemented, or negotiated?
- How does an old client fail when talking to a new encrypted client?
- What protocol version or feature flag is required?
- How is downgrade prevented?

### 7. Security Claims and Non-Claims

The design must state exactly what Envoix can claim after implementation.

Allowed target claim may be:

```text
File payloads are encrypted and authenticated at the application layer between paired peers.
```

Non-claims should include:

- not production-audited cryptography;
- not metadata privacy if metadata remains plaintext;
- not protection against compromised endpoints;
- not cloud account security;
- not automatic trust of unknown devices.

## Out of Scope

- Implementing encryption
- Multi-file manifest encryption
- Folder metadata privacy
- Offline package encryption
- Trusted-device identity based E2E implementation
- Multi-recipient encryption
- Key backup or recovery
- Security audit
- Production-grade security claims

## Acceptance Criteria

- A design document describes key derivation, encryption granularity, nonce strategy, and protocol negotiation.
- The design explains how encryption works with resume.
- The design explains how AEAD chunk authentication and BLAKE3 whole-file verification coexist.
- The design explicitly states what metadata remains plaintext in v1.
- The design defines compatibility behavior with older clients.
- The design lists allowed security claims and non-claims.
- Follow-up implementation issues can be created from the design without reopening the basic threat model.

## Follow-up Issues

- Implement single-file payload encryption.
- Extend encrypted transfer design to multi-file manifests.
- Add metadata encryption after manifest support exists.
- Add trusted-device identity based key agreement.
- Add encryption-focused test vectors and interop tests.
