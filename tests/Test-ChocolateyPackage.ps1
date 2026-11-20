# No install, download, process termination, registry writes, or API key needed.
[CmdletBinding()]
param([string]$BinaryPath)
$ErrorActionPreference = 'Stop'
$packageRoot = Join-Path (Split-Path -Parent $PSScriptRoot) 'packaging\chocolatey'
$tools = Join-Path $packageRoot 'tools'
. (Join-Path $tools 'helpers.ps1')

function Assert-True($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Assert-Throws([scriptblock]$Action, [string]$Expected) {
    try { & $Action } catch {
        if ($_.Exception.Message -like "*$Expected*") { return }
        throw
    }
    throw "Expected failure containing: $Expected"
}

# Mock process enumeration, including a separate dashboard and another install.
$processState = @{ Items = @() }
Set-Item Function:Get-Process -Value ({ param($Name, $ErrorAction) return $processState.Items }.GetNewClosure())
$target = Join-Path $tools 'claude-code-usage-monitor.exe'
$processState.Items = @([pscustomobject]@{ Path = 'C:\Other\claude-code-usage-monitor.exe' })
Assert-MonitorStopped $tools
$processState.Items += [pscustomobject]@{ Path = $target.ToUpperInvariant() }
Assert-Throws { Assert-MonitorStopped $tools } 'Save your work'
Assert-Throws { & (Join-Path $tools 'chocolateybeforemodify.ps1') } 'Save your work'
Assert-Throws { & (Join-Path $tools 'chocolateyuninstall.ps1') } 'Save your work'
Assert-Throws { & (Join-Path $tools 'chocolateyinstall.ps1') } 'Save your work'
$processState.Items = @([pscustomobject]@{ Path = $null })
Assert-Throws { Assert-MonitorStopped $tools } 'Cannot verify'
$processState.Items = @()

Assert-True (Test-MonitorStartupTarget $target $target) 'Unquoted startup target not recognized.'
Assert-True (Test-MonitorStartupTarget ('"' + $target + '"') $target) 'Quoted startup target not recognized.'
Assert-True (-not (Test-MonitorStartupTarget ($target + ' --dashboard') $target)) 'Unexpected startup arguments must be preserved.'
Assert-True (-not (Test-MonitorStartupTarget 'C:\Other\claude-code-usage-monitor.exe' $target)) 'Another installation must be preserved.'

foreach ($sid in @('S-1-5-21-1-2-3-1001', 'S-1-12-1-1111-2222-3333-4444')) {
    Assert-True (Test-MonitorUserSid $sid) "User hive not recognized: $sid"
}
foreach ($sid in @('.DEFAULT', 'S-1-5-18', 'S-1-5-19', 'S-1-5-21-1-2-3-1001_Classes', 'S-1-12-1-1111-2222-3333-4444_Classes')) {
    Assert-True (-not (Test-MonitorUserSid $sid)) "Non-user hive accepted: $sid"
}

Assert-True (Test-SupportedWindows 'AMD64' 10 19045) 'Windows 10 x64 must be supported.'
Assert-True (Test-SupportedWindows 'ARM64' 10 22000) 'Windows 11 on Arm emulates x64.'
Assert-True (-not (Test-SupportedWindows 'ARM64' 10 19045)) 'Windows 10 on Arm cannot run x64 binaries.'
Assert-True (-not (Test-SupportedWindows 'x86' 10 19045)) '32-bit Windows must be rejected.'
Assert-True (-not (Test-SupportedWindows 'AMD64' 6 9600)) 'Windows 8.1 must be rejected.'
# Read-only registry query; CI and development machines are supported hosts.
Assert-SupportedWindows

# Exercise loaded-user registry cleanup against an in-memory hive, including a
# different user from the elevated installer, an Entra ID account, and a value
# owned by another copy.
$runOwned = [pscustomobject]@{ Value = '"' + $target + '"'; Deleted = $false; Closed = $false }
$runEntra = [pscustomobject]@{ Value = $target; Deleted = $false; Closed = $false }
$runOther = [pscustomobject]@{ Value = 'C:\Other\claude-code-usage-monitor.exe'; Deleted = $false; Closed = $false }
foreach ($run in @($runOwned, $runEntra, $runOther)) {
    $run | Add-Member ScriptMethod GetValue { param($Name, $Default, $Options) return $this.Value }
    $run | Add-Member ScriptMethod DeleteValue { param($Name, $Throw) $this.Deleted = $true }
    $run | Add-Member ScriptMethod Close { $this.Closed = $true }
}
$users = [pscustomobject]@{ Owned = $runOwned; Entra = $runEntra; Other = $runOther; Closed = $false }
$users | Add-Member ScriptMethod GetSubKeyNames {
    return @('S-1-5-21-1-2-3-1001', 'S-1-12-1-1111-2222-3333-4444', 'S-1-5-21-1-2-3-1002', 'S-1-5-21-1-2-3-1001_Classes', '.DEFAULT')
}
$users | Add-Member ScriptMethod OpenSubKey {
    param($Path, $Writable)
    if ($Path.StartsWith('S-1-5-21-1-2-3-1001\')) { return $this.Owned }
    if ($Path.StartsWith('S-1-12-1-1111-2222-3333-4444\')) { return $this.Entra }
    if ($Path.StartsWith('S-1-5-21-1-2-3-1002\')) { return $this.Other }
    throw "Unexpected hive: $Path"
}
$users | Add-Member ScriptMethod Close { $this.Closed = $true }
Remove-MonitorStartupEntries $tools $users
Assert-True ($runOwned.Deleted -and $runEntra.Deleted -and -not $runOther.Deleted) 'Startup cleanup touched the wrong installation.'
Assert-True ($runOwned.Closed -and $runEntra.Closed -and $runOther.Closed) 'Registry handles not closed.'
Assert-True (-not $users.Closed) 'The caller-owned HKEY_USERS handle must stay open.'

# Run the installer with a mocked download helper to verify arguments without
# installing anything. Render tokens in memory as the package generator does.
$downloadState = @{ Arguments = $null }
Set-Item Function:Get-ChocolateyWebFile -Value ({
    param($PackageName, $FileFullPath, $Url64bit, $Checksum64, $ChecksumType64)
    $downloadState.Arguments = $PSBoundParameters
}.GetNewClosure())
$installer = (Get-Content (Join-Path $tools 'chocolateyinstall.ps1') -Raw).
    Replace('__VERSION__', '2.25.0').Replace('__CHECKSUM__', ('a' * 64))
# The script uses its own path for helper discovery; use a temporary package.
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('ccum-choco-test-' + [guid]::NewGuid().ToString('N'))
try {
    New-Item -ItemType Directory -Path $temporary | Out-Null
    Copy-Item (Join-Path $tools '*') $temporary
    $installPath = Join-Path $temporary 'chocolateyinstall.ps1'
    Set-Content -LiteralPath $installPath -Value $installer
    & $installPath
    Assert-True ($downloadState.Arguments.ChecksumType64 -eq 'sha256' -and $downloadState.Arguments.Checksum64 -eq ('a' * 64)) 'Missing pinned checksum.'
    Assert-True ($downloadState.Arguments.Url64bit -eq 'https://github.com/CodeZeno/Claude-Code-Usage-Monitor/releases/download/v2.25.0/claude-code-usage-monitor.exe') 'Download must target an exact release.'
    Assert-True ($downloadState.Arguments.FileFullPath -eq (Join-Path $temporary 'claude-code-usage-monitor.exe')) 'Executable must stay in the package directory.'
    Assert-True (Test-Path (Join-Path $temporary 'claude-code-usage-monitor.exe.gui')) 'Missing GUI shim marker.'
    Assert-True ((Get-Content (Join-Path $temporary 'chocolatey-package') -Raw).Trim() -eq 'claude-code-usage-monitor') 'Missing installation-channel marker.'
    if ($BinaryPath) {
        $binary = Get-Item -LiteralPath $BinaryPath
        $version = $binary.VersionInfo.FileVersion
        $generated = Join-Path $temporary 'generated'
        $generator = Join-Path $packageRoot 'New-Package.ps1'
        & $generator -Version $version -BinaryPath $binary.FullName -OutputDirectory $generated
        [xml]$manifest = Get-Content (Join-Path $generated 'claude-code-usage-monitor.nuspec') -Raw
        Assert-True ($manifest.package.metadata.version -eq $version) 'Generated package version mismatch.'
        foreach ($field in @('projectSourceUrl', 'docsUrl', 'iconUrl')) {
            Assert-True ([string]$manifest.package.metadata.$field -like 'https://*') "Generated package is missing $field."
        }
        $renderedInstall = Get-Content (Join-Path $generated 'tools\chocolateyinstall.ps1') -Raw
        $hash = (Get-FileHash $binary.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        Assert-True ($renderedInstall.Contains($hash) -and $renderedInstall.Contains("/v$version/")) 'Generated download is not pinned to the built binary.'
        Assert-True (-not $renderedInstall.Contains('__')) 'Unresolved package template token.'
        Assert-True (-not (Test-Path (Join-Path $generated 'tools\claude-code-usage-monitor.exe'))) 'Source package must not embed the binary.'
        Assert-Throws { & $generator -Version $version -BinaryPath $binary.FullName -OutputDirectory $generated } 'new output directory'
        Assert-Throws { & $generator -Version '0.0.0' -BinaryPath $binary.FullName -OutputDirectory (Join-Path $temporary 'wrong-version') } 'does not match'
    }
} finally {
    # Only remove the unique temporary directory created above.
    if ([IO.Path]::GetFullPath($temporary).StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()), [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $temporary -Recurse -Force
    }
}
Write-Host 'Chocolatey package safeguards passed.'
