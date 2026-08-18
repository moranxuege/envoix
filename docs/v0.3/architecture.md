# v0.3 target architecture

Status: accepted direction; implementation proceeds by milestone.

## 1. Context

The repository already has a capable authenticated transfer core, but its
product semantics are spread across Rust, Swift, and Kotlin. Frontends still
see legacy send/invite entry points and low-level session/transfer types. The
same Room lifecycle, remembered-device fallback, retry policy, outbox rules,
and activity projection are implemented more than once.

v0.3 keeps the working protocol and replaces this accidental application
architecture.

## 2. Architectural invariants

These rules are release gates, not suggestions.

1. A product rule has one owner.
2. Views and composables do not own networking, credentials, persistence, or
   retry policy.
3. Platform code owns operating-system effects, not product state transitions.
4. An application host communicates with the Engine through commands, events,
   snapshots, and explicit platform ports.
5. Network protocol types do not become UI state by re-export.
6. A Room ending cannot implicitly delete a Transfer or Relationship.
7. Received user files are outside build caches and migration cleanup.
8. Secret material is never stored in ordinary application state or emitted in
   diagnostics.
9. One process has one durable Engine owner. Multiple windows have independent
   presentation state, not independent databases or credential owners.
10. Platform limitations are modeled as capabilities and observable states;
    they are not hidden behind timeouts or platform-specific string errors.

## 3. Logical layers

```text
Native presentation
SwiftUI / Compose / WinUI / CLI
        |
        | intents and immutable UI state
        v
Platform host and adapters
files / clipboard / vault / discovery / background work / notifications
        |
        | commands, events, snapshots, and port results
        v
Envoix application Engine
Device / Relationship / Room / Transfer / Content / recovery
        |
        v
Authenticated transfer core
session / auth / pairing / transfer / protocol / rendezvous / iroh
        |
        v
Rendezvous and relay services
```

The first implementation step is logical modularization inside
`envoix-client`. A new crate is justified only if the resulting dependency
graph or build targets require one. v0.3 must not start by creating speculative
crates.

## 4. Domain model

### Device

A stable local description of an endpoint identity and its observed
capabilities. A display name is metadata and is not identity.

### Relationship

Durable trust established by a verified pairing transcript. It owns the peer
identity, credential generation, trust state, revocation state, and migration
metadata. It does not own current connectivity.

Revocation preserves Room and Transfer history but prevents any new Transfer
authorization or attempt from starting, including accept, start, resume, and
recovery. An already authenticated attempt may still record its verified
delivery proof, while reject, cancel, failure, and removal remain available for
safe settlement.

Remembered-generation fallback is bounded by shared Engine policy. A connector
or responder may try the next scheduled generation only after a pre-
authentication failure; success, authentication, or cancellation ends
fallback immediately on every host.

Expected states:

```text
unverified -> verifying -> trusted -> rotating -> trusted
                              |                    |
                              +-------> revoked <--+
```

### Room

A temporary authenticated rendezvous and connection context. A Room may be
created from a typed Room code, nearby discovery, or a low-level invitation
capability. It can authenticate a new Relationship or reconnect an existing
one.

The product state separates peer admission from successful authentication:

```text
connecting -> authenticating -> connected -> closed
```

Opening a replacement Room closes the previous Room in the same state
transition. It does not revoke the Relationship or remove Transfers attached
to the previous Room.

Room expiry means that the rendezvous context can no longer admit peers. It
does not invalidate a completed Relationship and does not define Transfer
lifetime.

### Transfer

A durable operation with stable identity, direction, participants, content
inventory, policy, progress, outcome, and recovery metadata. A Transfer may
survive process restart, connection loss, and Room replacement.

The Engine owns legal transitions. Frontends render the projected state and
submit user intent; they do not infer terminal state from strings or partial
files.

An authenticated incoming manifest creates an `offered` Transfer. It cannot
queue or start until the user accepts it. Rejection records a typed terminal
reason, so every frontend can present the same outcome without parsing prose.

Payload completion is not delivery. A Transfer moves through
`awaiting_delivery_proof` and becomes `delivered` only after the Engine has
verified the receiver's delivery proof. A structured retryable failure remains
`failed` until an explicit recovery command starts a new attempt; confirmed
byte progress survives that transition. Only terminal Transfers may be
explicitly removed from the product snapshot.

Session failures are projected once in `envoix-client` into a stable failure
code, phase, origin, retryability, recovery action, terminal outcome, and
session-retention disposition. Swift, Kotlin, CLI, and Agent adapters may
translate those typed values into their native binding types, but must not
maintain independent error-to-recovery or error-to-terminal-state tables or
parse diagnostic prose. Application contract v6 makes the fine-grained failure
codes canonical while preserving read compatibility for v1-v5 fixtures;
UniFFI API 15 carries the complete projection to Apple clients.

### Content

A typed description of material carried by a Transfer. v0.3 requires file and
directory content. It reserves a product boundary for later text and image
clipboard content without implementing a generic cross-device clipboard in
this release.

### Invite and send

`Invite` is a protocol-level, time-bounded capability that supplies enough
information to enter or locate a Room. It is not a durable product aggregate.

`send` is a UI/CLI verb. It resolves a target, creates a Transfer, attaches
Content, and asks the Engine to execute it. It is not a separate transport
model.

## 5. Application contract

The Engine boundary consists of three data flows.

### Commands

Commands express intent and return an acknowledgement or stable operation ID.
Representative commands include:

- create or join a Room;
- begin or verify pairing;
- create, accept, reject, pause, resume, cancel, or remove a Transfer;
- send Content to a trusted Device;
- rotate or revoke a Relationship;
- answer a platform action requested by the Engine.

### Events

Events are versioned, typed facts emitted by the Engine. They include Room,
Relationship, Transfer, capability, recovery, and user-action changes. A
single ordered event stream replaces independent progress callbacks, prose
errors, and platform-specific activity reconstruction.

### Snapshots

A snapshot is an immutable view of current application state used at startup,
after reconnecting a binding, or after an event gap. A frontend must be able to
rebuild its presentation from a snapshot plus subsequent events.

Commands and events must carry stable identifiers and typed errors. Bulk file
contents never cross this control boundary as JSON.

A live command is decided against one Engine snapshot and produces a typed
effect for an adapter to execute. The result of that work returns as ordered
events. Snapshot or event-log replay never executes effects.

## 6. Effects and platform ports

The Engine can request, but cannot implement, these operating-system effects:

- secure credential load/store/delete;
- source selection, staging, and destination publication;
- clipboard capture and publication;
- local discovery and native peer-to-peer transports;
- notifications and user-visible background execution;
- platform logging and diagnostics export;
- clock, randomness, and connectivity observation where tests need control.

Each port has an explicit capability report. Unsupported operations fail with
a typed unavailable/limited result rather than an arbitrary platform error.

## 7. Host topology

### Apple mobile

iPhone and iPad embed one Engine in the application process. Swift calls the
typed control surface through UniFFI and implements Apple ports. One app
process owns one Engine; each SwiftUI scene owns only presentation state.

The iPhone and iPad share a universal application and feature modules, but use
different root presentation shells:

- iPhone: compact navigation and user-initiated transfers;
- iPad: adaptive split navigation, dynamic window sizing, multiple scenes,
  drag/drop, keyboard, pointer, and context-menu behavior.

### macOS

The target topology is a signed application bundle containing a
background-capable per-user helper. The helper owns durable Engine state and
credentials; the SwiftUI app and CLI use an owner-only local control channel.
The exact helper packaging and Mac App Store policy require a milestone ADR,
but ad-hoc signing is not a supported v0.3 release mode.

### Android

Android embeds the Engine and exposes work through a Compose presentation and
OS-managed service/work APIs. Android Keystore, ContentResolver/MediaStore,
notifications, nearby discovery, and background scheduling remain Kotlin
adapters. Product state transitions do not remain in `TransferService` or
composables.

The default typed control binding is UniFFI after the application surface has
been reduced. Hand-written JNI remains only for proven platform or performance
boundaries and must not expose a parallel product state machine.

### Windows

Windows first receives a supported per-user Agent and CLI. The proposed native
GUI is a WinUI shell that talks to the Agent through an owner-only Named Pipe.
The GUI framework is confirmed only after the local control protocol is stable;
the temporary egui demo is not the foundation of the Windows product.

The Windows adapter derives its default local pipe name from the current user
SID. It creates the pipe with a protected DACL granting that SID alone, rejects
remote clients, claims the first pipe instance, and compares every connected
client process token SID with the owner before decoding a command. Native
Win32 calls are isolated to the security descriptor and token adapter because
Tokio exposes `SECURITY_ATTRIBUTES` as an unsafe raw-pointer boundary; no
protocol or Engine code uses `unsafe`.

### Linux and WSL

Linux/WSL runs a per-user Agent, normally through a systemd user service, with
the Rust CLI as its supported control surface. A Linux GUI is outside v0.3
unless a later decision adds it.

## 8. Local control protocol

Desktop GUI and CLI clients communicate with the Agent through a versioned
local protocol:

- Unix domain socket on macOS and Linux/WSL;
- Named Pipe on Windows;
- owner-only access plus peer identity validation;
- tagged commands, events, and errors with an explicit protocol version;
- bounded control messages;
- paths, durable handles, or stream IDs for content, never content bytes in a
  JSON object.

JSON Lines is acceptable for the initial control encoding because the traffic
is local and small. Its schema must be represented by Rust types and contract
fixtures. Encoding can change later without changing the Engine contract.

Agent protocol v4 introduced an explicit protocol version and bounded opaque
request ID on every command and response. Protocol v5 adds an Agent instance
ID, a monotonically increasing event sequence, and a bounded 1,024-event
in-memory log. A client starts from the secret-free Engine/status/Inbox
snapshot and its event cursor, then polls at most 256 subsequent events per
request. An Agent restart, future cursor, or retention gap returns the typed
`snapshot_required` response; clients never guess whether incremental state is
complete. Protocol v6 adds durable Transfer creation/list/get operations,
bounded local source paths, `TransferChanged` events, and a secret-free typed
diagnostic report. The Agent seals source content before atomically recording a
queued Transfer, so a successful creation response always names restartable
state. Requests are limited to 64 KiB and responses to 20 MiB. v3 through v5
requests receive `unsupported_protocol_version`; the Agent does not run a
legacy decoder.

## 9. Persistence and secret ownership

The Engine owns the versioned schema for non-secret product state:

- devices and relationship metadata;
- Rooms needed for recovery;
- Transfer records and outcomes;
- Inbox/Outbox metadata;
- capability and migration metadata.

The storage implementation is the bounded atomic-file Engine store selected by
[ADR 0001](adr/0001-engine-storage.md). Its single-writer constraint follows
the Engine ownership rule; strict validation, size bounds, last-known-good
recovery, and atomic activation are required parts of the store.

Engine schema v1 stores the immutable application snapshot, durable
Relationship routes and vault references, Inbox metadata, and migration
evidence. It stores neither payload bytes nor credential values. The owner
lock is held for the lifetime of the store, including migration.

The desktop Agent now projects pairing, generation rotation, revocation, and
Inbox updates into this schema. Its former ProductStore implementation is
compiled only as a v0.2 fixture writer; it is not a production runtime path.
The Unix control adapter sets the socket to owner-only mode and verifies each
accepted peer UID against the socket owner before decoding a command.

Secrets are referenced from product state and stored by a secure-vault port:

- Apple Keychain under stable signed access groups;
- Android Keystore-backed encryption;
- Windows user-scoped protected storage;
- an explicitly documented owner-only fallback where WSL lacks a system vault.

The Windows Agent persists only versioned DPAPI ciphertext scoped to the
current user. It supplies the credential reference as domain-separated
optional entropy, rejects unknown/plaintext formats, and always requests
non-interactive protection so credential access cannot trigger a prompt loop.
The protected envelope also carries a domain-separated integrity digest because
Microsoft's
[`CryptUnprotectData`](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptunprotectdata)
contract says callers should not rely on one particular DPAPI tamper result.

Presentation code and standalone CLI commands never read secrets. Vault access
occurs on Engine startup or first credential use, pairing, rotation, and
revocation. It is not triggered by rendering, progress updates, or reconnect
polling. Tests use an in-memory vault unless they explicitly test a platform
adapter.

## 10. Presentation architecture

Every native application uses one directional flow:

```text
View -> UI intent -> presenter -> Engine command
Engine event/snapshot -> presenter -> immutable UI state -> View
```

Shared UI assets are semantic rather than pixel-identical:

- design tokens and component states;
- product terminology and interaction rules;
- native localization catalogs;
- accessibility and input behavior requirements.

Strings are not moved into Rust. SwiftUI, Compose, and WinUI continue to use
their native resource and accessibility systems.

## 11. Dependency rules

- Platform applications may depend on the binding/control surface and
  platform adapters.
- The application layer may depend on session, transfer, protocol, and domain
  crates through explicit modules.
- Session and transfer crates may not depend on product presentation or OS
  storage.
- A binding may project the application contract; it may not independently
  decide retries, trust transitions, or terminal Transfer state.
- No application receives `pub use envoix_session::*` or equivalent wildcard
  access in the final v0.3 surface.

## 12. Open decisions

The following require focused ADRs at the milestone that first needs them:

1. exact macOS helper packaging and whether Mac App Store distribution is a
   future requirement;
2. final Windows GUI framework after Agent IPC validation;
3. whether Linux gains a graphical shell after v0.3;
4. the later cross-device clipboard consent and history policy.

None of these decisions blocks establishing the Engine boundary.
