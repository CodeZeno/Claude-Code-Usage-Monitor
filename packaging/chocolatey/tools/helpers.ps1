function Assert-MonitorStopped([string]$ToolsDirectory) {
    $target = [IO.Path]::GetFullPath((Join-Path $ToolsDirectory 'claude-code-usage-monitor.exe'))
    foreach ($process in @(Get-Process -Name 'claude-code-usage-monitor' -ErrorAction SilentlyContinue)) {
        # Check the real path, including separate dashboard processes. Never kill
        # processes by name, or discard unsaved Theme Studio changes.
        $path = $process.Path
        if (-not $path) {
            throw 'Cannot verify a running monitor process. Quit the monitor and dashboard in all user sessions, then retry.'
        }
        if ([string]::Equals($path, $target, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Save your work and quit the monitor and dashboard at '$target', then retry."
        }
    }
}

function Test-MonitorStartupTarget([string]$Value, [string]$Executable) {
    # Accept the current unquoted Run value and a quoted executable path. A Run
    # value for another copy (or one with additional arguments) is left alone.
    return [string]::Equals($Value.Trim().Trim('"'), $Executable, [StringComparison]::OrdinalIgnoreCase)
}

function Test-MonitorUserSid([string]$Sid) {
    # Local and Active Directory accounts (S-1-5-21-...) and Microsoft Entra ID
    # accounts (S-1-12-1-...). Skips service profiles and *_Classes hives.
    return $Sid -match '^S-1-(5-21|12-1)-\d+-\d+-\d+-\d+$'
}

function Remove-MonitorStartupEntries([string]$ToolsDirectory, $Users = [Microsoft.Win32.Registry]::Users) {
    $target = [IO.Path]::GetFullPath((Join-Path $ToolsDirectory 'claude-code-usage-monitor.exe'))
    # Include loaded user hives: an elevated Chocolatey session can belong to a
    # different user than the one who enabled startup. Do not load offline hives.
    # The caller owns $Users (HKEY_USERS by default), so it is not closed here.
    foreach ($sid in $Users.GetSubKeyNames()) {
        if (-not (Test-MonitorUserSid $sid)) { continue }
        $run = $Users.OpenSubKey("$sid\Software\Microsoft\Windows\CurrentVersion\Run", $true)
        if ($null -eq $run) { continue }
        try {
            $value = $run.GetValue('ClaudeCodeUsageMonitor', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            if ($value -is [string] -and (Test-MonitorStartupTarget $value $target)) {
                $run.DeleteValue('ClaudeCodeUsageMonitor', $false)
            }
        } finally { $run.Close() }
    }
}

function Test-SupportedWindows([string]$Architecture, [int]$MajorVersion, [int]$Build) {
    # The package ships only an x64 executable. Windows on Arm can emulate x64
    # starting with Windows 11 (build 22000); Windows 10 on Arm cannot run it.
    if ($MajorVersion -lt 10) { return $false }
    switch ($Architecture) {
        'AMD64' { return $true }
        'ARM64' { return $Build -ge 22000 }
        default { return $false }
    }
}

function Assert-SupportedWindows {
    # Read the registry rather than [Environment]::OSVersion, which depends on the
    # host process manifest, and the system architecture rather than the process's.
    $version = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
    $environment = Get-ItemProperty -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
    $major = 0
    if ($null -ne $version.PSObject.Properties['CurrentMajorVersionNumber']) { $major = [int]$version.CurrentMajorVersionNumber }
    $build = 0
    [void][int]::TryParse([string]$version.CurrentBuildNumber, [ref]$build)
    if (-not (Test-SupportedWindows ([string]$environment.PROCESSOR_ARCHITECTURE) $major $build)) {
        throw 'Claude Code Usage Monitor requires Windows 10 or 11 x64, or Windows 11 on Arm with x64 emulation.'
    }
}
