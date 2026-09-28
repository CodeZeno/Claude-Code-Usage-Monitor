param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
Initialize-TestSettings -TrayFixture
$executable = Install-PortableApp
$null = Start-TestApp $executable
Assert-Docked 'before-restart'
$samples = [Collections.Generic.List[object]]::new()
try {
    foreach ($cycle in 1..3) {
        "cycle=$cycle stopping Explorer" | Set-Content "$evidence\restart-phase.txt"
        Get-Process explorer | Where-Object SessionId -EQ $sessionId | Stop-Process -Force
        "cycle=$cycle waiting for shell" | Set-Content "$evidence\restart-phase.txt"
        Start-Sleep -Seconds 3
        if (-not @(Get-Process explorer -ErrorAction SilentlyContinue | Where-Object SessionId -EQ $sessionId).Count) { Start-Process explorer.exe }
        "cycle=$cycle sampling recovery" | Set-Content "$evidence\restart-phase.txt"
        $watch = [Diagnostics.Stopwatch]::StartNew()
        $lastPid = 0; $stableSince = 0
        do {
            $p = @(Get-AppProcesses)
            $currentPid = if ($p.Count -eq 1) { $p[0].Id } else { 0 }
            $samples.Add(@{cycle=$cycle; seconds=$watch.Elapsed.TotalSeconds; pids=@($p | ForEach-Object Id)})
            if ($samples.Count % 10 -eq 0) { ConvertTo-Json -InputObject @($samples.ToArray()) -Depth 6 | Set-Content "$evidence\restart-pids.json" -Encoding UTF8 }
            if ($currentPid -eq 0 -or $currentPid -ne $lastPid) { $stableSince = $watch.Elapsed.TotalSeconds }
            $lastPid = $currentPid
            Start-Sleep -Seconds 1
        } while ($watch.Elapsed.TotalSeconds -lt 60)
        Assert-Check "restart $cycle one stable PID for final 30 seconds" ($lastPid -ne 0 -and ($watch.Elapsed.TotalSeconds - $stableSince) -ge 30) $lastPid
        Assert-App $executable "restart-$cycle"
        Assert-Docked "restart-$cycle"
        Assert-Tray "restart-$cycle"
        Assert-Settings
    }
    Assert-NoPanic 'Explorer recovery'
} finally { ConvertTo-Json -InputObject @($samples.ToArray()) -Depth 6 | Set-Content "$evidence\restart-pids.json" -Encoding UTF8 }
