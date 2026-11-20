# Prepare package sources only. Packing and publishing are separate operations.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidatePattern('^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$')][string]$Version,
    [Parameter(Mandatory)][string]$BinaryPath,
    [Parameter(Mandatory)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$binary = Get-Item -LiteralPath $BinaryPath
if ($binary.VersionInfo.FileVersion -ne $Version) {
    throw "Executable version '$($binary.VersionInfo.FileVersion)' does not match '$Version'."
}
# Do not silently publish a different architecture under this x64-only package.
$stream = [IO.File]::OpenRead($binary.FullName)
$reader = New-Object IO.BinaryReader($stream)
try {
    if ($reader.ReadUInt16() -ne 0x5A4D) { throw 'Not a Windows executable.' }
    $stream.Position = 0x3C
    $stream.Position = $reader.ReadInt32()
    if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {
        throw 'The Chocolatey package requires an x64 PE executable.'
    }
} finally { $reader.Dispose() }
$checksum = (Get-FileHash -LiteralPath $binary.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
if (Test-Path -LiteralPath $OutputDirectory) {
    throw 'Use a new output directory to avoid packaging stale files.'
}
$output = New-Item -ItemType Directory -Path $OutputDirectory
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'tools') -Destination $output.FullName -Recurse
$utf8 = New-Object Text.UTF8Encoding($false)
foreach ($relative in @('claude-code-usage-monitor.nuspec', 'tools\chocolateyinstall.ps1')) {
    $template = Get-Content -LiteralPath (Join-Path $PSScriptRoot $relative) -Raw
    $rendered = $template.Replace('__VERSION__', $Version).Replace('__CHECKSUM__', $checksum)
    [IO.File]::WriteAllText((Join-Path $output.FullName $relative), $rendered, $utf8)
}
Write-Host "Prepared claude-code-usage-monitor $Version (SHA-256 $checksum) in $($output.FullName)"
