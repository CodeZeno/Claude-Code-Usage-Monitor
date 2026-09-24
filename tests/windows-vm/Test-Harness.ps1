#Requires -Version 5.1
# No Hyper-V, administrator rights, external modules, network or desktop actions needed.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module "$PSScriptRoot\Lab.psm1" -Force
$script:count = 0
function Check([string]$Name, [scriptblock]$Test) {
    & $Test
    $script:count++
    Write-Host "PASS $Name"
}
function Expect-Throw([scriptblock]$Action, [string]$Pattern) {
    $message = $null
    try { & $Action | Out-Null } catch { $message = $_.Exception.Message }
    if (-not $message -or $message -notlike "*$Pattern*") { throw "Expected failure containing '$Pattern'; got '$message'." }
}
function Require([bool]$Condition) { if (-not $Condition) { throw 'Check failed.' } }
$temporary = Join-Path ([IO.Path]::GetTempPath()) ("ccum-lab-tests-" + [guid]::NewGuid().ToString('N') + '.json')
try {
    Check 'all scripts parse in Windows PowerShell' {
        Get-ChildItem -LiteralPath $PSScriptRoot -File | Where-Object Extension -In '.ps1', '.psm1' | ForEach-Object {
            $tokens = $null; $errors = $null
            $null = [Management.Automation.Language.Parser]::ParseFile($_.FullName, [ref]$tokens, [ref]$errors)
            if ($errors.Count) { throw ($errors | Out-String) }
        }
    }
    $config = Read-LabConfig "$PSScriptRoot\lab.example.json"
    Check 'default matrix has six portable scenarios with OS-specific alignment' {
        $plan = @(Get-LabPlan $config)
        Require ($plan.Count -eq 6)
        Require (@($plan | Where-Object { $_.vm.os -eq 'windows10' -and $_.taskbar -in @('left', 'center') }).Count -eq 0)
    }
    Check 'all four flows create 24 distinct scenarios' {
        $plan = @(Get-LabPlan $config @('portable-launch', 'portable-update-helper', 'winget-install', 'winget-upgrade'))
        Require ($plan.Count -eq 24)
        Require (@($plan.id | Select-Object -Unique).Count -eq 24)
    }
    Check 'unsupported flows and taskbars fail' {
        Expect-Throw { Get-LabPlan $config @('portable-update') } 'Unknown flow'
        Expect-Throw { Get-LabPlan $config @('portable-launch') @('top') } 'Unknown taskbar'
    }
    Check 'tray regression excludes hidden taskbars' {
        $plan = @(Get-LabPlan $config @('portable-taskbar-tray'))
        Require ($plan.Count -eq 4)
        Require (@($plan | Where-Object taskbar -EQ 'auto-hide').Count -eq 0)
    }
    Check 'plan-only needs neither credentials nor binaries' {
        $plan = & "$PSScriptRoot\Invoke-Lab.ps1" -PlanOnly | ConvertFrom-Json
        Require ($plan.Count -eq 6)
    }
    Check 'VM selection narrows the matrix and rejects unknown names' {
        $plan = & "$PSScriptRoot\Invoke-Lab.ps1" -PlanOnly -VMNames CCUM-Win10 | ConvertFrom-Json
        Require ($plan.Count -eq 2)
        Expect-Throw { & "$PSScriptRoot\Invoke-Lab.ps1" -PlanOnly -VMNames Production } 'Unknown configured VM'
    }
    Check 'duplicate OS or unsafe VM name rejected' {
        $copy = Get-Content "$PSScriptRoot\lab.example.json" -Raw | ConvertFrom-Json
        $copy.vms[1].os = 'windows10'
        $copy | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $temporary
        Expect-Throw { Read-LabConfig $temporary } 'one windows10 and one windows11'
        $copy.vms[0].name = '*'
        $copy | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $temporary
        Expect-Throw { Read-LabConfig $temporary } 'VM names must start'
    }
    # Module-local substitutes exercise refusal paths without calling Hyper-V.
    $module = Get-Module Lab
    & $module {
        $script:fakeVM = [pscustomobject]@{ Name = 'CCUM-Win10'; Notes = ''; Id = [guid]::NewGuid() }
        $script:fakeSnapshots = @([pscustomobject]@{ Name = 'ccum-clean-desktop'; SnapshotType = 'Standard'; State = 'Running' })
        function script:Get-VM { $script:fakeVM }
        function script:Get-VMSnapshot { param($VM) $script:fakeSnapshots }
    }
    Check 'unmarked VM refused before snapshot use' {
        Expect-Throw { Assert-LabVM $config.vms[0] } 'missing the CCUM-DISPOSABLE-LAB'
    }
    & $module { $script:fakeVM.Notes = 'CCUM-DISPOSABLE-LAB' }
    Check 'explicitly marked VM with standard checkpoint accepted' {
        $target = Assert-LabVM $config.vms[0]
        Require ($target.VM.Name -eq 'CCUM-Win10')
    }
    & $module { $script:fakeSnapshots[0].SnapshotType = 'Recovery' }
    Check 'recovery checkpoint refused' {
        Expect-Throw { Assert-LabVM $config.vms[0] } 'standard checkpoint'
    }
    & $module { $script:fakeSnapshots[0].SnapshotType = 'Standard'; $script:fakeSnapshots[0].State = 'Off' }
    Check 'production or powered-off checkpoint without desktop memory refused' {
        Expect-Throw { Assert-LabVM $config.vms[0] } 'saved desktop memory'
    }
    Check 'saved and paused desktop checkpoints accepted' {
        foreach ($state in 'Saved', 'Paused') {
            & $module { param($value) $script:fakeSnapshots[0].State = $value } $state
            $target = Assert-LabVM $config.vms[0]
            Require ($target.Checkpoint.State -eq $state)
        }
    }
    & $module { $script:fakeSnapshots = @() }
    Check 'missing checkpoint refused' {
        Expect-Throw { Assert-LabVM $config.vms[0] } 'exactly one checkpoint'
    }
    Check 'desktop bridge C# compiles without invoking desktop APIs' {
        $source = Get-Content -LiteralPath "$PSScriptRoot\Invoke-GuestScenario.ps1" -Raw
        $match = [regex]::Match($source, "(?s)Add-Type -TypeDefinition @'\r?\n(.*?)\r?\n'@")
        Require $match.Success
        Add-Type -TypeDefinition $match.Groups[1].Value
    }
} finally {
    if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary }
    Remove-Module Lab -ErrorAction SilentlyContinue
}
Write-Host "$script:count harness checks passed. VM scenarios have not been executed."
