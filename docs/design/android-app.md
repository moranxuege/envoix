# Envoix for Android — design

Status: design (pre-implementation). The working prototype (commit `7328bb4`)
proved the hard part — the Rust core runs **in-process** via JNI. This doc is the
spec for the real app; the prototype's app layer is rebuilt against it.

## 1. Goals & non-goals

**Goals.** A phone app that sends/receives files through the same rendezvous
code flow as the CLI (`246810-cobalt-fox`), pairs with the CLI both directions,
survives backgrounding, saves received files where the user can reach them, and
exposes settings + readable/uploadable logs.

**Non-goals (for now).** LAN mDNS discovery UI, a persistent "always listening"
daemon, multi-file batches, iOS. The core supports resume and mDNS; the app
surfaces them later.

## 2. What we borrow (grounded in real apps)

Researched **Syncthing-Android** (native core + service), **LocalSend** (modern
transfer UX), and **magic-wormhole / Warp / croc** (code-phrase pairing).

| Concern | Reference decision | Envoix |
|---|---|---|
| Backgrounding | Syncthing: FGS + Doze exemption + notif. LocalSend: **none → can't bg, no resume** (cautionary) | **FGS `dataSync`** per transfer + progress notification + resume |
| Received files | LocalSend: **Downloads / SAF folder / gallery** via MediaStore, no broad perm | MediaStore → Downloads, tap-to-open |
| Settings ↔ config | Syncthing: **core config = source of truth** + app prefs + router | DataStore → render `config.toml` → pass to JNI |
| Pairing | wormhole: number+words, **tab-complete**, QR, auto-copy | wordlist autocomplete + QR + copy + deep link |
| Direct vs relay | Warp **shows it** ("P2P — typically faster") | **Direct/Relay badge** (our thesis) |
| Notifications | LocalSend: **none** (gap) | progress + complete/failed |
| Logging | Syncthing: in-app viewer + share + rotate; **no redaction** | viewer + rotate + share + **redact the code/token** + upload |
| Native core | Syncthing: Go subprocess (works). Ours: Rust needs the JVM context | **JNI in-process** (validated) |

## 3. Architecture

```
Compose UI  ──observe──▶  TransferRepository (state, history, config)
                                 │  start/cancel
                          TransferService  ── foreground service; owns transfers,
                                 │              holds the network, posts notification
                          Native (JNI)  ── ndk_context init + api::Client per transfer
```

- **`TransferService`** owns active transfers (not the ViewModel) → survives
  backgrounding, holds a wake/network hold only while transfers run, posts the
  progress notification.
- **`TransferRepository`** — single source of truth the UI observes: active
  transfers, history (Room), config. UI is stateless-ish.
- **`Native`** — the JNI bridge (already built): `initContext`, `initLogging`,
  `runTransfer(callback)`, plus a new **`cancel(id)`** into `transfer.cancel()`.

## 4. Screens (inventory — every element has an action; no dead buttons)

| Screen | Contents | Notes |
|---|---|---|
| **Home** | header ("Envoix"), active-count pill, **Logs** button; transfer cards (title · mono addr · **Direct/Relay badge** · progress · MB/s · ETA · size · cancel/dismiss); "New transfer" FAB | the demo's card list |
| **New transfer** (sheet) | Send / Receive; **code field with word-autocomplete** + "Scan QR"; (send) pick file; (receive-as-host later) show our code as text + QR + copy | wormhole entry UX |
| **Settings** | Broker · Relay · Identity (persistent/ephemeral) · **"Don't use VPN/Tailscale" toggle** (→ candidate deny CIDRs) + advanced CIDR list · default save location · Log level + verbose(iroh) · Upload logs (consent) · Theme | *new — demo had none* |
| **Logs** | the viewer (built) + rotation + **Copy / Share / Upload** + Clear | |
| **History** *(P4)* | past transfers; re-send / re-receive | Room-backed |

The demo's **Mobile/CLI/Desktop tabs and pause icons are intentionally dropped** —
tabs were a marketing device; pause needs core support we don't have. Nothing on
screen is decorative.

## 5. Backgrounding (the P1 fix)

- **Foreground service, type `dataSync`** (bounded transfers; Syncthing needs
  `specialUse` only because it runs forever). Ongoing notification with progress
  + **Cancel** action.
- **`POST_NOTIFICATIONS`** runtime permission (Android 13+) — the one new
  permission, only for the transfer notification.
- Transfer runs in the service scope, decoupled from the Activity → survives
  home/screen-off/app-switch. This is the bug you hit.
- **Resume** on process death: persist the transfer request; on relaunch offer to
  continue (the protocol has receiver-side resume — LocalSend's gap, our edge).
- Optional Doze/battery-optimization exemption prompt for long transfers; request
  only if we observe kills.

## 6. Settings ↔ `config.toml`

Reuse path (Syncthing's "core config is truth", adapted): typed settings live in
**DataStore** (observable UI), and on change we **render a `config.toml`** to the
app's files dir and pass its path to the JNI (`Client::from_runtime_sources`) —
so the app and CLI share one config format, no second parser, no drift.

Fields → config: broker, relay, identity, `[candidates]` allow/deny (the VPN
toggle writes `deny = ["100.64.0.0/10","fd7a:115c:a1e0::/48"]`), path policy.
App-only toggles (theme, save location, log level, telemetry) stay in DataStore.

## 7. Logging + upload

- **In-app viewer** (built) + **rotation**: `logs/envoix.N.log`, size-capped, keep
  last N; survives kill (needed for crash reports).
- **Redaction (required, unlike Syncthing):** the room code's word-part *is* the
  SPAKE2 password — audit the app + JNI so it never reaches a log line; redact
  tokens. Only the numeric room id may appear.
- **Level** from Settings (default `envoix=debug,warn`; a "verbose (iroh)" toggle
  folds in `iroh=debug`).
- **Upload → a small log-sink on the VPS** (co-located with the rdz):
  - New tiny Rust service (`apps/envoix-logsink`, axum) exposing `POST /logs`
    behind Cloudflare/TLS, guarded by a shared token; writes one rotated file per
    upload with device + app-version metadata. No auth beyond the token; logs are
    redacted before send.
  - App triggers: **on demand** ("Upload logs" in Settings/Logs) and **on crash**
    (uncaught handler → `crash-latest.log`; next launch detects it and asks
    consent to upload). Consent-gated always.

## 8. Received files

MediaStore, not broad storage (LocalSend's approach, avoids Syncthing's
`MANAGE_EXTERNAL_STORAGE` store-review flag):
- Default → **Downloads** (`MediaStore.Downloads`); media optionally to the
  gallery; or a user-picked **SAF folder**. No storage permission needed.
- Completed cards get **tap-to-open** (FileProvider `ACTION_VIEW`) and share.
- Also register `SEND` / `SEND_MULTIPLE` so **Photos/Files → Share → Envoix**
  pre-loads a file to send (LocalSend pattern).

## 9. Pairing UX

Our code *is* wormhole's shape (`246810-cobalt-fox` = channel-number + words):
- **Receiver entry:** number field + **word autocomplete constrained to the
  wordlist** (can't type an invalid word — wormhole's proven error-reducer), or
  **Scan QR**.
- **Advertiser side:** show the code as **text + QR + auto-copy**, plus an
  `envoix://…` deep link for phone-to-phone.
- **Error copy** must distinguish PAKE realities (Warp): *waiting for peer* vs
  *code expired* vs *pairing failed → regenerate* (single-use codes). This is
  exactly the confusion seen in CLI testing.

## 10. Phased plan

- **P0 — de-risk** *(patch already reverted)*: native `cancel`, received-files →
  Downloads/MediaStore.
- **P1 — backgrounding**: `TransferService` + notification + resume. *Fixes the
  real-phone timeout.*
- **P2 — settings → `config.toml`**: DataStore + render + Settings screen.
- **P3 — logging**: rotation + **redaction** + `envoix-logsink` on the VPS +
  upload/crash-consent.
- **P4 — polish**: pairing (wordlist autocomplete + QR + deep link), share-in,
  history, direct/relay badge refinements.

Each phase is a reviewable step; we build one at a time.

## 11. Process lifecycle & recovery policy (decided 2026-07-11)

Recovery is **user-open driven, by decision** — not an oversight of where
`restoreAll()` happens to be called. Triggers are exactly: app open
(`MainActivity.onCreate`) and explicit user actions (Resume/Reverify). There
is no background daemon and no sticky service.

Why this is the correct boundary for this product, per recovery case:

- **Resuming killed transfers**: resume is two-sided — it re-pairs through
  the broker, so it succeeds only if the peer is also alive and rejoining.
  When the OS killed the app, the connection (and usually the peer's
  attention) died with it; a background resume parks in an empty room and
  burns an attempt against nobody. Transfers here are attended, phone↔phone:
  the humans returning IS the recovery trigger.
- **Sender-side receipt polling (Unconfirmed)**: the mailbox is DESIGNED for
  arbitrary delay — the receiver posts once, durably; the sender verifies
  whenever it next runs. Background polling only moves the checkmark to a
  moment nobody is watching.
- **Platform headwind**: Android 14+ caps `dataSync` foreground services
  (~6h), throttles sticky restarts, and a persistent notification for a
  rarely-useful daemon is negative product value.

The ONE sanctioned future extension — not yet implemented: the receiver's
undischarged confirmation duty (`Completed` receive with
`proof_delivered = false`, i.e. the rdz was unreachable exactly at
completion) holds the SENDER's UX hostage until this app happens to reopen.
If background machinery is ever added, it is a single expedited WorkManager
one-shot with a network constraint, enqueued when a receipt POST fails:
idempotent (re-reads the record and re-posts), persisted by WorkManager
across process death, no notification, no daemon. Nothing else runs in the
background.
