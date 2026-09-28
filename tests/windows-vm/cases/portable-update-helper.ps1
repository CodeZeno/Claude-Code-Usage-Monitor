# Invoked only by the guarded guest runner.
param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Guest.Helpers.ps1" -Context $Context
Initialize-TestSettings
$executable = Install-PortableApp -Previous
$app = Start-TestApp $executable
# Tests the real replacement/relaunch helper, not the release check/download/UI.
Copy-Item -LiteralPath "$Root\previous.exe" -Destination "$Root\updater-helper.exe"
$size = (Get-Item -LiteralPath "$Root\candidate.exe").Length
$helperArgs = '--apply-update "{0}" "{1}" {2} {3} sha256:{4}' -f $executable, "$Root\candidate.exe", $app.Id, $size, $request.candidateHash.ToLowerInvariant()
$helper = Start-Process -FilePath "$Root\updater-helper.exe" -ArgumentList $helperArgs -PassThru
Stop-App
if (-not $helper.WaitForExit(90000)) { throw 'Updater helper timed out.' }
Assert-Check 'helper exit' ($helper.ExitCode -eq 0) $helper.ExitCode
Assert-Check 'candidate replaced executable' ((Get-FileHash -LiteralPath $executable).Hash -eq $request.candidateHash) $request.candidateHash
Assert-App $executable 'updated'
Assert-Settings
Test-AppRestart $executable
