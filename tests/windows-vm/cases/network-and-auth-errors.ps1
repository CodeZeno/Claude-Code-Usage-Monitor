param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
$executable = Install-PortableApp
$cachePath = "$(Split-Path $settingsPath)\usage-cache.json"
function Wait-PollState([string]$Pattern, [long]$After = 0) {
    return Wait-Condition {
        if (-not (Test-Path $cachePath)) { return $false }
        $cache = Get-Content $cachePath -Raw | ConvertFrom-Json
        if ($cache.updated_unix -le $After) { return $false }
        $accounts = @($cache.data.accounts | Where-Object { $_.provider -in 'claude','codex' })
        return $accounts.Count -eq 2 -and @($accounts | Where-Object { ($_.error | ConvertTo-Json -Compress) -notmatch $Pattern }).Count -eq 0
    } 90
}
Invoke-Subcase 'auth-offline-recovery' {
    $claudePath = "$Root\fake-claude.json"; $codexPath = "$Root\fake-codex.json"
    '{"claudeAiOauth":{"accessToken":"ccum-vm-invalid-token","expiresAt":4102444800000}}' | Set-Content $claudePath -Encoding ASCII
    '{"tokens":{"access_token":"ccum-vm-invalid-token","account_id":"ccum-vm-fake-account"}}' | Set-Content $codexPath -Encoding ASCII
    Write-Settings @{language='en'; poll_interval_ms=60000; show_claude_code=$true; show_codex=$true; last_update_check_unix=[DateTimeOffset]::UtcNow.ToUnixTimeSeconds(); accounts=@{
        claude=@{profiles=@(@{id='default'; name='Lab Claude'; credentials_path=$claudePath; enabled=$true}); selected='default'}
        codex=@{profiles=@(@{id='default'; name='Lab Codex'; credentials_path=$codexPath; enabled=$true}); selected='default'}
    }}
    $null = Invoke-HostAction 'audit-start'
    try {
        $null = Start-Process $executable -ArgumentList '--diagnose'
        Assert-App $executable 'network-online'
        Assert-Check 'fake credentials rejected by both services' (Wait-PollState 'auth_required|401|403') 'usage cache account errors'
    } finally {
        $control = Invoke-HostAction 'audit-stop'
        Copy-Item "$evidence\outbound-connections.json" "$evidence\audit-positive-control.json" -Force
        if (Test-Path $cachePath) { Copy-Item $cachePath "$evidence\auth-observed.json" -Force }
    }
    Assert-Check 'connection audit positive control captures app traffic' (@($control.connections).Count -gt 0) $control
    Copy-Item $cachePath "$evidence\auth-rejected.json"
    $tooltip = Get-TrayTooltip -ExpectedPattern 'rejected|401|403|Sign in again'
    Assert-Check 'rejected sign-in message' ($tooltip -match 'rejected|401|403|Sign in again') $tooltip -ContinueOnFailure
    $null = Invoke-HostAction 'network-disconnect'
    try {
        $before = (Get-Content $cachePath -Raw | ConvertFrom-Json).updated_unix
        Start-Sleep -Seconds 2
        # Explicit refresh retries an auth-paused source; timer polling must not.
        Send-WidgetMessage 0x8006
        Assert-Check 'offline produces network error for both services' (Wait-PollState 'network_error|request_failed' $before) 'usage cache account errors'
        Copy-Item $cachePath "$evidence\offline.json"
        $offlineTooltip = Get-TrayTooltip -ExpectedPattern 'unreachable|request failed'
        Assert-Check 'offline message distinct from sign-in rejection' ($offlineTooltip -ne $tooltip -and $offlineTooltip -match 'unreachable|request failed') $offlineTooltip -ContinueOnFailure
        $start = Get-ResourceSample
        $pollStart = ([regex]::Matches((Get-Content $logPath -Raw), 'poll started providers=')).Count
        $watch = [Diagnostics.Stopwatch]::StartNew()
        while ($watch.Elapsed.TotalSeconds -lt 90) { Start-Sleep -Seconds 1 }
        $end = Get-ResourceSample
        Assert-Check 'offline monitor PID unchanged' ($end.pid -eq $start.pid) @{before=$start.pid; after=$end.pid}
        $polls = ([regex]::Matches((Get-Content $logPath -Raw), 'poll started providers=')).Count - $pollStart
        @{start=$start; end=$end; polls=$polls} | ConvertTo-Json -Depth 5 | Set-Content "$evidence\offline-resources.json" -Encoding UTF8
        Assert-Check 'no offline CPU spin' (($end.cpu - $start.cpu) / ($end.seconds - $start.seconds) -lt 0.03) @{start=$start; end=$end}
        Assert-Check 'no offline retry storm' ($polls -le 3) $polls
    } finally { $null = Invoke-HostAction 'network-connect' }
    Start-Sleep -Seconds 5
    $before = (Get-Content $cachePath -Raw | ConvertFrom-Json).updated_unix
    Send-WidgetMessage 0x8006
    Assert-Check 'network recovers to auth rejection' (Wait-PollState 'auth_required|401|403' $before) 'fake credentials remain invalid after reconnection'
    Copy-Item $cachePath "$evidence\network-recovered.json"
    Assert-Check 'recovery message replaces offline error' ((Get-TrayTooltip -ExpectedPattern 'rejected|401|403|Sign in again') -match 'rejected|401|403|Sign in again') 'live tooltip'
    Assert-NoPanic 'network'
}
Invoke-Subcase 'privacy-disabled-providers' {
    Write-Settings @{language='en'; poll_interval_ms=3600000; show_claude_code=$false; show_codex=$false; show_antigravity=$false; show_opencode=$false; show_cursor=$false; show_grok=$false; last_update_check_unix=[DateTimeOffset]::UtcNow.ToUnixTimeSeconds()}
    $null = Invoke-HostAction 'audit-start'
    try {
        $null = Start-Process $executable -ArgumentList '--diagnose'
        Assert-App $executable 'privacy'
        $watch = [Diagnostics.Stopwatch]::StartNew()
        while ($watch.Elapsed.TotalSeconds -lt 60) { Start-Sleep -Seconds 1 }
    } finally { $capture = Invoke-HostAction 'audit-stop' }
    $saved = Get-Content $settingsPath -Raw | ConvertFrom-Json
    $enabled = @('show_claude_code','show_codex','show_antigravity','show_opencode','show_cursor','show_grok' | Where-Object { $saved.$_ })
    # Fail this precondition explicitly on releases that force Claude back on.
    Assert-Check 'all providers can remain disabled' ($enabled.Count -eq 0) $enabled -ContinueOnFailure
    Assert-Check 'no outbound TCP or UDP connections while disabled' (@($capture.connections).Count -eq 0) $capture
    Assert-NoPanic 'privacy'
}
Complete-Subcases
