# Prepare a release commit on main: move the CHANGELOG.md [Unreleased] entries
# into a dated section, set the Cargo.toml version, synchronize Cargo.lock, run
# the repository and dependency checks, and commit the result as vVERSION.
# Nothing is pushed or tagged; see scripts/Publish-Release.ps1.
[CmdletBinding()]
param(
    # patch, minor, major, or an explicit MAJOR.MINOR.PATCH version.
    [Parameter(Mandatory = $true)]
    [string]$Level,
    # Optional file whose contents become the commit body.
    [string]$BodyFile,
    [string]$Date = (Get-Date -Format 'yyyy-MM-dd'),
    [switch]$SkipDependencyChecks,
    # Skip fetching origin/main, for example when offline.
    [switch]$SkipRemoteCheck
)

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'ReleaseTools.psm1') -Force

$root = Get-RepositoryRoot
$releaseFiles = @('Cargo.toml', 'Cargo.lock', 'CHANGELOG.md')

Assert-OnMainBranch $root 'preparing a release'
Assert-CleanWorktree $root 'preparing a release'
Assert-RepositoryState $root

# Auto-merged Dependabot updates land on origin/main, so a release prepared from a
# main that lacks them would leave them out and could not be pushed.
if (-not $SkipRemoteCheck) {
    & git -C $root fetch -q origin main
    if ($LASTEXITCODE -ne 0) {
        throw 'Unable to fetch origin/main. Check the connection, or pass -SkipRemoteCheck.'
    }
    & git -C $root merge-base --is-ancestor origin/main HEAD
    if ($LASTEXITCODE -ne 0) {
        throw 'origin/main has commits that local main does not, such as auto-merged Dependabot updates. Run "git pull --rebase origin main" and prepare the release again.'
    }
}

$cargoTomlPath = Join-Path $root 'Cargo.toml'
$changelogPath = Join-Path $root 'CHANGELOG.md'
$current = Get-PackageVersionFromToml (Read-TextFile $cargoTomlPath)
$changelogText = Read-TextFile $changelogPath

# Auto-merged Dependabot pull requests carry no changelog entry; list them here.
$dependabotSubjects = @(Get-DependabotCommitSubjects $root "v$current")
$dependencyEntries = @(Get-DependencyUpdateEntries $dependabotSubjects $changelogText)
if ($dependencyEntries.Count -gt 0) {
    $changelogText = Add-UnreleasedEntries $changelogText 'Changed' $dependencyEntries
    Write-Host "Adding $($dependencyEntries.Count) changelog entries for Dependabot updates merged since v${current}:"
    foreach ($entry in $dependencyEntries) { Write-Host "  $entry" }
}

$changelog = Read-Changelog $changelogText
$entryCount = Get-UnreleasedEntryCount $changelog
if ($entryCount -eq 0) {
    throw "CHANGELOG.md has no entries under '## [Unreleased]'. Add the changes being released first."
}

$version = Get-NextVersion $current $Level.ToLowerInvariant()
$change = Get-VersionChangeLevel $current $version
$categories = @(Get-UnreleasedCategories $changelog)
if ($categories -contains 'Added' -and $change -eq 'patch') {
    throw "[Unreleased] lists Added entries, which need at least a minor release. Use 'minor' (or 'major') instead of a patch version."
}

& git -C $root rev-parse -q --verify "refs/tags/v$version" | Out-Null
if ($LASTEXITCODE -eq 0) {
    throw "Tag v$version already exists."
}

Write-Host "Preparing v$version ($change release after $current) with $entryCount changelog entries."

$committed = $false
try {
    Write-TextFile $cargoTomlPath (Set-PackageVersionInToml (Read-TextFile $cargoTomlPath) $version)
    Write-TextFile $changelogPath (Add-ChangelogRelease $changelogText $version $current $Date)

    # Synchronize only this package's version in Cargo.lock.
    $cargo = (Get-Command cargo -CommandType Application | Select-Object -First 1).Source
    $ErrorActionPreference = 'Continue'
    & $cargo metadata --format-version 1 --manifest-path $cargoTomlPath | Out-Null
    $metadataExit = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($metadataExit -ne 0) {
        throw 'cargo metadata failed while synchronizing Cargo.lock.'
    }
    $lockChange = @(Invoke-Git -C $root diff --numstat '--' Cargo.lock)
    if ($lockChange.Count -ne 1 -or $lockChange[0] -notmatch '^1\s+1\s') {
        throw "Cargo.lock changed more than this package's version ($($lockChange -join '; ')). Review the dependency changes separately."
    }

    Assert-RepositoryState $root
    if (-not $SkipDependencyChecks) {
        & (Join-Path $root 'tests/Test-Dependencies.ps1')
    }

    Invoke-Git -C $root add '--' @releaseFiles | Out-Null
    $staged = @(Invoke-Git -C $root diff --cached --name-only)
    $unexpected = @($staged | Where-Object { $releaseFiles -notcontains $_ })
    if ($unexpected.Count -gt 0) {
        throw "Unexpected staged files: $($unexpected -join ', ')"
    }

    $messagePath = [System.IO.Path]::GetTempFileName()
    try {
        $message = "v$version"
        if ($BodyFile) {
            $body = (Read-TextFile (Resolve-Path $BodyFile)).Trim()
            if ($body) { $message += "`n`n$body" }
        }
        Write-TextFile $messagePath ($message + "`n")
        Invoke-Git -C $root commit -q -F $messagePath | Out-Null
        $committed = $true
    } finally {
        Remove-Item -LiteralPath $messagePath -ErrorAction SilentlyContinue
    }
} finally {
    if (-not $committed) {
        # The worktree was clean on entry, so restoring these files is safe.
        & git -C $root reset -q -- @releaseFiles
        & git -C $root checkout -q -- @releaseFiles
        Write-Host 'Release preparation failed; restored Cargo.toml, Cargo.lock and CHANGELOG.md.'
    }
}

$commit = (Invoke-Git -C $root rev-parse --short HEAD) | Select-Object -First 1
Write-Host "Committed v$version as $commit. Nothing was pushed or tagged."
Write-Host 'Next: push main (workflow -p), then publish the tag (workflow -d).'
