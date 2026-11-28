# Measure the release build's footprint and compare it with a baseline:
# - the size of the release executable;
# - the packages in Cargo.lock;
# - the time and peak memory to render every bundled theme with --render-theme.
#
# CI runs this after the release build. On main it saves the result as the
# baseline; on a pull request it compares with the baseline of the base branch.
# Executable growth over -FailPercent fails unless -AllowGrowth is given (CI
# passes it when a maintainer adds the footprint-approved label). Growth over
# -WarnPercent and new packages are warnings. Timings and memory vary between
# machines and are reported, with a warning over -TimingWarnPercent, but never fail.
#
# Locally, before and after a change:
#   ./tests/Test-Footprint.ps1 -Output before.json
#   ./tests/Test-Footprint.ps1 -Baseline before.json
[CmdletBinding()]
param(
    [string]$Executable = 'target/release/claude-code-usage-monitor.exe',
    [string]$Baseline,
    [string]$Output,
    [double]$WarnPercent = 5,
    [double]$FailPercent = 20,
    [double]$TimingWarnPercent = 50,
    [int]$Runs = 3,
    [switch]$AllowGrowth
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class FootprintNative {
    [StructLayout(LayoutKind.Sequential)]
    struct ProcessMemoryCounters {
        public uint cb;
        public uint PageFaultCount;
        public UIntPtr PeakWorkingSetSize;
        public UIntPtr WorkingSetSize;
        public UIntPtr QuotaPeakPagedPoolUsage;
        public UIntPtr QuotaPagedPoolUsage;
        public UIntPtr QuotaPeakNonPagedPoolUsage;
        public UIntPtr QuotaNonPagedPoolUsage;
        public UIntPtr PagefileUsage;
        public UIntPtr PeakPagefileUsage;
    }

    [DllImport("psapi.dll", SetLastError = true)]
    static extern bool GetProcessMemoryInfo(IntPtr process, out ProcessMemoryCounters counters, uint size);

    // Windows keeps the counters while a handle to the exited process is open.
    public static ulong PeakWorkingSet(IntPtr process) {
        ProcessMemoryCounters counters;
        if (!GetProcessMemoryInfo(process, out counters, (uint)Marshal.SizeOf(typeof(ProcessMemoryCounters)))) {
            throw new Win32Exception();
        }
        return counters.PeakWorkingSetSize.ToUInt64();
    }
}
'@

function Resolve-RepositoryPath([string]$Path) {
    if ([IO.Path]::IsPathRooted($Path)) { return $Path }
    return Join-Path $root $Path
}

function Get-LockPackages {
    $packages = New-Object System.Collections.Generic.List[string]
    $name = $null
    foreach ($line in Get-Content -LiteralPath (Join-Path $root 'Cargo.lock')) {
        if ($line -match '^name = "(.+)"$') {
            $name = $Matches[1]
        } elseif ($name -and $line -match '^version = "(.+)"$') {
            $packages.Add("$name $($Matches[1])")
            $name = $null
        }
    }
    return @($packages | Sort-Object -Unique)
}

function Measure-ThemeRenders([string]$Exe) {
    $themes = @(Get-ChildItem -LiteralPath (Join-Path $root 'src/themes') -Filter '*.json' | Sort-Object Name)
    if ($themes.Count -eq 0) { throw 'No bundled themes found in src/themes.' }
    $work = Join-Path ([IO.Path]::GetTempPath()) ("ccum-footprint-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work | Out-Null
    try {
        $totals = New-Object System.Collections.Generic.List[double]
        $peak = [uint64]0
        for ($run = 0; $run -lt $Runs; $run++) {
            $elapsed = 0.0
            foreach ($theme in $themes) {
                $out = Join-Path $work "run$run"
                $watch = [Diagnostics.Stopwatch]::StartNew()
                $process = Start-Process -FilePath $Exe -NoNewWindow -PassThru `
                    -RedirectStandardOutput (Join-Path $work 'stdout.txt') `
                    -RedirectStandardError (Join-Path $work 'stderr.txt') `
                    -ArgumentList @('--render-theme', "`"$($theme.FullName)`"", '--out', "`"$out`"")
                $handle = $process.Handle
                $process.WaitForExit()
                $watch.Stop()
                if ($process.ExitCode -ne 0) {
                    $errors = Get-Content -Raw -LiteralPath (Join-Path $work 'stderr.txt')
                    throw "Rendering $($theme.Name) failed with exit code $($process.ExitCode): $errors"
                }
                $elapsed += $watch.Elapsed.TotalMilliseconds
                $themePeak = [FootprintNative]::PeakWorkingSet($handle)
                if ($themePeak -gt $peak) { $peak = $themePeak }
                $process.Dispose()
            }
            $totals.Add($elapsed)
        }
        # The median run, so one slow start on a shared runner does not count.
        $sorted = @($totals | Sort-Object)
        return [pscustomobject]@{
            themes = $themes.Count
            render_ms = [math]::Round($sorted[[int][math]::Floor($sorted.Count / 2)])
            render_peak_bytes = $peak
        }
    } finally {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Get-Commit {
    if ($env:GITHUB_SHA) { return $env:GITHUB_SHA }
    $commit = & git -C $root rev-parse HEAD 2>$null
    if ($LASTEXITCODE -eq 0) { return "$commit".Trim() }
    return 'unknown'
}

function Format-Change([double]$Before, [double]$After) {
    if ($Before -le 0) { return 'n/a' }
    $percent = ($After - $Before) / $Before * 100
    return ('{0:+0.0;-0.0;0.0}%' -f $percent)
}

function Get-Percent([double]$Before, [double]$After) {
    if ($Before -le 0) { return 0 }
    return ($After - $Before) / $Before * 100
}

function Format-MB([double]$Bytes) { return ('{0:N2} MB' -f ($Bytes / 1MB)) }

$exe = Resolve-RepositoryPath $Executable
if (-not (Test-Path -LiteralPath $exe)) {
    throw "$exe does not exist. Build it first with scripts/Build-Release.ps1 or cargo build --release --locked."
}

$renders = Measure-ThemeRenders $exe
$current = [pscustomobject]@{
    commit = Get-Commit
    exe_bytes = (Get-Item -LiteralPath $exe).Length
    packages = Get-LockPackages
    themes = $renders.themes
    render_ms = $renders.render_ms
    render_peak_bytes = $renders.render_peak_bytes
}

$previous = $null
if ($Baseline) {
    $baselinePath = Resolve-RepositoryPath $Baseline
    if (Test-Path -LiteralPath $baselinePath) {
        $previous = Get-Content -Raw -LiteralPath $baselinePath | ConvertFrom-Json
    }
}

$warnings = New-Object System.Collections.Generic.List[string]
$failures = New-Object System.Collections.Generic.List[string]
$summary = New-Object System.Collections.Generic.List[string]
$summary.Add('## Footprint')
$summary.Add('')

if ($null -eq $previous) {
    $summary.Add('No baseline to compare with; these values become one when this commit reaches main.')
    $summary.Add('')
    $summary.Add('| Measure | Value |')
    $summary.Add('| --- | ---: |')
    $summary.Add("| Release executable | $(Format-MB $current.exe_bytes) ($($current.exe_bytes) bytes) |")
    $summary.Add("| Cargo.lock packages | $($current.packages.Count) |")
    $summary.Add("| Render $($current.themes) bundled themes | $($current.render_ms) ms |")
    $summary.Add("| Peak memory while rendering | $(Format-MB $current.render_peak_bytes) |")
} else {
    $exeGrowth = Get-Percent $previous.exe_bytes $current.exe_bytes
    $previousPackages = @($previous.packages)
    $currentPackages = @($current.packages)
    $previousNames = @($previousPackages | ForEach-Object { ($_ -split ' ')[0] } | Sort-Object -Unique)
    $currentNames = @($currentPackages | ForEach-Object { ($_ -split ' ')[0] } | Sort-Object -Unique)
    $addedNames = @($currentNames | Where-Object { $previousNames -notcontains $_ })
    $removedNames = @($previousNames | Where-Object { $currentNames -notcontains $_ })
    $changedVersions = @($currentPackages | Where-Object {
        $previousPackages -notcontains $_ -and $addedNames -notcontains (($_ -split ' ')[0])
    })

    $summary.Add("Compared with $($previous.commit).")
    $summary.Add('')
    $summary.Add('| Measure | Baseline | This build | Change |')
    $summary.Add('| --- | ---: | ---: | ---: |')
    $summary.Add("| Release executable | $(Format-MB $previous.exe_bytes) | $(Format-MB $current.exe_bytes) | $(Format-Change $previous.exe_bytes $current.exe_bytes) |")
    $summary.Add("| Cargo.lock packages | $($previousPackages.Count) | $($currentPackages.Count) | $($currentPackages.Count - $previousPackages.Count) |")
    $summary.Add("| Render bundled themes | $($previous.render_ms) ms | $($current.render_ms) ms | $(Format-Change $previous.render_ms $current.render_ms) |")
    $summary.Add("| Peak memory while rendering | $(Format-MB $previous.render_peak_bytes) | $(Format-MB $current.render_peak_bytes) | $(Format-Change $previous.render_peak_bytes $current.render_peak_bytes) |")

    if ($addedNames.Count -gt 0) { $summary.Add(''); $summary.Add("New crates: $($addedNames -join ', ')") }
    if ($removedNames.Count -gt 0) { $summary.Add(''); $summary.Add("Removed crates: $($removedNames -join ', ')") }
    if ($changedVersions.Count -gt 0) { $summary.Add(''); $summary.Add("New versions: $($changedVersions -join ', ')") }

    $exeMessage = 'The release executable grew {0:N1}% ({1} -> {2}).' -f $exeGrowth, (Format-MB $previous.exe_bytes), (Format-MB $current.exe_bytes)
    if ($exeGrowth -gt $FailPercent) {
        if ($AllowGrowth) {
            $warnings.Add("$exeMessage A maintainer accepted it with the footprint-approved label.")
        } else {
            $failures.Add("$exeMessage Growth over $FailPercent% needs a strong reason: explain it in the pull request; a maintainer who accepts it adds the footprint-approved label and re-runs this job.")
        }
    } elseif ($exeGrowth -gt $WarnPercent) {
        $warnings.Add("$exeMessage Explain growth over $WarnPercent% in the pull request.")
    }
    if ($addedNames.Count -gt 0) {
        $warnings.Add("New crates in Cargo.lock: $($addedNames -join ', '). Explain why an existing dependency does not do the job.")
    }
    $renderGrowth = Get-Percent $previous.render_ms $current.render_ms
    if ($renderGrowth -gt $TimingWarnPercent) {
        $warnings.Add(('Rendering the bundled themes took {0:N0}% longer ({1} ms -> {2} ms). Timings vary between runners; re-run before investigating.' -f $renderGrowth, $previous.render_ms, $current.render_ms))
    }
    $memoryGrowth = Get-Percent $previous.render_peak_bytes $current.render_peak_bytes
    if ($memoryGrowth -gt $TimingWarnPercent) {
        $warnings.Add(('Peak memory while rendering grew {0:N0}% ({1} -> {2}).' -f $memoryGrowth, (Format-MB $previous.render_peak_bytes), (Format-MB $current.render_peak_bytes)))
    }
}

foreach ($message in $warnings) { $summary.Add(''); $summary.Add("Warning: $message") }
foreach ($message in $failures) { $summary.Add(''); $summary.Add("**Failed:** $message") }

$text = $summary -join "`n"
Write-Host $text
if ($env:GITHUB_STEP_SUMMARY) {
    Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value $text -Encoding utf8
}
if ($env:GITHUB_ACTIONS -eq 'true') {
    foreach ($message in $warnings) { Write-Host "::warning title=Footprint::$message" }
    foreach ($message in $failures) { Write-Host "::error title=Footprint::$message" }
}

if ($Output) {
    $outputPath = Resolve-RepositoryPath $Output
    $outputFolder = Split-Path -Parent $outputPath
    if ($outputFolder -and -not (Test-Path -LiteralPath $outputFolder)) {
        New-Item -ItemType Directory -Path $outputFolder | Out-Null
    }
    $json = $current | ConvertTo-Json -Depth 3
    [IO.File]::WriteAllText($outputPath, $json + "`n", (New-Object System.Text.UTF8Encoding($false)))
}

if ($failures.Count -gt 0) {
    throw 'Footprint check failed.'
}
Write-Host 'Footprint check passed.'
