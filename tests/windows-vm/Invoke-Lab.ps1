#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$ConfigPath,
    [Alias('Flows')][string[]]$Tests,
    [string[]]$Suite,
    [string[]]$Taskbars,
    [string[]]$VMNames,
    [string]$CandidateExe,
    [string]$PreviousExe,
    [string]$CandidateVersion,
    [string]$PreviousVersion,
    [ValidateSet('compact', 'classic')][string]$TrayTheme = 'compact',
    [System.Management.Automation.PSCredential]$Credential,
    [string]$OutputRoot,
    [ValidateRange(60, 7200)][int]$ScenarioTimeoutSeconds,
    [switch]$PlanOnly,
    [switch]$ListTests
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $ConfigPath) { $ConfigPath = "$PSScriptRoot\lab.example.json" }
if (-not $OutputRoot) { $OutputRoot = "$PSScriptRoot\artifacts" }
Import-Module "$PSScriptRoot\Lab.psm1" -Force
. "$PSScriptRoot\Host.Actions.ps1"
$catalog = Read-LabCatalog
if ($ListTests) {
    $catalog.tests | Select-Object id, description, @{Name='suites';Expression={$_.suites -join ', '}}, @{Name='requires';Expression={$_.requires -join ', '}}, timeoutSeconds
    return
}
$config = Read-LabConfig $ConfigPath
$plan = @(Get-LabPlan -Config $config -Tests $Tests -Taskbars $Taskbars -Suite $Suite -Catalog $catalog)
if ($VMNames) {
    foreach ($name in $VMNames) { if ($name -notin $config.vms.name) { throw "Unknown configured VM: $name" } }
    $plan = @($plan | Where-Object { $_.vm.name -in $VMNames })
}
if ($plan.Count -eq 0) { throw 'The selected matrix is empty.' }
if ($PlanOnly) {
    ConvertTo-Json -InputObject $plan -Depth 8
    return
}
$runnable = @($plan | Where-Object status -NE 'skipped')
if ($runnable.Count -eq 0) { throw 'The selected matrix has no supported scenarios. See -PlanOnly for skip reasons.' }
$inputs = @{ candidateExe = $CandidateExe; previousExe = $PreviousExe; candidateVersion = $CandidateVersion; previousVersion = $PreviousVersion }
Assert-LabInputs -Plan $plan -Inputs $inputs
if (-not $Credential) { throw 'Supply -Credential (Get-Credential) for the guest local administrator.' }
foreach ($vm in ($config.vms | Where-Object { $_.name -in $runnable.vm.name })) {
    if (($Credential.UserName -split '\\')[-1] -ne $vm.guestUser) { throw 'Credential username must match guestUser on both VMs.' }
}
Import-Module Hyper-V -ErrorAction Stop
# Validate every target before restoring any checkpoint.
$targets = @{}
foreach ($definition in ($config.vms | Where-Object { $_.name -in $runnable.vm.name })) { $targets[$definition.name] = Assert-LabVM $definition }
$runId = [guid]::NewGuid().ToString('N')
$runRoot = Join-Path ([IO.Path]::GetFullPath($OutputRoot)) $runId
$null = New-Item -ItemType Directory -Path $runRoot
ConvertTo-Json -InputObject $plan -Depth 8 | Set-Content -LiteralPath "$runRoot\plan.json" -Encoding UTF8
$results = @(New-LabResults -Plan $plan -RunRoot $runRoot)
Write-LabReport $results $runRoot
# Prevent concurrent checkpoint restores by other harness invocations.
$mutex = [Threading.Mutex]::new($false, 'Global\CCUM-WindowsVMLab')
$lockHeld = $false
try {
    try { $lockHeld = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $lockHeld = $true }
    if (-not $lockHeld) { throw 'Another VM lab run is active.' }
    foreach ($scenario in $runnable) {
        $target = $targets[$scenario.vm.name]
        $evidence = Join-Path $runRoot $scenario.id
        $null = New-Item -ItemType Directory -Path $evidence
        $session = $null
        $hostState = @{}
        $result = $results | Where-Object id -EQ $scenario.id
        $result.status = 'error'
        $result.startedUtc = [DateTime]::UtcNow.ToString('o')
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $timeout = if ($ScenarioTimeoutSeconds) { $ScenarioTimeoutSeconds } else { $scenario.test.timeoutSeconds }
        $guestRoot = "C:\CCUM-Lab\$runId"
        try {
            Write-Host "Running $($scenario.id)"
            Restore-VMSnapshot -VMSnapshot $target.Checkpoint -Confirm:$false
            if ((Get-VM -Id $target.VM.Id).State -ne 'Running') { Start-VM -VM $target.VM }
            $deadline = [DateTime]::UtcNow.AddSeconds(120)
            do {
                try { $session = New-PSSession -VMId $target.VM.Id -Credential $Credential -ErrorAction Stop } catch { Start-Sleep -Seconds 2 }
            } until ($session -or [DateTime]::UtcNow -gt $deadline)
            if (-not $session) { throw 'PowerShell Direct unavailable after 120 seconds; check guest credentials and integration services.' }
            Invoke-Command -Session $session -ScriptBlock {
                param($root, $user)
                $ErrorActionPreference = 'Stop'
                $null = New-Item -ItemType Directory -Path "$root\evidence" -Force
                $null = New-Item -ItemType Directory -Path "$root\cases" -Force
                # PowerShell Direct is elevated; the desktop task uses a limited token.
                $acl = Get-Acl -LiteralPath $root
                $rule = [Security.AccessControl.FileSystemAccessRule]::new("$env:COMPUTERNAME\$user", 'Modify', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
                $acl.SetAccessRule($rule)
                Set-Acl -LiteralPath $root -AclObject $acl
            } -ArgumentList $guestRoot, $scenario.vm.guestUser
            Copy-Item -ToSession $session -LiteralPath "$PSScriptRoot\Invoke-GuestScenario.ps1" -Destination "$guestRoot\Invoke-GuestScenario.ps1"
            Copy-Item -ToSession $session -LiteralPath "$PSScriptRoot\support" -Destination "$guestRoot\support" -Recurse
            Copy-Item -ToSession $session -LiteralPath "$PSScriptRoot\$($scenario.test.script)" -Destination "$guestRoot\$($scenario.test.script)"
            foreach ($name in @('candidateExe', 'previousExe')) {
                if ($name -in $scenario.test.requires) {
                    $filename = if ($name -eq 'candidateExe') { 'candidate.exe' } else { 'previous.exe' }
                    Copy-Item -ToSession $session -LiteralPath $inputs[$name] -Destination "$guestRoot\$filename"
                }
            }
            $request = @{
                runId = $runId; id = $scenario.id; flow = $scenario.flow; taskbar = $scenario.taskbar; os = $scenario.vm.os
                candidateVersion = $CandidateVersion; previousVersion = $PreviousVersion; trayTheme = $TrayTheme
                script = $scenario.test.script
                candidateHash = $(if ('candidateExe' -in $scenario.test.requires) { (Get-FileHash -LiteralPath $inputs.candidateExe).Hash } else { $null })
            } | ConvertTo-Json
            Invoke-Command -Session $session -ScriptBlock {
                param($root, $json, $user, $timeout)
                $ErrorActionPreference = 'Stop'
                Set-Content -LiteralPath "$root\request.json" -Value $json -Encoding UTF8
                $principal = New-ScheduledTaskPrincipal -UserId "$env:COMPUTERNAME\$user" -LogonType Interactive -RunLevel Limited
                $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$root\Invoke-GuestScenario.ps1`" -Root `"$root`"" -WorkingDirectory $root
                $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds $timeout)
                $null = Register-ScheduledTask -TaskName 'CCUM-Lab-Scenario' -Action $action -Principal $principal -Settings $settings -Force
                Start-ScheduledTask -TaskName 'CCUM-Lab-Scenario'
            } -ArgumentList $guestRoot, $request, $scenario.vm.guestUser, $timeout
            $deadline = [DateTime]::UtcNow.AddSeconds($timeout)
            $taskGrace = [Diagnostics.Stopwatch]::StartNew()
            do {
                Start-Sleep -Seconds 2
                $hostRequest = Invoke-Command -Session $session -ScriptBlock {
                    param($root)
                    if (Test-Path "$root\host-request.json") { Get-Content "$root\host-request.json" -Raw }
                } -ArgumentList $guestRoot
                if ($hostRequest) {
                    $actionRequest = $hostRequest | ConvertFrom-Json
                    Write-Host "Host action for $($scenario.id): $($actionRequest.action)"
                    Invoke-LabHostRequest $actionRequest $scenario $target ([ref]$session) $Credential $guestRoot $hostState
                    if ($actionRequest.action -eq 'reboot') { $taskGrace.Restart() }
                }
                $taskState = Invoke-Command -Session $session -ScriptBlock {
                    param($root)
                    @{done=(Test-Path -LiteralPath "$root\evidence\result.json"); state=[string](Get-ScheduledTask -TaskName CCUM-Lab-Scenario).State; code=(Get-ScheduledTaskInfo -TaskName CCUM-Lab-Scenario).LastTaskResult}
                } -ArgumentList $guestRoot
                $done = $taskState.done
                if (-not $done -and $taskState.state -ne 'Running' -and $taskGrace.Elapsed.TotalSeconds -gt 30) {
                    throw "Guest task ended without a result (state=$($taskState.state), exit=$($taskState.code)). Inspect guest.log and task.json."
                }
            } until ($done -or [DateTime]::UtcNow -gt $deadline)
            if (-not $done) { throw 'Guest scenario timed out. Ensure the checkpoint contains an unlocked console desktop for guestUser.' }
            $guestResult = Invoke-Command -Session $session -ScriptBlock { param($root) Get-Content -LiteralPath "$root\evidence\result.json" -Raw } -ArgumentList $guestRoot | ConvertFrom-Json
            if ($guestResult.id -ne $scenario.id -or $guestResult.runId -ne $runId) { throw 'Result identity mismatch.' }
            if ($guestResult.status -notin @('passed', 'failed', 'error')) { throw 'Invalid guest result status.' }
            $result.status = $guestResult.status
            $result.error = $guestResult.error
            $result.failedCheck = (@($guestResult.checks | Where-Object passed -EQ $false | ForEach-Object name) -join '; ')
            if ($guestResult.PSObject.Properties['cleanupError']) { $result.error = "$($result.error) Guest cleanup: $($guestResult.cleanupError)" }
        } catch {
            $result.status = 'error'
            $result.error = $_.Exception.Message
            $null = New-Item -ItemType Directory -Path "$evidence\evidence" -Force
            @($_.Exception.ToString(), $_.ScriptStackTrace, $_.InvocationInfo.PositionMessage) |
                Set-Content "$evidence\evidence\host-error.txt" -Encoding UTF8
        } finally {
            # Preserve guest diagnostics even if a host transition broke the
            # control transport. The interactive task has a separate lifetime.
            if (-not $session -or $session.State -ne 'Opened') {
                try {
                    if ((Get-VM -Id $target.VM.Id).State -eq 'Running') {
                        if ($session) { Remove-PSSession $session -ErrorAction SilentlyContinue }
                        $session = New-PSSession -VMId $target.VM.Id -Credential $Credential -ErrorAction Stop
                    }
                } catch { $result.evidenceErrors += "Evidence reconnect: $($_.Exception.Message)" }
            }
            if ($session) {
                try {
                    Invoke-Command -Session $session -ScriptBlock {
                        param($root)
                        $ErrorActionPreference = 'Stop'
                        $task = Get-ScheduledTask -TaskName 'CCUM-Lab-Scenario' -ErrorAction SilentlyContinue
                        if ($task) {
                            Stop-ScheduledTask -InputObject $task
                            $stopDeadline = [DateTime]::UtcNow.AddSeconds(10)
                            while ((Get-ScheduledTask -TaskName 'CCUM-Lab-Scenario').State -eq 'Running' -and [DateTime]::UtcNow -lt $stopDeadline) { Start-Sleep -Milliseconds 250 }
                            Get-ScheduledTaskInfo -InputObject $task | Select-Object LastRunTime, LastTaskResult |
                                ConvertTo-Json | Set-Content -LiteralPath "$root\evidence\task.json" -Encoding UTF8
                        }
                    } -ArgumentList $guestRoot
                } catch { $result.evidenceErrors += "Task shutdown: $($_.Exception.Message)" }
                try { $result.evidenceErrors += @(Copy-LabEvidence -Session $session -GuestRoot $guestRoot -Destination "$evidence\evidence") }
                catch { $result.evidenceErrors += "Evidence listing: $($_.Exception.Message)" }
                if ($result.evidenceErrors.Count -and $result.status -eq 'passed') { $result.status = 'error' }
                Remove-PSSession $session -ErrorAction SilentlyContinue
            }
            # Always return the disposable VM to its baseline, including on failure.
            try { Restore-LabHostState $target $hostState }
            catch { $result.status = 'error'; $result.cleanupError = "Host state: $($_.Exception.Message)" }
            try { Restore-VMSnapshot -VMSnapshot $target.Checkpoint -Confirm:$false }
            catch { $result.status = 'error'; $result.cleanupError = "$($result.cleanupError) Checkpoint: $($_.Exception.Message)" }
            $result.durationSeconds = [Math]::Round($timer.Elapsed.TotalSeconds, 2)
            Write-LabReport $results $runRoot
            Write-Host "$($result.status.ToUpperInvariant()) $($result.id) ($($result.durationSeconds)s)"
        }
        if ($result.cleanupError) { throw "Checkpoint cleanup failed; stopping lab. See $runRoot\summary.json" }
    }
} finally {
    try {
        foreach ($result in $results | Where-Object status -EQ 'not-run') { $result.reason = 'Run stopped before this scenario started.' }
        Write-LabReport $results $runRoot
    } finally {
        if ($lockHeld) { $mutex.ReleaseMutex() }
        $mutex.Dispose()
    }
}
Write-Host "Evidence: $runRoot"
if (@($results | Where-Object { $_.status -notin @('passed', 'skipped') }).Count) { throw 'One or more VM scenarios failed or did not run. See summary.md and summary.json.' }
