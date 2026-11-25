# Tag the released version at HEAD and push the tag, which starts the Release
# workflow (CI, dependency checks, GitHub release, WinGet and Chocolatey).
# Requires a release commit from scripts/New-Release.ps1 that is already on origin/main.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path $PSScriptRoot 'ReleaseTools.psm1') -Force

$root = Get-RepositoryRoot

Assert-OnMainBranch $root 'publishing a release'
Assert-CleanWorktree $root 'publishing a release'
Assert-RepositoryState $root

$changelog = Read-Changelog (Read-TextFile (Join-Path $root 'CHANGELOG.md'))
$pending = Get-UnreleasedEntryCount $changelog
if ($pending -gt 0) {
    throw "CHANGELOG.md has $pending [Unreleased] entries that this tag would ship without listing. Prepare a release first (workflow -r patch|minor|major)."
}

$version = Get-PackageVersionFromToml (Read-TextFile (Join-Path $root 'Cargo.toml'))
$tag = "v$version"

& git -C $root rev-parse -q --verify "refs/tags/$tag" | Out-Null
if ($LASTEXITCODE -eq 0) {
    throw "Tag $tag already exists locally. Prepare a new release (workflow -r ...) before publishing."
}
& git -C $root ls-remote --exit-code --tags origin "refs/tags/$tag" | Out-Null
if ($LASTEXITCODE -eq 0) {
    throw "Tag $tag already exists on origin."
}
if ($LASTEXITCODE -ne 2) {
    throw "Unable to check origin for tag $tag (git ls-remote exit code $LASTEXITCODE)."
}

Invoke-Git -C $root fetch -q origin main | Out-Null
$head = (Invoke-Git -C $root rev-parse HEAD) | Select-Object -First 1
$remote = (Invoke-Git -C $root rev-parse origin/main) | Select-Object -First 1
if ($head -ne $remote) {
    & git -C $root merge-base --is-ancestor HEAD origin/main
    if ($LASTEXITCODE -eq 0) {
        throw 'origin/main has commits that local main does not. Pull them and prepare the release again.'
    }
    throw 'HEAD is not on origin/main. Push main first (workflow -p) so the tagged commit has passed CI on main.'
}

Invoke-Git -C $root tag $tag | Out-Null
Invoke-Git -C $root push origin $tag | Out-Null
Write-Host "Pushed $tag. The Release workflow now runs CI and publishes the release."
