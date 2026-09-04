# Envoix v0.3 operations and recovery

Status: active runbook

This runbook covers the current `envoix.cc` deployment and supported desktop
Agents. It does not place credentials, private keys, or bearer tokens in the
repository.

## Service inventory

| Component | Current route | Durable state | Owner |
| --- | --- | --- | --- |
| Broker | `6de87065…b92@47.237.15.48:8445` over UDP | 32-byte iroh endpoint secret key | server operator |
| Relay | `https://relay.envoix.cc:8444` | TLS/relay configuration and ACME state | server operator |
| Diagnostic collection | disabled by client default; optional HTTPS service | memory-only reports plus token files | server operator |
| Linux/WSL Agent | per-user systemd service | Engine schema v2, vault fallback, settings, Inbox | local user |
| Windows Agent | per-user scheduled task | Engine schema v2, DPAPI vault, settings, Inbox | local user |
| macOS helper | signed per-user app helper | Engine schema v2, Keychain references, Inbox | local user |

The full broker address and relay URL have one compiled source in
`crates/envoix-client/src/configuration.rs`. Apple and Android consume the FFI
projection and must not duplicate these constants.

## Routine health checks

On the service host, verify process state, recent structured errors, listening
ports, certificate validity, and free disk before changing anything. The broker
uses UDP 8445; TCP-only checks do not prove it is reachable. The relay uses its
configured HTTPS/QUIC ports and can carry full payload volume.

The authoritative broker test is a real creator/joiner pairing from outside
the server network. Keep the server log open and require a typed match rather
than interpreting a UDP scan as success. Follow
[Rendezvous deployment](../rendezvous-deployment.md) for commands and current
limits.

For a desktop Agent:

```bash
envoix agent status
envoix agent diagnostics
envoix devices list
envoix transfers list
envoix inbox latest
envoix inbox set-directory /absolute/path
```

Status must report Agent protocol 14 and Engine schema 2. A CLI/Agent protocol
mismatch is an installation error, not a recoverable network timeout.

## Change and rollout order

1. Record the current binary revision, service configuration, endpoint id,
   client defaults, certificates, and health evidence.
2. Back up only durable service state and verify its owner-only permissions.
3. Build and test the candidate. Do not build on the service host when a pinned
   CI artifact is available.
4. Replace the relay or broker binary without regenerating the broker key.
5. Restart one service and verify its identity, ports, logs, and an external
   pairing.
6. Change the compiled deployment defaults only after the endpoint is healthy.
7. Update installed Agent settings and remembered Relationship routes, then
   verify reconnect and a reference transfer before retiring the old route.

Broker Room state is memory-only. A restart drops parked or authenticating
Rooms; it does not affect completed Relationships or received files. Schedule a
restart as a brief pairing interruption and never promise seamless Room
continuity across it.

## Backup policy

The broker endpoint key is identity, not replaceable cache. Back it up encrypted
with metadata recording host, endpoint id, creation date, file mode, and restore
test. A restore is valid only when the resulting endpoint id is unchanged.

Back up desktop state only while its Agent/helper is stopped or through a
future application-level snapshot command. Preserve file ownership and modes.
Treat vault material and Engine state as one consistency set: restoring only
one can produce unusable Relationships. The Inbox is user content and follows
the user's ordinary backup policy; it is never part of state cleanup.

Do not collect bearer tokens, Apple signing identities, Android keystores,
notary credentials, or plaintext vault values in an ordinary support archive.

## Recovery decisions

| Symptom | Safe action | Do not do |
| --- | --- | --- |
| Broker starts with a different endpoint id | stop rollout; restore the last verified key and recheck ownership/mode | silently update clients and strand every existing route |
| Broker or relay update fails | restore the previous binary/config while retaining key and certificate state | generate a new identity as a rollback |
| Agent reports protocol mismatch | install the CLI and Agent from the same build, then restart | retry network pairing with mixed binaries |
| Agent reports unsupported legacy state | make a backup, run confirmed allowlisted state cleanup, reinstall, and re-pair | delete the Inbox or silently import v0.2 credentials |
| Current Engine snapshot is corrupt | let the Engine validate and recover its previous snapshot; retain both for diagnosis | hand-edit JSON or replace it with an empty file |
| Remembered reconnect fails after route migration | compare both peers' Relationship routes and generations, then use typed diagnostics; re-pair only after preserving evidence | restore retired broker credentials or copy secret blobs between devices |
| Transfer is paused/failed | use the explicit retry/resume action after resolving the typed cause | mark it Delivered from file presence alone |
| Device is lost or no longer trusted | revoke/forget the Relationship on every reachable peer | delete received files or unrelated history |

On Linux/WSL and Windows, ordinary `envoix agent uninstall` retains settings,
Engine state, credentials, and Inbox. The destructive test-cycle reset is
`envoix agent uninstall --delete-state --yes`; its allowlist still excludes the
Inbox and unknown files.

## Diagnostic service policy

Remote upload is off by default. When deliberately enabled:

- use a public TLS listener with `--tls-cert` and `--tls-key`, or bind plain
  HTTP only to loopback behind a correctly configured TLS reverse proxy;
- use separate owner-only upload and view token files;
- never use `--unsafe-open-log-view` on a public or shared network;
- keep the default one-hour memory-only retention unless an incident has an
  approved shorter requirement;
- tell the user what report is being uploaded and require their explicit click;
- rotate a token after exposure or an incident, and restart the service to load
  it;
- enforce client-source limits at a reverse proxy because the application sees
  that proxy as its socket peer.

The application limits authenticated uploads to 3 per minute with burst 5 per
socket source, and report views to 60 per minute with burst 120. Limit state is
capped at 4,096 source/operation entries and expires after ten idle minutes.

## Decommissioning

Before retiring a broker or relay, prove that compiled defaults, installed
Agent settings, and every retained Relationship route have moved. Revoke DNS
records and firewall ports only after a reference matrix no longer observes the
old address. Destroy old endpoint and TLS keys through the provider's secure
deletion process; do not leave them in shell history, repository artifacts, or
world-readable backups.
