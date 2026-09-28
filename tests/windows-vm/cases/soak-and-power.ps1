param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
$executable = Install-PortableApp
$now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
$themePath = "$Root\countdown-probe.json"
# These three mutually exclusive layers expose the app's actual reset.seconds
# binding as a pixel oracle. Negative values are red and can never pass as zero.
$layers = @(
    @{id='positive'; render='claude.session.reset.seconds > 0'; color='#0000FFFF'},
    @{id='expired'; render='claude.session.reset.seconds == 0'; color='#00FF00FF'},
    @{id='negative'; render='claude.session.reset.seconds < 0'; color='#FF0000FF'}
)
$children = @($layers | ForEach-Object { @{id=$_.id; name=$_.id; render=$_.render; width='240'; height='64'; background=@{type='colour'; colour=@{color=$_.color}}} })
$children += @{id='countdown'; name='Countdown text'; width='240'; height='64'; content=@{type='text'; template='{claude.session.reset.seconds}'; font_size='24'; color=@{color='#FFFFFFFF'}}}
@{schema_version=1; id='countdown-probe'; name='Countdown probe'; surfaces=@(@{
    id='main'; name='Countdown'; width='240'; height='64'; render='time.now.unix > 0'
    placement=@{nest='floating'; reference=@{region='monitor'; display=0}; horizontal='left'; vertical='top'; surface_horizontal='left'; surface_vertical='top'; offset_x=200; offset_y=200}
    children=$children
})} | ConvertTo-Json -Depth 15 | Set-Content $themePath -Encoding ASCII
Write-Settings @{language='en'; poll_interval_ms=3600000; active_theme_path=$themePath; last_update_check_unix=$now}
$section = @{available=$true; percentage=42.0; resets_at=@{secs_since_epoch=$now+3600; nanos_since_epoch=0}}
@{updated_unix=$now; poll_ok=$true; data=@{claude_code=@{session=$section; weekly=$section}}} | ConvertTo-Json -Depth 10 | Set-Content "$(Split-Path $settingsPath)\usage-cache.json" -Encoding ASCII
$null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
Assert-App $executable 'soak-initial'
$originalPid = (Get-AppProcesses)[0].Id
$samples = [Collections.Generic.List[object]]::new()
$failedCycles = [Collections.Generic.List[int]]::new()
$forcedDashboardStops = [Collections.Generic.List[int]]::new()
$completedCycles = 0
try {
    $probeThemeLoaded = (Get-Content $settingsPath -Raw | ConvertFrom-Json).active_theme_path -eq $themePath
    Assert-Check 'countdown probe theme selected' $probeThemeLoaded $themePath -ContinueOnFailure
    $countdownReady = $probeThemeLoaded -and [CCUMScenarioDesktop]::SamplePixel((Get-Widget).Handle,210,45) -eq 0x0000ff
    # The current monitor does not load usage-cache.json (the dashboard does).
    # Keep this unmet fixture precondition visible, and still collect the soak.
    Assert-Check 'provider countdown fixture loaded' $countdownReady 'Requires a positive live provider reset; a cache-only fixture is unsupported by this monitor' -ContinueOnFailure
    @{ready=$countdownReady; themeLoaded=$probeThemeLoaded; source='usage-cache.json'; reason=if($countdownReady){'positive provider reset rendered'}elseif(-not $probeThemeLoaded){'probe theme rejected; countdown assertions not executed'}else{'monitor did not load the cached provider reset; countdown assertions not executed'}} |
        ConvertTo-Json | Set-Content "$evidence\countdown-coverage.json" -Encoding UTF8
    # Warm up allocator/font caches before measuring repeated dashboard lifetimes.
    foreach ($cycle in 1..50) {
        try {
            $dashboard = Open-Dashboard $executable "soak-dashboard-$cycle"
            Close-Dashboard $dashboard
            Assert-Check "soak-dashboard-$cycle processes exit" (Wait-Condition {
                $live = @(Get-AppProcesses)
                $live.Count -eq 1 -and $live[0].Id -eq $originalPid
            } 15) $originalPid
            $completedCycles++
        } catch {
            $failedCycles.Add($cycle)
            $Context.Checks.Add([pscustomobject]@{name="soak dashboard cycle $cycle"; passed=$false; detail=$_.Exception.ToString()})
            try { Save-Screenshot "soak-cycle-$cycle-failure" } catch { Write-Warning $_ }
            # Preserve the failure, but do not let a stuck dashboard prevent an
            # independent idle/power measurement of a healthy monitor. Never
            # restart or terminate the original monitor to manufacture recovery.
            $cleanupWatch = [Diagnostics.Stopwatch]::StartNew()
            do {
                foreach ($child in @(Get-AppProcesses | Where-Object Id -NE $originalPid)) {
                    $forcedDashboardStops.Add($child.Id)
                    Stop-Process -Id $child.Id -Force -ErrorAction SilentlyContinue
                }
                Start-Sleep -Seconds 1
                $remaining = @(Get-AppProcesses | Where-Object Id -NE $originalPid)
            } while ($remaining.Count -and $cleanupWatch.Elapsed.TotalSeconds -lt 15)
            $survivors = @(Get-AppProcesses)
            Assert-Check 'failed cycle cleanup retains original monitor only' ($survivors.Count -eq 1 -and $survivors[0].Id -eq $originalPid) $originalPid
        }
        if ($cycle -ge 10) { $samples.Add((Get-ResourceSample)) }
        if ($cycle % 10 -eq 0) { Save-Screenshot "soak-cycle-$cycle" }
    }
    Assert-Check 'all 50 dashboards close normally' ($completedCycles -eq 50) @{completed=$completedCycles; failed=@($failedCycles.ToArray()); forcedStops=@($forcedDashboardStops.ToArray())} -ContinueOnFailure
    if ($failedCycles.Count -eq 0) {
        # Forced cleanup would invalidate a claim about normal dashboard teardown.
        $cycleFirst = $samples[0]; $cycleLast = $samples[$samples.Count-1]
        Assert-Check 'dashboard cycles retain memory plateau' (($cycleLast.memory-$cycleFirst.memory) -lt 16MB) @{first=$cycleFirst; last=$cycleLast} -ContinueOnFailure
        Assert-Check 'dashboard cycles release handles' (($cycleLast.handles-$cycleFirst.handles) -le 20) @{first=$cycleFirst; last=$cycleLast} -ContinueOnFailure
        Assert-Check 'dashboard cycles release threads' (($cycleLast.threads-$cycleFirst.threads) -le 2) @{first=$cycleFirst; last=$cycleLast} -ContinueOnFailure
    }
    $null = Invoke-HostAction 'pause-resume'
    Assert-App $executable 'after-resume'
    Assert-Check 'resume retains monitor PID' ((Get-AppProcesses)[0].Id -eq $originalPid) $originalPid
    $jump = Invoke-HostAction 'clock-forward'
    $jump | ConvertTo-Json | Set-Content "$evidence\clock-jump.json" -Encoding UTF8
    Assert-Check 'clock advanced two hours' (([DateTime]$jump.after - [DateTime]$jump.before).TotalMinutes -ge 119) $jump
    if ($countdownReady) {
        Assert-Check 'countdown recalculates to zero after jump' (Wait-Condition { [CCUMScenarioDesktop]::SamplePixel((Get-Widget).Handle,210,45) -eq 0x00ff00 } 15) 'green zero-countdown layer' -ContinueOnFailure
    }
    Save-Screenshot 'countdown-after-clock-jump'
    # Use a monotonic stopwatch: a wall-clock deadline would finish immediately
    # after the clock-forward hook, defeating the soak.
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $steady = [Collections.Generic.List[object]]::new()
    while ($watch.Elapsed.TotalMinutes -lt 30) {
        Start-Sleep -Seconds 30
        $sample = Get-ResourceSample
        $samples.Add($sample); $steady.Add($sample)
        ConvertTo-Json -InputObject @($samples.ToArray()) -Depth 5 | Set-Content "$evidence\resource-samples.json" -Encoding UTF8
        Assert-Check 'soak PID unchanged' ($sample.pid -eq $originalPid) $sample.pid
        if ($countdownReady) { Assert-Check 'countdown never becomes negative' ([CCUMScenarioDesktop]::SamplePixel((Get-Widget).Handle,210,45) -eq 0x00ff00) 'zero remains green' -ContinueOnFailure }
    }
    $first = $steady[0]; $last = $steady[$steady.Count-1]
    Assert-Check 'idle CPU below one percent of one core' (($last.cpu-$first.cpu)/($last.seconds-$first.seconds) -lt 0.01) @{first=$first; last=$last} -ContinueOnFailure
    # Compare averages in separated ten-minute windows, not a single noisy sample.
    foreach ($property in 'memory','handles','threads') {
        $early = ($steady | Select-Object -First 20 | Measure-Object -Property $property -Average).Average
        $late = ($steady | Select-Object -Last 20 | Measure-Object -Property $property -Average).Average
        $limit = @{memory=4MB; handles=5; threads=1}[$property]
        Assert-Check "steady $property plateaus" (($late-$early) -le $limit) @{earlyAverage=$early; lateAverage=$late; tolerance=$limit} -ContinueOnFailure
    }
    Assert-NoPanic 'soak'
    Save-Screenshot 'soak-final'
} finally {
    ConvertTo-Json -InputObject @($samples.ToArray()) -Depth 5 | Set-Content "$evidence\resource-samples.json" -Encoding UTF8
    @{completedNormally=$completedCycles; failedCycles=@($failedCycles.ToArray()); forcedDashboardStops=@($forcedDashboardStops.ToArray()); normalTeardownPlateauChecked=($completedCycles -eq 50 -and $failedCycles.Count -eq 0)} |
        ConvertTo-Json -Depth 5 | Set-Content "$evidence\dashboard-cycle-coverage.json" -Encoding UTF8
}
