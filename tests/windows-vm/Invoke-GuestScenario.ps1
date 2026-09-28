#Requires -Version 5.1
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Root)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# This script changes taskbar settings and installs software. Refuse host execution.
if ($Root -notmatch '^C:\\CCUM-Lab\\[a-f0-9]{32}$' -or
    -not (Test-Path -LiteralPath 'C:\CCUM-Lab\guest-enabled') -or
    (Get-CimInstance Win32_ComputerSystem).Model -ne 'Virtual Machine') {
    throw 'Run only inside an explicitly prepared disposable Hyper-V guest.'
}
$request = Get-Content -LiteralPath "$Root\request.json" -Raw | ConvertFrom-Json
$Context = [pscustomobject]@{
    Root = $Root; Request = $request; Evidence = "$Root\evidence"
    SessionId = (Get-Process -Id $PID).SessionId
    SettingsPath = "$env:APPDATA\ClaudeCodeUsageMonitor\settings.json"
    CandidateExe = "$Root\candidate.exe"; PreviousExe = "$Root\previous.exe"
    Checks = [Collections.Generic.List[object]]::new()
    Phase = 'initial'
}
if (Test-Path "$Root\continuation.json") {
    $continuation = Get-Content "$Root\continuation.json" -Raw | ConvertFrom-Json
    $Context.Phase = $continuation.phase
    foreach ($check in $continuation.checks) { $Context.Checks.Add($check) }
}
. "$Root\support\Guest.Helpers.ps1" -Context $Context
$result = [ordered]@{ runId = $request.runId; id = $request.id; status = 'error'; error = $null; checks = @(); startedUtc = [DateTime]::UtcNow.ToString('o') }
$transcribing = $false
$executingCase = $false
try {
    Start-Transcript -LiteralPath "$evidence\guest.log" -Append | Out-Null
    $transcribing = $true
    Assert-Check 'interactive session' ($sessionId -gt 0) $sessionId
    $os = Get-CimInstance Win32_OperatingSystem
    $build = [int]$os.BuildNumber
    Assert-Check 'expected guest OS' (($request.os -eq 'windows10' -and $build -ge 19041 -and $build -lt 22000) -or ($request.os -eq 'windows11' -and $build -ge 22000)) $build
    $os | Select-Object Caption, Version, BuildNumber | ConvertTo-Json | Set-Content -LiteralPath "$evidence\os.json" -Encoding UTF8
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    Add-Type -Path "$Root\support\Desktop.cs"
    Assert-Check 'unlocked input desktop' ([CCUMLabDesktop]::IsDefaultDesktop()) $sessionId
    if ($Context.Phase -eq 'initial') {
        Assert-Check 'clean app processes' (@(Get-AppProcesses).Count -eq 0) 'baseline must have no running monitor'
        Assert-Check 'clean settings' (-not (Test-Path -LiteralPath $settingsPath)) $settingsPath
        Set-TestTaskbar
    }
    # The host validates the catalogue; validate the relative entry point here too.
    if ($request.script -notmatch '^cases/[a-z0-9-]+\.ps1$') { throw 'Invalid case script path.' }
    $executingCase = $true
    & "$Root\$($request.script)" -Context $Context
    if (@($Context.Checks | Where-Object passed -EQ $false).Count) { throw 'One or more checks failed; see result.json for all failures.' }
    $result.status = 'passed'
} catch {
    $result.error = $_.Exception.Message
    # Keep inner managed/native helper stacks, not only PowerShell's wrapper
    # message, so automation exceptions can be distinguished from app failures.
    try {
        @($_.Exception.ToString(), $_.ScriptStackTrace, $_.InvocationInfo.PositionMessage) |
            Set-Content -LiteralPath "$evidence\guest-error.txt" -Encoding UTF8
    } catch { $result['errorEvidenceError'] = $_.Exception.Message }
    $failedChecks = @($Context.Checks | Where-Object passed -EQ $false)
    if ($executingCase -and $failedChecks.Count) { $result.status = 'failed' }
    try { Save-Screenshot 'failure' } catch { $result['screenshotError'] = $_.Exception.Message }
} finally {
    try { Stop-App } catch { $result.status = 'error'; $result['cleanupError'] = $_.Exception.Message }
    foreach ($file in @($settingsPath, "$env:TEMP\claude-code-usage-monitor.log")) {
        if (Test-Path -LiteralPath $file) { Copy-Item -LiteralPath $file -Destination $evidence -Force -ErrorAction Continue }
    }
    $result.checks = @($Context.Checks.ToArray())
    $result['finishedUtc'] = [DateTime]::UtcNow.ToString('o')
    if ($transcribing) { Stop-Transcript | Out-Null }
    $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath "$evidence\result.tmp" -Encoding UTF8
    Move-Item -LiteralPath "$evidence\result.tmp" -Destination "$evidence\result.json" -Force
}
if ($result.status -ne 'passed') { exit 1 }
