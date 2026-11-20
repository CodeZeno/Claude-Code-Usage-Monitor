$ErrorActionPreference = 'Stop'
$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
. (Join-Path $toolsDir 'helpers.ps1')
Assert-MonitorStopped $toolsDir
Remove-MonitorStartupEntries $toolsDir
# Chocolatey removes the package directory and automatic shim. Leave AppData,
# credentials, and other installations untouched.
