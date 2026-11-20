$ErrorActionPreference = 'Stop'
$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
. (Join-Path $toolsDir 'helpers.ps1')

Assert-SupportedWindows
Assert-MonitorStopped $toolsDir

$packageArgs = @{
    PackageName = 'claude-code-usage-monitor'
    FileFullPath = Join-Path $toolsDir 'claude-code-usage-monitor.exe'
    Url64bit = 'https://github.com/CodeZeno/Claude-Code-Usage-Monitor/releases/download/v__VERSION__/claude-code-usage-monitor.exe'
    Checksum64 = '__CHECKSUM__'
    ChecksumType64 = 'sha256'
}
Get-ChocolateyWebFile @packageArgs | Out-Null
# The .gui file and package marker ship alongside the executable. Chocolatey
# generates and removes the command shim automatically. Do not launch on install.
