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
        $sourceFiles = @(Get-ChildItem -LiteralPath $PSScriptRoot -File) + @(Get-ChildItem -LiteralPath "$PSScriptRoot\cases", "$PSScriptRoot\support" -File -Recurse)
        $sourceFiles | Where-Object Extension -In '.ps1', '.psm1' | ForEach-Object {
            $tokens = $null; $errors = $null
            $null = [Management.Automation.Language.Parser]::ParseFile($_.FullName, [ref]$tokens, [ref]$errors)
            if ($errors.Count) { throw ($errors | Out-String) }
        }
    }
    $config = Read-LabConfig "$PSScriptRoot\lab.example.json"
    # Fixed planner fixtures keep new catalogue entries from changing unit-test
    # expectations. The live catalogue is still validated and discovered below.
    $originalIds = @('portable-launch', 'portable-dashboard-warp', 'portable-taskbar-tray', 'portable-update-helper', 'winget-install', 'winget-upgrade')
    $fixtureCatalog = Read-LabCatalog
    $fixtureCatalog.tests = @($fixtureCatalog.tests | Where-Object id -In $originalIds)
    Check 'default smoke suite runs baseline on both operating systems' {
        $plan = @(Get-LabPlan $config -Catalog $fixtureCatalog)
        Require ($plan.Count -eq 2)
        Require (@($plan | Where-Object taskbar -NE 'baseline').Count -eq 0)
    }
    Check 'legacy flow selection preserves 24 runnable scenarios and records exclusions' {
        $plan = @(Get-LabPlan $config @('portable-launch', 'portable-update-helper', 'winget-install', 'winget-upgrade') -Catalog $fixtureCatalog)
        Require (@($plan | Where-Object status -EQ 'pending').Count -eq 24)
        Require (@($plan | Where-Object status -EQ 'skipped').Count -eq 8)
        Require (@($plan.id | Select-Object -Unique).Count -eq 32)
    }
    Check 'unsupported flows and taskbars fail' {
        Expect-Throw { Get-LabPlan $config @('portable-update') } 'Unknown test'
        Expect-Throw { Get-LabPlan $config @('portable-launch') @('top') } 'Unknown taskbar'
    }
    Check 'tray regression excludes hidden taskbars' {
        $plan = @(Get-LabPlan $config @('portable-taskbar-tray'))
        Require (@($plan | Where-Object status -EQ 'pending').Count -eq 4)
        Require (@($plan | Where-Object taskbar -EQ 'auto-hide').Count -eq 0)
    }
    Check 'plan-only needs neither credentials nor binaries' {
        $plan = & "$PSScriptRoot\Invoke-Lab.ps1" -PlanOnly | ConvertFrom-Json
        $expected = @(Get-LabPlan $config)
        Require (@($plan).Count -eq $expected.Count)
        Require ((@($plan.id) -join ',') -eq ($expected.id -join ','))
    }
    Check 'VM selection narrows the matrix and rejects unknown names' {
        $plan = & "$PSScriptRoot\Invoke-Lab.ps1" -PlanOnly -VMNames CCUM-Win10 | ConvertFrom-Json
        Require (@($plan).Count -gt 0)
        Require (@($plan | Where-Object { $_.vm.name -ne 'CCUM-Win10' }).Count -eq 0)
        Expect-Throw { & "$PSScriptRoot\Invoke-Lab.ps1" -PlanOnly -VMNames Production } 'Unknown configured VM'
    }
    Check 'catalogue lists every registered test without needing a VM config' {
        $listed = @(& "$PSScriptRoot\Invoke-Lab.ps1" -ListTests -ConfigPath 'missing.json')
        Require ($listed.Count -eq (Read-LabCatalog).tests.Count)
        Require ('portable-dashboard-warp' -in $listed.id)
    }
    Check 'all six tests retain the original 34 runnable combinations' {
        $plan = @(Get-LabPlan -Config $config -Tests $originalIds -Catalog $fixtureCatalog)
        Require (@($plan | Where-Object status -EQ 'pending').Count -eq 34)
        Require (@($plan.id | Select-Object -Unique).Count -eq $plan.Count)
    }
    Check 'suite selection deduplicates tests and keeps catalogue order' {
        $plan = @(Get-LabPlan -Config $config -Suite smoke,regression -Catalog $fixtureCatalog)
        Require (@($plan | Where-Object status -EQ 'pending').Count -eq 16)
        Require (@($plan.id | Select-Object -Unique).Count -eq $plan.Count)
        Require ($plan[0].flow -eq 'portable-launch')
        Require (@($plan | Where-Object { $_.status -eq 'skipped' -and -not $_.reason }).Count -eq 0)
        Expect-Throw { Get-LabPlan -Config $config -Suite missing } 'Unknown suite'
        Expect-Throw { Get-LabPlan -Config $config -Tests portable-launch -Suite smoke } 'either -Tests'
    }
    Check 'legacy Flows alias and explicit taskbar override still work' {
        $plan = & "$PSScriptRoot\Invoke-Lab.ps1" -PlanOnly -Flows portable-launch -Taskbars baseline | ConvertFrom-Json
        Require ($plan.Count -eq 2)
        $plan = @(Get-LabPlan -Config $config -Suite regression -Taskbars auto-hide -Catalog $fixtureCatalog)
        Require (@($plan | Where-Object status -EQ 'pending').Count -eq 4)
        Require (@($plan | Where-Object status -EQ 'skipped').Count -eq 2)
    }
    Check 'catalogue rejects duplicate IDs, missing scripts, traversal and invalid requirements' {
        foreach ($mutation in @(
            { param($c) $c.tests[1].id = $c.tests[0].id },
            { param($c) $c.tests[0].script = 'cases/missing.ps1' },
            { param($c) $c.tests[0].script = 'cases/../Invoke-Lab.ps1' },
            { param($c) $c.tests[0].requires = @('password') },
            { param($c) $c.tests[0].requires = @('previousExe') },
            { param($c) $c.tests[0].timeoutSeconds = 0 },
            { param($c) $c.tests[0].suites = @('unknown') },
            { param($c) $c.tests[0].operatingSystems = @('linux') },
            { param($c) $c.tests[0].taskbars = @() },
            { param($c) $c.tests[0].requires = 'candidateExe' },
            { param($c) $c.tests[0].id = 'Uppercase' }
        )) {
            $catalog = Read-LabCatalog
            & $mutation $catalog
            $catalog | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $temporary
            Expect-Throw { Read-LabCatalog $temporary } ''
        }
    }
    Check 'a new catalogue entry is discovered without a runner change' {
        $catalog = Read-LabCatalog
        $extra = $catalog.tests | Where-Object id -EQ 'portable-launch' | ConvertTo-Json | ConvertFrom-Json
        $extra.id = 'future-test'
        $extra.operatingSystems = @('windows11')
        $catalog.tests += $extra
        $catalog | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $temporary
        $loaded = Read-LabCatalog $temporary
        $plan = @(Get-LabPlan -Config $config -Catalog $loaded -Tests future-test -Taskbars baseline)
        Require ($plan.Count -eq 2)
        Require ($plan[0].status -eq 'skipped')
        Require ($plan[1].status -eq 'pending')
        Require ($plan[1].test.script -eq 'cases/portable-launch.ps1')
    }
    Check 'required inputs are validated before credentials or Hyper-V' {
        Expect-Throw { & "$PSScriptRoot\Invoke-Lab.ps1" -Suite smoke } 'require -candidateExe'
        $plan = @(Get-LabPlan -Config $config -Tests winget-upgrade -Taskbars baseline)
        Expect-Throw { Assert-LabInputs $plan @{candidateVersion='2.0.0'} } 'require -previousVersion'
        Expect-Throw { Assert-LabInputs $plan @{candidateVersion='latest'; previousVersion='1.0.0'} } 'published version'
        Expect-Throw { Assert-LabInputs $plan @{candidateVersion='2.0.0'; previousVersion='2.0.0'} } 'older than'
        Assert-LabInputs $plan @{candidateVersion='2.0.0'; previousVersion='1.0.0'}
        $skipped = @(Get-LabPlan -Config $config -Tests portable-taskbar-tray -Taskbars auto-hide)
        Assert-LabInputs $skipped @{}
    }
    Check 'reports retain unexecuted scenarios and distinguish failures from errors' {
        $reportRoot = Join-Path ([IO.Path]::GetTempPath()) ('ccum-report-' + [guid]::NewGuid().ToString('N'))
        $null = New-Item -ItemType Directory -Path $reportRoot
        try {
            $plan = @(Get-LabPlan -Config $config -Suite regression)
            $results = @(New-LabResults $plan $reportRoot)
            $results[0].status = 'failed'; $results[0].failedCheck = 'widget visible'; $results[0].error = "bad | bounds`nnext line"
            $results[1].status = 'error'; $results[1].evidenceErrors = @('locked transcript')
            Write-LabReport $results $reportRoot
            $saved = Get-Content "$reportRoot\summary.json" -Raw | ConvertFrom-Json
            Require ($saved.Count -eq $plan.Count)
            Require (@($saved | Where-Object status -EQ 'not-run').Count -gt 0)
            Require (@($saved | Where-Object status -EQ 'skipped').Count -gt 0)
            $markdown = Get-Content "$reportRoot\summary.md" -Raw
            Require ($markdown.Contains('widget visible'))
            Require ($markdown.Contains('bad \| bounds next line'))
            Require ($markdown.Contains('locked transcript'))
        } finally {
            # Only this test's freshly-created, exact temp directory is removed.
            if ((Split-Path -Parent ([IO.Path]::GetFullPath($reportRoot))) -ne ([IO.Path]::GetTempPath()).TrimEnd('\')) { throw 'Unexpected report temp path.' }
            Remove-Item -LiteralPath $reportRoot -Recurse -Force
        }
    }
    Check 'locked evidence is retried and does not prevent other files being collected' {
        $module = Get-Module Lab
        & $module {
            $script:copied = [Collections.Generic.List[string]]::new()
            $script:attempts = 0
            function script:Invoke-Command { param($Session, $ScriptBlock, $ArgumentList) @('guest.log', 'result.json', 'nested\screen.png') }
            function script:Copy-Item {
                param($FromSession, $LiteralPath, $Destination, [switch]$Force, $ErrorAction)
                if ($LiteralPath -like '*guest.log') { $script:attempts++; throw 'locked' }
                $script:copied.Add($LiteralPath)
            }
            function script:Start-Sleep { param($Milliseconds) }
            function script:New-Item { param($ItemType, $Path, [switch]$Force) }
            $errors = @(Copy-LabEvidence -Session 'mock' -GuestRoot 'C:\guest' -Destination 'C:\evidence')
            if ($errors.Count -ne 1 -or $script:attempts -ne 3 -or $script:copied.Count -ne 2) { throw 'Evidence recovery regression.' }
        }
        Import-Module "$PSScriptRoot\Lab.psm1" -Force
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
    Check 'desktop bridges compile and keyboard input layout matches Windows ABI' {
        Add-Type -Path "$PSScriptRoot\support\Desktop.cs"
        Add-Type -Path "$PSScriptRoot\support\ScenarioDesktop.cs" -ReferencedAssemblies System.Drawing
        $contrastType = [CCUMScenarioDesktop].GetNestedType('HighContrast', [Reflection.BindingFlags]::NonPublic)
        Require ($contrastType.GetField('Scheme').FieldType -eq [IntPtr])
        Require ([Runtime.InteropServices.Marshal]::SizeOf([Activator]::CreateInstance($contrastType)) -eq (8 + [IntPtr]::Size))
        $iconType = [CCUMScenarioDesktop].GetNestedType('IconId', [Reflection.BindingFlags]::NonPublic)
        $iconSize = if ([IntPtr]::Size -eq 8) { 40 } else { 28 }
        Require ([Runtime.InteropServices.Marshal]::SizeOf([Activator]::CreateInstance($iconType)) -eq $iconSize)
        $source = Get-Content -LiteralPath "$PSScriptRoot\cases\portable-dashboard-warp.ps1" -Raw
        $match = [regex]::Match($source, "(?s)Add-Type -TypeDefinition @'\r?\n(.*?)\r?\n'@")
        Require $match.Success
        Add-Type -TypeDefinition $match.Groups[1].Value
        $inputType = [CCUMWarpDesktop].GetNestedType('INPUT', [Reflection.BindingFlags]::NonPublic)
        $expectedSize = if ([IntPtr]::Size -eq 8) { 40 } else { 28 }
        Require ([Runtime.InteropServices.Marshal]::SizeOf([Activator]::CreateInstance($inputType)) -eq $expectedSize)
    }
    Check 'blank dashboard content cannot pass using its decorated frame' {
        $bitmap = [Drawing.Bitmap]::new(320,240)
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.Clear([Drawing.Color]::White)
            foreach ($i in 0..11) {
                $pen = [Drawing.Pen]::new([Drawing.Color]::FromArgb($i*18,$i*18,$i*18))
                try { $graphics.DrawRectangle($pen,$i,$i,319-2*$i,239-2*$i) } finally { $pen.Dispose() }
            }
            $blank = [CCUMScenarioDesktop]::MeasureContent($bitmap)
            Require ($blank.DistinctColors -eq 1 -and $blank.ForegroundPixels -eq 0 -and $blank.Contrast -eq 1)
            foreach ($i in 0..9) {
                $brush = [Drawing.SolidBrush]::new([Drawing.Color]::FromArgb($i*12,$i*12,$i*12))
                try { $graphics.FillRectangle($brush,50+$i*5,60,4,50) } finally { $brush.Dispose() }
            }
            $painted = [CCUMScenarioDesktop]::MeasureContent($bitmap)
            Require ($painted.DistinctColors -ge 8 -and $painted.ForegroundPixels -ge 20 -and $painted.Contrast -ge 1.5)
        } finally { $graphics.Dispose(); $bitmap.Dispose() }
    }
    Check 'all ten scenarios are registered and nightly is isolated' {
        $catalog = Read-LabCatalog
        $added = @('first-run-clean-profile','explorer-restart-recovery','portable-update-failures','settings-resilience','startup-with-windows','display-scale-change','system-theme-switch','network-and-auth-errors','locale-and-theme-gallery','soak-and-power')
        foreach ($id in $added) { Require ($id -in $catalog.tests.id) }
        $nightly = @(Get-LabPlan $config -Suite nightly)
        Require ($nightly.Count -eq 2)
        Require (@($nightly | Where-Object flow -NE 'soak-and-power').Count -eq 0)
        Require (@(Get-LabPlan $config -Suite smoke,regression | Where-Object flow -EQ 'soak-and-power').Count -eq 0)
        Require ($nightly[0].test.timeoutSeconds -gt 3600)
    }
    Check 'Explorer recovery covers every supported layout' {
        $plan = @(Get-LabPlan $config -Tests explorer-restart-recovery)
        Require (@($plan | Where-Object status -EQ 'pending').Count -eq 6)
        Require (@($plan | Where-Object status -EQ 'skipped').Count -eq 2)
    }
    Check 'gallery fixtures cover current locales and built-ins' {
        $source = Get-Content "$PSScriptRoot\cases\locale-and-theme-gallery.ps1" -Raw
        $verticalSource = Get-Content "$PSScriptRoot\cases\vertical-taskbar.ps1" -Raw
        $locales = @(Get-ChildItem "$PSScriptRoot\..\..\src\localization\locales" -Filter '*.toml')
        Require ($locales.Count -eq 14)
        foreach ($locale in $locales) { Require ($source.Contains("'$($locale.BaseName)'")) }
        $engine = Get-Content "$PSScriptRoot\..\..\src\theme_engine.rs" -Raw
        $builtins = [regex]::Match($engine, '(?s)const BUILTIN_THEME_SOURCES.*?= &\[(.*?)\];').Groups[1].Value
        $files = [regex]::Matches($builtins, '/([a-z0-9-]+)\.json')
        Require ($files.Count -eq 3)
        foreach ($file in $files) {
            $id = $file.Groups[1].Value
            Require ($source.Contains("'$id'") -or $verticalSource.Contains("$id.json"))
        }
    }
    Check 'catalogue rejects arbitrary host actions and excessive timeouts' {
        foreach ($mutation in @(
            { param($c) $c.tests[0] | Add-Member NoteProperty hostActions @('exec') -Force },
            { param($c) $c.tests[0] | Add-Member NoteProperty hostActions 'reboot' -Force },
            { param($c) $c.tests[0].timeoutSeconds=7201 }
        )) {
            $catalog = Read-LabCatalog
            & $mutation $catalog
            $catalog | ConvertTo-Json -Depth 8 | Set-Content $temporary
            Expect-Throw { Read-LabCatalog $temporary } ''
        }
    }
    Check 'host actions reject stale identities and undeclared actions' {
        . "$PSScriptRoot\Host.Actions.ps1"
        $scenario = @(Get-LabPlan $config -Tests startup-with-windows)[0]
        $runId = 'a' * 32
        $request = [pscustomobject]@{id=('b'*32); runId=$runId; scenarioId=$scenario.id; action='reboot'}
        Assert-HostRequest $request $scenario $runId
        $request.runId = 'c' * 32
        Expect-Throw { Assert-HostRequest $request $scenario $runId } 'identity mismatch'
        $request.runId = $runId; $request.scenarioId = 'other-scenario'
        Expect-Throw { Assert-HostRequest $request $scenario $runId } 'identity mismatch'
        $request.scenarioId = $scenario.id; $request.action = 'network-disconnect'
        Expect-Throw { Assert-HostRequest $request $scenario $runId } 'not permitted'
        $request.action = 'reboot'; $request.id = '..\escape'
        Expect-Throw { Assert-HostRequest $request $scenario $runId } 'identity mismatch'
    }
    Check 'pause hook resumes the same VM even when the wait fails' {
        & {
            . "$PSScriptRoot\Host.Actions.ps1"
            $runId = 'a'*32
            $scenario = @(Get-LabPlan $config -Tests soak-and-power)[0]
            $request = [pscustomobject]@{id=('b'*32); runId=$runId; scenarioId=$scenario.id; action='pause-resume'}
            $target = [pscustomobject]@{VM='verified-vm'}
            $state = @{}; $session = 'mock'
            $script:paused = $null; $script:resumed = $null
            function Suspend-VM { param($VM) $script:paused = $VM }
            function Resume-VM { param($VM) $script:resumed = $VM }
            function Remove-PSSession { param($Session) }
            function Start-Sleep { param($Seconds) throw 'simulated interruption' }
            Expect-Throw { Invoke-LabHostRequest $request $scenario $target ([ref]$session) $null 'unused' $state } 'simulated interruption'
            Require ($script:paused -eq 'verified-vm' -and $script:resumed -eq 'verified-vm')
        }
    }
    Check 'pause hook replaces its transport before responding to the guest' {
        & {
            . "$PSScriptRoot\Host.Actions.ps1"
            $runId = 'a'*32
            $scenario = @(Get-LabPlan $config -Tests soak-and-power)[0]
            $request = [pscustomobject]@{id=('b'*32); runId=$runId; scenarioId=$scenario.id; action='pause-resume'}
            $target = [pscustomobject]@{VM=[pscustomobject]@{Id='verified-id'}}
            $session='old'; $state=@{}; $script:events=[Collections.Generic.List[string]]::new()
            function Remove-PSSession { param($Session) $script:events.Add("remove:$Session") }
            function Suspend-VM { param($VM) $script:events.Add('pause') }
            function Resume-VM { param($VM) $script:events.Add('resume') }
            function Start-Sleep { param($Seconds) }
            function New-PSSession { param($VMId,$Credential,$ErrorAction) Require ($VMId -eq 'verified-id'); $script:events.Add('connect'); 'new' }
            function Invoke-Command { param($Session,$ScriptBlock,$ArgumentList) $script:events.Add("respond:$Session") }
            Invoke-LabHostRequest $request $scenario $target ([ref]$session) $null 'unused' $state
            Require ($session -eq 'new' -and ($script:events -join ',') -eq 'remove:old,pause,resume,connect,respond:new')
        }
    }
    Check 'network hook restores switch names even if Hyper-V mutates adapter objects' {
        & {
            . "$PSScriptRoot\Host.Actions.ps1"
            $runId = 'a'*32
            $scenario = @(Get-LabPlan $config -Tests network-and-auth-errors)[0]
            $request = [pscustomobject]@{id=('b'*32); runId=$runId; scenarioId=$scenario.id; action='network-disconnect'}
            $target = [pscustomobject]@{VM='verified-vm'}
            $script:adapter = [pscustomobject]@{Id='nic-1'; SwitchId=[guid]::NewGuid(); SwitchName='Original Switch'}
            $script:connectedSwitch = $null
            function Get-VMNetworkAdapter { param($VM) $script:adapter }
            function Disconnect-VMNetworkAdapter { param($VMNetworkAdapter) $VMNetworkAdapter.SwitchName='' }
            function Connect-VMNetworkAdapter { param($VMNetworkAdapter,$SwitchName) $script:connectedSwitch=$SwitchName }
            function Invoke-Command { param($Session,$ScriptBlock,$ArgumentList) }
            $session='mock'; $state=@{}
            Invoke-LabHostRequest $request $scenario $target ([ref]$session) $null 'unused' $state
            Require ($state.adapters[0].SwitchName -eq 'Original Switch')
            $request.action='network-connect'
            Invoke-LabHostRequest $request $scenario $target ([ref]$session) $null 'unused' $state
            Require ($script:connectedSwitch -eq 'Original Switch' -and -not $state.ContainsKey('adapters'))
        }
    }
    Check 'display configuration is deferred and restored only while powered off' {
        & {
            . "$PSScriptRoot\Host.Actions.ps1"
            $runId = 'a'*32
            $scenario = @(Get-LabPlan $config -Tests display-scale-change)[0]
            $request = [pscustomobject]@{id=('b'*32); runId=$runId; scenarioId=$scenario.id; action='display-modes'}
            $target = [pscustomobject]@{VM=[pscustomobject]@{Id='verified-vm'}}
            $script:video = [pscustomobject]@{ResolutionType='Single'; HorizontalResolution=1920; VerticalResolution=1080}
            $script:power = 'Running'; $script:videoWrites = 0
            function Get-VMVideo { param($VM) $script:video }
            function Get-VM { param($Id) [pscustomobject]@{State=$script:power} }
            function Stop-VM { param($VM,[switch]$TurnOff,[switch]$Confirm) Require $TurnOff; $script:power='Off' }
            function Set-VMVideo {
                param($VM,$ResolutionType,$HorizontalResolution,$VerticalResolution)
                Require ($script:power -eq 'Off' -and $ResolutionType -eq 'Single')
                $script:videoWrites++
            }
            function Invoke-Command { param($Session,$ScriptBlock,$ArgumentList) }
            $session='mock'; $state=@{}
            Invoke-LabHostRequest $request $scenario $target ([ref]$session) $null 'unused' $state
            Require ($state.displayModesPending -and $script:videoWrites -eq 0)
            $script:video.ResolutionType='Maximum'
            Restore-LabHostState $target $state
            Require ($script:videoWrites -eq 1 -and $state.video.ResolutionType -eq 'Single')
        }
    }
    Check 'a locked progress snapshot cannot change assertion outcomes' {
        & {
            $ast = [Management.Automation.Language.Parser]::ParseFile("$PSScriptRoot\support\Guest.Helpers.ps1", [ref]$null, [ref]$null)
            $definition = $ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Assert-Check'}, $true)
            . ([scriptblock]::Create($definition.Extent.Text))
            $Context = [pscustomobject]@{Checks=[Collections.Generic.List[object]]::new()}
            $evidence = Join-Path ([IO.Path]::GetTempPath()) ('ccum-progress-' + [guid]::NewGuid().ToString('N'))
            $null = New-Item -ItemType Directory $evidence
            $lock = [IO.File]::Open("$evidence\progress.json", 'Create', 'ReadWrite', 'None')
            try {
                Assert-Check 'real pass' $true 'kept'
                Expect-Throw { Assert-Check 'real failure' $false 'kept' } 'Assertion failed: real failure'
                Require ($Context.Checks.Count -eq 2 -and $Context.Checks[0].passed -and -not $Context.Checks[1].passed)
                Assert-Check 'retained nonfatal failure' $false 'kept' -ContinueOnFailure
                Require ($Context.Checks.Count -eq 3 -and -not $Context.Checks[2].passed)
            } finally {
                $lock.Dispose()
                if ((Split-Path -Parent ([IO.Path]::GetFullPath($evidence))) -ne ([IO.Path]::GetTempPath()).TrimEnd('\')) { throw 'Unexpected progress temp path.' }
                Remove-Item -LiteralPath $evidence -Recurse -Force
            }
        }
    }
    Check 'gallery is embedded in summary and unsafe paths are rejected' {
        $reportRoot = Join-Path ([IO.Path]::GetTempPath()) ('ccum-report-' + [guid]::NewGuid().ToString('N'))
        $null = New-Item -ItemType Directory -Path $reportRoot
        try {
            $plan = @(Get-LabPlan $config -Tests locale-and-theme-gallery)
            $results = @(New-LabResults $plan $reportRoot)
            $dir = "$($results[0].evidence)\evidence"
            $null = New-Item -ItemType Directory $dir -Force
            '[{"label":"Japanese / classic","file":"gallery-ja-classic.png"}]' | Set-Content "$dir\gallery.json"
            Write-LabReport $results $reportRoot
            Require ((Get-Content "$reportRoot\summary.md" -Raw).Contains('![Japanese / classic]'))
            '[{"label":"escape","file":"../../outside.png"}]' | Set-Content "$dir\gallery.json"
            Expect-Throw { Write-LabReport $results $reportRoot } 'Invalid gallery filename'
        } finally {
            if ((Split-Path -Parent ([IO.Path]::GetFullPath($reportRoot))) -ne ([IO.Path]::GetTempPath()).TrimEnd('\')) { throw 'Unexpected report temp path.' }
            Remove-Item -LiteralPath $reportRoot -Recurse -Force
        }
    }
} finally {
    if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary }
    Remove-Module Lab -ErrorAction SilentlyContinue
}
Write-Host "$script:count harness checks passed. VM scenarios have not been executed."
