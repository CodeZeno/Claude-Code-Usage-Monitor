param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
Write-Settings @{language='en'; poll_interval_ms=900000}
$executable = Install-PortableApp
$null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
Assert-App $executable 'theme-initial'
$originalPid = (Get-AppProcesses)[0].Id
$initialWidget = Get-Widget
$dockedWidth = $initialWidget.Right - $initialWidget.Left
$gallery = [Collections.Generic.List[object]]::new()
try {
    foreach ($placement in 'taskbar','floating') {
        $lightHash = $null
        $settings = Get-Content $settingsPath -Raw | ConvertFrom-Json
        # Keep the built-in's natural taskbar anchor. Forcing x=200 overlaps
        # Windows 10's pinned buttons and tests collision ejection, not colour.
        $override = if ($placement -eq 'floating') {
            @{nest='floating'; monitor_index=0; screen_x=200; screen_y=200; tray_offset=0}
        } else { $null }
        $settings | Add-Member NoteProperty placement_override $override -Force
        Write-Settings $settings
        Send-WidgetMessage 0x8005
        Start-Sleep -Seconds 3
        foreach ($mode in 'light','dark','high-contrast','dark-restored') {
            $label = "theme-$placement-$mode"
            try {
                $light = if ($mode -eq 'light') { 1 } else { 0 }
                $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
                # Leaving high contrast restores a saved Windows theme; apply our
                # requested light/dark values after that restoration, not before it.
                [CCUMScenarioDesktop]::SetHighContrast($mode -eq 'high-contrast')
                Set-ItemProperty $key AppsUseLightTheme $light -Type DWord
                Set-ItemProperty $key SystemUsesLightTheme $light -Type DWord
                [CCUMScenarioDesktop]::BroadcastTheme()
                Start-Sleep -Seconds 3
                Assert-Check 'Windows theme preference applied' ((Get-ItemPropertyValue $key SystemUsesLightTheme) -eq $light) $mode
                $w = Get-Widget
                Assert-Check 'requested widget placement applied' (($w.Parent -eq 0) -eq ($placement -eq 'floating')) $w
                Assert-Check 'theme change retained process' ((Get-AppProcesses)[0].Id -eq $originalPid) $originalPid
                # Sample actual rendered foreground/background populations. Retain raw
                # pixels so sparse low-contrast labels can also be reviewed individually.
                Assert-Image $label $w.Handle 4.5
                # Measure actual label-only regions: a bright progress bar must not
                # hide unreadable text elsewhere. Coordinates are from Classic v1.
                $scale = [CCUMScenarioDesktop]::GetDpiForWindow([IntPtr]$w.Handle) / 96.0
                $inset = [Math]::Max(0, (($w.Right-$w.Left)-$dockedWidth)/2)
                foreach ($row in @(@{name='session'; y=5}, @{name='weekly'; y=28})) {
                    $pixels = [CCUMScenarioDesktop]::CaptureRegion($w.Handle, [int]($inset+13*$scale), [int]($row.y*$scale), [int](18*$scale), [int](13*$scale), "$evidence\$label-$($row.name)-text.png")
                    $pixels | ConvertTo-Json | Set-Content "$evidence\$label-$($row.name)-contrast.json" -Encoding UTF8
                    Assert-Check "$label $($row.name) text contrast" ($pixels.ForegroundPixels -ge 6 -and $pixels.Contrast -ge 4.5) $pixels
                }
                $hash = (Get-FileHash "$evidence\$label.png").Hash
                if ($mode -eq 'light') { $lightHash = $hash }
                if ($mode -eq 'dark') { Assert-Check 'light and dark actually redraw different pixels' ($hash -ne $lightHash) @{light=$lightHash; dark=$hash} }
            } catch {
                $Context.Checks.Add([pscustomobject]@{name=$label; passed=$false; detail=$_.Exception.Message})
                Save-Screenshot "$label-failure"
            } finally {
                if (Test-Path "$evidence\$label.png") { $gallery.Add(@{label="$placement $mode"; file="$label.png"}) }
            }
        }
    }
    Assert-NoPanic 'theme switches'
    Complete-Subcases
} finally {
    [CCUMScenarioDesktop]::SetHighContrast($false)
    ConvertTo-Json -InputObject @($gallery.ToArray()) | Set-Content "$evidence\gallery.json" -Encoding UTF8
}
