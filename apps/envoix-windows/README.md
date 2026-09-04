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

## Verification

### Visual snapshot without stealing the desktop

The renderer can capture its own framebuffer and exit. This is deliberately
separate from an operating-system screenshot so a full-screen application
cannot obscure the evidence:

```powershell
$env:ENVOIX_UI_SCREENSHOT = "$PWD\envoix-devices.bmp"
$env:ENVOIX_UI_PAGE = "devices" # devices, activity, inbox, or settings
.\target\release\envoix-windows.exe
Remove-Item Env:ENVOIX_UI_SCREENSHOT
Remove-Item Env:ENVOIX_UI_PAGE
```

Screenshot mode starts without taking focus and ignores pointer input. Do not
publish a snapshot that contains a private device label or Inbox filename.

### Foreground file-picker and delivery proof

The foreground gesture is a separate gate from rendering and Agent transfer
tests. Use a non-sensitive fixture and retain these observations together:

1. Record the sender fixture size and SHA-256.
2. In the GUI, choose a paired device, open the native picker, select the
   fixture, and confirm its name appears in the Room before pressing Send.
3. Record the returned transfer ID and wait for Activity to say `已送达`.
   Queued or 100% uploaded is not sufficient.
4. On the receiver, reveal the saved Inbox root and compare its size and
   SHA-256 with the sender fixture.
5. Repeat once after closing the GUI immediately after queueing, proving that
   the per-user Agent, rather than the window, owns completion.

### Authenticode and SmartScreen

Signing requires a Windows code-signing identity; the Apple development Team
does not provide one. A release-signing rehearsal must use the CA or signing
service's RFC 3161 timestamp URL and then pass both native checks:

```powershell
signtool sign /fd SHA256 /td SHA256 /tr <timestamp-url> /sha1 <thumbprint> `
  .\Envoix-Windows-x86_64.exe
signtool verify /pa /all /v .\Envoix-Windows-x86_64.exe
Get-AuthenticodeSignature .\Envoix-Windows-x86_64.exe | Format-List *
```

`Get-AuthenticodeSignature` must report `Valid`, the signer subject must match
the intended publisher, and the timestamp must remain valid with the signing
certificate offline. SmartScreen is a separate reputation gate: download the
exact release asset over HTTPS in a clean Windows VM or Windows Sandbox so it
has Mark-of-the-Web, verify its published SHA-256, and record the publisher and
SmartScreen result. A locally copied or self-signed executable does not prove
public-download reputation.

Primary references: [Microsoft SignTool][signtool],
[Get-AuthenticodeSignature][authenticode], and
[Microsoft Defender SmartScreen][smartscreen].

[signtool]: https://learn.microsoft.com/windows/win32/seccrypto/signtool
[authenticode]: https://learn.microsoft.com/powershell/module/microsoft.powershell.security/get-authenticodesignature
[smartscreen]: https://learn.microsoft.com/windows/security/operating-system-security/virus-and-threat-protection/microsoft-defender-smartscreen/
