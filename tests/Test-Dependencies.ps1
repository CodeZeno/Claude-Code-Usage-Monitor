# Run the same dependency gates as .github/workflows/dependency-security.yml.
# Install the scanner versions documented in docs/dependency-security.md first.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$cargoCommand = (Get-Command cargo -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source

function Invoke-CargoCheck([string[]]$CargoArguments, [string]$FailureMessage) {
    # Windows PowerShell can turn redirected native stderr (including normal
    # progress and warnings) into errors. The scanner's exit code is authoritative.
    $ErrorActionPreference = 'Continue'
    & $cargoCommand @CargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw $FailureMessage
    }
}

Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    # Refuse a stale lockfile before auditing it. Do not update dependencies here.
    Invoke-CargoCheck @('metadata', '--locked', '--format-version', '1') `
        'Cargo metadata failed. Synchronize and review Cargo.lock before running dependency checks.' > $null
    Invoke-CargoCheck @('audit', '--file', 'Cargo.lock') `
        'cargo audit failed. Resolve the finding (or install the documented scanner) before building.'
    Invoke-CargoCheck @('deny', '--locked', 'check') `
        'cargo deny failed. Resolve the finding (or install the documented scanner) before building.'

    Write-Host 'Dependency checks passed.'
} finally {
    Pop-Location
}
