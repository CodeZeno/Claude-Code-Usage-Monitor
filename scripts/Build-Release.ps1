# Build target/release/claude-code-usage-monitor.exe the supported way: run the
# dependency gates, close instances of this repository's executable, then
# `cargo build --release --locked` in the normal target directory.
[CmdletBinding()]
param(
    [switch]$SkipDependencyChecks
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'ReleaseTools.psm1') -Force

$root = Get-RepositoryRoot
$exe = [System.IO.Path]::GetFullPath((Join-Path $root 'target/release/claude-code-usage-monitor.exe'))

function Get-RepositoryInstance {
    @(Get-Process -Name 'claude-code-usage-monitor' -ErrorAction SilentlyContinue | Where-Object {
        $_.Path -and [string]::Equals([System.IO.Path]::GetFullPath($_.Path), $exe, [System.StringComparison]::OrdinalIgnoreCase)
    })
}

if ($env:CARGO_TARGET_DIR) {
    throw 'CARGO_TARGET_DIR is set. Build in the normal target/release directory instead.'
}

if (-not $SkipDependencyChecks) {
    & (Join-Path $root 'tests/Test-Dependencies.ps1')
}

# Only this repository's executable, matched by full path, is closed.
$instances = Get-RepositoryInstance
if ($instances.Count -gt 0) {
    Write-Host "Closing $($instances.Count) running instance(s) of $exe."
    if (-not ('CcumBuildWindows' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CcumBuildWindows {
    delegate bool EnumProc(IntPtr hwnd, IntPtr data);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr data);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint processId);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr wparam, IntPtr lparam);
    // The widget is reparented into the taskbar, so Process.CloseMainWindow finds
    // nothing; the hidden controller window shuts the application down on WM_CLOSE.
    public static int Close(int processId) {
        int posted = 0;
        EnumWindows((hwnd, data) => {
            uint owner;
            GetWindowThreadProcessId(hwnd, out owner);
            if (owner == (uint)processId && PostMessage(hwnd, 0x0010, IntPtr.Zero, IntPtr.Zero)) { posted++; }
            return true;
        }, IntPtr.Zero);
        return posted;
    }
}
'@
    }
    foreach ($process in $instances) { [void][CcumBuildWindows]::Close($process.Id) }
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-RepositoryInstance).Count -gt 0 -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
    }
    foreach ($process in (Get-RepositoryInstance)) {
        Write-Host "Stopping process $($process.Id), which did not close normally."
        Stop-Process -Id $process.Id -Force
        $process.WaitForExit(10000) | Out-Null
    }
}

Push-Location $root
try {
    $cargo = (Get-Command cargo -CommandType Application | Select-Object -First 1).Source
    $ErrorActionPreference = 'Continue'
    & $cargo build --release --locked
    if ($LASTEXITCODE -ne 0) {
        throw 'cargo build --release --locked failed.'
    }
} finally {
    Pop-Location
}
Write-Host "Built $exe"
