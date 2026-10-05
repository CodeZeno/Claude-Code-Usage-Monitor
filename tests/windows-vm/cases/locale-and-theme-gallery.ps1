param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
$executable = Install-PortableApp
$gallery = [Collections.Generic.List[object]]::new()
# Covers LanguageId::ALL and horizontal built-ins. Classic Vertical is covered
# on Windows 10 by vertical-taskbar.ps1; Test-Harness verifies catalogue drift.
$languages = @('en','de','es','fr','ja','ko','nl','pl','pt-BR','ru','th','tr','zh-CN','zh-TW')
$themes = @('classic-usage-widget','compact-fluent-quad')
try {
    # Install managed built-ins using the real app before selecting their paths.
    Initialize-TestSettings
    $null = Start-TestApp $executable
    Stop-App
    foreach ($language in $languages) {
        foreach ($theme in $themes) {
            $label = "gallery-$language-$theme"
            Invoke-Subcase $label {
                $themePath = "$(Split-Path $settingsPath)\themes\$theme.json"
                Assert-Check 'managed built-in theme exists' (Test-Path $themePath) $themePath
                Write-Settings @{language=$language; poll_interval_ms=900000; active_theme_path=$themePath}
                $null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
                Assert-App $executable "$label-widget"
                Assert-Image "$label-widget" (Get-Widget).Handle
                $gallery.Add(@{label="$language / $theme widget"; file="$label-widget.png"})
                $dashboard = Open-Dashboard $executable $label
                Assert-Image "$label-dashboard" $dashboard
                $gallery.Add(@{label="$language / $theme dashboard"; file="$label-dashboard.png"})
                Close-Dashboard $dashboard
                $saved = Get-Content $settingsPath -Raw | ConvertFrom-Json
                Assert-Check 'requested language retained' ($saved.language -eq $language) $saved.language
                Assert-Check 'requested built-in retained' ($saved.active_theme_path -eq $themePath) $saved.active_theme_path
                Assert-NoPanic $label
            }
        }
    }
    Invoke-Subcase 'malformed-custom-theme' {
        [IO.File]::WriteAllText("$Root\broken-theme.json", '{"surfaces":[')
        Write-Settings @{language='en'; poll_interval_ms=900000; active_theme_path="$Root\broken-theme.json"}
        $null = Start-Process $executable -ArgumentList '--diagnose --no-poll'
        Assert-App $executable 'malformed-theme-fallback'
        $saved = Get-Content $settingsPath -Raw | ConvertFrom-Json
        Assert-Check 'malformed theme falls back to built-in' ((Split-Path $saved.active_theme_path -Leaf) -in @($themes | ForEach-Object { "$_.json" })) $saved.active_theme_path
        Assert-Image 'malformed-theme-fallback' (Get-Widget).Handle
        Assert-NoPanic 'malformed-theme'
    }
    Complete-Subcases
} finally { ConvertTo-Json -InputObject @($gallery.ToArray()) | Set-Content "$evidence\gallery.json" -Encoding UTF8 }
