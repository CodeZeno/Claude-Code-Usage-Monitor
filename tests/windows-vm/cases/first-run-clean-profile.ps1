param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
Assert-Check 'no cached usage in clean profile' (-not (Test-Path "$(Split-Path $settingsPath)\usage-cache.json")) 'usage-cache.json'
foreach ($path in @("$env:USERPROFILE\.claude\.credentials.json", "$env:USERPROFILE\.codex\auth.json", "$env:APPDATA\Claude\config.json")) {
    Assert-Check 'no credentials in clean profile' (-not (Test-Path $path)) $path
}
foreach ($name in 'CLAUDE_CONFIG_DIR','CODEX_HOME','ANTHROPIC_API_KEY','OPENAI_API_KEY') {
    Assert-Check 'no credential environment override' ([string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable($name))) $name
}
$executable = Install-PortableApp
Invoke-Subcase 'first-run-widget' {
    $null = Start-Process $executable -ArgumentList '--diagnose'
    Assert-App $executable 'first-run'
    Assert-Check 'default settings created' (Wait-Condition { Test-Path $settingsPath }) $settingsPath
    $settings = Get-Content $settingsPath -Raw | ConvertFrom-Json
    Assert-Check 'sensible default poll interval' ($settings.poll_interval_ms -ge 60000 -and $settings.poll_interval_ms -le 3600000) $settings.poll_interval_ms
    Assert-Check 'Claude enabled by default' $settings.show_claude_code $settings
    Assert-Tray 'first-run'
    Assert-Check 'first-run poll completes' (Wait-Condition { Test-Path "$(Split-Path $settingsPath)\usage-cache.json" } 120) 'usage cache'
    $tooltip = Get-TrayTooltip
    Assert-Check 'tooltip explains missing sign-in' ($tooltip -match 'sign in|login|anmelden|Anmeldung') $tooltip
    Assert-Image 'first-run-widget' (Get-Widget).Handle
    Assert-NoPanic 'first-run'
}
Invoke-Subcase 'first-run-dashboard' {
    # Exercise --dashboard with no settings, not only an already initialized profile.
    if (Test-Path $settingsPath) { Remove-Item -LiteralPath $settingsPath }
    $cache = "$(Split-Path $settingsPath)\usage-cache.json"
    if (Test-Path $cache) { Remove-Item -LiteralPath $cache }
    $dashboard = Open-Dashboard $executable 'first-run-dashboard'
    Assert-Image 'first-run-dashboard' $dashboard
    Close-Dashboard $dashboard
    Assert-App $executable 'after-dashboard'
    Assert-NoPanic 'first-run-dashboard'
}
Complete-Subcases
