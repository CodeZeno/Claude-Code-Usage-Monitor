# Check the repository rules that CI and the release scripts enforce:
# - files ignored by .gitignore (including AGENTS.md and workflow.cmd) are not tracked;
# - .gitattributes normalizes text files and none is stored with CRLF;
# - Cargo.lock records the Cargo.toml package version;
# - CHANGELOG.md is well formed, its newest release matches Cargo.toml, and
#   unreleased changes are listed under '## [Unreleased]'.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Import-Module (Join-Path (Split-Path -Parent $PSScriptRoot) 'scripts/ReleaseTools.psm1') -Force

$root = Get-RepositoryRoot
Assert-RepositoryState $root
Write-Host 'Repository checks passed.'
