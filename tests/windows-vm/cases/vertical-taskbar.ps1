param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
Assert-Check 'vertical taskbars supported' ($request.os -eq 'windows10') $request.os
$executable = Install-PortableApp
$gallery = [Collections.Generic.List[object]]::new()
Set-TestTaskbar -PromoteTray

function Stop-App {
    # Normal shutdown removes tray icons before the next screenshot launch.
    # Killing the process leaves stale Explorer slots that consume vertical space.
    foreach ($process in @(Get-AppProcesses)) {
        $owner = @([CCUMScenarioDesktop]::HostWindows($process.Id) | Where-Object Class -EQ 'ClaudeCodeUsageMonitor')
        if ($owner.Count -eq 1) { [CCUMScenarioDesktop]::Post($owner[0].Handle, 0x10, 0, 0) }
        Assert-Check 'normal monitor shutdown completed' ($process.WaitForExit(15000)) $process.Id
    }
    Start-Sleep -Seconds 2
}

function Set-TaskbarEdge([string]$Edge) {
    # The native taskbar menu and dragging are also available on disposable
    # unactivated lab installs whose Settings personalization controls are disabled.
    $bar = [CCUMScenarioDesktop]::Taskbar()
    $menuY = if (($bar.Right-$bar.Left) -gt ($bar.Bottom-$bar.Top)) {
        [int](($bar.Top+$bar.Bottom)/2)
    } else {
        $pointY = [int]($bar.Top + ($bar.Bottom-$bar.Top)/3)
        if (@(Get-AppProcesses).Count -eq 1) {
            $widget = Get-Widget
            if ($widget.Parent -ne 0) { $pointY = [Math]::Min($pointY, $widget.Top-4) }
        }
        $pointY
    }
    [CCUMScenarioDesktop]::RightClickPoint([int](($bar.Left+$bar.Right)/2), $menuY)
    Assert-Check 'native taskbar menu opens' (Wait-Condition { [CCUMScenarioDesktop]::MenuVisible() } 5) 'shell menu'
    if ([CCUMScenarioDesktop]::MenuItemChecked('Lock the taskbar')) {
        [CCUMScenarioDesktop]::ChooseMenuItem('Lock the taskbar')
        Assert-Check 'native unlock menu action completed' (Wait-Condition { -not [CCUMScenarioDesktop]::MenuVisible() } 5) 'native shell action'
    } else {
        # Clicking (10,10) to dismiss would open Start on a left taskbar.
        [Windows.Forms.SendKeys]::SendWait('{ESC}')
        Assert-Check 'native taskbar menu dismissed' (Wait-Condition { -not [CCUMScenarioDesktop]::MenuVisible() } 5) 'Escape'
    }
    $bar = [CCUMScenarioDesktop]::Taskbar()
    $screen = [Windows.Forms.SystemInformation]::VirtualScreen
    $start = [CCUMLabDesktop+WindowInfo]::new()
    if (($bar.Right-$bar.Left) -gt ($bar.Bottom-$bar.Top)) {
        $start.Left = [int](($bar.Left+$bar.Right)/2)-15
        $start.Top = $bar.Top; $start.Bottom = $bar.Bottom
    } else {
        # Use the empty strip below pinned apps and above the theme widget.
        $start.Left = [int](($bar.Left+$bar.Right)/2)-15
        $start.Top = $menuY-1
        $start.Bottom = $start.Top+2
    }
    $x = switch ($Edge) { 'Right' { $screen.Right-1 } 'Left' { $screen.Left+1 } default { [int](($screen.Left+$screen.Right)/2) } }
    $y = switch ($Edge) { 'Bottom' { $screen.Bottom-1 } 'Top' { $screen.Top+1 } default { [int](($screen.Top+$screen.Bottom)/2) } }
    [CCUMLabDesktop]::PhysicalDragTo($start, $x, $y)
    $applied = Wait-Condition {
        $bar = [CCUMScenarioDesktop]::Taskbar()
        if ($Edge -in @('Left','Right')) {
            ($bar.Right-$bar.Left) -lt ($bar.Bottom-$bar.Top) -and
                (($Edge -eq 'Right' -and $bar.Right -eq $screen.Right) -or ($Edge -eq 'Left' -and $bar.Left -eq $screen.Left))
        } else {
            ($bar.Right-$bar.Left) -gt ($bar.Bottom-$bar.Top) -and
                (($Edge -eq 'Bottom' -and $bar.Bottom -eq $screen.Bottom) -or ($Edge -eq 'Top' -and $bar.Top -eq $screen.Top))
        }
    } 20
    Assert-Check 'actual taskbar edge applied' $applied @{edge=$Edge; taskbar=[CCUMScenarioDesktop]::Taskbar()}
    [CCUMScenarioDesktop]::HoverPoint(400,200)
    Start-Sleep -Seconds 4
}

function Capture-Fallback([string]$Label, [string]$Description) {
    Assert-App $executable "$Label-desktop"
    Send-WidgetMessage 0x8005
    Assert-Check "$Label floating fallback" (Wait-Condition { (Get-Widget).Parent -eq 0 } 20) 'incompatible taskbar host'
    Start-Sleep -Seconds 6
    $w = Get-Widget
    $screen = [Windows.Forms.SystemInformation]::VirtualScreen
    $bar = [CCUMScenarioDesktop]::Taskbar()
    Assert-Check "$Label fully visible" ($w.Left -ge $screen.Left -and $w.Right -le $screen.Right -and $w.Top -ge $screen.Top -and $w.Bottom -le $screen.Bottom) $w
    Assert-Check "$Label beside taskbar" ($w.Right -le $bar.Left -or $w.Left -ge $bar.Right -or $w.Bottom -le $bar.Top -or $w.Top -ge $bar.Bottom) @{widget=$w; taskbar=$bar}
    Assert-Image $Label $w.Handle
    Assert-Tray $Label
    Save-Screenshot "$Label-desktop"
    $gallery.Add(@{label="$Description (widget)"; file="$Label.png"})
    $gallery.Add(@{label="$Description (desktop)"; file="$Label-desktop.png"})
    Assert-NoPanic $Label
}

function Select-VerticalTheme([string]$Path, [switch]$AllProviders, [switch]$LockTaskbar) {
    Write-Settings @{
        language='en'; poll_interval_ms=900000; custom_theme_enabled=$true; active_theme_path=$Path
        lock_taskbar=[bool]$LockTaskbar
        show_claude_code=$true; show_codex=[bool]$AllProviders; show_antigravity=[bool]$AllProviders
        show_opencode=[bool]$AllProviders; show_cursor=[bool]$AllProviders; show_grok=[bool]$AllProviders; show_copilot=[bool]$AllProviders
        last_update_check_unix=[DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    }
}

function Capture-Vertical([string]$Label, [string]$Description) {
    Assert-App $executable "$Label-desktop"
    # --no-poll skips initial tray registration. Reload the unchanged settings
    # through the app's normal message, as in the tray-host regression case.
    Send-WidgetMessage 0x8005
    Start-Sleep -Seconds 6
    $explorerPid = @(Get-Process explorer | Where-Object SessionId -EQ $sessionId)[0].Id
    $taskbarHandle = ([CCUMScenarioDesktop]::TaskbarHandle()).ToInt64()
    [CCUMScenarioDesktop]::HostWindows($explorerPid) | Where-Object Parent -EQ $taskbarHandle |
        ConvertTo-Json -Depth 6 | Set-Content "$evidence\$Label-shell-children.json" -Encoding UTF8
    Assert-Docked $Label
    $w = Get-Widget
    Assert-Image $Label $w.Handle
    Assert-Tray $Label
    $bar = [CCUMScenarioDesktop]::Taskbar()
    $dpi = [CCUMScenarioDesktop]::GetDpiForWindow([IntPtr]$w.Handle)
    @{widget=$w; taskbar=$bar; dpi=$dpi; sampleUsage=$Label.Contains('sample')} |
        ConvertTo-Json -Depth 6 | Set-Content "$evidence\$Label-geometry.json" -Encoding UTF8
    $null = [CCUMScenarioDesktop]::Capture(([CCUMScenarioDesktop]::TaskbarHandle()).ToInt64(), "$evidence\$Label-taskbar.png")
    Save-Screenshot "$Label-desktop"
    $gallery.Add(@{label="$Description (widget)"; file="$Label.png"})
    $gallery.Add(@{label="$Description (taskbar)"; file="$Label-taskbar.png"})
    $gallery.Add(@{label="$Description (desktop)"; file="$Label-desktop.png"})
    Assert-NoPanic $Label
}

function Test-LiveThemeSelection([string]$ClassicPath, [string]$VerticalPath) {
    $originalPid = (Get-AppProcesses)[0].Id
    $owner = @([CCUMScenarioDesktop]::HostWindows($originalPid) | Where-Object Class -EQ 'ClaudeCodeUsageMonitor')[0]
    $before = Get-Widget
    [CCUMScenarioDesktop]::Post($owner.Handle, 0x8005, 0, 0)
    Start-Sleep -Seconds 3
    $after = Get-Widget
    Assert-Check 'unchanged refresh retains fallback position' ($after.Parent -eq 0 -and $before.Left -eq $after.Left -and $before.Top -eq $after.Top) @{before=$before;after=$after}
    foreach ($kind in @('desktop', 'tray_icon', 'floating', 'floating-override', 'taskbar')) {
        if ($kind -eq 'taskbar') {
            Select-VerticalTheme $VerticalPath
        } else {
            $path = "$Root\live-host-$kind.json"
            $authoredHost = if ($kind -eq 'floating-override') { 'taskbar' } else { $kind }
            @{
                schema_version=1; id="live-host-$kind"; name="Live host $kind"
                surfaces=@(@{id='main'; name='Host'; width='120'; height='32';
                    placement=@{nest=$authoredHost; reference=@{region='monitor'; display=0}; horizontal='left'; vertical='top'; surface_horizontal='left'; surface_vertical='top'; offset_x=400; offset_y=300};
                    background=@{type='colour'; colour=@{color='#2080D0FF'}};
                    children=@(@{id='host-label'; name='Host label'; width='canvas.width'; height='32';
                        content=@{type='text'; template='Host'; font_family='Segoe UI'; font_size='12'; color=@{color='#FFFFFF'}}})})
            } | ConvertTo-Json -Depth 12 | Set-Content $path -Encoding ASCII
            Select-VerticalTheme $path
            if ($kind -eq 'floating-override') {
                $settings = Get-Content $settingsPath -Raw | ConvertFrom-Json
                $settings | Add-Member -NotePropertyName placement_override -NotePropertyValue @{nest='floating'; monitor_index=0; screen_x=500; screen_y=350; tray_offset=0}
                Write-Settings $settings
            }
        }
        [CCUMScenarioDesktop]::Post($owner.Handle, 0x8005, 0, 0)
        $ready = Wait-Condition {
            $visible = @([CCUMScenarioDesktop]::HostWindows($originalPid) | Where-Object Visible)
            switch ($kind) {
                'desktop' { $visible.Count -eq 1 -and $visible[0].Class -eq 'CCUMDesktopSurface' -and $visible[0].Parent -ne 0 }
                'tray_icon' { $visible.Count -eq 0 -and [CCUMScenarioDesktop]::TrayStatus($owner.Handle, 1000) -eq 0 }
                'taskbar' { $visible.Count -eq 1 -and [CCUMScenarioDesktop]::IsTaskbarChild($visible[0].Handle) }
                default {
                    $x = if ($kind -eq 'floating-override') { 500 } else { 400 }
                    $y = if ($kind -eq 'floating-override') { 350 } else { 300 }
                    $visible.Count -eq 1 -and $visible[0].Parent -eq 0 -and
                        [Math]::Abs($visible[0].Bounds.Left-$x) -le 2 -and [Math]::Abs($visible[0].Bounds.Top-$y) -le 2
                }
            }
        } 30
        Assert-Check "live selection from fallback honors $kind host and placement" $ready ([CCUMScenarioDesktop]::HostWindows($originalPid))
        Assert-Check "live $kind selection retains process" ((Get-AppProcesses)[0].Id -eq $originalPid) $originalPid
        $label = "live-$($kind.Replace('_','-'))-desktop"
        # Native position becomes observable before layered painting completes.
        Start-Sleep -Seconds 2
        foreach ($window in @([CCUMScenarioDesktop]::HostWindows($originalPid) | Where-Object Visible)) {
            Assert-Check "$label responsive" ([CCUMScenarioDesktop]::Responding($window.Handle)) $window.Handle
            Assert-Image "$label-widget" $window.Handle
        }
        Save-Screenshot $label
        $gallery.Add(@{label="Live selection from fallback / $kind"; file="$label.png"})
        Assert-NoPanic "live $kind selection"
        if ($kind -ne 'taskbar') {
            # Exercise every selection from an active capacity fallback.
            Select-VerticalTheme $ClassicPath
            [CCUMScenarioDesktop]::Post($owner.Handle, 0x8005, 0, 0)
            Assert-Check "live $kind returns to default fallback" (Wait-Condition {
                $visible = @([CCUMScenarioDesktop]::HostWindows($originalPid) | Where-Object Visible)
                $visible.Count -eq 1 -and $visible[0].Parent -eq 0 -and $visible[0].Bounds.Left -gt 1000
            } 30) 'right taskbar fallback'
        }
    }
}

try {
    # Install the actual managed theme before moving the taskbar.
    Write-Settings @{language='en'; poll_interval_ms=900000; last_update_check_unix=[DateTimeOffset]::UtcNow.ToUnixTimeSeconds()}
    $null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
    Assert-App $executable 'baseline-installed'
    Stop-App
    $themePath = "$(Split-Path $settingsPath)\themes\classic-vertical.json"
    Assert-Check 'managed Classic Vertical installed' (Test-Path $themePath) $themePath
    Set-TaskbarEdge 'Right'
    # Reproduce #155 with the default settings before selecting the new theme.
    $null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
    Capture-Fallback 'right-classic-default' 'Right edge / unchanged default theme / automatic floating fallback'
    $savedClassic = (Get-Content $settingsPath -Raw | ConvertFrom-Json).active_theme_path
    Assert-Check 'default Classic remains selected' ((Split-Path $savedClassic -Leaf) -eq 'classic-usage-widget.json') $savedClassic
    Test-LiveThemeSelection $savedClassic $themePath
    Stop-App
    Select-VerticalTheme "$(Split-Path $settingsPath)\themes\compact-fluent-quad.json" -LockTaskbar
    $null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
    Capture-Fallback 'right-compact-default' 'Right edge / Compact Fluent Quad / automatic floating fallback'
    Assert-Check 'visibility fallback retains taskbar lock' ((Get-Content $settingsPath -Raw | ConvertFrom-Json).lock_taskbar) 'locked incompatible theme remains visible'
    Stop-App
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
    Set-ItemProperty $key AppsUseLightTheme 0 -Type DWord
    Set-ItemProperty $key SystemUsesLightTheme 0 -Type DWord
    [CCUMScenarioDesktop]::BroadcastTheme()
    Select-VerticalTheme $themePath -AllProviders
    $null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
    Capture-Vertical 'right-dark-original' 'Right edge / dark / original built-in loading state'
    Stop-App

    # Screenshot fixture: alter only usage bindings in a separate copy. Window
    # geometry, fonts, provider layout and native hosting remain the real theme.
    # No credentials or service traffic are required for representative values.
    $fixture = Get-Content $themePath -Raw | ConvertFrom-Json
    $fixture.id = 'classic-vertical-sample'
    $fixture.name = 'Classic Vertical sample usage fixture'
    foreach ($object in $fixture.surfaces[0].children) {
        if ($object.id -match '^(claude|codex|antigravity|opencode|cursor|grok|copilot)-(session|weekly)-') {
            $provider = $Matches[1]; $window = $Matches[2]
            if ($object.content.type -eq 'progress') {
                $object.render = '1'
                $object.content.value = if ($window -eq 'session') { '42' } else { '69' }
            } elseif ($object.content.type -eq 'text') {
                $badge = '{' + $provider + '.' + $window + '.display:usage_badge}'
                $object.content.template = $object.content.template.Replace($badge, $(if ($window -eq 'session') { '42%' } else { '69%' }))
                $object.content.template = $object.content.template.Replace(('{' + $provider + '.weekly.label}'), $(if ($provider -in @('grok','copilot')) { '30d' } else { '7d' }))
            }
        }
    }
    $fixturePath = "$Root\vertical-sample.json"
    [IO.File]::WriteAllText($fixturePath, ($fixture | ConvertTo-Json -Depth 30), [Text.UTF8Encoding]::new($false))
    Copy-Item $fixturePath "$evidence\vertical-sample-theme.json"
    Select-VerticalTheme $fixturePath -AllProviders
    $null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
    Capture-Vertical 'right-dark-sample' 'Right edge / dark / all providers / sample usage'
    $originalPid = (Get-AppProcesses)[0].Id
    Set-ItemProperty $key AppsUseLightTheme 1 -Type DWord
    Set-ItemProperty $key SystemUsesLightTheme 1 -Type DWord
    [CCUMScenarioDesktop]::BroadcastTheme()
    Start-Sleep -Seconds 3
    Capture-Vertical 'right-light-sample' 'Right edge / light / all providers / sample usage'
    Assert-Check 'live theme change retained process' ((Get-AppProcesses)[0].Id -eq $originalPid) $originalPid
    Assert-Check 'light and dark pixels differ' ((Get-FileHash "$evidence\right-dark-sample.png").Hash -ne (Get-FileHash "$evidence\right-light-sample.png").Hash) 'native window screenshots'
    Set-TaskbarEdge 'Left'
    Capture-Vertical 'left-light-sample' 'Left edge / light / all providers / sample usage'
    Assert-Check 'live edge change retained process' ((Get-AppProcesses)[0].Id -eq $originalPid) $originalPid
    Stop-App
    $null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
    Capture-Vertical 'left-restarted-sample' 'Left edge / restart / saved theme / sample usage'
    $orientationPid = (Get-AppProcesses)[0].Id
    $savedTheme = (Get-Content $settingsPath -Raw | ConvertFrom-Json).active_theme_path
    Set-TaskbarEdge 'Bottom'
    Capture-Fallback 'bottom-vertical-sample' 'Bottom edge / Classic Vertical / automatic floating fallback / sample usage'
    Assert-Check 'horizontal edge change retained process' ((Get-AppProcesses)[0].Id -eq $orientationPid) $orientationPid
    Assert-Check 'fallback retains selected theme' ((Get-Content $settingsPath -Raw | ConvertFrom-Json).active_theme_path -eq $savedTheme) $savedTheme
    Set-ItemProperty $key AppsUseLightTheme 0 -Type DWord
    Set-ItemProperty $key SystemUsesLightTheme 0 -Type DWord
    [CCUMScenarioDesktop]::BroadcastTheme()
    Capture-Fallback 'bottom-dark-vertical-sample' 'Bottom edge / dark floating card / sample usage'
    Set-ItemProperty $key AppsUseLightTheme 1 -Type DWord
    Set-ItemProperty $key SystemUsesLightTheme 1 -Type DWord
    [CCUMScenarioDesktop]::BroadcastTheme()
    Set-TaskbarEdge 'Right'
    Capture-Vertical 'right-returned-sample' 'Right edge / automatic redocking / sample usage'
    Assert-Check 'automatic redocking retained process' ((Get-AppProcesses)[0].Id -eq $orientationPid) $orientationPid
    Stop-App
    Select-VerticalTheme $fixturePath
    $null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
    Capture-Vertical 'right-single-sample' 'Right edge / one provider / sample usage'
} finally {
    ConvertTo-Json -InputObject @($gallery.ToArray()) | Set-Content "$evidence\gallery.json" -Encoding UTF8
}
