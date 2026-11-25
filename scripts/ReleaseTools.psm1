# Shared release metadata rules for tests/Test-Repository.ps1 and the release scripts.
# Compatible with Windows PowerShell 5.1 and PowerShell 7.
Set-StrictMode -Version 3.0

$script:PackageName = 'claude-code-usage-monitor'
$script:RepositoryUrl = 'https://github.com/CodeZeno/Claude-Code-Usage-Monitor'
$script:Categories = @('Added', 'Changed', 'Deprecated', 'Removed', 'Fixed', 'Security')
$script:Utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$script:LocalOnlyFiles = @('AGENTS.md', 'workflow.cmd')

function Get-RepositoryRoot {
    Split-Path -Parent $PSScriptRoot
}

function Read-TextFile([string]$Path) {
    [System.IO.File]::ReadAllText($Path, $script:Utf8NoBom)
}

function Write-TextFile([string]$Path, [string]$Text) {
    [System.IO.File]::WriteAllText($Path, $Text, $script:Utf8NoBom)
}

function Get-Newline([string]$Text) {
    if ($Text.Contains("`r`n")) { return "`r`n" }
    return "`n"
}

function Split-Lines([string]$Text) {
    $Text -split "`r?`n"
}

function Invoke-Git {
    # Quote '--' when calling this: PowerShell drops a bare -- passed to a function.
    $output = & git @args
    if ($LASTEXITCODE -ne 0) {
        throw "git $($args -join ' ') failed with exit code $LASTEXITCODE."
    }
    $output
}

function Test-ReleaseVersion([string]$Version) {
    $Version -match '^\d+\.\d+\.\d+$'
}

function ConvertTo-ReleaseVersion([string]$Version) {
    if (-not (Test-ReleaseVersion $Version)) {
        throw "'$Version' is not a MAJOR.MINOR.PATCH version."
    }
    [System.Version]::Parse($Version)
}

function Get-NextVersion([string]$Current, [string]$Level) {
    $version = ConvertTo-ReleaseVersion $Current
    switch ($Level) {
        'major' { return "$($version.Major + 1).0.0" }
        'minor' { return "$($version.Major).$($version.Minor + 1).0" }
        'patch' { return "$($version.Major).$($version.Minor).$($version.Build + 1)" }
    }
    $next = ConvertTo-ReleaseVersion $Level
    if ($next -le $version) {
        throw "Release version $Level must be greater than the current version $Current."
    }
    $Level
}

# Returns 'major', 'minor' or 'patch' for the most significant part that changed.
function Get-VersionChangeLevel([string]$From, [string]$To) {
    $old = ConvertTo-ReleaseVersion $From
    $new = ConvertTo-ReleaseVersion $To
    if ($new.Major -ne $old.Major) { return 'major' }
    if ($new.Minor -ne $old.Minor) { return 'minor' }
    'patch'
}

function Get-PackageVersionFromToml([string]$Text) {
    $section = ''
    foreach ($line in (Split-Lines $Text)) {
        if ($line -match '^\s*\[([^\]]+)\]\s*$') {
            $section = $Matches[1]
            continue
        }
        if ($section -eq 'package' -and $line -match '^version\s*=\s*"([^"]+)"\s*$') {
            return $Matches[1]
        }
    }
    throw 'Cargo.toml has no [package] version.'
}

function Set-PackageVersionInToml([string]$Text, [string]$Version) {
    $newline = Get-Newline $Text
    $lines = @(Split-Lines $Text)
    $section = ''
    for ($index = 0; $index -lt $lines.Count; $index++) {
        if ($lines[$index] -match '^\s*\[([^\]]+)\]\s*$') {
            $section = $Matches[1]
            continue
        }
        if ($section -eq 'package' -and $lines[$index] -match '^version\s*=\s*"[^"]+"\s*$') {
            $lines[$index] = "version = `"$Version`""
            return ($lines -join $newline)
        }
    }
    throw 'Cargo.toml has no [package] version.'
}

function Get-LockedPackageVersion([string]$Text) {
    $pattern = '(?m)^name = "' + [regex]::Escape($script:PackageName) + '"\r?\nversion = "([^"]+)"'
    $match = [regex]::Match($Text, $pattern)
    if (-not $match.Success) {
        throw "Cargo.lock has no entry for $script:PackageName."
    }
    $match.Groups[1].Value
}

# Parses CHANGELOG.md into release sections and link references, collecting
# structural problems instead of stopping at the first one.
function Read-Changelog([string]$Text) {
    $releases = New-Object System.Collections.Generic.List[object]
    $links = [ordered]@{}
    $problems = New-Object System.Collections.Generic.List[string]
    $release = $null
    $category = $null
    $lineNumber = 0

    foreach ($line in (Split-Lines $Text)) {
        $lineNumber++
        if ($line -match '^## ') {
            $category = $null
            if ($line -eq '## [Unreleased]') {
                $release = [pscustomobject]@{ Name = 'Unreleased'; Date = $null; Line = $lineNumber; Categories = New-Object System.Collections.Generic.List[object] }
            } elseif ($line -match '^## \[(\d+\.\d+\.\d+)\] - (\d{4}-\d{2}-\d{2})$') {
                $release = [pscustomobject]@{ Name = $Matches[1]; Date = $Matches[2]; Line = $lineNumber; Categories = New-Object System.Collections.Generic.List[object] }
            } else {
                $problems.Add("CHANGELOG.md:${lineNumber}: '$line' must be '## [Unreleased]' or '## [MAJOR.MINOR.PATCH] - YYYY-MM-DD'.")
                $release = $null
                continue
            }
            $releases.Add($release)
        } elseif ($line -match '^### (.*)$') {
            $name = $Matches[1]
            if ($null -eq $release) {
                $problems.Add("CHANGELOG.md:${lineNumber}: '### $name' must be inside a '## [...]' release section.")
                continue
            }
            if ($script:Categories -cnotcontains $name) {
                $problems.Add("CHANGELOG.md:${lineNumber}: '### $name' is not a changelog category. Use one of: $($script:Categories -join ', ').")
            }
            if (@($release.Categories | Where-Object { $_.Name -ceq $name }).Count -gt 0) {
                $problems.Add("CHANGELOG.md:${lineNumber}: '### $name' appears twice in [$($release.Name)]. Merge the entries into one category.")
            }
            $category = [pscustomobject]@{ Name = $name; Line = $lineNumber; Entries = 0 }
            $release.Categories.Add($category)
        } elseif ($line -match '^- ') {
            if ($null -ne $category) {
                $category.Entries++
            } elseif ($null -ne $release) {
                $problems.Add("CHANGELOG.md:${lineNumber}: entry in [$($release.Name)] must be under a '### Added/Changed/Fixed/Removed' category.")
            }
        } elseif ($line -match '^\[([^\]]+)\]:\s*(\S+)\s*$') {
            $release = $null
            $category = $null
            if ($links.Contains($Matches[1])) {
                $problems.Add("CHANGELOG.md:${lineNumber}: link reference [$($Matches[1])] is defined twice.")
            } else {
                $links[$Matches[1]] = $Matches[2]
            }
        }
    }

    [pscustomobject]@{
        Releases = $releases.ToArray()
        Links = $links
        Problems = $problems.ToArray()
    }
}

function Get-UnreleasedEntryCount($Changelog) {
    $unreleased = @($Changelog.Releases | Where-Object { $_.Name -eq 'Unreleased' })
    if ($unreleased.Count -eq 0) { return 0 }
    $count = 0
    foreach ($category in $unreleased[0].Categories) { $count += $category.Entries }
    $count
}

function Get-UnreleasedCategories($Changelog) {
    $unreleased = @($Changelog.Releases | Where-Object { $_.Name -eq 'Unreleased' })
    if ($unreleased.Count -eq 0) { return @() }
    @($unreleased[0].Categories | Where-Object { $_.Entries -gt 0 } | ForEach-Object { $_.Name })
}

function Get-CompareUrl([string]$From, [string]$To) {
    "$script:RepositoryUrl/compare/$From...$To"
}

# Returns the problems that make CHANGELOG.md inconsistent with itself or with
# the Cargo.toml package version (the newest released version).
function Test-ChangelogText([string]$Text, [string]$PackageVersion) {
    $changelog = Read-Changelog $Text
    $problems = New-Object System.Collections.Generic.List[string]
    foreach ($problem in $changelog.Problems) { $problems.Add($problem) }

    $releases = $changelog.Releases
    $unreleased = @($releases | Where-Object { $_.Name -eq 'Unreleased' })
    $dated = @($releases | Where-Object { $_.Name -ne 'Unreleased' })

    if ($unreleased.Count -gt 1) {
        $problems.Add("CHANGELOG.md:$($unreleased[1].Line): only one '## [Unreleased]' section is allowed.")
    }
    if ($unreleased.Count -gt 0 -and $releases[0].Name -ne 'Unreleased') {
        $problems.Add("CHANGELOG.md:$($unreleased[0].Line): '## [Unreleased]' must be the first release section.")
    }
    foreach ($section in $unreleased) {
        foreach ($category in $section.Categories) {
            if ($category.Entries -eq 0) {
                $problems.Add("CHANGELOG.md:$($category.Line): remove the empty '### $($category.Name)' heading from [Unreleased].")
            }
        }
    }

    $seen = @{}
    for ($index = 0; $index -lt $dated.Count; $index++) {
        $release = $dated[$index]
        if ($seen.ContainsKey($release.Name)) {
            $problems.Add("CHANGELOG.md:$($release.Line): version $($release.Name) appears twice.")
        }
        $seen[$release.Name] = $true
        $parsedDate = [datetime]::MinValue
        if (-not [datetime]::TryParseExact($release.Date, 'yyyy-MM-dd', [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::None, [ref]$parsedDate)) {
            $problems.Add("CHANGELOG.md:$($release.Line): $($release.Date) is not a valid date.")
        }
        if ($release.Categories.Count -eq 0) {
            $problems.Add("CHANGELOG.md:$($release.Line): [$($release.Name)] has no entries.")
        }
        foreach ($category in $release.Categories) {
            if ($category.Entries -eq 0) {
                $problems.Add("CHANGELOG.md:$($category.Line): '### $($category.Name)' in [$($release.Name)] has no entries.")
            }
        }
        if ($index -gt 0) {
            $newer = $dated[$index - 1]
            if ((ConvertTo-ReleaseVersion $release.Name) -ge (ConvertTo-ReleaseVersion $newer.Name)) {
                $problems.Add("CHANGELOG.md:$($release.Line): $($release.Name) must be older than $($newer.Name) above it; list releases newest first.")
            }
            if ([string]::CompareOrdinal($release.Date, $newer.Date) -gt 0) {
                $problems.Add("CHANGELOG.md:$($release.Line): $($release.Name) is dated after $($newer.Name) above it.")
            }
        }
        if (-not $changelog.Links.Contains($release.Name)) {
            $problems.Add("CHANGELOG.md:$($release.Line): add a [$($release.Name)]: link reference at the bottom of the changelog.")
        }
    }

    if ($dated.Count -eq 0) {
        $problems.Add('CHANGELOG.md has no released versions.')
    } else {
        $newest = $dated[0].Name
        if ($newest -ne $PackageVersion) {
            $problems.Add("Cargo.toml version $PackageVersion does not match the newest CHANGELOG.md release $newest. Leave the version unchanged and add entries under '## [Unreleased]'; scripts/New-Release.ps1 sets the version when a release is prepared.")
        }
        if ($dated.Count -gt 1) {
            $expected = Get-CompareUrl "v$($dated[1].Name)" "v$newest"
            if ($changelog.Links.Contains($newest) -and $changelog.Links[$newest] -ne $expected) {
                $problems.Add("CHANGELOG.md: the [$newest] link must be $expected")
            }
        }
        if ($unreleased.Count -gt 0) {
            $expected = Get-CompareUrl "v$newest" 'HEAD'
            if (-not $changelog.Links.Contains('Unreleased')) {
                $problems.Add("CHANGELOG.md: add '[Unreleased]: $expected' at the bottom of the changelog.")
            } elseif ($changelog.Links['Unreleased'] -ne $expected) {
                $problems.Add("CHANGELOG.md: the [Unreleased] link must be $expected")
            }
        }
    }

    foreach ($name in $changelog.Links.Keys) {
        if ($name -eq 'Unreleased') {
            if ($unreleased.Count -eq 0) {
                $problems.Add("CHANGELOG.md: remove the [Unreleased] link or add a '## [Unreleased]' section.")
            }
        } elseif ((Test-ReleaseVersion $name) -and -not $seen.ContainsKey($name)) {
            $problems.Add("CHANGELOG.md: link reference [$name] has no matching release section.")
        }
    }

    $problems.ToArray()
}

# Moves the [Unreleased] entries into a dated release section, keeping an empty
# [Unreleased] heading at the top, and updates the comparison links.
function Add-ChangelogRelease([string]$Text, [string]$Version, [string]$PreviousVersion, [string]$Date) {
    $newline = Get-Newline $Text
    $output = New-Object System.Collections.Generic.List[string]
    $found = $false
    foreach ($line in (Split-Lines $Text)) {
        if ($line -eq '## [Unreleased]') {
            $output.Add($line)
            $output.Add('')
            $output.Add("## [$Version] - $Date")
            $found = $true
        } elseif ($line -notmatch '^\[Unreleased\]:') {
            $output.Add($line)
        }
    }
    if (-not $found) {
        throw "CHANGELOG.md has no '## [Unreleased]' section to release."
    }
    while ($output.Count -gt 0 -and $output[$output.Count - 1] -eq '') {
        $output.RemoveAt($output.Count - 1)
    }
    $output.Add("[$Version]: $(Get-CompareUrl "v$PreviousVersion" "v$Version")")
    $output.Add("[Unreleased]: $(Get-CompareUrl "v$Version" 'HEAD')")
    ($output -join $newline) + $newline
}

# Subjects of the Dependabot commits on HEAD since $SinceRef (normally the last
# release tag), oldest first. Auto-merged Dependabot pull requests cannot add
# their own changelog entries; New-Release.ps1 writes them from these subjects.
function Get-DependabotCommitSubjects([string]$Root, [string]$SinceRef) {
    & git -C $Root rev-parse -q --verify "$SinceRef^{commit}" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "$SinceRef not found; Dependabot updates were not added to the changelog."
        return @()
    }
    $log = @(Invoke-Git -C $Root log --reverse --no-merges '--format=%an%x09%s' "$SinceRef..HEAD")
    @($log | Where-Object { $_ -match "^dependabot\[bot\]`t" } | ForEach-Object { ($_ -split "`t", 2)[1] })
}

# Turns Dependabot commit subjects ("chore(deps): bump ureq from 3.5.1 to 3.5.2 (#227)")
# into changelog entries, skipping updates the changelog already describes by
# pull request link or by package and versions.
function Get-DependencyUpdateEntries([string[]]$Subjects, [string]$ChangelogText) {
    $entries = New-Object System.Collections.Generic.List[string]
    foreach ($subject in $Subjects) {
        $title = $subject.Trim()
        $link = ''
        if ($title -match '^(.*?)\s*\(#(\d+)\)$') {
            $title = $Matches[1]
            $number = $Matches[2]
            if ($ChangelogText.Contains("/pull/$number)")) { continue }
            $link = " ([#$number]($script:RepositoryUrl/pull/$number))"
        }
        $title = $title -replace '^[a-z]+(\([^)]*\))?!?:\s*', ''
        if ($title -match '^bump (\S+) from (\S+) to (\S+)') {
            $name = $Matches[1]; $from = $Matches[2]; $to = $Matches[3]
            $described = [regex]::Escape("``$name``") + '[^\r\n]*' + [regex]::Escape("from $from to $to")
            if ($ChangelogText -match $described) { continue }
            $entry = "Update ``$name`` from $from to $to."
        } elseif ($title -match '^bump the (\S+) group') {
            $entry = "Update the ``$($Matches[1])`` dependency group: $title."
        } else {
            $entry = "$title."
        }
        $entry = $entry.Substring(0, 1).ToUpperInvariant() + $entry.Substring(1)
        $entries.Add("- $entry$link")
    }
    $entries.ToArray()
}

# Appends entries to a category of the [Unreleased] section, adding the
# category heading in Keep a Changelog order when it is missing.
function Add-UnreleasedEntries([string]$Text, [string]$Category, [string[]]$Entries) {
    if (@($Entries).Count -eq 0) { return $Text }
    if ($script:Categories -cnotcontains $Category) { throw "'$Category' is not a changelog category." }
    $newline = Get-Newline $Text
    $lines = New-Object System.Collections.Generic.List[string]
    foreach ($line in (Split-Lines $Text)) { $lines.Add($line) }
    $start = $lines.IndexOf('## [Unreleased]')
    if ($start -lt 0) { throw "CHANGELOG.md has no '## [Unreleased]' section." }
    $end = $start + 1
    while ($end -lt $lines.Count -and $lines[$end] -notmatch '^## |^\[[^\]]+\]:') { $end++ }
    $last = $end - 1
    while ($last -gt $start -and $lines[$last] -eq '') { $last-- }

    $order = [array]::IndexOf($script:Categories, $Category)
    $heading = -1
    $before = -1
    for ($i = $start + 1; $i -le $last; $i++) {
        if ($lines[$i] -match '^### (.*)$') {
            if ($Matches[1] -ceq $Category) {
                $heading = $i
            } elseif ($before -lt 0 -and [array]::IndexOf($script:Categories, $Matches[1]) -gt $order) {
                $before = $i
            }
        }
    }

    if ($heading -ge 0) {
        $insert = $heading + 1
        for ($j = $heading + 1; $j -le $last -and $lines[$j] -notmatch '^### '; $j++) {
            if ($lines[$j] -ne '') { $insert = $j + 1 }
        }
        $block = @($Entries)
        if ($insert -eq $heading + 1) { $block = @('') + $block }
        $lines.InsertRange($insert, [string[]]$block)
    } elseif ($before -ge 0) {
        $lines.InsertRange($before, [string[]](@("### $Category", '') + @($Entries) + @('')))
    } else {
        $lines.InsertRange($last + 1, [string[]](@('', "### $Category", '') + @($Entries)))
    }
    $lines -join $newline
}

# Returns problems with the files the repository must not track and with the
# Cargo.toml, Cargo.lock and CHANGELOG.md release metadata.
function Test-RepositoryState([string]$Root) {
    $problems = New-Object System.Collections.Generic.List[string]

    $ignored = @(Invoke-Git -C $Root ls-files -ci --exclude-standard)
    foreach ($path in $ignored) {
        $problems.Add("$path is ignored by .gitignore but tracked or staged. Untrack it and keep the local file: git rm --cached -- `"$path`"")
    }
    foreach ($path in $script:LocalOnlyFiles) {
        if (@(Invoke-Git -C $Root ls-files '--' $path).Count -gt 0) {
            $problems.Add("$path is a local-only file and must not be tracked: git rm --cached -- $path")
        }
        & git -C $Root check-ignore -q -- $path
        if ($LASTEXITCODE -ne 0) {
            $problems.Add("$path must stay listed in .gitignore.")
        }
    }

    $packageVersion = Get-PackageVersionFromToml (Read-TextFile (Join-Path $Root 'Cargo.toml'))
    $lockedVersion = Get-LockedPackageVersion (Read-TextFile (Join-Path $Root 'Cargo.lock'))
    if ($lockedVersion -ne $packageVersion) {
        $problems.Add("Cargo.lock records $script:PackageName $lockedVersion but Cargo.toml is $packageVersion. Run 'cargo metadata --format-version 1' and review the Cargo.lock diff.")
    }

    foreach ($problem in (Test-ChangelogText (Read-TextFile (Join-Path $Root 'CHANGELOG.md')) $packageVersion)) {
        $problems.Add($problem)
    }

    $problems.ToArray()
}

function Assert-RepositoryState([string]$Root) {
    $problems = @(Test-RepositoryState $Root)
    if ($problems.Count -gt 0) {
        foreach ($problem in $problems) { Write-Host "  - $problem" }
        throw "Repository checks failed with $($problems.Count) problem(s)."
    }
}

function Assert-CleanWorktree([string]$Root, [string]$Action) {
    $status = @(Invoke-Git -C $Root status --porcelain)
    if ($status.Count -gt 0) {
        throw "The working tree has uncommitted changes. Commit or stash them before $Action."
    }
}

function Assert-OnMainBranch([string]$Root, [string]$Action) {
    $branch = (Invoke-Git -C $Root rev-parse --abbrev-ref HEAD) | Select-Object -First 1
    if ($branch -ne 'main') {
        throw "Switch to main before $Action (current branch: $branch)."
    }
}

Export-ModuleMember -Function `
    Get-RepositoryRoot, Read-TextFile, Write-TextFile, Invoke-Git, `
    ConvertTo-ReleaseVersion, Get-NextVersion, Get-VersionChangeLevel, `
    Get-PackageVersionFromToml, Set-PackageVersionInToml, Get-LockedPackageVersion, `
    Read-Changelog, Get-UnreleasedEntryCount, Get-UnreleasedCategories, `
    Test-ChangelogText, Add-ChangelogRelease, Test-RepositoryState, `
    Get-DependabotCommitSubjects, Get-DependencyUpdateEntries, Add-UnreleasedEntries, `
    Assert-RepositoryState, Assert-CleanWorktree, Assert-OnMainBranch
