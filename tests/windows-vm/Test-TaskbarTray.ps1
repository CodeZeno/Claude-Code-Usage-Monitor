# Dot-sourced only by the guarded interactive guest scenario.
function Test-TaskbarTray {
    $samples = [Collections.Generic.List[object]]::new()
    $icons = [Collections.Generic.List[object]]::new()
    function Sample-Tray([string]$Phase, [int]$Milliseconds) {
        $deadline = [DateTime]::UtcNow.AddMilliseconds($Milliseconds)
        do {
            [Windows.Forms.Application]::DoEvents()
            $windows = @([CCUMLabDesktop]::WindowsForProcess((Get-AppProcesses)[0].Id) | Where-Object Visible)
            if ($windows.Count -ne 1) { throw "Expected one widget; found $($windows.Count)" }
            $samples.Add([pscustomobject]@{ phase=$Phase; utc=[DateTime]::UtcNow.ToString('o'); widget=$windows[0]; tray=[CCUMLabDesktop]::Tray() })
            Start-Sleep -Milliseconds 100
        } while ([DateTime]::UtcNow -lt $deadline)
    }
    try {
        Sample-Tray 'settled' 4000
        $initial = $samples[$samples.Count - 1]
        if ($request.trayTheme -eq 'compact') {
            Assert-Check 'compact fixture loaded' (($initial.widget.Right - $initial.widget.Left) -eq 120 -and ($initial.widget.Bottom - $initial.widget.Top) -eq 32) $initial.widget
        }
        # Exercise the actual drag/drop window procedure, including saved settings.
        $distance = if ($request.trayTheme -eq 'classic') { -120 } else { -80 }
        [CCUMLabDesktop]::Drag($initial.widget, $distance)
        Sample-Tray 'dropped' 3000
        $dropped = $samples[$samples.Count - 1]
        Assert-Check 'drag remains docked' ($dropped.widget.Parent -ne 0) $dropped.widget
        Save-Screenshot 'tray-before'
        Set-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer' -Name EnableAutoTray -Value 0 -Type DWord
        foreach ($i in 1..2) {
            $icon = [Windows.Forms.NotifyIcon]::new()
            $icon.Icon = [Drawing.SystemIcons]::Information
            $icon.Text = "CCUM tray regression $i"
            $icon.Visible = $true
            $icons.Add($icon)
        }
        if ($request.os -eq 'windows11') {
            # Windows 11 ignores EnableAutoTray. Promote only this fixture's icons.
            [Windows.Forms.Application]::DoEvents()
            Start-Sleep -Seconds 1
            Get-ChildItem 'HKCU:\Control Panel\NotifyIconSettings' | ForEach-Object {
                $entry = Get-ItemProperty $_.PSPath
                if ($entry.PSObject.Properties['InitialTooltip'] -and $entry.InitialTooltip -like 'CCUM tray regression *') {
                    Set-ItemProperty $_.PSPath -Name IsPromoted -Value 1 -Type DWord
                }
            }
        }
        Sample-Tray 'expanded' 8000
        Save-Screenshot 'tray-expanded'
        foreach ($icon in $icons) { $icon.Dispose() }
        $icons.Clear()
        Sample-Tray 'restored' 8000
        Save-Screenshot 'tray-restored'
        $expanded = @($samples | Where-Object phase -EQ 'expanded')
        $restored = $samples[$samples.Count - 1]
        $minimumTrayLeft = ($expanded.tray.Left | Measure-Object -Minimum).Minimum
        # Fail rather than claiming coverage if this shell keeps all icons in overflow.
        Assert-Check 'tray actually expanded' ($minimumTrayLeft -lt $dropped.tray.Left) @{before=$dropped.tray.Left; expanded=$minimumTrayLeft}
        $undocked = @($samples | Where-Object { $_.phase -in 'expanded','restored' -and $_.widget.Parent -eq 0 })
        Assert-Check 'no false auto-ejection' ($undocked.Count -eq 0) $undocked.Count
        $unobstructed = @($expanded | Where-Object { $_.tray.Left -ge $dropped.widget.Right })
        Assert-Check 'unobstructed expansion observed' ($unobstructed.Count -gt 0) $unobstructed.Count
        $jumps = @($unobstructed | Where-Object { [Math]::Abs($_.widget.Left - $dropped.widget.Left) -gt 1 })
        Assert-Check 'fixed anchor stays stationary' ($jumps.Count -eq 0) @{before=$dropped.widget.Left; positions=@($unobstructed.widget.Left | Select-Object -Unique)}
        Assert-Check 'returns to chosen position' ([Math]::Abs($restored.widget.Left - $dropped.widget.Left) -le 1) $restored.widget
    } finally {
        foreach ($icon in $icons) { $icon.Dispose() }
        ConvertTo-Json -InputObject @($samples.ToArray()) -Depth 6 | Set-Content "$evidence\tray-samples.json" -Encoding UTF8
    }
}
