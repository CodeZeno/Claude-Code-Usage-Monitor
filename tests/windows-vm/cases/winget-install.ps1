# Invoked only by the guarded guest runner.
param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
Initialize-TestSettings
$executable = Install-WinGetApp $request.candidateVersion
$app = Start-TestApp $executable
Test-AppRestart $executable
Toggle-Startup
Assert-StartupTarget $executable -ContinueAfterQuoteFailure
Remove-WinGetApp $executable
Assert-StartupDisabled
