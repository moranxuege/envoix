# ADR 0002: Host the macOS Engine in a signed per-user helper

Status: accepted

Date: 2026-08-20

## Context

Before the API 22 host preparation, the macOS application constructed an
in-memory `FfiApplicationEngine` inside the SwiftUI process, and its
remembered-credential compatibility path could use an owner-only file store.
Those were development bridges, not the v0.3 release architecture: quitting
the GUI removed the Engine owner, a CLI could not share that owner, and an
ad-hoc signature did not provide a stable Keychain identity across upgrades.

v0.3 requires one durable Engine owner, one writer for its state directory,
and one trusted host for credentials. The GUI and CLI must not open a second
Engine or read credentials directly. At the same time, ordinary CI must remain
able to compile and run isolated tests without access to a paid signing
certificate or a developer's Keychain.

This ADR chooses the host, identity, lifecycle, and release boundary. It does
not define a second application-control or vault API. The shared persistent
Engine and vault UniFFI binding remains the source of truth for that contract.

## Decision

### Host topology

The v0.3 direct-distribution application contains a background-only helper app
at `Contents/Library/LoginItems/EnvoixEngineHelper.app`, registered as a
per-user login item with `SMAppService`. `launchd` starts and supervises it in
the logged-in user's domain. The helper is not privileged and never runs as
root.

The helper is the only macOS process that:

- opens the durable Engine state directory and holds its lifetime owner lock;
- owns the long-lived Engine/control handle;
- implements the Apple secure-vault port and accesses product credentials;
- executes background Engine work after the SwiftUI application exits.

The SwiftUI application and the CLI are control clients. They obtain snapshots,
submit commands, and observe events through the helper. If the helper is
disabled, unavailable, or incompatible, clients report that state explicitly;
they do not fall back to a process-local Engine, the remembered-peer JSON
store, or file credentials.

This decision is specific to macOS. iPhone and iPad continue to embed one
process-owned Engine shared by all scenes.

### Stable signing and credential identity

The production identities are fixed as follows:

| Component | Identifier | Signing identity | Credential access |
| --- | --- | --- | --- |
| SwiftUI application | `com.envoix.app` | Team `6638TTB2SF` | No Engine credential group |
| Engine helper and launchd label | `com.envoix.app.engine-helper` | Team `6638TTB2SF` | Engine credential group |
| Engine Keychain access group | `6638TTB2SF.com.envoix.engine.credentials` | Entitlement source form: `$(AppIdentifierPrefix)com.envoix.engine.credentials` | Helper only |

The release gate verifies that `AppIdentifierPrefix` resolves to
`6638TTB2SF.` and that the installed helper's designated requirement contains
both its bundle identifier and Team ID. The helper is embedded and signed as
nested code by the same release pipeline as the main application. The GUI and
CLI receive only opaque credential references and secret-free status over the
control channel; granting them the helper's Keychain group would violate the
host boundary.

Keychain interaction, authorization, or identity failures fail closed. The
helper surfaces the shared typed interaction-required or unavailable result
once and suspends the affected operation. Rendering, status polling, reconnect,
and automatic retry must not repeat a Keychain request or create a prompt loop.
Distributable configurations never substitute `MacOSFileCredentialStore` when
Keychain access fails. File credential storage is allowed only when explicitly
injected by an isolated test or a clearly named development-only configuration.

### Owner-only control channel

The helper exposes the shared versioned Agent control protocol over one Unix
domain socket below an owner-only runtime directory. The helper creates the
directory with mode `0700`, the socket with mode `0600`, rejects symlinks or
paths owned by another user, and checks the peer effective UID on every
accepted connection before decoding a bounded request. A peer whose UID does
not match the helper's UID is rejected.

This boundary protects users from other operating-system accounts; it does not
claim to distinguish mutually hostile processes already running as the same
user. Protocol version and capability negotiation must reject an incompatible
GUI or CLI instead of starting a second owner. No credential bytes are valid
control-protocol payloads.

### Installation, upgrade, and removal

The lifecycle is per user and has no privileged installer step:

1. The signed application bundle contains the signed helper. On explicit
   enablement, the GUI registers that embedded login item with `SMAppService`.
   A pending System Settings approval is shown as service state, not bypassed
   by launching an unmanaged helper.
2. `launchd` starts at most one registered helper for the user. The helper
   acquires the durable Engine lock before accepting clients. Failure to
   acquire the lock is terminal for that instance.
3. An upgrade replaces the containing application at the same installation
   location and refreshes the registration when required. The old helper
   stops accepting work, checkpoints through the shared Engine API, closes
   clients, and exits; `launchd` then starts the newly signed embedded helper.
   GUI, helper, Engine binding, and control-protocol compatibility are checked
   before state-changing commands are accepted.
4. Normal uninstall first unregisters the login item and stops the helper so
   it can close the Engine and release its lock. It removes the registration,
   executable code, and stale socket only. Durable Engine state, Keychain
   items, remembered Relationship data, Outbox data, and received files are
   retained by default. A future explicit erase-data operation must be a
   separate, user-confirmed flow and is outside this phase.

Moving the application bundle to the Trash without running the supported
uninstall flow is not treated as authorization to erase data or credentials.
A later installation may repair or remove a stale registration after verifying
the same Team ID and bundle identifiers.

### Build and release paths

There are three distinct artifact classes:

- **CI compile/test only:** a machine without Apple certificates may disable
  code signing, or Xcode may apply an ad-hoc signature solely where a local
  build/test tool requires one. Tests inject an in-memory or isolated fake
  vault. Such artifacts must be labelled non-distributable and do not validate
  helper registration, designated requirements, or production Keychain access.
- **Signed development:** integration and clean-user Keychain tests use an
  Apple Development identity from Team `6638TTB2SF`, the production bundle
  identifiers, and the production access-group shape. These tests use
  namespaced disposable items and never enumerate, overwrite, or delete an
  existing user's Envoix credentials.
- **Release:** the main application, helper, and nested code are signed with a
  Developer ID Application identity from Team `6638TTB2SF`, use the hardened
  runtime and reviewed entitlements, and are archived together. The archive is
  notarized, the ticket is stapled, and signature, designated-requirement,
  Gatekeeper, and staple verification must all pass before publication.

Missing release signing, entitlement, or notarization inputs fail the release
job. An ad-hoc signature is never a v0.3 distribution or release fallback.
Mac App Store packaging is not part of the v0.3 path; its sandbox, login-item,
and review constraints require a separate ADR before that channel is added.

### API 24 host/control integration and deferred legacy migration

API 22 introduced the shared persistent Engine and vault UniFFI binding. API
23 adds the shared `FfiAgentHost` and `FfiAgentControlClient` boundary with the
`agent_host_control_v1` capability. Apple host wiring consumes those contracts
without a Swift-owned parallel protocol:

- opening one durable Engine at a caller-supplied state directory, including
  lifetime locking and current-store recovery;
- the versioned command, event, snapshot, and capability semantics used by
  embedded hosts;
- injection of the secure-vault port using opaque vault references and
  credential bytes confined to its dedicated callback, with typed
  interaction-required, authorization, cancellation, unavailable, and
  corruption outcomes;
- Engine-owned snapshot and command coverage for device lists, pairing,
  credential rotation, and revocation.
- one typed desktop host with explicit startup, readiness, terminal failure,
  and idempotent awaitable shutdown; the host owns the Engine, vault, socket,
  and control handles until shutdown completes;
- one typed control client that performs protocol compatibility checks inside
  the shared binding and never exposes Agent JSON to Swift. Mobile builds keep
  the same binding surface but return `UnsupportedPlatform` instead of
  starting a desktop host.

API 24 adds `agent_host_control_v2` and the Agent protocol v11 atomic
`join_pairing` request. Its invitation and one-time verification code are
bounded, redacted authentication inputs. The helper performs the Room join,
verification transcript, Engine commit, and Keychain write before returning a
secret-free device summary. A foreground macOS Room that discovers a remote
verification request closes its unverified GUI session and reconnects through
this request; the creator keeps the invitation available until expiry so this
ownership handoff does not require a credential export.

The macOS connection hub also projects helper-owned devices through the typed
`ListDevices` response. Its Send picker, pending Finder selection, and device
drop target submit validated local paths through `CreateTransfer`; the helper
seals the content before returning a non-secret transfer identifier. The GUI
does not open the Engine or copy a Relationship credential. Active transfer
progress remains a separate snapshot/event projection milestone.

API 24 still does not expose an Engine-store origin, recovery report, or
migration report through UniFFI. Apple acceptance tests therefore provide
external evidence for fresh state opens, current-schema reopen, owner
exclusion, awaited shutdown, and legacy-state rejection; they must not
describe that evidence as a runtime report or as proof that Apple inspected or
imported v0.2 data. This phase does not read, migrate, or delete
`remembered-peers-v1.json`, legacy file credentials or Keychain items,
`RememberedRoomOutbox`, or received files.

Any legacy Apple migration is separate future work. It requires explicit
approval, a versioned import contract, and independent evidence covering the
source inventory, validated destination, rollback, and retention behavior.
Until then, legacy data remains retained but outside the API 24 Agent owner.

The Apple stage-B implementation embeds `EnvoixEngineHelper.app` at
`Contents/Library/LoginItems`, registers it only after an explicit Settings
action through `SMAppService`, and gives only that helper the production
Keychain access-group entitlement. The helper constructs `FfiAgentHost` with
`AppleApplicationVault`, validates typed readiness, monitors terminal state,
and awaits `shutdown()` before exiting. The GUI constructs only
`FfiAgentControlClient` for the stable owner-only Unix socket and performs no
Engine open or vault operation. Disabled, approval-pending, unavailable, and
incompatible states fail closed without an in-memory or legacy-store fallback.

Debug and ordinary test builds deliberately omit the helper Keychain
entitlement and remain ad-hoc compile/test artifacts. The `macos-release`
pipeline requires a Team `6638TTB2SF` Developer ID Application identity and a
notarytool Keychain profile, then verifies the nested helper, Team IDs, bundle
IDs, designated requirements, helper-only access group, hardened signing,
notarization, staple, and Gatekeeper assessment. The pipeline existing in the
repository is not itself notarization evidence; only a successful run with the
release Keychain inputs can supply that evidence.

## Alternatives considered

### Keep the Engine in the SwiftUI application

This preserves the current shape but makes GUI lifetime equal Engine lifetime,
cannot give the CLI a single durable owner, and permits multiple GUI processes
or scenes to compete for storage. It is incompatible with the v0.3 ownership
rule.

### Let every client open the durable Engine directly

A filesystem lock could reject the losing clients, but the CLI and GUI would
still have to own vault access and retry service startup. That duplicates the
trusted boundary and turns normal client concurrency into ownership failure.

### Use file credentials in unsigned release builds

Owner-only modes are useful for isolated tests and constrained non-Apple hosts,
but they do not provide the stable signed Keychain identity required for macOS
upgrades. Silent fallback would also hide signing defects and could split one
Relationship across two credential stores.

### Ship through the Mac App Store in v0.3

Committing to App Sandbox and store review constraints now would change the
helper and IPC packaging before the persistent binding is available. v0.3 uses
the notarized Developer ID path and leaves a store build as a later, explicit
decision.

## Consequences

- The GUI can exit while one durable per-user Engine continues work, and the
  CLI shares that same owner.
- Credential access has one small, stable, signed boundary; the app, CLI, and
  presentation layers do not gain secret access.
- Compile-only CI remains certificate-independent, but it cannot claim release
  signing, login-item, or Keychain evidence.
- Helper lifecycle and upgrade compatibility require real-host tests in
  addition to hosted Swift tests.
- API 23 integration leaves legacy stores in place without treating them as
  migration input or as the target architecture.
