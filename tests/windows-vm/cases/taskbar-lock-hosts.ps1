param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
if ($request.os -eq 'windows10') { Set-TestTaskbar -PromoteTray }
$executable = Install-PortableApp

function Stop-App {
    foreach ($process in @(Get-AppProcesses)) {
        $owner = @([CCUMScenarioDesktop]::HostWindows($process.Id) | Where-Object Class -EQ 'ClaudeCodeUsageMonitor')
        if ($owner.Count -eq 1) { [CCUMScenarioDesktop]::Post($owner[0].Handle, 0x10, 0, 0) }
        if (-not $process.WaitForExit(5000)) {
            try { Stop-Process -Id $process.Id -Force }
            catch { if (-not $process.HasExited) { throw } }
            if (-not $process.WaitForExit(10000)) { throw 'Host monitor did not exit.' }
        }
    }
}

function Host-Windows { @([CCUMScenarioDesktop]::HostWindows((Get-AppProcesses)[0].Id)) }
function Owner-Window { @(Host-Windows | Where-Object Class -EQ 'ClaudeCodeUsageMonitor')[0] }
function Host-Ready([string]$Kind) {
    $visible = @(Host-Windows | Where-Object Visible)
    switch ($Kind) {
        'tray_icon' { return $visible.Count -eq 0 -and [CCUMScenarioDesktop]::TrayStatus((Owner-Window).Handle, 1000) -eq 0 }
        'desktop' { return $visible.Count -eq 1 -and $visible[0].Class -eq 'CCUMDesktopSurface' -and $visible[0].Parent -ne 0 }
        'floating' { return $visible.Count -eq 1 -and $visible[0].Parent -eq 0 }
        'taskbar' { return $visible.Count -eq 1 -and [CCUMScenarioDesktop]::IsTaskbarChild($visible[0].Handle) }
    }
    return $false
}
function Assert-Host([string]$HostKind, [string]$Label) {
    Assert-Check "$Label presentation ready" (Wait-Condition { Host-Ready $HostKind } 40) $HostKind
    $windows = @(Host-Windows)
    $visible = @($windows | Where-Object Visible)
    switch ($HostKind) {
        'tray_icon' {
            Assert-Check "$Label no visible widget" ($visible.Count -eq 0) $windows
            Assert-Check "$Label real tray registration" ([CCUMScenarioDesktop]::TrayStatus((Owner-Window).Handle, 1000) -eq 0) 'theme tray root'
        }
        'desktop' {
            Assert-Check "$Label desktop presenter" ($visible.Count -eq 1 -and $visible[0].Class -eq 'CCUMDesktopSurface') $windows
            Assert-Check "$Label desktop parent" ($visible[0].Parent -ne 0 -and -not [CCUMScenarioDesktop]::IsTaskbarChild($visible[0].Handle)) $visible[0]
        }
        'floating' {
            Assert-Check "$Label floating popup" ($visible.Count -eq 1 -and $visible[0].Parent -eq 0) $windows
        }
        'taskbar' { Assert-Docked $Label }
    }
    foreach ($window in $visible) {
        Assert-Check "$Label responsive" (Wait-Condition { [CCUMScenarioDesktop]::Responding($window.Handle) } 40) $window.Handle
    }
}
function Start-HostApp([string]$Kind) {
    Start-Sleep -Milliseconds 750
    $null = Start-Process $executable -ArgumentList '--diagnose --no-poll' -WindowStyle Hidden
    Assert-Check 'host app single instance' (Wait-Condition { @(Get-AppProcesses).Count -eq 1 } 15) $executable
    # Restored Windows 11 guests can take over a minute to create the first window.
    Assert-Check 'host owner created' (Wait-Condition { @(Host-Windows | Where-Object Class -EQ 'ClaudeCodeUsageMonitor').Count -eq 1 } 90) $executable
    # --no-poll deliberately skips initial tray registration. A settings reload
    # registers the theme icon without requesting provider polling.
    if ($Kind -eq 'tray_icon') { [CCUMScenarioDesktop]::Post((Owner-Window).Handle, 0x8005, 0, 0) }
    # The owner HWND is created before host presenters and initial positioning.
    Assert-Check 'host presentation ready' (Wait-Condition { Host-Ready $Kind } 90) $Kind
    Start-Sleep -Seconds 2
}

foreach ($variant in @('taskbar', 'tray_icon', 'desktop', 'floating', 'floating-override')) {
    Invoke-Subcase $variant {
        $hostKind = if ($variant -eq 'floating-override') { 'floating' } else { $variant }
        $authoredHost = if ($variant -eq 'floating-override') { 'taskbar' } else { $hostKind }
        $themePath = "$Root\host-$variant.json"
        $placement = @{nest=$authoredHost; reference=@{region='monitor'; display=0}; horizontal='left'; vertical='top'; surface_horizontal='left'; surface_vertical='top'; offset_x=300; offset_y=250}
        if ($authoredHost -eq 'taskbar') {
            $placement = @{nest='taskbar'; reference=@{region='system_tray'; display=0}; horizontal='left'; vertical='bottom'; surface_horizontal='right'; surface_vertical='bottom'; offset_x=-80}
        }
        @{
            schema_version=1; id="host-$variant"; name="Host $variant"
            surfaces=@(@{id='main'; name='Host'; width='120'; height='32'; placement=$placement;
                background=@{type='colour'; colour=@{color='#2080D0FF'}}})
        } | ConvertTo-Json -Depth 12 | Set-Content $themePath -Encoding ASCII
        $settings = @{language='en'; poll_interval_ms=900000; lock_taskbar=$false; custom_theme_enabled=$true; active_theme_path=$themePath}
        if ($variant -eq 'floating-override') {
            $settings.placement_override=@{nest='floating'; monitor_index=0; screen_x=400; screen_y=300; tray_offset=0}
        }
        Write-Settings $settings
        Start-HostApp $hostKind
        Assert-Host $hostKind "$variant unlocked"
        $settings.lock_taskbar=$true
        Write-Settings $settings
        [CCUMScenarioDesktop]::Post((Owner-Window).Handle, 0x8005, 0, 0)
        Start-Sleep -Seconds 3
        Assert-Host $hostKind "$variant locked"
        if ($hostKind -in @('taskbar', 'floating')) {
            $before = Get-Widget
            [CCUMLabDesktop]::PhysicalDragTo($before, 650, 550)
            Start-Sleep -Seconds 2
            Assert-Host $hostKind "$variant after drag"
            $after = Get-Widget
            if ($hostKind -eq 'floating') {
                Assert-Check "$variant can move while lock enabled" ([Math]::Abs($after.Left-$before.Left) -gt 20 -or [Math]::Abs($after.Top-$before.Top) -gt 20) @{before=$before;after=$after}
                Assert-Check "$variant saved floating host" ((Get-Content $settingsPath -Raw | ConvertFrom-Json).placement_override.nest -eq 'floating') $settingsPath
            }
        }
        $beforeRestart = @(Host-Windows | Where-Object Visible)
        Stop-App
        Start-HostApp $hostKind
        Assert-Host $hostKind "$variant restart"
        Assert-Check "$variant saved lock" ((Get-Content $settingsPath -Raw | ConvertFrom-Json).lock_taskbar) $settingsPath
        if ($hostKind -eq 'floating') {
            $afterRestart = Get-Widget
            Assert-Check "$variant retains dragged position after restart" ([Math]::Abs($afterRestart.Left-$beforeRestart[0].Bounds.Left) -le 2 -and [Math]::Abs($afterRestart.Top-$beforeRestart[0].Bounds.Top) -le 2) $afterRestart
        }
        $saved = Get-Content $settingsPath -Raw | ConvertFrom-Json
        $saved.lock_taskbar=$false
        Write-Settings $saved
        [CCUMScenarioDesktop]::Post((Owner-Window).Handle, 0x8005, 0, 0)
        Start-Sleep -Seconds 2
        Assert-Host $hostKind "$variant unlocked again"
        Save-Screenshot "host-$variant"
        Assert-NoPanic $variant
    }
}
Complete-Subcases
