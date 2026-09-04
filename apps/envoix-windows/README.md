# Envoix for Windows

`envoix-windows` is the graphical Windows shell for the persistent per-user
Envoix Agent. It is a native Windows executable built from Rust and egui; it is
not a web view and does not embed a second transfer Engine.

The GUI uses the typed `AgentRequest`/`AgentResponse` contract over the
owner-only Named Pipe. The Agent remains the only owner of durable Engine
state, paired-device credentials, the Inbox, and active network sessions.
Closing the GUI therefore does not stop queued or in-flight work.

## Development

Run checks through the repository build-cache guard:

```powershell
bash scripts/with-build-cache-guard.sh cargo.exe clippy --locked `
  -p envoix-windows --all-targets -- -D warnings
bash scripts/with-build-cache-guard.sh cargo.exe test --locked `
  -p envoix-windows
bash scripts/with-build-cache-guard.sh cargo.exe build --locked --release `
  -p envoix-windows -p envoix-cli -p envoix-agent
```

Keep `envoix-windows.exe`, `envoix.exe`, and `envoix-agent.exe` together for a
development run. The release bundle uses the equivalent architecture-suffixed
names. If no Agent is reachable, the GUI can install and start that sibling
CLI/Agent pair without elevation.

The interface loads an installed Microsoft YaHei or SimSun font as a CJK
fallback. It never writes invitation material, verification codes, or raw
credentials to logs.
