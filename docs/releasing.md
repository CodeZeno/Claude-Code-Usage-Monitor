# Releasing

Merged changes accumulate under `## [Unreleased]` in [CHANGELOG.md](../CHANGELOG.md).
The package version in `Cargo.toml` stays at the last release until a maintainer
prepares the next one, so `main` can take any number of pull requests between
releases.

The scripts run in Windows PowerShell 5.1 or PowerShell 7. Release preparation and
publishing require a clean working tree on `main`.

## Day to day

Each merged change adds an entry under `## [Unreleased]`, in one of the
`Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, or `Security` categories, and
leaves the version unchanged. Pushing `main` publishes nothing.

`tests/Test-Repository.ps1` runs in CI and should run before each commit. It checks
that:

- no ignored or local-only file (`AGENTS.md`, `workflow.cmd`) is tracked;
- `Cargo.lock` records the same package version as `Cargo.toml`;
- the newest dated changelog release equals the `Cargo.toml` version, so a version
  bump without a release section fails;
- changelog headings, categories, version order, dates, and comparison links are
  consistent, including `[Unreleased]: .../compare/vNEWEST...HEAD`.

`tests/Test-ReleaseTools.ps1` tests the shared logic in `scripts/ReleaseTools.psm1`.

### Dependabot updates

`.github/workflows/dependabot-auto-merge.yml` turns on auto-merge for Dependabot
pull requests that only make a patch-level Cargo update. GitHub squash-merges them
once the required `CI passed` check succeeds, which includes the dependency
security checks. Minor and major updates, the `egui` group (see
[dependency security](dependency-security.md)) and GitHub Actions updates wait for
a maintainer as before.

Auto-merged updates carry no changelog entry. `scripts/New-Release.ps1` adds one
under `Changed` for each Dependabot commit since the previous release tag, unless
an entry already names that pull request or the same version change, so a
maintainer can still write a more detailed entry by hand.

This needs two repository settings: **Allow auto-merge** (Settings, General), and
a branch protection rule or ruleset on `main` that requires the `CI passed` status
check. Without the required check, auto-merge would merge without waiting for CI.

Because these merges happen on GitHub, pull before pushing local work
(`git pull --rebase origin main`, then `workflow -p`). A merge made with the
workflow's token does not start another CI run on `main`; the pull request's
`CI passed` run already covered the merged commit's content.

## When to release

Release in batches rather than once per merge. Merged pull requests and
dependency updates collect under `[Unreleased]` and ship together, for example
once a week, or sooner for a user-facing fix or a security advisory that affects
the shipped application. A dependency update alone is rarely a reason for a
release: the scheduled dependency checks report advisories, and the next release
picks the update up.

## Preparing a release

```powershell
./scripts/New-Release.ps1 -Level minor        # or patch, major, or 2.26.0
```

The script:

1. fetches `origin/main` and refuses when it has commits that local `main` lacks
   (pass `-SkipRemoteCheck` when offline);
2. adds changelog entries for Dependabot updates merged since the last release;
3. requires `[Unreleased]` entries, and refuses a patch release when any are `Added`;
4. sets the `Cargo.toml` version, synchronizes `Cargo.lock`, and refuses any lockfile
   change other than this package's version;
5. moves the entries into `## [VERSION] - YYYY-MM-DD`, keeps an empty
   `## [Unreleased]` heading, and updates the comparison links;
6. runs `tests/Test-Repository.ps1` and `tests/Test-Dependencies.ps1`;
7. commits `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` as `vVERSION`.

If any step fails, the three files are restored. Nothing is pushed or tagged.
Use `-BodyFile` for a commit body and `-Date` to override today's date.

`scripts/Build-Release.ps1` runs the dependency gates, closes running copies of this
repository's `target/release` executable, and builds it with `cargo build --release --locked`.

## Publishing

Push `main` first, so the release commit runs CI there, then:

```powershell
./scripts/Publish-Release.ps1
```

It tags `vVERSION` at `HEAD` and pushes the tag, which starts the Release workflow.
That workflow runs CI and the dependency checks again before building and publishing
the GitHub release, WinGet, and Chocolatey packages. The script refuses when:

- `[Unreleased]` has entries, which the tag would ship without listing;
- the tag already exists locally or on `origin`;
- `HEAD` is not `origin/main`.

## Maintainer wrapper

The local, untracked `workflow.cmd` runs each step separately or in sequence
(`-r`, `-b`, `-p`, `-d`):

```text
workflow -p                    push main; changes stay unreleased (pull first)
workflow -r minor -m "body"    prepare the release commit
workflow -b                    build target/release
workflow -r patch -p -d        prepare, push, and publish
workflow -d                    publish an already pushed release commit
```
