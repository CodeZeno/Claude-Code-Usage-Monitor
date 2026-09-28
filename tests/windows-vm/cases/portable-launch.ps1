# Invoked only by the guarded guest runner.
param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Guest.Helpers.ps1" -Context $Context
Initialize-TestSettings
$executable = Install-PortableApp
$app = Start-TestApp $executable
Test-AppRestart $executable
