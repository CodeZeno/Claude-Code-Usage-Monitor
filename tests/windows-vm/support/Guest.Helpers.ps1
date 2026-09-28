param([Parameter(Mandatory)]$Context)
# Dot-source into the runner/case scope. All per-scenario state comes from Context.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$Root = $Context.Root
if ($Root -notmatch '^C:\\CCUM-Lab\\[a-f0-9]{32}$' -or
    -not (Test-Path -LiteralPath 'C:\CCUM-Lab\guest-enabled') -or
    (Get-CimInstance Win32_ComputerSystem).Model -ne 'Virtual Machine') {
    throw 'Run only inside an explicitly prepared disposable Hyper-V guest.'
}
$request = $Context.Request
$evidence = $Context.Evidence
$sessionId = $Context.SessionId
$appName = 'claude-code-usage-monitor'
$packageId = 'CodeZeno.ClaudeCodeUsageMonitor'
$settingsPath = $Context.SettingsPath

function Assert-Check([string]$Name, [bool]$Condition, $Detail, [switch]$ContinueOnFailure) {
    $Context.Checks.Add([pscustomobject]@{ name = $Name; passed = $Condition; detail = $Detail })
    # Keep the last completed assertion available even if a desktop API stalls
    # and the host must time out the scheduled task before its finally block.
    try {
        @{ name = $Name; passed = $Condition; atUtc = [DateTime]::UtcNow.ToString('o') } |
            ConvertTo-Json | Set-Content -LiteralPath "$evidence\progress.json" -Encoding UTF8
    } catch {
        # A reader may hold this advisory file open. Never let progress reporting
        # replace the real assertion outcome; result.json retains every check.
        Write-Verbose "Progress snapshot unavailable: $($_.Exception.Message)"
    }
    if (-not $Condition -and -not $ContinueOnFailure) { throw [InvalidOperationException]::new("Assertion failed: $Name ($Detail)") }
}
function Get-AppProcesses {
    @(Get-Process -Name $appName -ErrorAction SilentlyContinue | Where-Object SessionId -EQ $sessionId)
}
function Stop-App {
    foreach ($process in (Get-AppProcesses)) {
        Stop-Process -Id $process.Id -Force
        Wait-Process -Id $process.Id -Timeout 15 -ErrorAction SilentlyContinue
    }
}
function Save-Screenshot([string]$Name) {
    if (-not [CCUMLabDesktop]::IsDefaultDesktop()) { throw 'The input desktop is locked or unavailable.' }
    $bounds = [Windows.Forms.SystemInformation]::VirtualScreen
    $bitmap = [Drawing.Bitmap]::new($bounds.Width, $bounds.Height)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($bounds.Location, [Drawing.Point]::Empty, $bounds.Size)
        $bitmap.Save("$evidence\$Name.png", [Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
}
function Assert-App([string]$Executable, [string]$Label) {
    $deadline = [DateTime]::UtcNow.AddSeconds(40)
    do {
        $processes = @(Get-AppProcesses)
        $windows = @()
        if ($processes.Count -eq 1) { $windows = @([CCUMLabDesktop]::WindowsForProcess($processes[0].Id)) }
        $visible = @($windows | Where-Object { $_.Visible -and $_.Right -gt $_.Left -and $_.Bottom -gt $_.Top })
        if ($visible.Count -gt 0) { break }
        Start-Sleep -Milliseconds 500
    } until ([DateTime]::UtcNow -gt $deadline)
    Assert-Check "$Label single instance" ($processes.Count -eq 1) $processes.Count
    Assert-Check "$Label executable path" ($processes[0].Path -eq $Executable) $processes[0].Path
    Assert-Check "$Label visible widget" ($visible.Count -gt 0) $windows
    # Auto-hide may put the widget off-screen by design. Record, do not mislabel it as clipping.
    if ($request.taskbar -ne 'auto-hide') {
        $screen = [Windows.Forms.SystemInformation]::VirtualScreen
        $intersecting = @($visible | Where-Object { $_.Right -gt $screen.Left -and $_.Left -lt $screen.Right -and $_.Bottom -gt $screen.Top -and $_.Top -lt $screen.Bottom })
        Assert-Check "$Label intersects display" ($intersecting.Count -gt 0) $visible
    }
    ConvertTo-Json -InputObject $windows -Depth 5 | Set-Content -LiteralPath "$evidence\$Label-windows.json" -Encoding UTF8
    Save-Screenshot $Label
}
function Assert-Settings {
    $settings = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
    $language = if ($settings -and $settings.PSObject.Properties['language']) { $settings.language } else { $null }
    $interval = if ($settings -and $settings.PSObject.Properties['poll_interval_ms']) { $settings.poll_interval_ms } else { $null }
    Assert-Check 'language preserved' ($language -eq 'de') $language
    Assert-Check 'poll interval preserved' ($interval -eq 900000) $interval
}
function Invoke-WinGet([string[]]$Arguments) {
    $output = & winget @Arguments 2>&1
    $code = $LASTEXITCODE
    Add-Content -LiteralPath "$evidence\winget.log" -Value (($Arguments -join ' ') + "`r`n" + ($output -join "`r`n") + "`r`nExit: $code")
    Assert-Check ('winget ' + $Arguments[0]) ($code -eq 0) $code
}
function Find-WinGetExe {
    $packages = "$env:LOCALAPPDATA\Microsoft\WinGet\Packages"
    $matches = @(Get-ChildItem -LiteralPath $packages -Directory | Where-Object Name -Like "$packageId`_*" |
        Get-ChildItem -Recurse -File -Filter "$appName.exe")
    if ($matches.Count -ne 1) { throw "Expected one per-user WinGet executable; found $($matches.Count)." }
    return $matches[0].FullName
}
function Assert-Version([string]$Executable, [string]$Expected) {
    $version = (Get-Item -LiteralPath $Executable).VersionInfo.ProductVersion
    Assert-Check 'installed product version' ($version -eq $Expected -or $version -eq "$Expected.0") $version
}

function Set-TestTaskbar {
    param([switch]$PromoteTray)
    if ($request.taskbar -in @('left', 'center') -or ($PromoteTray -and $request.os -eq 'windows10')) {
        if ($request.taskbar -in @('left', 'center')) {
            Assert-Check 'alignment supported' ($request.os -eq 'windows11') $request.os
            $alignment = if ($request.taskbar -eq 'left') { 0 } else { 1 }
            $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced'
            Set-ItemProperty -LiteralPath $key -Name TaskbarAl -Value $alignment -Type DWord
        } else {
            # Windows 10 reads this on Explorer startup. Make new icons visible.
            Set-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer' -Name EnableAutoTray -Value 0 -Type DWord
        }
        Get-Process explorer | Where-Object SessionId -EQ $sessionId | Stop-Process -Force
        Start-Sleep -Seconds 3
        if (-not @(Get-Process explorer -ErrorAction SilentlyContinue | Where-Object SessionId -EQ $sessionId).Count) {
            Start-Process explorer.exe
        }
        Start-Sleep -Seconds 5
        if ($request.taskbar -in @('left', 'center')) {
            Assert-Check 'alignment preference applied' ((Get-ItemProperty -LiteralPath $key).TaskbarAl -eq $alignment) $alignment
        }
    }
    $state = [CCUMLabDesktop]::TaskbarState($request.taskbar -eq 'auto-hide')
    Assert-Check 'expected auto-hide state' ((($state -band 1) -eq 1) -eq ($request.taskbar -eq 'auto-hide')) $state
    @{ taskbar = $request.taskbar; appbarState = $state; screen = [Windows.Forms.SystemInformation]::VirtualScreen.ToString() } |
        ConvertTo-Json | Set-Content -LiteralPath "$evidence\desktop.json" -Encoding UTF8
}

function Initialize-TestSettings {
    param([switch]$TrayFixture)
    $null = New-Item -ItemType Directory -Path (Split-Path -Parent $settingsPath) -Force
    '{"language":"de","poll_interval_ms":900000}' | Set-Content -LiteralPath $settingsPath -Encoding ASCII
    if ($TrayFixture -and $request.trayTheme -eq 'compact') {
        # Fit the native 40px Windows 10 bar as well as Windows 11's 48px bar.
        $themePath = "$Root\tray-fixture.json"
        @{
            schema_version=1; id='tray-fixture'; name='Tray regression fixture'
            surfaces=@(@{
                id='main'; name='Tray regression'; width='120'; height='32'
                background=@{type='colour'; colour=@{color='#2080D0FF'}}
                placement=@{reference=@{region='system_tray'; display=0}; nest='taskbar'; horizontal='left'; vertical='bottom'; surface_horizontal='right'; surface_vertical='bottom'; offset_x=-80}
            })
        } | ConvertTo-Json -Depth 8 | Set-Content $themePath -Encoding ASCII
        @{language='de'; poll_interval_ms=900000; custom_theme_enabled=$true; active_theme_path=$themePath} |
            ConvertTo-Json | Set-Content $settingsPath -Encoding ASCII
    }
}

function Install-PortableApp {
    param([switch]$Previous)
    Assert-Check 'candidate transport hash' ((Get-FileHash -LiteralPath $Context.CandidateExe).Hash -eq $request.candidateHash) $request.candidateHash
    $null = New-Item -ItemType Directory -Path "$Root\App With Spaces" -Force
    $path = "$Root\App With Spaces\$appName.exe"
    $initial = if ($Previous) { $Context.PreviousExe } else { $Context.CandidateExe }
    Copy-Item -LiteralPath $initial -Destination $path
    return $path
}
function Install-WinGetApp {
    param([string]$Version)
    $null = Get-Command winget -ErrorAction Stop
    $existing = & winget list --id $packageId --exact --source winget --disable-interactivity --accept-source-agreements 2>&1
    Assert-Check 'package absent before install' ($LASTEXITCODE -eq -1978335212) ($existing -join "`n")
    Invoke-WinGet -Arguments @('install', '--id', $packageId, '--exact', '--version', $Version, '--source', 'winget', '--scope', 'user', '--accept-package-agreements', '--accept-source-agreements', '--disable-interactivity')
    $path = Find-WinGetExe
    Assert-Version $path $Version
    return $path
}
function Start-TestApp {
    param([string]$Executable)
    $process = Start-Process -FilePath $Executable -ArgumentList '--diagnose' -PassThru
    Assert-App $Executable 'initial'
    Assert-Settings
    Copy-Item -LiteralPath "$env:TEMP\claude-code-usage-monitor.log" -Destination "$evidence\initial-diagnostics.log"
    return $process
}
function Test-AppRestart {
    param([string]$Executable)
    Stop-App
    $null = Start-Process -FilePath $Executable -ArgumentList '--diagnose-append', '--diagnose' -PassThru
    Assert-App $Executable 'restarted'
    Assert-Settings
    # Retain where evidence collection stops if the VM's task deadline expires.
    $metadataWatch = [Diagnostics.Stopwatch]::StartNew()
    'hash-start' | Set-Content "$evidence\executable-metadata-phase.txt"
    $hash = (Get-FileHash -LiteralPath $Executable).Hash
    "version-start after $($metadataWatch.Elapsed.TotalSeconds) seconds" | Set-Content "$evidence\executable-metadata-phase.txt"
    $version = (Get-Item -LiteralPath $Executable).VersionInfo.ProductVersion
    "complete after $($metadataWatch.Elapsed.TotalSeconds) seconds" | Set-Content "$evidence\executable-metadata-phase.txt"
    @{ path = $Executable; sha256 = $hash; productVersion = $version } |
        ConvertTo-Json | Set-Content -LiteralPath "$evidence\executable.json" -Encoding UTF8
}
function Remove-WinGetApp {
    param([string]$Executable)
    Invoke-WinGet -Arguments @('list', '--id', $packageId, '--exact', '--source', 'winget', '--disable-interactivity', '--accept-source-agreements')
    Stop-App
    Invoke-WinGet -Arguments @('uninstall', '--id', $packageId, '--exact', '--source', 'winget', '--scope', 'user', '--disable-interactivity')
    Assert-Check 'uninstalled executable' (-not (Test-Path -LiteralPath $Executable)) $Executable
}
