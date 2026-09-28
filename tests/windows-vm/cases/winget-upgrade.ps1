# Invoked only by the guarded guest runner.
param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
Initialize-TestSettings
$executable = Install-WinGetApp $request.previousVersion
$app = Start-TestApp $executable
Toggle-Startup
Assert-StartupTarget $executable -ContinueAfterQuoteFailure
Stop-App
Invoke-WinGet -Arguments @('upgrade', '--id', $packageId, '--exact', '--version', $request.candidateVersion, '--source', 'winget', '--scope', 'user', '--accept-package-agreements', '--accept-source-agreements', '--disable-interactivity')
$executable = Find-WinGetExe
Assert-Version $executable $request.candidateVersion
$null = Start-Process -FilePath $executable -ArgumentList '--diagnose-append', '--diagnose' -PassThru
Assert-App $executable 'updated'
Assert-Settings
Assert-StartupTarget $executable -ContinueAfterQuoteFailure
Test-AppRestart $executable
Remove-WinGetApp $executable
Assert-StartupDisabled
