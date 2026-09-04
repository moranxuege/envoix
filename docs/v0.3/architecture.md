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
7. Received user files are outside build caches and lifecycle state cleanup.
8. Secret material is never stored in ordinary application state or emitted in
   diagnostics.
9. One process has one durable Engine owner. Multiple windows have independent
   presentation state, not independent databases or credential owners.
10. Platform limitations are modeled as capabilities and observable states;
    they are not hidden behind timeouts or platform-specific string errors.

## 3. Logical layers

```text
Native presentation
SwiftUI / Compose / Windows GUI / CLI
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
identity, credential generation, trust state, and revocation state. It does
not own current connectivity.

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
UniFFI API 25 carries the complete projection to Apple and Android clients;
application binding v1 projects application contract v6 as typed
Command/Event/Snapshot/Effect values without JSON orchestration.

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

Shell selection follows the scene's horizontal size class rather than the
hardware model. A narrow iPad window therefore uses the compact stack, while a
regular-width iPad window uses persistent split navigation. Page selection and
its return context are scene-local; durable transfers remain process-owned.

`AppleApplicationRuntime` is the process owner for Nearby discovery, Room
control, remembered reconnect, and the durable outbox. Each window contributes
an explicit lifecycle lease. Closing or backgrounding one window cannot stop a
platform effect still requested by another, and only one active window owns
global invitation and verification prompts at a time. System pairing is also a
process lease, so a second window cannot restart discovery while Apple pairing
has temporarily quiesced it.

### macOS

The target topology is a signed application bundle containing a
background-capable per-user helper. The helper owns durable Engine state and
credentials; the SwiftUI app and CLI use an owner-only local control channel.
Helper packaging and the Developer ID distribution policy are fixed by
[ADR-0002](adr/0002-macos-engine-helper.md); ad-hoc signing is not a supported
v0.3 release mode.

On macOS, the CLI's default control endpoint is the signed helper socket at
`~/Library/Application Support/com.envoix.app/agent-v1/agent.sock`; it does not
use the Linux/WSL `~/.local/state/envoix` default. Explicit
`--agent-endpoint`, `ENVOIX_AGENT_ENDPOINT`, and the compatibility
`ENVOIX_AGENT_SOCKET` override remain available for isolated tests. Helper
registration and lifecycle stay in the signed app's Settings surface rather
than the Linux/Windows managed-service subcommands.

### Android

Android embeds the Engine and exposes work through a Compose presentation and
OS-managed service/work APIs. Android Keystore, ContentResolver/MediaStore,
notifications, nearby discovery, and background scheduling remain Kotlin
adapters. Product state transitions do not remain in `TransferService` or
composables. The process opens one persistent Engine handle; the migrated
Relationship slice reads and mutates only that Engine state.

Compose feature screens receive immutable UI state and intent callbacks. For
example, the Connection Hub renders `DiscoveryUiState` and nearby presence
values; the Android host owns permission launchers, settings persistence, and
discovery commands. A Composable never retains a service or ViewModel merely
to invoke platform effects. The Connection Hub is the sole nearby-discovery
entry point; the retired standalone discovery and mode-first send/receive home
screens are not part of the v0.3 application topology.

The Room screen receives that same discovery snapshot and explicit nearby
offer callbacks. Destination inspection and SAF authorization stay in the
Activity adapter; the Composable receives only the resulting destination
projection and picker intent callbacks.

Transfer setup consumes one immutable `TransferSetupPreferences` projection.
Both active and remembered Rooms use the same projection, so the shared sheet
does not observe or persist application settings itself.

Source pickers emit URI intents through `TransferSourcePreparationIntents`.
The Activity-scoped coordinator owns provider inspection, private staging, and
Manifest v2 job mutations; the shared sheet renders only
`TransferDraftPreparationState` and never opens the job store or
`ContentResolver` directly.

The Settings screen renders a settings snapshot and
`SettingsDiagnosticsUiState`. The Activity applies persistence and launches
runtime permission requests, while `SettingsDiagnosticsViewModel` owns and
stops the Wi-Fi Aware diagnostic probe outside the Composable lifetime.
The unreachable legacy log screen is not a v0.3 navigation surface; active
Transfer diagnostics remain contextual and use the host-owned upload/copy
effects.

The default typed control binding is UniFFI after the application surface has
been reduced. Hand-written JNI remains only for proven platform or performance
boundaries and must not expose a parallel product state machine.

### Windows

Windows has a supported per-user Agent and CLI plus the `envoix-windows`
graphical shell. The shell is a Windows-native Rust executable using egui and
talks to the Agent through the same typed owner-only Named Pipe contract as the
CLI. It is not the retired v0.2 desktop demo: it never constructs an Engine,
opens the Engine store, or obtains a credential. Its worker projects Agent
snapshots and sends typed requests for pairing, device revocation, Transfer
creation, offer decisions, Inbox inspection, diagnostics, and lifecycle
recovery. Closing the shell leaves the Agent and transfer queue running.

The Windows adapter derives its default local pipe name from the current user
SID. It creates the pipe with a protected DACL granting that SID alone, rejects
remote clients, claims the first pipe instance, and compares every connected
client process token SID with the owner before decoding a command. Native
Win32 calls are isolated to the security descriptor and token adapter because
Tokio exposes `SECURITY_ATTRIBUTES` as an unsafe raw-pointer boundary; no
protocol or Engine code uses `unsafe`.

The CLI installs the paired Windows binaries under
`%LOCALAPPDATA%\Envoix\bin` and registers a per-user Task Scheduler definition
under a task name derived from the same owner SID. Its logon trigger uses
`InteractiveToken` and `LeastPrivilege`, so installation stores no password and
requires no administrator elevation. The task runs a single Agent instance,
has no execution time limit, and uses the schema's one-minute minimum restart
interval after failure. Start, stop, and restart wait for the prior executable
image to be released before continuing, avoiding a stop/start race without
parsing localized command output.

Windows update replaces each installed binary atomically while retaining task
settings, compatible Engine state, credentials, and Inbox. A breaking Engine
schema change requires confirmed state cleanup and re-pairing. Default
uninstall removes the task and binaries but retains data. Its separately
confirmed cleanup mode
removes only explicit Agent-owned state entries and settings; Inbox and unknown
files remain. When uninstall runs from the installed CLI itself, a bounded,
hidden system cleanup process removes that locked executable after it exits.
The graphical binary resolves the CLI and Agent only from its own application
directory when offering installation recovery. Both development names and the
architecture-suffixed release names are explicit allowlists; it does not search
an arbitrary working directory. The Windows CI job runs
[`windows-agent-lifecycle-test.ps1`](../../scripts/windows-agent-lifecycle-test.ps1)
against an isolated temporary product root. It refuses to replace an existing
Envoix task and covers install, stop, start, restart, update, both uninstall
policies, self-removal, and Inbox preservation.

### Linux and WSL

Linux/WSL runs a per-user Agent, normally through a systemd user service, with
the Rust CLI as its supported control surface. A Linux GUI is outside v0.3
unless a later decision adds it.

The CLI installs and updates the paired `envoix` and `envoix-agent` binaries in
place, preserving settings and compatible durable state across updates. A
breaking Engine schema change requires confirmed state cleanup and re-pairing.
Uninstall removes the user unit and binaries by default without deleting data.
Its separately
confirmed state-cleanup mode removes only explicit Agent-owned state entries;
received Inbox files are never part of lifecycle cleanup.

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
state. One scheduler per authenticated Relationship offers that Transfer over
the existing bidirectional Room, then runs the directional Manifest v2 data
plane only after the peer accepts. A Relationship sends one Transfer at a time;
progress is checkpointed every 4 MiB and at payload completion. Queued and
nonterminal in-flight Transfers are eligible again after process restart, while
paused and failed Transfers are never retried implicitly. Peer decline, busy,
expiry, and invalid-offer decisions remain typed rejection outcomes.
Protocol v7 adds bounded, secret-free summaries for incoming offers that exceed
the automatic receive limit or half of currently allocatable Inbox space. The
Agent keeps at most 64 such offers in memory and starts no payload transfer
until the owner approves one through the local control protocol. Approval and
rejection are single-use decisions; Room closure, Relationship revocation, or
Agent restart discards the pending summary without persisting its directional
invitation.
Protocol v8 introduced selected transfer paths in at most 256 transient Agent
records. Snapshots and event polling expose typed `lan`, `direct`, `relay`,
`wifi_aware`, or `other` values without retaining raw peer addresses or relay
URLs. Direct-address classification is diagnostic only: it never changes
Relationship authentication, authorization, or candidate selection. A path is
removed when its transfer settles and is never written to product state.
Protocol v9 binds diagnostics to Engine schema v2. Protocol v10 adds the
typed Apple Keychain credential-protection diagnostic. Protocol v11 adds
Agent-owned first-contact `join_pairing`: the request carries only bounded
ephemeral authentication inputs, the Agent performs verification and vault
commit atomically, and the response contains only a device summary. Requests are
limited to 64 KiB and responses to 20 MiB. v3 through v10 requests receive
`unsupported_protocol_version`; the Agent does not execute a legacy decoder.
Protocol v12 adds bounded Relationship route replacement for the test-cycle
migration path. Protocol v13 adds typed pause, resume, recovery, cancellation,
and terminal-history removal for Agent-owned Transfers. Active attempts are
stopped only after their authoritative state transition is persisted. Protocol
v14 adds a separate Agent-owned Inbox preference and bounded live Transfer
telemetry. Rate, ETA, phase, path, and content previews are ephemeral and never
enter Engine state; durable byte checkpoints and terminal outcomes remain the
restart authority.

## 9. Persistence and secret ownership

The Engine owns the versioned schema for non-secret product state:

- devices and relationship metadata;
- Rooms needed for recovery;
- Transfer records and outcomes;
- Inbox/Outbox metadata;
- capability metadata.

The storage implementation is the bounded atomic-file Engine store selected by
[ADR 0001](adr/0001-engine-storage.md). Its single-writer constraint follows
the Engine ownership rule; strict validation, size bounds, last-known-good
recovery, and atomic activation are required parts of the store.

Engine schema v2 stores the immutable application snapshot, durable
Relationship routes and vault references, and Inbox metadata. It stores
neither payload bytes, credential values, nor legacy migration evidence. The
owner lock is held for the lifetime of the store.

The desktop Agent now projects pairing, generation rotation, revocation, and
Inbox updates into this schema. The former ProductStore implementation and
importer are absent from the runtime. With no schema v2 state, recognized v0.2
or schema v1 state fails explicitly and requires a test-build reset and
re-pairing; received files remain outside that reset.
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
adapter. The Engine host injects the `SecureVaultPort`; the contract exchanges
only validated vault references and zeroizing, non-serializable secret values,
and represents required user interaction as a typed result.

The Apple/Android UniFFI boundary enforces the same ownership rule. Room and
application snapshots and general transfer observers never contain credential
bytes. The dedicated `FfiRememberedCredentialVault` session callback hands a
new or rotated opaque credential to its platform owner. The persistent Engine
uses `FfiApplicationVault` to store, load, and delete that material by a
bounded non-secret reference. Loading a credential for an authenticated native
operation is an explicit trusted host call; UI state receives only its opaque
process reference.

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

Strings remain inside each platform presentation target and never move into
the shared Engine or protocol crates. SwiftUI and Compose continue to use their
native resource and accessibility systems. The Windows shell owns its
presentation text and uses AccessKit plus an installed Windows CJK font
fallback; localization and accessibility verification remain release gates.

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
2. Windows installer, Authenticode, and SmartScreen policy for the GUI bundle;
3. whether Linux gains a graphical shell after v0.3;
4. the later cross-device clipboard consent and history policy.

None of these decisions blocks establishing the Engine boundary.
