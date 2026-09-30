param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
Add-Type -Path "$Root\support\TaskbarControls.cs"

function Stop-App {
    # Normal teardown releases the taskbar child and tray callbacks before the
    # next launch. Retain forced cleanup for the unchanged PR's hung drop path.
    $processes = @(Get-AppProcesses)
    foreach ($process in $processes) {
        foreach ($window in @([CCUMLabDesktop]::WindowsForProcess($process.Id) | Where-Object Visible)) {
            [CCUMScenarioDesktop]::Post($window.Handle, 0x10, 0, 0)
        }
    }
    foreach ($process in $processes) {
        if (-not $process.WaitForExit(5000)) {
            try { Stop-Process -Id $process.Id -Force }
            catch { if (-not $process.HasExited) { throw } }
            if (-not $process.WaitForExit(10000)) { throw 'Monitor did not exit after forced cleanup.' }
        }
    }
}

function Start-LockApp([string]$Executable) {
    # Give shell callbacks from the preceding subcase time to settle.
    Start-Sleep -Milliseconds 750
    $null = Start-Process -FilePath $Executable -ArgumentList '--diagnose --no-poll' -WindowStyle Hidden
    Assert-App $Executable 'lock-launch'
    Assert-Check 'lock widget ready' (Wait-Condition { Widget-Ready } 40) $Executable
}
function Widget-Ready {
    try { $widget = Get-Widget } catch { return $false }
    return [CCUMScenarioDesktop]::Responding($widget.Handle)
}
function Lock-Settings([bool]$Locked) {
    @{language='en'; poll_interval_ms=900000; lock_taskbar=$Locked;
      custom_theme_enabled=$true; active_theme_path=$themePath;
      placement_override=@{nest='taskbar'; monitor_index=0; screen_x=$collisionX; screen_y=0; tray_offset=0}}
}
function Toggle-LockMenu([bool]$Expected = $true) {
    # The widget and tray root both open the same Classic native menu. Use the
    # widget's real right-click handler so shell overflow does not hide input.
    # Re-read bounds after style/placement changes have finished. A watchdog
    # move can still race physical input, so retry the click without changing settings.
    $opened = $false
    for ($attempt=0; $attempt -lt 3 -and -not $opened; $attempt++) {
        Assert-Check 'lock menu target ready' (Wait-Condition { Widget-Ready } 40) 'widget'
        [CCUMScenarioDesktop]::RightClick((Get-Widget).Handle)
        $opened = Wait-Condition { [CCUMScenarioDesktop]::MenuVisible() } 10
        if (-not $opened) { [CCUMScenarioDesktop]::DismissShellFlyout() }
    }
    Assert-Check 'lock menu opened' $opened 'native menu'
    [CCUMScenarioDesktop]::ChooseMenuItem('Lock in taskbar')
    Assert-Check 'lock menu closes after selection' (Wait-Condition { -not [CCUMScenarioDesktop]::MenuVisible() } 10) 'native menu'
    $savedState = { (Get-Content $settingsPath -Raw | ConvertFrom-Json).lock_taskbar -eq $Expected }.GetNewClosure()
    Assert-Check 'lock menu saved expected state' (Wait-Condition $savedState 10) $Expected
}

# Use an actual application task button and a measured one-pixel overlap.
# This exercises Explorer's accessibility sampler and watchdog, rather than
# posting the auto-eject message or changing window parenting on its behalf.
if ($request.os -eq 'windows10') { Set-TestTaskbar -PromoteTray }
# Keep shell/UIA requests responsive while the scenario thread waits.
$fixturePath = "$Root\collision-fixture.ps1"
@'
Add-Type -AssemblyName System.Windows.Forms
$form = [Windows.Forms.Form]::new()
$form.Text = 'CCUM collision fixture'
$form.Width = 320; $form.Height = 180
$form.ShowInTaskbar = $true
[Windows.Forms.Application]::Run($form)
'@ | Set-Content $fixturePath -Encoding ASCII
$fixture = Start-Process powershell.exe -ArgumentList "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$fixturePath`"" -PassThru
try {
    $ready = [Diagnostics.Stopwatch]::StartNew()
    do {
        if ($fixture.HasExited) { throw 'Collision fixture exited before its task button was ready.' }
        $controls = @([CCUMTaskbarControls]::Read([CCUMScenarioDesktop]::TaskbarHandle().ToInt64()))
        # Win11 can group PowerShell/Form windows under the default Terminal.
        $button = @($controls | Where-Object { -not $_.offscreen -and $_.role -in 50000,50007 -and $_.name -match 'CCUM collision fixture|(?:Windows PowerShell|Terminal).*running' } | Sort-Object { $_.Rect.Right } -Descending | Select-Object -First 1)
        if ($button.Count -eq 1) { break }
        Start-Sleep -Milliseconds 500
    } while ($ready.Elapsed.TotalSeconds -lt 25)
    $controls | ConvertTo-Json -Depth 6 | Set-Content "$evidence\taskbar-controls.json" -Encoding UTF8
    Assert-Check 'real application task button measured' ($button.Count -eq 1) $button
    $bar = [CCUMScenarioDesktop]::Taskbar()
    $collisionX = [int]$button[0].Rect.Right - 1 - $bar.Left
    # A compact fixture fits both the 40px Win10 and 48px Win11 taskbars.
    # Include a real theme tray root so its icon ID is discoverable on both OSes.
    $themePath = "$Root\lock-fixture.json"
    @{
        schema_version=1; id='lock-fixture'; name='Taskbar lock regression'
        surfaces=@(
            @{id='main'; name='Lock widget'; width='120'; height='32';
              background=@{type='colour'; colour=@{color='#2080D0FF'}};
              mouse_events=@{right_click='show_context_menu("classic-v1")'};
              placement=@{reference=@{region='system_tray'; display=0}; nest='taskbar'; horizontal='left'; vertical='bottom'; surface_horizontal='right'; surface_vertical='bottom'}},
            @{id='tray'; name='Lock tray'; width='64'; height='64';
              background=@{type='colour'; colour=@{color='#2080D0FF'}};
              mouse_events=@{right_click='show_context_menu("classic-v1")'};
              placement=@{reference=@{region='system_tray'; display=0}; nest='tray_icon'}}
        )
    } | ConvertTo-Json -Depth 10 | Set-Content $themePath -Encoding ASCII
    $executable = Install-PortableApp

    Invoke-Subcase 'baseline-collision' {
        Write-Settings (Lock-Settings $false)
        Start-LockApp $executable
        Assert-Check 'baseline auto-ejects on one-pixel collision' (Wait-Condition { (Get-Widget).Parent -eq 0 } 20) (Get-Widget)
        Assert-Check 'baseline records real watchdog ejection' ((Get-Content $logPath -Raw) -match 'taskbar collision: auto-ejected') 'diagnostic log'
        Start-Sleep -Seconds 3
        Assert-Check 'baseline remains floating while blocked' ((Get-Widget).Parent -eq 0) (Get-Widget)
        Save-Screenshot 'baseline-collision'
    }
    if ($request.flow -eq 'taskbar-collision-baseline') { Complete-Subcases; return }
    Invoke-Subcase 'enable-while-ejected' {
        Write-Settings (Lock-Settings $false)
        Start-LockApp $executable
        Assert-Check 'unlocked candidate auto-ejects' (Wait-Condition { (Get-Widget).Parent -eq 0 } 20) (Get-Widget)
        Save-Screenshot 'candidate-unlocked'
        Toggle-LockMenu
        Assert-Check 'enabling lock immediately redocks' (Wait-Condition { [CCUMScenarioDesktop]::IsTaskbarChild((Get-Widget).Handle) } 5) (Get-Widget)
        Start-Sleep -Seconds 8
        Assert-Docked 'locked despite collision'
        Save-Screenshot 'candidate-locked'
        Stop-App
        Start-LockApp $executable
        Start-Sleep -Seconds 8
        Assert-Docked 'lock survives restart'
        Assert-Check 'saved lock survives restart' ((Get-Content $settingsPath -Raw | ConvertFrom-Json).lock_taskbar) $settingsPath
        Toggle-LockMenu $false
        Assert-Check 'unlock restores watchdog ejection' (Wait-Condition { (Get-Widget).Parent -eq 0 } 20) (Get-Widget)
        Toggle-LockMenu
        Assert-Check 're-enabling lock redocks again' (Wait-Condition { [CCUMScenarioDesktop]::IsTaskbarChild((Get-Widget).Handle) } 5) (Get-Widget)
    }
    Invoke-Subcase 'locked-drag' {
        $settings = Lock-Settings $true
        $settings.Remove('placement_override')
        Write-Settings $settings
        Start-LockApp $executable
        Start-Sleep -Seconds 5
        $before = Get-Widget
        [CCUMLabDesktop]::PhysicalDragTo($before, $before.Left+15-80, ($before.Top+$before.Bottom)/2)
        Start-Sleep -Seconds 3
        Assert-Docked 'locked drag along taskbar'
        $after = Get-Widget
        Assert-Check 'locked widget remains movable' ([Math]::Abs($after.Left-$before.Left) -gt 20) @{before=$before; after=$after}
        $saved = (Get-Content $settingsPath -Raw | ConvertFrom-Json).placement_override
        # Capture the destination before the drag. Avoid querying Explorer's
        # tray geometry while the widget holds mouse capture.
        $bar = [CCUMScenarioDesktop]::Taskbar()
        [CCUMLabDesktop]::PhysicalDragTo($after, $after.Left+15, $bar.Top-300)
        Assert-Check 'outside drop stays responsive' (Wait-Condition { [CCUMScenarioDesktop]::Responding($after.Handle) } 10) $after.Handle
        Assert-Check 'outside drop snaps back' (Wait-Condition { [CCUMScenarioDesktop]::IsTaskbarChild((Get-Widget).Handle) } 10) (Get-Widget)
        Assert-Docked 'locked outside drop'
        Assert-Check 'outside drop retains saved dock placement' (((Get-Content $settingsPath -Raw | ConvertFrom-Json).placement_override | ConvertTo-Json -Compress) -eq ($saved | ConvertTo-Json -Compress)) $saved
        Save-Screenshot 'locked-outside-drop'
        Assert-NoPanic 'taskbar lock'
    }
    Invoke-Subcase 'settings-enable-while-ejected' {
        Write-Settings (Lock-Settings $false)
        Start-LockApp $executable
        Assert-Check 'settings candidate auto-ejects' (Wait-Condition { (Get-Widget).Parent -eq 0 } 20) (Get-Widget)
        Write-Settings (Lock-Settings $true)
        Send-WidgetMessage 0x8005
        Assert-Check 'external settings enable redocks' (Wait-Condition { [CCUMScenarioDesktop]::IsTaskbarChild((Get-Widget).Handle) } 5) (Get-Widget)
        Start-Sleep -Seconds 8
        Assert-Docked 'settings lock despite collision'
        Save-Screenshot 'settings-locked'
    }
    Complete-Subcases
} finally {
    if (-not $fixture.HasExited) {
        # Post to the owned Form, avoiding a synchronous console/window close.
        [CCUMScenarioDesktop]::CloseWindow($fixture.Id, 'CCUM collision fixture')
        if (-not $fixture.WaitForExit(5000)) {
            try { Stop-Process -Id $fixture.Id -Force }
            catch { if (-not $fixture.HasExited) { throw } }
            if (-not $fixture.WaitForExit(10000)) { throw 'Collision fixture did not exit.' }
        }
    }
    $fixture.Dispose()
}
