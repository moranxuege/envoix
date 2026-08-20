[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Cli,

    [Parameter(Mandatory = $true)]
    [string]$Agent
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Invoke-Checked {
    param(
        [string]$Executable,
        [string[]]$ArgumentList
    )

    $output = & $Executable @ArgumentList
    if ($LASTEXITCODE -ne 0) {
        throw "$Executable exited with code $LASTEXITCODE"
    }
    $output
}

function Invoke-AgentStatusAttempt {
    param([string]$Executable)

    $previousPreference = $ErrorActionPreference
    $output = $null
    $exitCode = -1
    try {
        $ErrorActionPreference = "SilentlyContinue"
        $output = & $Executable --json agent status 2>$null
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousPreference
    }
    [pscustomobject]@{
        ExitCode = $exitCode
        Output = $output
    }
}

function Invoke-IgnoringFailure {
    param(
        [string]$Executable,
        [string[]]$ArgumentList
    )

    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "SilentlyContinue"
        & $Executable @ArgumentList 2>$null | Out-Null
    }
    finally {
        $ErrorActionPreference = $previousPreference
    }
}

function Wait-AgentStatus {
    param(
        [string]$Executable,
        [int]$PreviousPid = 0
    )

    for ($attempt = 0; $attempt -lt 50; $attempt++) {
        $result = Invoke-AgentStatusAttempt -Executable $Executable
        if ($result.ExitCode -eq 0) {
            $status = ($result.Output | ConvertFrom-Json).status
            if ($PreviousPid -eq 0 -or $status.pid -ne $PreviousPid) {
                return $status
            }
        }
        Start-Sleep -Milliseconds 100
    }
    throw "Agent did not become ready with a new process ID"
}

function Wait-PathRemoved {
    param([string]$Path)

    for ($attempt = 0; $attempt -lt 100 -and (Test-Path -LiteralPath $Path); $attempt++) {
        Start-Sleep -Milliseconds 100
    }
    if (Test-Path -LiteralPath $Path) {
        throw "path was not removed: $Path"
    }
}

function Assert-True {
    param(
        [bool]$Condition,
        [string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

$sourceCli = (Resolve-Path -LiteralPath $Cli).Path
$sourceAgent = (Resolve-Path -LiteralPath $Agent).Path
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("envoix-lifecycle-" + [Guid]::NewGuid().ToString("N"))
$productRoot = Join-Path $testRoot "Envoix"
$installedCli = Join-Path $productRoot "bin\envoix.exe"
$installedAgent = Join-Path $productRoot "bin\envoix-agent.exe"
$settings = Join-Path $productRoot "config\agent.json"
$engineLock = Join-Path $productRoot "engine.lock"
$inboxMarker = Join-Path $productRoot "inbox\received.txt"
$unknownMarker = Join-Path $productRoot "user-note.txt"
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$taskName = "Envoix Agent $sid"
$existingTask = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if ($null -ne $existingTask) {
    throw "refusing to replace existing scheduled task: $taskName"
}

$hadLocalAppData = Test-Path Env:LOCALAPPDATA
$previousLocalAppData = $env:LOCALAPPDATA
$env:LOCALAPPDATA = $testRoot
$completed = $false

try {
    Invoke-Checked -Executable $sourceCli -ArgumentList @(
        "agent", "install", "--agent-binary", $sourceAgent, "--device-name", "Windows Lifecycle Test"
    ) | Out-Null
    $installed = Wait-AgentStatus -Executable $installedCli
    $task = Get-ScheduledTask -TaskName $taskName -ErrorAction Stop
    Assert-True ($task.Principal.RunLevel -eq "Limited") "task must run at limited privilege"
    Assert-True ($task.Principal.LogonType -eq "Interactive") "task must use an interactive token"

    New-Item -ItemType Directory -Path (Split-Path $inboxMarker) -Force | Out-Null
    [System.IO.File]::WriteAllText($inboxMarker, "received bytes")
    [System.IO.File]::WriteAllText($unknownMarker, "user owned")
    $settingsHash = (Get-FileHash -LiteralPath $settings -Algorithm SHA256).Hash
    $engineLockCreated = (Get-Item -LiteralPath $engineLock).CreationTimeUtc.Ticks

    Invoke-Checked -Executable $installedCli -ArgumentList @("agent", "stop") | Out-Null
    $stoppedStatus = Invoke-AgentStatusAttempt -Executable $installedCli
    Assert-True ($stoppedStatus.ExitCode -ne 0) "Agent remained reachable after stop returned"
    Assert-True ((Get-ScheduledTask -TaskName $taskName).State -eq "Ready") "stopped task must be ready"

    Invoke-Checked -Executable $installedCli -ArgumentList @("agent", "start") | Out-Null
    $started = Wait-AgentStatus -Executable $installedCli -PreviousPid $installed.pid
    Invoke-Checked -Executable $installedCli -ArgumentList @("agent", "restart") | Out-Null
    $restarted = Wait-AgentStatus -Executable $installedCli -PreviousPid $started.pid

    Invoke-Checked -Executable $sourceCli -ArgumentList @(
        "agent", "update", "--agent-binary", $sourceAgent
    ) | Out-Null
    $updated = Wait-AgentStatus -Executable $sourceCli -PreviousPid $restarted.pid
    Assert-True ($settingsHash -eq (Get-FileHash -LiteralPath $settings -Algorithm SHA256).Hash) "update changed settings"
    Assert-True ($engineLockCreated -eq (Get-Item -LiteralPath $engineLock).CreationTimeUtc.Ticks) "update replaced Engine state"
    Assert-True ([System.IO.File]::ReadAllText($inboxMarker) -eq "received bytes") "update changed Inbox content"
    Assert-True ([System.IO.File]::ReadAllText($unknownMarker) -eq "user owned") "update changed an unknown file"

    Invoke-Checked -Executable $installedCli -ArgumentList @("agent", "uninstall") | Out-Null
    Wait-PathRemoved -Path $installedCli
    Assert-True ($null -eq (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue)) "default uninstall left the task"
    Assert-True (-not (Test-Path -LiteralPath $installedAgent)) "default uninstall left the Agent binary"
    Assert-True (Test-Path -LiteralPath $settings) "default uninstall removed settings"
    Assert-True (Test-Path -LiteralPath $engineLock) "default uninstall removed Engine state"
    Assert-True (Test-Path -LiteralPath $inboxMarker) "default uninstall removed Inbox content"
    Assert-True (Test-Path -LiteralPath $unknownMarker) "default uninstall removed an unknown file"

    Invoke-Checked -Executable $sourceCli -ArgumentList @(
        "agent", "install", "--agent-binary", $sourceAgent, "--device-name", "Windows Lifecycle Test"
    ) | Out-Null
    Wait-AgentStatus -Executable $installedCli | Out-Null
    Invoke-Checked -Executable $installedCli -ArgumentList @("agent", "stop") | Out-Null

    $fixtureFiles = @(
        "agent.sock",
        "identity.key",
        "engine-state-v2.json",
        "engine-state-v2.previous.json",
        "engine-state-v1.json",
        "engine-state-v1.previous.json",
        "migration\backup.json",
        "vault\credential",
        "product\product-state-v1.json",
        "outbox\jobs\job.json",
        "transfer-state-v2\checkpoint.json"
    )
    foreach ($relativePath in $fixtureFiles) {
        $path = Join-Path $productRoot $relativePath
        New-Item -ItemType Directory -Path (Split-Path $path) -Force | Out-Null
        [System.IO.File]::WriteAllText($path, "managed")
    }

    Invoke-Checked -Executable $installedCli -ArgumentList @(
        "agent", "uninstall", "--delete-state", "--yes"
    ) | Out-Null
    Wait-PathRemoved -Path $installedCli
    $managedEntries = @(
        "agent.sock",
        "identity.key",
        "engine-state-v2.json",
        "engine-state-v2.previous.json",
        "engine-state-v1.json",
        "engine-state-v1.previous.json",
        "engine.lock",
        "migration",
        "vault",
        "product",
        "outbox",
        "transfer-state-v2"
    )
    foreach ($entry in $managedEntries) {
        Assert-True (-not (Test-Path -LiteralPath (Join-Path $productRoot $entry))) "state cleanup left managed entry: $entry"
    }
    Assert-True (-not (Test-Path -LiteralPath $settings)) "state cleanup left settings"
    Assert-True ([System.IO.File]::ReadAllText($inboxMarker) -eq "received bytes") "state cleanup changed Inbox content"
    Assert-True ([System.IO.File]::ReadAllText($unknownMarker) -eq "user owned") "state cleanup changed an unknown file"
    Assert-True ($null -eq (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue)) "state cleanup left the task"
    Assert-True (-not (Test-Path -LiteralPath $installedAgent)) "state cleanup left the Agent binary"

    [pscustomobject]@{
        ProtocolVersion = $updated.protocol_version
        InstallPid = $installed.pid
        StartPid = $started.pid
        RestartPid = $restarted.pid
        UpdatePid = $updated.pid
        DefaultUninstallPreservedData = $true
        StateCleanupPreservedInbox = $true
    } | ConvertTo-Json -Compress
    $completed = $true
}
finally {
    if ($null -ne (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue)) {
        Invoke-IgnoringFailure -Executable $sourceCli -ArgumentList @(
            "agent", "uninstall", "--delete-state", "--yes"
        )
    }
    if ($null -ne (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue)) {
        $schtasks = Join-Path $env:SystemRoot "System32\schtasks.exe"
        Invoke-IgnoringFailure -Executable $schtasks -ArgumentList @("/End", "/TN", $taskName)
        Invoke-IgnoringFailure -Executable $schtasks -ArgumentList @(
            "/Delete", "/TN", $taskName, "/F"
        )
    }
    for ($attempt = 0; $attempt -lt 100 -and (Test-Path -LiteralPath $testRoot); $attempt++) {
        try {
            Remove-Item -LiteralPath $testRoot -Recurse -Force
        }
        catch {
            if ($attempt -eq 99) {
                throw
            }
            Start-Sleep -Milliseconds 100
        }
    }
    if ($hadLocalAppData) {
        $env:LOCALAPPDATA = $previousLocalAppData
    }
    else {
        Remove-Item Env:LOCALAPPDATA
    }
}

if (-not $completed -or (Test-Path -LiteralPath $testRoot)) {
    throw "Windows Agent lifecycle test did not clean its isolated root"
}
