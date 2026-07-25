# Pairing authentication

Envoix authenticates the data connection before any transfer frame. QUIC
provides transport encryption, pairing authenticates the intended peer and
invitation context, and BLAKE3 verifies transferred content.

## InviteV2

InviteV2 uses the existing Ed25519 SPAKE2 implementation under the explicit
suite name `spake2-ed25519-sha256-hkdf-hmac`. The full-ticket, Room control, and
data authentication passwords are separately derived with HKDF-SHA256. The
ticket and Room Code are never sent as plaintext protocol fields.

For Room-Code bootstrap, the invitation joiner initiates a control SPAKE2 and
the creator responds. HMAC-SHA256 key confirmation binds the selected
bootstrap, room locator, creator/joiner roles, both nonces, and both SPAKE
messages. The resulting key seals the creator's JCS public context and endpoint
descriptor. A Room joiner authenticates the context before creating an output
or data endpoint.

Both bootstrap paths finish with data-plane SPAKE2 over the live QUIC
connection. The InviteV2 exporter call uses:

- label: `envoix-auth-invite-v2`;
- context: the framed invitation binding;
- output: 32 bytes from the TLS exporter.

The confirmation transcript includes:

- InviteV2 domain and protocol version;
- sender and receiver identities;
- sender and receiver nonces and SPAKE messages;
- `invite_id` and `context_commitment`;
- the selected bootstrap method;
- creator and joiner transfer roles;
- the authenticated control transcript hash, or explicit absence for a direct
  path;
- TLS exporter bytes;
- mutual Remember consent and contributions, when used;
- a role-specific confirmation label.

This binds possession of the invitation secret to one transfer direction,
invitation, control exchange, and QUIC channel. Reflection, cross-invite use,
role changes, method downgrade, and exporter substitution fail confirmation.

## Remember negotiation

Remember is an optional extension of the existing confirmation exchange, not a
third handshake. Each peer advertises consent. Only if both consent does each
send a fresh 256-bit contribution; both contributions and the invitation
binding are combined into the returned value. The authentication crate does
not persist it. Issue 58 owns user consent, secure storage, and rotation.

## Compatibility developer modes

Manual and mDNS developer modes may still use `Spake2SharedToken`. They are not
invitations and cannot be reached as a fallback from InviteV2. Their exporter
label remains `envoix-auth-spake2-v1` with context `pairing`.

## SPAKE2 caveat

The Rust `spake2` dependency used here is not independently audited. Treat this
backend as experimental despite the transcript, role, and exporter bindings.
