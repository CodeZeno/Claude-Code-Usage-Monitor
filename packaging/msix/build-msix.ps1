[CmdletBinding()]
param(
    [ValidateRange(0, 65535)]
    [int]$Revision = 0,

    [string]$CertificateThumbprint,

    [switch]$CreateAndTrustLocalTestCertificate,

    [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$targetRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'target\msix'))
$cargoTargetRoot = Join-Path $targetRoot 'cargo-target'
$stagingRoot = Join-Path $targetRoot 'staging'
$manifestSource = Join-Path $PSScriptRoot 'AppxManifest.xml'
$assetsSource = Join-Path $PSScriptRoot 'Assets'
$cargoToml = Join-Path $repoRoot 'Cargo.toml'
$binaryPath = Join-Path $cargoTargetRoot 'release\claude-code-usage-monitor.exe'
$bridgeBinaryPath = Join-Path $cargoTargetRoot 'release\aum-quota.exe'
$publisher = 'CN=AIUsageMonitorLocalTrial'
$packageName = 'Ysawase.AIUsageMonitor.LocalTrial'

function Assert-PathUnderTarget([string]$Path) {
    $fullPath = [System.IO.Path]::GetFullPath($Path)
    $targetPrefix = $targetRoot.TrimEnd('\') + '\'
    if (-not $fullPath.StartsWith($targetPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to modify a path outside the MSIX target directory: $fullPath"
    }
}

function Find-WindowsSdkTool([string]$Name) {
    $sdkBin = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $candidate = Get-ChildItem -LiteralPath $sdkBin -Directory -ErrorAction Stop |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+\.\d+$' } |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName "x64\$Name" } |
        Where-Object { Test-Path -LiteralPath $_ } |
        Select-Object -First 1
    if (-not $candidate) {
        throw "Unable to locate $Name in the Windows 10 SDK."
    }
    return $candidate
}

function Get-MsixVersion([string]$ManifestPath, [int]$VersionRevision) {
    $manifestText = Get-Content -Raw -LiteralPath $ManifestPath
    $match = [regex]::Match(
        $manifestText,
        '(?m)^version\s*=\s*"(?<version>[^\"]+)"\s*$'
    )
    if (-not $match.Success) {
        throw 'Unable to read the Cargo package version.'
    }

    $cargoVersion = $match.Groups['version'].Value
    $coreMatch = [regex]::Match($cargoVersion, '^(?<major>\d+)\.(?<minor>\d+)\.(?<patch>\d+)(?<suffix>[-+].*)?$')
    if (-not $coreMatch.Success) {
        throw "Cargo version '$cargoVersion' is not a supported semantic version."
    }

    $parts = @(
        [int]$coreMatch.Groups['major'].Value,
        [int]$coreMatch.Groups['minor'].Value,
        [int]$coreMatch.Groups['patch'].Value,
        $VersionRevision
    )
    foreach ($part in $parts) {
        if ($part -lt 0 -or $part -gt 65535) {
            throw "MSIX version components must be between 0 and 65535: $cargoVersion"
        }
    }
    if ($coreMatch.Groups['suffix'].Success) {
        Write-Warning "Cargo prerelease/build suffix is not representable in MSIX; use -Revision to keep package versions unique."
    }
    return ($parts -join '.')
}

function New-LocalTestCertificate {
    $existing = Get-ChildItem Cert:\CurrentUser\My |
        Where-Object {
            $basicConstraints = $_.Extensions |
                Where-Object { $_.Oid.Value -eq '2.5.29.19' } |
                Select-Object -First 1
            $codeSigningEku = $_.Extensions |
                Where-Object { $_.Oid.Value -eq '2.5.29.37' } |
                ForEach-Object { $_.Format($false) } |
                Where-Object { $_ -match '1\.3\.6\.1\.5\.5\.7\.3\.3|Code Signing' } |
                Select-Object -First 1
            $_.Subject -eq $publisher -and
                $_.NotAfter -gt (Get-Date).AddDays(1) -and
                $_.HasPrivateKey -and
                $basicConstraints -and
                -not $basicConstraints.CertificateAuthority -and
                $codeSigningEku
        } |
        Sort-Object NotAfter -Descending |
        Select-Object -First 1
    if ($existing) {
        return $existing
    }

    return New-SelfSignedCertificate `
        -Type Custom `
        -Subject $publisher `
        -FriendlyName 'AI Usage Monitor local MSIX trial' `
        -CertStoreLocation 'Cert:\CurrentUser\My' `
        -KeyAlgorithm RSA `
        -KeyLength 2048 `
        -HashAlgorithm SHA256 `
        -KeyUsage DigitalSignature `
        -TextExtension @(
            '2.5.29.37={text}1.3.6.1.5.5.7.3.3',
            '2.5.29.19={text}'
        ) `
        -NotAfter (Get-Date).AddYears(1)
}

$msixVersion = Get-MsixVersion -ManifestPath $cargoToml -VersionRevision $Revision
$features = @('antigravity')
if ($features -contains 'self-update') {
    throw 'The MSIX build must never include the self-update feature.'
}

if (-not $SkipBuild) {
    $cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (-not (Test-Path -LiteralPath $cargo)) {
        throw "Cargo was not found at $cargo"
    }
    & $cargo build --release --locked --no-default-features --features ($features -join ',') --target-dir $cargoTargetRoot
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo release build failed with exit code $LASTEXITCODE."
    }
}

if (-not (Test-Path -LiteralPath $binaryPath)) {
    throw "Release executable not found: $binaryPath"
}
if (-not (Test-Path -LiteralPath $bridgeBinaryPath)) {
    throw "Release executable not found: $bridgeBinaryPath"
}
if (-not (Test-Path -LiteralPath $manifestSource)) {
    throw "MSIX manifest not found: $manifestSource"
}
if (-not (Test-Path -LiteralPath $assetsSource)) {
    throw "MSIX assets not found: $assetsSource"
}

Assert-PathUnderTarget $stagingRoot
if (Test-Path -LiteralPath $stagingRoot) {
    Remove-Item -LiteralPath $stagingRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $stagingRoot -Force | Out-Null
Copy-Item -LiteralPath $binaryPath -Destination $stagingRoot
Copy-Item -LiteralPath $bridgeBinaryPath -Destination $stagingRoot
Copy-Item -LiteralPath $assetsSource -Destination $stagingRoot -Recurse
Copy-Item -LiteralPath $manifestSource -Destination (Join-Path $stagingRoot 'AppxManifest.xml')

$stagedManifestPath = Join-Path $stagingRoot 'AppxManifest.xml'
[xml]$stagedManifest = Get-Content -Raw -LiteralPath $stagedManifestPath
$stagedManifest.Package.Identity.Version = $msixVersion
$stagedManifest.Package.Identity.Publisher = $publisher
$stagedManifest.Package.Identity.Name = $packageName
$stagedManifest.Save($stagedManifestPath)

$makeAppx = Find-WindowsSdkTool 'makeappx.exe'
$signTool = Find-WindowsSdkTool 'signtool.exe'
$packagePath = Join-Path $targetRoot "AIUsageMonitor.LocalTrial_${msixVersion}_x64.msix"
Assert-PathUnderTarget $packagePath
if (Test-Path -LiteralPath $packagePath) {
    Remove-Item -LiteralPath $packagePath -Force
}

& $makeAppx pack /d $stagingRoot /p $packagePath /o
if ($LASTEXITCODE -ne 0) {
    throw "MakeAppx failed with exit code $LASTEXITCODE."
}

$certificate = $null
if ($CreateAndTrustLocalTestCertificate) {
    $certificate = New-LocalTestCertificate
    $CertificateThumbprint = $certificate.Thumbprint
    $cerPath = Join-Path $targetRoot 'AIUsageMonitor.LocalTrial.cer'
    Export-Certificate -Cert $certificate -FilePath $cerPath -Force | Out-Null
    $trusted = Get-ChildItem Cert:\LocalMachine\TrustedPeople |
        Where-Object { $_.Thumbprint -eq $certificate.Thumbprint } |
        Select-Object -First 1
    if (-not $trusted) {
        Import-Certificate -FilePath $cerPath -CertStoreLocation Cert:\LocalMachine\TrustedPeople | Out-Null
    }
}

if (-not $CertificateThumbprint) {
    throw 'Specify -CertificateThumbprint or use -CreateAndTrustLocalTestCertificate.'
}

$signingCertificate = Get-ChildItem Cert:\CurrentUser\My |
    Where-Object { $_.Thumbprint -eq $CertificateThumbprint } |
    Select-Object -First 1
if (-not $signingCertificate) {
    throw "Signing certificate was not found in Cert:\CurrentUser\My: $CertificateThumbprint"
}
if ($signingCertificate.Subject -ne $publisher) {
    throw "Certificate subject '$($signingCertificate.Subject)' does not match manifest publisher '$publisher'."
}

& $signTool sign /fd SHA256 /sha1 $CertificateThumbprint /s My $packagePath
if ($LASTEXITCODE -ne 0) {
    throw "SignTool failed with exit code $LASTEXITCODE."
}

& $signTool verify /pa /v $packagePath
if ($LASTEXITCODE -ne 0) {
    throw "Signed package verification failed with exit code $LASTEXITCODE."
}

[pscustomobject]@{
    Package = $packagePath
    Version = $msixVersion
    Publisher = $publisher
    CertificateThumbprint = $CertificateThumbprint
    Features = ($features -join ',')
    SelfUpdateIncluded = $false
}
