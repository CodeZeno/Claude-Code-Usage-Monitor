#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$ConfigPath = "$PSScriptRoot\lab.example.json",
    [string[]]$Flows = @('portable-launch'),
    [string[]]$Taskbars = @('baseline', 'auto-hide', 'left', 'center'),
    [string[]]$VMNames,
    [string]$CandidateExe,
    [string]$PreviousExe,
    [string]$CandidateVersion,
    [string]$PreviousVersion,
    [ValidateSet('compact', 'classic')][string]$TrayTheme = 'compact',
    [System.Management.Automation.PSCredential]$Credential,
    [string]$OutputRoot = "$PSScriptRoot\artifacts",
    [ValidateRange(60, 3600)][int]$ScenarioTimeoutSeconds = 600,
    [switch]$PlanOnly
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot\Lab.psm1" -Force
$config = Read-LabConfig $ConfigPath
$plan = @(Get-LabPlan $config $Flows $Taskbars)
if ($VMNames) {
    foreach ($name in $VMNames) { if ($name -notin $config.vms.name) { throw "Unknown configured VM: $name" } }
    $plan = @($plan | Where-Object { $_.vm.name -in $VMNames })
}
if ($plan.Count -eq 0) { throw 'The selected matrix is empty.' }
if ($PlanOnly) {
    $plan | ConvertTo-Json -Depth 6
    return
}
if (-not $Credential) { throw 'Supply -Credential (Get-Credential) for the guest local administrator.' }
foreach ($vm in ($config.vms | Where-Object { $_.name -in $plan.vm.name })) {
    if (($Credential.UserName -split '\\')[-1] -ne $vm.guestUser) { throw 'Credential username must match guestUser on both VMs.' }
}
if ($Flows -match '^portable-') {
    if (-not $CandidateExe -or -not (Test-Path -LiteralPath $CandidateExe -PathType Leaf)) { throw 'Portable scenarios require -CandidateExe.' }
    $CandidateExe = (Resolve-Path -LiteralPath $CandidateExe).Path
}
if ($Flows -contains 'portable-update-helper') {
    if (-not $PreviousExe -or -not (Test-Path -LiteralPath $PreviousExe -PathType Leaf)) { throw 'The helper scenario requires -PreviousExe.' }
    $PreviousExe = (Resolve-Path -LiteralPath $PreviousExe).Path
    if ((Get-FileHash -LiteralPath $PreviousExe).Hash -eq (Get-FileHash -LiteralPath $CandidateExe).Hash) { throw 'Old and candidate binaries must differ.' }
    $oldVersion = [version](Get-Item -LiteralPath $PreviousExe).VersionInfo.ProductVersion
    $newVersion = [version](Get-Item -LiteralPath $CandidateExe).VersionInfo.ProductVersion
    if ($newVersion -le $oldVersion) { throw 'Candidate product version must be newer than PreviousExe.' }
}
if ($Flows -match '^winget-' -and $CandidateVersion -notmatch '^\d+\.\d+\.\d+$') { throw 'WinGet scenarios require a published -CandidateVersion (x.y.z).' }
if ($Flows -contains 'winget-upgrade') {
    if ($PreviousVersion -notmatch '^\d+\.\d+\.\d+$' -or [version]$PreviousVersion -ge [version]$CandidateVersion) { throw 'Specify a published -PreviousVersion older than CandidateVersion.' }
}
Import-Module Hyper-V -ErrorAction Stop
# Validate every target before restoring any checkpoint.
$targets = @{}
foreach ($definition in ($config.vms | Where-Object { $_.name -in $plan.vm.name })) { $targets[$definition.name] = Assert-LabVM $definition }
$runId = [guid]::NewGuid().ToString('N')
$runRoot = Join-Path ([IO.Path]::GetFullPath($OutputRoot)) $runId
$null = New-Item -ItemType Directory -Path $runRoot
$plan | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath "$runRoot\plan.json" -Encoding UTF8
$results = [Collections.Generic.List[object]]::new()
# Prevent concurrent checkpoint restores by other harness invocations.
$mutex = [Threading.Mutex]::new($false, 'Global\CCUM-WindowsVMLab')
$lockHeld = $false
try {
    try { $lockHeld = $mutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $lockHeld = $true }
    if (-not $lockHeld) { throw 'Another VM lab run is active.' }
    foreach ($scenario in $plan) {
        $target = $targets[$scenario.vm.name]
        $evidence = Join-Path $runRoot $scenario.id
        $null = New-Item -ItemType Directory -Path $evidence
        $session = $null
        $result = [ordered]@{ id = $scenario.id; status = 'failed'; error = $null; cleanupError = $null; evidence = $evidence }
        try {
            Write-Host "Running $($scenario.id)"
            Restore-VMSnapshot -VMSnapshot $target.Checkpoint -Confirm:$false
            if ((Get-VM -Id $target.VM.Id).State -ne 'Running') { Start-VM -VM $target.VM }
            $deadline = [DateTime]::UtcNow.AddSeconds(120)
            do {
                try { $session = New-PSSession -VMId $target.VM.Id -Credential $Credential -ErrorAction Stop } catch { Start-Sleep -Seconds 2 }
            } until ($session -or [DateTime]::UtcNow -gt $deadline)
            if (-not $session) { throw 'PowerShell Direct unavailable after 120 seconds; check guest credentials and integration services.' }
            $guestRoot = "C:\CCUM-Lab\$runId"
            Invoke-Command -Session $session -ScriptBlock {
                param($root, $user)
                $ErrorActionPreference = 'Stop'
                $null = New-Item -ItemType Directory -Path "$root\evidence" -Force
                # PowerShell Direct is elevated; the desktop task uses a limited token.
                $acl = Get-Acl -LiteralPath $root
                $rule = [Security.AccessControl.FileSystemAccessRule]::new("$env:COMPUTERNAME\$user", 'Modify', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
                $acl.SetAccessRule($rule)
                Set-Acl -LiteralPath $root -AclObject $acl
            } -ArgumentList $guestRoot, $scenario.vm.guestUser
            Copy-Item -ToSession $session -LiteralPath "$PSScriptRoot\Invoke-GuestScenario.ps1" -Destination "$guestRoot\Invoke-GuestScenario.ps1"
            if ($scenario.flow -eq 'portable-taskbar-tray') {
                Copy-Item -ToSession $session -LiteralPath "$PSScriptRoot\Test-TaskbarTray.ps1" -Destination "$guestRoot\Test-TaskbarTray.ps1"
            }
            if ($scenario.flow -eq 'portable-dashboard-warp') {
                Copy-Item -ToSession $session -LiteralPath "$PSScriptRoot\Test-DashboardWarp.ps1" -Destination "$guestRoot\Test-DashboardWarp.ps1"
            }
            if ($scenario.flow -like 'portable-*') {
                Copy-Item -ToSession $session -LiteralPath $CandidateExe -Destination "$guestRoot\candidate.exe"
                if ($scenario.flow -eq 'portable-update-helper') { Copy-Item -ToSession $session -LiteralPath $PreviousExe -Destination "$guestRoot\previous.exe" }
            }
            $request = @{
                runId = $runId; id = $scenario.id; flow = $scenario.flow; taskbar = $scenario.taskbar; os = $scenario.vm.os
                candidateVersion = $CandidateVersion; previousVersion = $PreviousVersion; trayTheme = $TrayTheme
                candidateHash = $(if ($scenario.flow -like 'portable-*') { (Get-FileHash -LiteralPath $CandidateExe).Hash } else { $null })
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
            } -ArgumentList $guestRoot, $request, $scenario.vm.guestUser, $ScenarioTimeoutSeconds
            $deadline = [DateTime]::UtcNow.AddSeconds($ScenarioTimeoutSeconds)
            do {
                Start-Sleep -Seconds 2
                $done = Invoke-Command -Session $session -ScriptBlock { param($root) Test-Path -LiteralPath "$root\evidence\result.json" } -ArgumentList $guestRoot
            } until ($done -or [DateTime]::UtcNow -gt $deadline)
            if (-not $done) { throw 'Guest scenario timed out. Ensure the checkpoint contains an unlocked console desktop for guestUser.' }
            $guestResult = Invoke-Command -Session $session -ScriptBlock { param($root) Get-Content -LiteralPath "$root\evidence\result.json" -Raw } -ArgumentList $guestRoot | ConvertFrom-Json
            if ($guestResult.id -ne $scenario.id -or $guestResult.runId -ne $runId) { throw 'Result identity mismatch.' }
            $result.status = $guestResult.status
            $result.error = $guestResult.error
        } catch {
            $result.error = $_.Exception.Message
        } finally {
            if ($session) {
                try {
                    Invoke-Command -Session $session -ScriptBlock {
                        param($root)
                        $ErrorActionPreference = 'Stop'
                        $task = Get-ScheduledTask -TaskName 'CCUM-Lab-Scenario' -ErrorAction SilentlyContinue
                        if ($task) {
                            Stop-ScheduledTask -InputObject $task
                            Get-ScheduledTaskInfo -InputObject $task | Select-Object LastRunTime, LastTaskResult |
                                ConvertTo-Json | Set-Content -LiteralPath "$root\evidence\task.json" -Encoding UTF8
                        }
                    } -ArgumentList $guestRoot
                    Copy-Item -FromSession $session -LiteralPath "$guestRoot\evidence" -Destination $evidence -Recurse -Force
                } catch { $result.status = 'failed'; $result.error = "$($result.error) Evidence collection: $($_.Exception.Message)" }
                Remove-PSSession $session
            }
            # Always return the disposable VM to its baseline, including on failure.
            try { Restore-VMSnapshot -VMSnapshot $target.Checkpoint -Confirm:$false }
            catch { $result.status = 'failed'; $result.cleanupError = $_.Exception.Message }
            $results.Add([pscustomobject]$result)
            ConvertTo-Json -InputObject @($results.ToArray()) -Depth 8 | Set-Content -LiteralPath "$runRoot\summary.json" -Encoding UTF8
        }
        if ($result.cleanupError) { throw "Checkpoint cleanup failed; stopping lab. See $runRoot\summary.json" }
    }
} finally {
    if ($lockHeld) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}
Write-Host "Evidence: $runRoot"
if (@($results | Where-Object status -NE 'passed').Count) { throw 'One or more VM scenarios failed. See summary.json.' }
