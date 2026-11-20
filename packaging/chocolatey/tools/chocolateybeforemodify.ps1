$ErrorActionPreference = 'Stop'
$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
. (Join-Path $toolsDir 'helpers.ps1')
# Warns before Chocolatey touches the installed package. Chocolatey may report a
# failure here only as a warning, so the install script (upgrades) and uninstall
# script enforce the same check before changing files or startup entries.
Assert-MonitorStopped $toolsDir
