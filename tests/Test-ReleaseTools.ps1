# Regression tests for scripts/ReleaseTools.psm1. Runs in Windows PowerShell 5.1
# and PowerShell 7 without Pester.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path (Split-Path -Parent $PSScriptRoot) 'scripts/ReleaseTools.psm1') -Force

$script:Failures = New-Object System.Collections.Generic.List[string]
$script:Count = 0

function Assert-True([bool]$Condition, [string]$Name) {
    $script:Count++
    if (-not $Condition) { $script:Failures.Add($Name) }
}

function Assert-Equal($Expected, $Actual, [string]$Name) {
    Assert-True ($Expected -ceq $Actual) "$Name (expected '$Expected', got '$Actual')"
}

function Assert-Problem([string[]]$Problems, [string]$Pattern, [string]$Name) {
    Assert-True (@($Problems | Where-Object { $_ -match $Pattern }).Count -gt 0) "$Name (no problem matching '$Pattern' in: $($Problems -join ' | '))"
}

function Assert-Throws([scriptblock]$Script, [string]$Name) {
    $threw = $false
    try { & $Script | Out-Null } catch { $threw = $true }
    Assert-True $threw "$Name (did not throw)"
}

$repo = 'https://github.com/CodeZeno/Claude-Code-Usage-Monitor'

# $Unreleased is untyped: a [string] parameter would turn $null into ''.
function New-Changelog($Unreleased = $null, [string]$UnreleasedLink = "$repo/compare/v1.1.0...HEAD") {
    $lines = @('# Changelog', '', 'Intro text.', '')
    if ($null -ne $Unreleased) {
        $lines += '## [Unreleased]'
        $lines += ''
        if ($Unreleased) { $lines += $Unreleased; $lines += '' }
    }
    $lines += @(
        '## [1.1.0] - 2026-02-01', '', '### Added', '', '- New thing.', '',
        '## [1.0.0] - 2026-01-01', '', '### Fixed', '', '- Old thing.', '',
        "[1.0.0]: $repo/commit/abc",
        "[1.1.0]: $repo/compare/v1.0.0...v1.1.0"
    )
    if ($null -ne $Unreleased -and $UnreleasedLink) { $lines += "[Unreleased]: $UnreleasedLink" }
    ($lines -join "`n") + "`n"
}

# Valid changelogs.
Assert-Equal 0 @(Test-ChangelogText (New-Changelog) '1.1.0').Count 'released changelog without Unreleased is valid'
Assert-Equal 0 @(Test-ChangelogText (New-Changelog '') '1.1.0').Count 'empty Unreleased heading is valid'
Assert-Equal 0 @(Test-ChangelogText (New-Changelog "### Fixed`n`n- Pending fix.") '1.1.0').Count 'Unreleased with entries is valid'

# Version and structure problems.
Assert-Problem @(Test-ChangelogText (New-Changelog "### Fixed`n`n- Pending fix.") '1.2.0') 'does not match the newest' 'bumped Cargo version without release is rejected'
Assert-Problem @(Test-ChangelogText (New-Changelog '### Fixed') '1.1.0') 'empty .### Fixed' 'empty Unreleased category is rejected'
Assert-Problem @(Test-ChangelogText (New-Changelog "### Improved`n`n- Thing.") '1.1.0') 'not a changelog category' 'unknown category is rejected'
Assert-Problem @(Test-ChangelogText (New-Changelog "- Loose entry.") '1.1.0') 'must be under a' 'entry without category is rejected'
Assert-Problem @(Test-ChangelogText (New-Changelog '' '') '1.1.0') 'add .\[Unreleased\]:' 'missing Unreleased link is rejected'
Assert-Problem @(Test-ChangelogText (New-Changelog '' "$repo/compare/v1.0.0...HEAD") '1.1.0') 'Unreleased\] link must be' 'stale Unreleased link is rejected'
Assert-Problem @(Test-ChangelogText ((New-Changelog) + "[Unreleased]: $repo/compare/v1.1.0...HEAD`n") '1.1.0') 'remove the \[Unreleased\] link' 'Unreleased link without heading is rejected'

$misplaced = (New-Changelog) -replace '## \[1\.0\.0\]', "## [Unreleased]`n`n## [1.0.0]"
Assert-Problem @(Test-ChangelogText $misplaced '1.1.0') 'must be the first release section' 'Unreleased below a release is rejected'
$unordered = (New-Changelog) -replace '1\.0\.0\] - 2026-01-01', '1.2.0] - 2026-01-01' -replace '\[1\.0\.0\]: ', '[1.2.0]: '
Assert-Problem @(Test-ChangelogText $unordered '1.1.0') 'must be older than' 'out-of-order versions are rejected'
Assert-Problem @(Test-ChangelogText ((New-Changelog) -replace '2026-01-01', '2026-03-01') '1.1.0') 'is dated after' 'out-of-order dates are rejected'
Assert-Problem @(Test-ChangelogText ((New-Changelog) -replace '## \[1\.1\.0\] - ', '## 1.1.0 - ') '1.1.0') 'must be .## \[Unreleased\]' 'malformed heading is rejected'
Assert-Problem @(Test-ChangelogText ((New-Changelog) -replace "\[1\.0\.0\]: [^\n]+\n", '') '1.1.0') 'add a \[1\.0\.0\]: link' 'missing version link is rejected'
Assert-Problem @(Test-ChangelogText ((New-Changelog) -replace 'v1\.0\.0\.\.\.v1\.1\.0', 'v0.9.0...v1.1.0') '1.1.0') 'link must be' 'wrong newest compare link is rejected'
Assert-Problem @(Test-ChangelogText ((New-Changelog) -replace '### Fixed', "### Fixed`n`n- Extra.`n`n### Fixed") '1.1.0') 'appears twice' 'duplicate category is rejected'

# Preparing a release keeps an empty Unreleased heading and moves the entries.
$pending = New-Changelog "### Added`n`n- Pending feature."
$released = Add-ChangelogRelease $pending '1.2.0' '1.1.0' '2026-03-01'
Assert-Equal 0 @(Test-ChangelogText $released '1.2.0').Count 'released changelog is valid'
Assert-True ($released.Contains("## [Unreleased]`n`n## [1.2.0] - 2026-03-01`n`n### Added`n`n- Pending feature.")) 'entries move under the new release'
Assert-True ($released.EndsWith("[1.2.0]: $repo/compare/v1.1.0...v1.2.0`n[Unreleased]: $repo/compare/v1.2.0...HEAD`n")) 'release links are appended'
Assert-Equal 0 (Get-UnreleasedEntryCount (Read-Changelog $released)) 'Unreleased is empty after release'
Assert-Equal 1 (Get-UnreleasedEntryCount (Read-Changelog $pending)) 'Unreleased entry count'
Assert-Equal 'Added' ((Get-UnreleasedCategories (Read-Changelog $pending)) -join ',') 'Unreleased categories'
Assert-Throws { Add-ChangelogRelease (New-Changelog) '1.2.0' '1.1.0' '2026-03-01' } 'release without Unreleased section throws'

$crlf = Add-ChangelogRelease ($pending -replace "`n", "`r`n") '1.2.0' '1.1.0' '2026-03-01'
Assert-True ($crlf.Contains("## [Unreleased]`r`n`r`n## [1.2.0]") -and -not ($crlf -match "[^`r]`n")) 'CRLF line endings are preserved'

# Versions.
Assert-Equal '1.2.4' (Get-NextVersion '1.2.3' 'patch') 'patch bump'
Assert-Equal '1.3.0' (Get-NextVersion '1.2.3' 'minor') 'minor bump resets patch'
Assert-Equal '2.0.0' (Get-NextVersion '1.2.3' 'major') 'major bump resets minor and patch'
Assert-Equal '1.5.0' (Get-NextVersion '1.2.3' '1.5.0') 'explicit version'
Assert-Throws { Get-NextVersion '1.2.3' '1.2.3' } 'explicit version must increase'
Assert-Throws { Get-NextVersion '1.2.3' 'v1.3' } 'explicit version must be MAJOR.MINOR.PATCH'
Assert-Equal 'minor' (Get-VersionChangeLevel '1.2.3' '1.3.0') 'minor change level'
Assert-Equal 'patch' (Get-VersionChangeLevel '1.2.3' '1.2.9') 'patch change level'
Assert-Equal 'major' (Get-VersionChangeLevel '1.2.3' '2.0.0') 'major change level'

# Cargo metadata.
$toml = "[package]`nname = `"claude-code-usage-monitor`"`nversion = `"1.2.3`"`n`n[dependencies.windows]`nversion = `"0.62`"`n"
Assert-Equal '1.2.3' (Get-PackageVersionFromToml $toml) 'package version is read from [package]'
$bumped = Set-PackageVersionInToml $toml '1.3.0'
Assert-Equal '1.3.0' (Get-PackageVersionFromToml $bumped) 'package version is updated'
Assert-True ($bumped.Contains("[dependencies.windows]`nversion = `"0.62`"")) 'dependency versions are untouched'
$lock = "[[package]]`nname = `"chrono`"`nversion = `"0.4.0`"`n`n[[package]]`nname = `"claude-code-usage-monitor`"`nversion = `"1.2.3`"`n"
Assert-Equal '1.2.3' (Get-LockedPackageVersion $lock) 'locked package version'

# Changelog entries for auto-merged Dependabot pull requests.
$described = New-Changelog "### Changed`n`n- Update the locked ``zip`` dependency from 9.3.0 to 9.6.1 for import. ([#228]($repo/pull/228))"
$entries = @(Get-DependencyUpdateEntries @(
        'chore(deps): bump ureq from 3.5.1 to 3.5.2 (#227)',
        'chore(deps): bump zip from 9.3.0 to 9.6.1',
        'chore(deps): bump lucide-icons from 1.67.0 to 1.73.0 (#228)',
        'chore(deps): bump the github-actions group with 2 updates (#230)',
        'Bump serde from 1.0.1 to 1.0.2'
    ) $described)
Assert-Equal 3 $entries.Count 'updates already in the changelog are skipped'
Assert-Equal "- Update ``ureq`` from 3.5.1 to 3.5.2. ([#227]($repo/pull/227))" $entries[0] 'bump with a pull request number'
Assert-Equal "- Update the ``github-actions`` dependency group: bump the github-actions group with 2 updates. ([#230]($repo/pull/230))" $entries[1] 'grouped bump'
Assert-Equal "- Update ``serde`` from 1.0.1 to 1.0.2." $entries[2] 'bump without a pull request number'

$withNew = Add-UnreleasedEntries $described 'Changed' @("- Update ``a`` from 1 to 2.")
Assert-Equal 0 @(Test-ChangelogText $withNew '1.1.0').Count 'entries appended to an existing category keep the changelog valid'
Assert-True ($withNew.Contains("([#228]($repo/pull/228))`n- Update ``a`` from 1 to 2.`n`n## [1.1.0]")) 'entry follows the existing entries'

$intoEmpty = Add-UnreleasedEntries (New-Changelog '') 'Changed' @("- Update ``a`` from 1 to 2.")
Assert-Equal 0 @(Test-ChangelogText $intoEmpty '1.1.0').Count 'a category is added to an empty Unreleased section'
Assert-True ($intoEmpty.Contains("## [Unreleased]`n`n### Changed`n`n- Update ``a`` from 1 to 2.`n`n## [1.1.0]")) 'new category layout'

$ordered = Add-UnreleasedEntries (New-Changelog "### Added`n`n- New.`n`n### Fixed`n`n- Fix.") 'Changed' @("- Update ``a`` from 1 to 2.")
Assert-Equal 0 @(Test-ChangelogText $ordered '1.1.0').Count 'a category inserted between others keeps the changelog valid'
Assert-True ($ordered.Contains("- New.`n`n### Changed`n`n- Update ``a`` from 1 to 2.`n`n### Fixed")) 'Changed goes between Added and Fixed'
Assert-Equal $described (Add-UnreleasedEntries $described 'Changed' @()) 'no entries leaves the changelog unchanged'
Assert-Throws { Add-UnreleasedEntries (New-Changelog) 'Changed' @('- X.') } 'a changelog without Unreleased is refused'

# Line endings: a scratch repository with and without .gitattributes.
$eolRepo = Join-Path ([System.IO.Path]::GetTempPath()) ("eol-" + [guid]::NewGuid())
New-Item -ItemType Directory $eolRepo | Out-Null
try {
    Invoke-Git -C $eolRepo init -q | Out-Null
    Invoke-Git -C $eolRepo config core.autocrlf false | Out-Null
    Invoke-Git -C $eolRepo config core.eol lf | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $eolRepo 'Cargo.toml'), "[package]`r`n")
    Invoke-Git -C $eolRepo add Cargo.toml | Out-Null
    $eolProblems = @(Test-LineEndings $eolRepo)
    Assert-Problem $eolProblems "must set '\* text=auto'" 'a repository without .gitattributes is reported'
    Assert-Problem $eolProblems 'Cargo.toml is stored with crlf' 'a text file stored with CRLF is reported'

    [System.IO.File]::WriteAllText((Join-Path $eolRepo '.gitattributes'), "* text=auto`n")
    Invoke-Git -C $eolRepo add --renormalize . | Out-Null
    Invoke-Git -C $eolRepo add .gitattributes | Out-Null
    Assert-Equal 0 @(Test-LineEndings $eolRepo).Count 'renormalized text files pass'
} finally {
    Remove-Item -Recurse -Force $eolRepo
}

if ($script:Failures.Count -gt 0) {
    foreach ($failure in $script:Failures) { Write-Host "FAILED: $failure" }
    throw "$($script:Failures.Count) of $script:Count release tool checks failed."
}
Write-Host "All $script:Count release tool checks passed."
