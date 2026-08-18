# v0.3 engineering standard

Status: normative for v0.3 changes. Repository `AGENTS.md` remains applicable;
this document adds architecture-specific gates.

## 1. Change contract

Before implementation, every slice states:

- the behavior being preserved or changed;
- the owning layer before and after the change;
- supported hosts affected;
- compatibility and migration consequences;
- tests that demonstrate completion;
- rollback or recovery behavior for persisted state.

If ownership is unclear, stop and resolve it in the architecture document or a
short ADR before writing code.

## 2. Vertical slices

Prefer one end-to-end product transition over a horizontal rewrite. A normal
slice contains:

1. a shared model/reducer or application-contract change;
2. one binding/control projection;
3. one platform consumer migration;
4. characterization and new behavior tests;
5. removal of only the compatibility code made unreachable by the slice.

Do not mix formatting, unrelated cleanup, generated binding output, dependency
upgrades, and behavior changes in one commit.

## 3. Dependency ownership

### Core

Protocol, authentication, transfer, and session crates own wire and data-plane
behavior. They do not import product presentation, platform persistence, or OS
UI APIs.

### Application

`envoix-client` owns product identifiers, commands, events, snapshots,
reducers, recovery policy, capability projection, and the product-store
contract. It calls lower layers explicitly and does not export them wholesale.

### Bindings and control protocol

Bindings translate types, ownership, cancellation, and threading. They do not
contain fallback policy, trust transitions, or terminal Transfer decisions.

### Platforms

Swift, Kotlin, Windows, and Agent adapters own OS effects. Presenters own only
presentation state and localization. Views render state and emit UI intent.

## 4. Rust rules

- Prefer pure reducers and explicit effects for product state transitions.
- Use newtypes for stable identifiers and security-relevant values.
- Serialized enums are versioned and tagged; changing a tag is a compatibility
  change.
- Errors crossing an application boundary have a stable code, phase,
  retriability, and safe diagnostic detail.
- Do not match English error strings to make product decisions.
- Do not add wildcard public re-exports across architectural layers.
- Avoid `unsafe`. Any new `unsafe` block documents the invariant, why a safe
  API is insufficient, and the test or review that protects the invariant.
- Secrets use zeroizing containers where practical and never implement
  accidental `Debug` output.

## 5. Swift rules

- One concurrency adapter/actor owns each long-lived Engine handle.
- A SwiftUI `View` does not call FFI, Keychain, filesystem, or network APIs
  directly.
- Observable presentation stores expose immutable screen state and accept
  intents.
- Scene-local navigation/selection state is not stored in the global Engine.
- App-wide Engine and vault ownership is not created independently per iPad
  window.
- Platform-specific behavior is isolated behind protocols or target-specific
  adapters rather than scattered `#if` blocks.
- User-facing strings use native localization catalogs, not inline bilingual
  selection, on every migrated screen.

## 6. Kotlin rules

- Compose functions render immutable state and emit intents; they do not start
  transfers, parse binding JSON, or write persistence.
- A ViewModel/presenter collects the Engine event stream and projects UI state;
  it does not reimplement Engine transitions.
- Android services adapt lifecycle, notifications, foreground work, and OS
  handles. They do not become an alternate product store.
- Coroutines have explicit owner scopes and cancellation behavior at the
  binding boundary.
- User-facing strings use Android resources on every migrated screen.

## 7. Platform security rules

- Production diagnostics are HTTPS-only and fail closed.
- Logs and events never contain Room secrets, verification codes, credential
  material, unredacted tokens, or file contents.
- Paths and device labels are redacted or included only with explicit report
  consent.
- Apple distribution targets use stable team signing; ad-hoc Keychain storage
  is development-only and must not be a release fallback.
- Keychain/Keystore/Windows vault access is owned by the Engine host, not UI or
  CLI presentation code.
- Unit tests use fake vaults. Tests that touch a real platform vault are named,
  isolated, and never enumerate or modify unrelated user items.
- Local Agent sockets/pipes are owner-only and validate their peer.
- Received files, App Group data, Inbox data, and transfer staging are never
  included in build-cache cleanup paths.

## 8. Test strategy

| Layer | Required evidence |
| --- | --- |
| pure model/reducer | table-driven transitions, invalid transitions, duplicate/out-of-order events, property tests where useful |
| protocol/session/transfer | unit/integration tests, malformed and boundary inputs, delivery/resume invariants |
| application contract | command/event/snapshot reconstruction, serialization fixtures, cancellation and recovery |
| binding/control | type parity, version mismatch, lifetime, cancellation, event-gap recovery |
| platform adapter | hosted unit tests plus real API/instrumentation tests where simulation is insufficient |
| presentation | state rendering, interaction, accessibility, adaptive layout |
| persistence/migration | version fixtures, interruption, corruption, atomic activation, received-file preservation |
| cross-device | named source/target builds, immutable revisions, result evidence, clean-state requirements |

A flaky UI test is reported and fixed or quarantined with an owner and issue. A
silent retry is not evidence of a stable gate.

## 9. Build and cache discipline

All direct Cargo, Xcode, and Gradle builds use
`scripts/with-build-cache-guard.sh` as required by `AGENTS.md`. Prefer
`scripts/apple-dev.sh` and its stable cache roots for Apple work. Dedicated
temporary DerivedData is allowed only for milestone evidence and must use the
guard's marked cache path.

Run the smallest relevant test first, then the milestone gate. Do not run
multiple writers against a shared Apple/Cargo build cache.

## 10. Documentation and decisions

The v0.3 documents are maintained with the code. A decision needs an ADR when
it:

- changes an accepted architectural invariant;
- chooses a persistent storage or UI/runtime technology;
- changes a serialized or wire compatibility promise;
- changes credential ownership or threat model;
- adds a new long-lived process or trust boundary;
- removes a supported host or distribution form.

An ADR contains:

```text
Title
Status: proposed | accepted | superseded
Context
Decision
Alternatives considered
Consequences
Compatibility/security impact
Verification
```

Historical documents are marked superseded with a link; they are not silently
rewritten to pretend the old design never existed.

## 11. Commit and push policy

- One commit has one reviewable purpose.
- Tests or documentation that define a behavior land with or before its
  implementation.
- Generated files are isolated when their volume would obscure semantic code.
- Commit messages use an imperative summary and identify the subsystem when it
  improves clarity.
- A milestone branch is pushed after every verified, clean commit.
- No pull request is created unless the repository owner explicitly requests
  one.
- Never force-push shared milestone history without explicit approval.

## 12. Definition of ready

A slice is ready to implement when:

- its owner and layer are explicit;
- current behavior has a test or reproducible evidence;
- the desired transition and error behavior are defined;
- migration/security impact is understood;
- the affected host verification commands are known.

## 13. Definition of done

A slice is done when:

- required tests pass through the build guard;
- affected supported hosts compile or have documented deferred evidence;
- no new duplicate product policy exists in a binding or UI;
- compatibility and documentation are current;
- the diff contains no unrelated cleanup;
- the tree is clean after a focused commit and the commit is pushed;
- milestone evidence records the exact revision tested.
