param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Display.Helpers.ps1" -Context $Context
if ($Context.Phase -eq 'initial') {
    # Existing lab checkpoints force a single resolution. Boot with a mode list
    # before launching the monitor; all measured changes then happen live.
    $null = Invoke-HostAction 'display-modes'
    Save-Continuation 'display-ready'
    Invoke-HostAction 'reboot'
}
if ($Context.Phase -ne 'display-ready') { throw "Unexpected display phase: $($Context.Phase)" }
$settings = @{language='en'; poll_interval_ms=900000}
if ($request.flow -eq 'taskbar-lock-display') {
    # Exercise docking independently of Classic's fixed 46px height, which
    # exceeds a standard Win10 taskbar. This 36px fixture fits both OSes.
    $themePath = "$Root\display-lock-fixture.json"
    @{
        schema_version=1; id='display-lock-fixture'; name='Display lock regression'
        surfaces=@(@{
            id='main'; name='Display lock'; width='200'; height='36'
            background=@{type='colour'; colour=@{color='#2080D0FF'}}
            placement=@{reference=@{region='system_tray'; display=0}; nest='taskbar'; horizontal='left'; vertical='bottom'; surface_horizontal='right'; surface_vertical='bottom'; offset_x=-80}
            children=@(@{id='label'; name='Lock label'; x='8'; y='6'; width='180'; height='24';
                content=@{type='text'; template='Taskbar lock'; font_family='Segoe UI'; font_size='16'; color=@{color='#FFFFFFFF'}}})
        })
    } | ConvertTo-Json -Depth 10 | Set-Content $themePath -Encoding ASCII
    $settings.lock_taskbar=$true
    $settings.custom_theme_enabled=$true
    $settings.active_theme_path=$themePath
}
Write-Settings $settings
$executable = Install-PortableApp
$null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
Assert-App $executable 'display-initial'
$originalPid = (Get-AppProcesses)[0].Id
$gallery = [Collections.Generic.List[object]]::new()
try {
    Start-Process 'ms-settings:display'
    # Windows limits scaling choices by effective desktop height. Exercise all
    # four scales at 1440p, then a lower resolution at its supported 100% scale.
    foreach ($resolution in @(@{width=2560; height=1440; scales=@(100,125,150,200)}, @{width=1280; height=720; scales=@(100)})) {
        Set-DisplayChoice 'Scal|Dpi' '^100%'
        Set-DisplayChoice 'Resolution' ("{0}\s*[x\u00d7]\s*{1}" -f $resolution.width,$resolution.height)
        foreach ($scale in $resolution.scales) {
            $label = "display-$($resolution.width)-$($resolution.height)-$scale"
            try {
                # At 720p Windows offers only 100% and disables the scale combo.
                # An already-applied DPI is valid; do not require an enabled
                # control for a no-op. The actual DPI assertion below still runs.
                if ([CCUMScenarioDesktop]::GetDpiForWindow([IntPtr](Get-Widget).Handle) -ne [uint32](96 * $scale / 100)) {
                    Set-DisplayChoice 'Scal|Dpi' ("^$scale%")
                }
                # Give the shell and asynchronous occupancy sampler time to
                # update. An immediate parent check can pass before ejection.
                Start-Sleep -Seconds 5
                $applied = Wait-Condition { [CCUMScenarioDesktop]::GetDpiForWindow([IntPtr](Get-Widget).Handle) -eq [uint32](96 * $scale / 100) } 20
                Assert-Check 'actual widget DPI changed' $applied $scale
                $screen = [CCUMScenarioDesktop]::DisplaySize()
                Assert-Check 'actual resolution changed' ($screen.Width -eq $resolution.width -and $screen.Height -eq $resolution.height) $screen.ToString()
                Assert-Check 'display change retained process' ((Get-AppProcesses)[0].Id -eq $originalPid) $originalPid
                Assert-Check 'docking settles after display change' (Wait-Condition { [CCUMScenarioDesktop]::IsTaskbarChild((Get-Widget).Handle) } 20) $scale
                Assert-Docked "scale-$scale"
                $w = Get-Widget; $bar = [CCUMScenarioDesktop]::Taskbar()
                Assert-Check 'widget height fits taskbar' (($w.Bottom - $w.Top) -le ($bar.Bottom - $bar.Top) -and ($w.Bottom - $w.Top) -ge ($bar.Bottom - $bar.Top) * 0.7) @{widget=$w; bar=$bar}
                Assert-Image $label $w.Handle
            } catch {
                # Retain the failure and exercise later live transitions too.
                # Do not restart the app, which would hide recovery defects.
                $Context.Checks.Add([pscustomobject]@{name=$label; passed=$false; detail=$_.Exception.Message})
            } finally {
                Save-Screenshot "$label-desktop"
                $gallery.Add(@{label="$($resolution.width)x$($resolution.height), $scale percent"; file="$label-desktop.png"})
            }
        }
    }
    Assert-NoPanic 'display changes'
} finally { ConvertTo-Json -InputObject @($gallery.ToArray()) | Set-Content "$evidence\gallery.json" -Encoding UTF8 }
