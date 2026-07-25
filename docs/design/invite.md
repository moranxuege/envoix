# Directional InviteV2

Status: implemented by Issue 57A.

InviteV2 is the only invitation contract used by the Rust core, QR renderers,
clipboards, deep links, the CLI, Apple, and Android. Carriers pass the complete
payload string unchanged. The legacy `envoix:` and `envoix://pair/` formats are
unsupported and are never reinterpreted as Room Codes.

Manual shared-token and mDNS developer modes are separate compatibility
features. They are not invitations and cannot fall back from a failed InviteV2
attempt.

## Direction and lifetime

Every invitation contains explicit, complementary transfer roles:

- `creator_transfer_role`: `sender` or `receiver`;
- `joiner_transfer_role`: the other role.

Send and Receive flows supply their local role to the parser and reject a
contradiction before staging, output-directory mutation, Activity creation, or
network activity. A deep link may parse for routing and opens the encoded
joiner role.

Invitations expire five minutes after creation. `expires_at <= now` is expired.
A creator secret is available, then in progress, then consumed after one
successful transfer. A concurrent use or second successful use is replay.
Transport retry may reuse the same in-progress secret only before
authentication and before expiry.

## Complete payload

The wire form is:

```text
envoix://invite/v2/<unpadded-base64url>
```

The decoded value is canonical JSON (RFC 8785 JCS for the schema's integer and
ASCII-key subset). Encoded payloads are limited to 8 KiB and decoded JSON to
4 KiB. Parsers reject noncanonical JSON, duplicate or unknown fields, malformed
fixed-length values and URLs, duplicate relays or capabilities, unsupported
versions, and invalid role pairs.

The public context contains:

- a random 128-bit `invite_id`;
- the six-digit `room_id`, which is only a broker lookup locator;
- protocol and invitation versions;
- creator and joiner transfer roles;
- broker descriptor and relay URL list;
- expiry;
- capabilities;
- advertised bootstrap methods.

The complete invitation additionally presents a random 256-bit ticket:

```json
{
  "bootstrap_methods": [
    {
      "id": "full-ticket-v1",
      "pake": "spake2-ed25519-sha256-hkdf-hmac",
      "ticket_commitment": "..."
    },
    {
      "id": "room-code-v1",
      "pake": "spake2-ed25519-sha256-hkdf-hmac",
      "room_id": "123456"
    }
  ],
  "presented_credential": {
    "method": "full-ticket-v1",
    "ticket": "..."
  }
}
```

The typed Room Code is external to this JSON. A carrier selects exactly one
advertised method: scanned or pasted complete payloads select
`full-ticket-v1`; typed codes select `room-code-v1`. Authentication failure
never falls back to another method.

## Commitments and capabilities

`context_commitment` is SHA-256 over the JCS public context.
`ticket_commitment` is SHA-256 over the ticket. Changing the invite ID, roles,
broker, relays, versions, capabilities, expiry, or methods invalidates
authentication.

Capabilities have this shape:

```json
{
  "required": [],
  "optional": ["manifest-v1"]
}
```

Both arrays are unique and sorted by ASCII byte order. Names match
`[a-z0-9][a-z0-9-]{0,63}`. Unknown required capabilities fail before network
activity; unknown optional capabilities are ignored. The initial registry is:

- `manifest-v1`: implemented Manifest transfer-set support;
- `remembered-peer-v1`: reserved for Issue 58 and not emitted yet.

Directional binding, single-file transfer, context commitments, and
exporter-bound data authentication are base InviteV2 behavior, not
capabilities.

## Room Codes

A Room Code contains six uniformly sampled decimal digits plus eight uniformly
sampled lowercase Base36 characters:

```text
123456-k7m4-9v2d
```

The canonical display and wire form has both hyphens. Input accepts only that
form or `123456k7m49v2d`; ASCII uppercase is normalized. Whitespace, other
separators, Unicode, suffixes, and partial input fail closed.

The complete normalized code is the Room control-PAKE password input. Only the
six decimal digits reach the broker. The eight Base36 characters provide the
human-code secret boundary; broker-side online-abuse controls are tracked by
Issue 57B.

## Authentication flow

All SPAKE passwords are HKDF-SHA256 derived and domain separated. The current
SPAKE2 backend is Ed25519 and the advertised suite name is
`spake2-ed25519-sha256-hkdf-hmac`.

The invitation joiner is always the control-PAKE initiator and the creator is
the responder, independently of transfer direction or arrival order. The
broker matches only opposite invitation sides, complementary transfer roles,
and a carrier-selected method advertised by the creator.

Room-Code flow:

```text
Room-Code control SPAKE2
→ sealed public-context authentication
→ sealed endpoint-descriptor exchange
→ fresh data-auth password derivation
→ exporter-bound data SPAKE2
→ transfer frames
```

The complete-ticket flow derives its data-auth password from the ticket. It
still uses the broker to exchange an endpoint when no authenticated direct
endpoint is present.

The data authentication uses an InviteV2-specific TLS exporter label and the
framed invitation binding as exporter context. Its confirmation transcript
binds the invite ID, context commitment, selected bootstrap method, both
transfer roles, control transcript hash when a control path was used, both
SPAKE contributions and nonces, and TLS exporter bytes. No transfer frame is
accepted before confirmation.

The optional Remember hook extends the same confirmation exchange. Only after
mutual consent does each peer contribute a fresh 256-bit value; the combined
result is returned to the caller and is not persisted. Issue 58 owns the UI,
storage, and rotation policy.

## Secret handling

Tickets, complete Room Codes, commitments, and derived values use redacting
types. Durable transfer records contain opaque secret references rather than
raw invitation credentials. Frontend restore summaries, Activity projections,
platform extras, accessibility descriptions, and diagnostics do not expose
the complete payload. The payload is visible only at the intentional initial
QR/share presentation.

The private secret-store reference remains necessary for relaunch. Mobile
production backends must be Keychain/Keystore-backed, and desktop development
must use a restrictive private backend. Global durable broker consumption and
abuse counters remain Issue 57B.
