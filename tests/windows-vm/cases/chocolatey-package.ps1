# Invoked only by the guarded guest runner. Chocolatey commands run elevated
# through the host's allowlisted action; the monitor runs in this desktop task.
param([Parameter(Mandatory)]$Context)
. "$($Context.Root)\support\Scenario.Helpers.ps1" -Context $Context

$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
function Invoke-Choco([string]$Operation, [string]$Version, [string]$Label, [int]$TimeoutSeconds = 300) {
    $arguments = @{ operation = $Operation }
    if ($Version) { $arguments.version = $Version }
    $data = Invoke-HostAction 'chocolatey' $arguments $TimeoutSeconds
    @("operation: $Operation $Version", "exit: $($data.exitCode)", "installed: $($data.installedVersion)", '', $data.output) |
        Set-Content -LiteralPath "$evidence\choco-$Label.txt" -Encoding UTF8
    return $data
}
function Get-StartupValue {
    $entry = Get-ItemProperty -LiteralPath $runKey -ErrorAction SilentlyContinue
    if ($entry -and $entry.PSObject.Properties['ClaudeCodeUsageMonitor']) { return $entry.ClaudeCodeUsageMonitor }
    return $null
}
function Start-ViaShim([string]$Shim, [string]$Arguments, [string]$Label) {
    $info = [Diagnostics.ProcessStartInfo]::new($Shim, $Arguments)
    $info.UseShellExecute = $false
    $launcher = [Diagnostics.Process]::Start($info)
    try {
        # A GUI shim starts the target and returns instead of waiting for it.
        Assert-Check "$Label shim returns without waiting" ($launcher.WaitForExit(20000)) $Shim
        Assert-Check "$Label shim exit code" ($launcher.ExitCode -eq 0) $launcher.ExitCode
    } finally {
        if (-not $launcher.HasExited) { Stop-Process -Id $launcher.Id -Force }
        $launcher.Dispose()
    }
}
function Open-TrayMenuItem([string]$Label) {
    Assert-Tray $Label
    $owner = Get-TrayOwner
    $iconId = Get-RegisteredTrayIcon
    Assert-Check "tray icon for $Label" ($iconId -ge 0) $iconId
    [CCUMScenarioDesktop]::Post($owner.Handle, 0x8003, $iconId, 0x205)
    Assert-Check "menu opened for $Label" (Wait-Condition { [CCUMScenarioDesktop]::MenuVisible() } 10) 'native popup'
    [CCUMScenarioDesktop]::ChooseMenuItem($Label)
}
function Assert-Refused([string]$Operation, [string]$Version, [string]$Label, [string]$Executable, [string]$ExpectedVersion, [string]$Hash) {
    $before = @(Get-AppProcesses | ForEach-Object Id | Sort-Object)
    $data = Invoke-Choco $Operation $Version $Label
    Assert-Check "$Label refused" ($data.exitCode -ne 0) $data.exitCode -ContinueOnFailure
    Assert-Check "$Label explains how to proceed" ($data.output -match 'Save your work and quit the monitor and dashboard') 'choco output' -ContinueOnFailure
    $after = @(Get-AppProcesses | ForEach-Object Id | Sort-Object)
    Assert-Check "$Label did not terminate processes" (($before -join ',') -eq ($after -join ',')) @{ before = $before; after = $after }
    Assert-Check "$Label kept installed version" ($data.installedVersion -eq $ExpectedVersion) $data.installedVersion
    Assert-Check "$Label kept executable" ((Test-Path -LiteralPath $Executable) -and (Get-FileHash -LiteralPath $Executable).Hash -eq $Hash) $Executable
    return $data
}

Initialize-TestSettings
# Windows 10 hides new notification icons until Explorer restarts with them shown.
if ($request.os -eq 'windows10') { Set-TestTaskbar -PromoteTray }
Assert-Check 'candidate transport hash' ((Get-FileHash -LiteralPath $Context.CandidateExe).Hash -eq $request.candidateHash) $request.candidateHash
$bootstrap = Invoke-Choco 'bootstrap' $null 'bootstrap' 900
Assert-Check 'chocolatey bootstrapped' ($bootstrap.exitCode -eq 0) $bootstrap.exitCode
$chocoRoot = $bootstrap.root
$packageDir = "$chocoRoot\lib\claude-code-usage-monitor\tools"
$executable = "$packageDir\claude-code-usage-monitor.exe"
$shim = "$chocoRoot\bin\claude-code-usage-monitor.exe"

# Render the working tree's package with the real generator, then point the
# pinned download at the transported candidate. The older package reuses the
# same binary under a lower version to exercise Chocolatey's upgrade path.
$candidateVersion = (Get-Item -LiteralPath $Context.CandidateExe).VersionInfo.FileVersion
$previousVersion = '1.0.0'
$build = "$Root\choco-build"
& "$Root\packaging\chocolatey\New-Package.ps1" -Version $candidateVersion -BinaryPath $Context.CandidateExe -OutputDirectory "$build\$candidateVersion"
Copy-Item -LiteralPath "$build\$candidateVersion" -Destination "$build\$previousVersion" -Recurse
$utf8 = [Text.UTF8Encoding]::new($false)
$downloadUrl = [uri]::new($Context.CandidateExe).AbsoluteUri
foreach ($version in @($candidateVersion, $previousVersion)) {
    $install = "$build\$version\tools\chocolateyinstall.ps1"
    $published = "https://github.com/CodeZeno/Claude-Code-Usage-Monitor/releases/download/v$candidateVersion/claude-code-usage-monitor.exe"
    $text = [IO.File]::ReadAllText($install)
    Assert-Check "rendered URL $version" ($text.Contains($published)) $published
    [IO.File]::WriteAllText($install, $text.Replace($published, $downloadUrl), $utf8)
    $nuspec = "$build\$version\claude-code-usage-monitor.nuspec"
    $text = [IO.File]::ReadAllText($nuspec).Replace("<version>$candidateVersion</version>", "<version>$version</version>")
    [IO.File]::WriteAllText($nuspec, $text, $utf8)
    $pack = Invoke-Choco 'pack' $version "pack-$version"
    Assert-Check "packed $version" ($pack.exitCode -eq 0) $pack.exitCode
}

$install = Invoke-Choco 'install' $previousVersion 'install'
Assert-Check 'install succeeds' ($install.exitCode -eq 0 -and $install.installedVersion -eq $previousVersion) @{ exit = $install.exitCode; version = $install.installedVersion }
Assert-Check 'executable downloaded and verified' ((Get-FileHash -LiteralPath $executable).Hash -eq $request.candidateHash) $executable
Assert-Check 'channel marker installed' ((Get-Content -LiteralPath "$packageDir\chocolatey-package" -Raw).Trim() -eq 'claude-code-usage-monitor') $packageDir
Assert-Check 'GUI shim created' (Test-Path -LiteralPath $shim) $shim
Assert-Check 'install does not launch the monitor' (@(Get-AppProcesses).Count -eq 0) (@(Get-AppProcesses).Count)
$hash = $request.candidateHash

Start-ViaShim $shim '--diagnose' 'widget'
Assert-App $executable 'shim-launch'
Assert-Settings
# Automatic checks are non-interactive and must not raise the Chocolatey prompt.
Start-Sleep -Seconds 5
$monitor = (Get-AppProcesses)[0]
Assert-Check 'no update prompt at startup' ([CCUMScenarioDesktop]::ReadDialog($monitor.Id, 'Updates').Length -eq 0) 'Updates dialog'

Assert-Tray 'chocolatey-startup'
Toggle-Startup
Assert-StartupTarget $executable
$startup = Get-StartupValue

# The update action reports Chocolatey ownership instead of checking GitHub.
Open-TrayMenuItem 'Nach Updates suchen'
$null = Wait-Condition { [CCUMScenarioDesktop]::ReadDialog($monitor.Id, 'Updates').Length -gt 0 } 30
$message = [CCUMScenarioDesktop]::ReadDialog($monitor.Id, 'Updates')
$message | Set-Content -LiteralPath "$evidence\update-message.txt" -Encoding UTF8
Save-Screenshot 'update-message'
Assert-Check 'update action shows Chocolatey command' ($message.Contains('choco upgrade claude-code-usage-monitor')) $message
Assert-Check 'update message is localized' ($message.Contains('von Chocolatey verwaltet')) $message
Assert-Check 'update message dismissed' (Wait-Condition { [CCUMScenarioDesktop]::DismissDialog($monitor.Id, 'Updates') } 10) 'OK button'
Assert-Check 'update action left the executable alone' ((Get-FileHash -LiteralPath $executable).Hash -eq $hash) $executable

# A separate dashboard process launched through the shim also blocks upgrades.
Start-ViaShim $shim '--dashboard --diagnose --diagnose-append' 'dashboard'
Assert-Check 'shim forwards --dashboard' (Wait-Condition { [CCUMScenarioDesktop]::Dashboard($executable) -ne 0 } 40) $executable
$dashboard = [CCUMScenarioDesktop]::Dashboard($executable)
Save-Screenshot 'dashboard-open'
$refused = Assert-Refused 'upgrade' $candidateVersion 'upgrade-dashboard-open' $executable $previousVersion $hash
Assert-Check 'before-modify guard ran' ($refused.output -match '(?i)chocolateyBeforeModify') 'choco output' -ContinueOnFailure
Close-Dashboard $dashboard
$null = Assert-Refused 'upgrade' $candidateVersion 'upgrade-monitor-open' $executable $previousVersion $hash
Assert-Check 'refused upgrade kept startup entry' ((Get-StartupValue) -eq $startup) (Get-StartupValue)

Stop-App
$upgrade = Invoke-Choco 'upgrade' $candidateVersion 'upgrade'
Assert-Check 'upgrade succeeds after quitting' ($upgrade.exitCode -eq 0 -and $upgrade.installedVersion -eq $candidateVersion) @{ exit = $upgrade.exitCode; version = $upgrade.installedVersion }
Assert-Check 'upgrade keeps executable path' ((Get-FileHash -LiteralPath $executable).Hash -eq $hash) $executable
Assert-Check 'upgrade keeps startup entry' ((Get-StartupValue) -eq $startup) (Get-StartupValue)
Assert-Check 'upgrade does not relaunch the monitor' (@(Get-AppProcesses).Count -eq 0) (@(Get-AppProcesses).Count)
Start-ViaShim $shim '--diagnose --diagnose-append' 'relaunch'
Assert-App $executable 'post-upgrade'
Assert-Settings

$null = Assert-Refused 'uninstall' $null 'uninstall-monitor-open' $executable $candidateVersion $hash
Stop-App
$uninstall = Invoke-Choco 'uninstall' $null 'uninstall'
Assert-Check 'uninstall succeeds after quitting' ($uninstall.exitCode -eq 0 -and -not $uninstall.installedVersion) @{ exit = $uninstall.exitCode; version = $uninstall.installedVersion }
Assert-Check 'uninstall removed executable' (-not (Test-Path -LiteralPath $executable)) $executable
Assert-Check 'uninstall removed shim' (-not (Test-Path -LiteralPath $shim)) $shim
Assert-StartupDisabled
Assert-Settings

# Another copy keeps running, and keeps its startup entry, across uninstall.
$reinstall = Invoke-Choco 'install' $candidateVersion 'reinstall'
Assert-Check 'reinstall succeeds' ($reinstall.exitCode -eq 0) $reinstall.exitCode
$portable = Install-PortableApp
$null = Start-Process -FilePath $portable -ArgumentList '--diagnose', '--diagnose-append' -PassThru
Assert-App $portable 'portable'
Assert-Tray 'portable-startup'
Toggle-Startup
Assert-StartupTarget $portable
$portableStartup = Get-StartupValue
$other = Invoke-Choco 'uninstall' $null 'uninstall-with-portable-running'
Assert-Check 'uninstall ignores another running copy' ($other.exitCode -eq 0 -and -not $other.installedVersion) @{ exit = $other.exitCode; version = $other.installedVersion }
Assert-Check 'portable copy still running' (@(Get-AppProcesses | Where-Object Path -EQ $portable).Count -eq 1) $portable
Assert-Check 'portable startup entry preserved' ((Get-StartupValue) -eq $portableStartup) (Get-StartupValue)
Assert-Check 'portable executable preserved' (Test-Path -LiteralPath $portable) $portable
Assert-Tray 'portable-cleanup'
Toggle-Startup
Assert-StartupDisabled
