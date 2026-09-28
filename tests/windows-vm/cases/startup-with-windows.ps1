param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
$executable = "$Root\App With Spaces\$appName.exe"
if ($Context.Phase -eq 'initial') {
    Initialize-TestSettings
    $executable = Install-PortableApp -Previous
    $null = Start-TestApp $executable
    Toggle-Startup
    Assert-StartupTarget $executable -ContinueAfterQuoteFailure
    Save-Continuation 'after-sign-in'
    Invoke-HostAction 'reboot'
} elseif ($Context.Phase -eq 'after-sign-in') {
    Assert-App $executable 'automatic-sign-in'
    Assert-StartupTarget $executable -ContinueAfterQuoteFailure
    Assert-Tray 'automatic-sign-in'
    Assert-Settings
    Invoke-PortableUpdate $executable
    Assert-StartupTarget $executable -ContinueAfterQuoteFailure
    Assert-Settings
    Toggle-Startup
    Assert-StartupDisabled
    Save-Screenshot 'startup-disabled'
} else { throw "Unexpected startup phase: $($Context.Phase)" }
