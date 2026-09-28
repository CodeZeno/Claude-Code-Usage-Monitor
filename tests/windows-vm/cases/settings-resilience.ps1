param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context
$executable = Install-PortableApp
foreach ($mode in 'corrupt','truncated','utf8-bom','unknown-fields','previous-release','read-only') {
    Invoke-Subcase $mode {
        Initialize-TestSettings
        switch ($mode) {
            'corrupt' { [IO.File]::WriteAllText($settingsPath, 'this is not json') }
            'truncated' { [IO.File]::WriteAllText($settingsPath, '{"language":"de","poll_interval_ms":') }
            'utf8-bom' { [IO.File]::WriteAllText($settingsPath, '{"language":"de","poll_interval_ms":900000}', [Text.UTF8Encoding]::new($true)) }
            'unknown-fields' { Write-Settings @{language='de'; poll_interval_ms=900000; future_setting=@{enabled=$true}} }
            'previous-release' {
                # Obtain a real file emitted by PreviousExe, not a hand-written approximation.
                # A separate directory avoids overwriting an image whose exited
                # process/antivirus handles Windows has not released yet.
                $previousDirectory = "$Root\Previous App With Spaces"
                $null = New-Item -ItemType Directory $previousDirectory -Force
                $previousExecutable = "$previousDirectory\$appName.exe"
                Copy-Item $Context.PreviousExe $previousExecutable -Force
                $null = Start-TestApp $previousExecutable
                Stop-App
                Copy-Item $settingsPath "$evidence\previous-release-settings.json"
            }
            'read-only' { (Get-Item $settingsPath).IsReadOnly = $true }
        }
        $before = (Get-FileHash $settingsPath).Hash
        Copy-Item $settingsPath "$evidence\$mode-input.json" -Force
        try {
            $null = Start-Process $executable -ArgumentList '--diagnose'
            Assert-App $executable $mode
            if ($mode -in 'corrupt','truncated') {
                $saved = Get-Content $settingsPath -Raw | ConvertFrom-Json
                Assert-Check "$mode defaults restored" ($saved.poll_interval_ms -ge 60000 -and $saved.show_claude_code) $saved
            } else { Assert-Settings }
            if ($mode -eq 'read-only') {
                Assert-Check 'read-only original preserved' ((Get-FileHash $settingsPath).Hash -eq $before) $before
                Assert-Check 'save failure is logged' ((Get-Content $logPath -Raw) -match '(?i)unable to persist|unable to save|settings.*(failed|error)|Unable to replace the settings file') 'diagnostic log'
            }
            Assert-NoPanic $mode
        } finally {
            if (Test-Path $settingsPath) {
                Copy-Item $settingsPath "$evidence\$mode-output.json" -Force
                (Get-Item $settingsPath).IsReadOnly = $false
            }
        }
    }
}
Complete-Subcases
