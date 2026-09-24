Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Read-LabConfig {
    param([Parameter(Mandatory)][string]$Path)
    $config = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if ($config.schemaVersion -ne 1) { throw 'Unsupported lab schemaVersion.' }
    if (@($config.vms).Count -ne 2) { throw 'The proof of concept requires exactly two VMs.' }
    $names = @{}
    $systems = @{}
    foreach ($vm in $config.vms) {
        if ($vm.name -notmatch '^CCUM-[A-Za-z0-9-]+$') { throw 'VM names must start with CCUM- and contain only letters, digits and hyphens.' }
        if ($names.ContainsKey($vm.name)) { throw 'Duplicate VM name.' }
        $names[$vm.name] = $true
        if ($vm.os -notin @('windows10', 'windows11') -or $systems.ContainsKey($vm.os)) { throw 'Configure one windows10 and one windows11 VM.' }
        $systems[$vm.os] = $true
        if ([string]::IsNullOrWhiteSpace($vm.checkpoint) -or [string]::IsNullOrWhiteSpace($vm.guestUser)) { throw 'checkpoint and guestUser are required.' }
    }
    return $config
}

function Get-LabPlan {
    param([Parameter(Mandatory)]$Config,
        [string[]]$Flows = @('portable-launch'),
        [string[]]$Taskbars = @('baseline', 'auto-hide', 'left', 'center'))
    foreach ($flow in $Flows) {
        if ($flow -notin @('portable-launch', 'portable-taskbar-tray', 'portable-update-helper', 'winget-install', 'winget-upgrade')) { throw "Unknown flow: $flow" }
    }
    foreach ($taskbar in $Taskbars) {
        if ($taskbar -notin @('baseline', 'auto-hide', 'left', 'center')) { throw "Unknown taskbar: $taskbar" }
    }
    foreach ($vm in $Config.vms) {
        foreach ($flow in ($Flows | Select-Object -Unique)) {
            foreach ($taskbar in ($Taskbars | Select-Object -Unique)) {
                if ($flow -eq 'portable-taskbar-tray' -and $taskbar -eq 'auto-hide') { continue }
                if ($vm.os -eq 'windows10' -and $taskbar -in @('left', 'center')) { continue }
                [pscustomobject]@{ id = "$($vm.name)-$flow-$taskbar"; vm = $vm; flow = $flow; taskbar = $taskbar }
            }
        }
    }
}

function Assert-LabVM {
    param([Parameter(Mandatory)]$Definition)
    # Resolve by exact name: do not let Hyper-V wildcard matching select other VMs.
    $matches = @(Get-VM -ErrorAction Stop | Where-Object Name -EQ $Definition.name)
    if ($matches.Count -ne 1) { throw "Expected exactly one VM named $($Definition.name)." }
    $vm = $matches[0]
    if ($vm.Notes -notmatch '(?m)^CCUM-DISPOSABLE-LAB\s*$') { throw "VM $($vm.Name) is missing the CCUM-DISPOSABLE-LAB notes marker." }
    $snapshots = @(Get-VMSnapshot -VM $vm | Where-Object Name -EQ $Definition.checkpoint)
    if ($snapshots.Count -ne 1) { throw "Expected exactly one checkpoint named $($Definition.checkpoint)." }
    if ([string]$snapshots[0].SnapshotType -ne 'Standard') { throw 'Use a standard checkpoint that preserves the signed-in desktop.' }
    # SnapshotType classifies user/recovery/replica snapshots, not the VM's
    # Standard/Production checkpoint setting. Production and powered-off
    # checkpoints have no desktop memory to resume.
    if ([string]$snapshots[0].State -notin @('Running', 'Saved', 'Paused')) {
        throw 'Use a checkpoint with saved desktop memory, captured while the guest is signed in and unlocked.'
    }
    [pscustomobject]@{ VM = $vm; Checkpoint = $snapshots[0] }
}

Export-ModuleMember -Function Read-LabConfig, Get-LabPlan, Assert-LabVM
