#Requires -Version 5.1
# Run as LabUser in the guest's interactive desktop, through a scheduled task.
param([switch]$UseInstalled)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ((Get-CimInstance Win32_ComputerSystem).Model -ne 'Virtual Machine' -or
    -not (Test-Path C:\CCUM-Lab\guest-enabled) -or (Get-Process -Id $PID).SessionId -eq 0) { throw 'Prepared interactive lab guest required.' }
$result = @{ status = 'failed'; error = $null }
try {
    $root = 'C:\CCUM-Lab\downloads'
    if (-not $UseInstalled) {
        $deps = @(Get-ChildItem -LiteralPath "$root\x64" -Filter *.appx | Select-Object -ExpandProperty FullName)
        Add-AppxPackage -Path "$root\Microsoft.DesktopAppInstaller_8wekyb3d8bbwe.msixbundle" -DependencyPath $deps
    }
    winget --version | Set-Content C:\CCUM-Lab\winget-version.txt
    if ($LASTEXITCODE -ne 0) { throw 'WinGet command unavailable.' }
    winget source update --name winget --disable-interactivity 2>&1 | Out-File C:\CCUM-Lab\winget-source.log
    # Source update can return zero even on cancellation; require a successful lookup.
    winget show --id CodeZeno.ClaudeCodeUsageMonitor --exact --source winget --accept-source-agreements --disable-interactivity 2>&1 |
        Out-File C:\CCUM-Lab\winget-package.log
    if ($LASTEXITCODE -ne 0) { throw "WinGet package lookup failed: $LASTEXITCODE" }
    $result.status = 'passed'
} catch { $result.error = $_.Exception.Message }
$result | ConvertTo-Json | Set-Content C:\CCUM-Lab\winget-ready.json -Encoding UTF8
if ($result.status -ne 'passed') { exit 1 }
