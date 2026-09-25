# Dependency security

Dependabot checks Cargo dependencies and GitHub Actions weekly and opens update
pull requests. Review the manifest and lockfile diff and run the Windows tests
before merging. The egui family is grouped because its versions are coupled.

The `Dependency security` workflow runs on pull requests, branch pushes, a daily
schedule, and manual dispatch. The release workflow calls the same checks and
waits for them before building or publishing. Checks use read-only repository
permissions and do not require secrets.

- `cargo audit` checks the entire committed `Cargo.lock` against the current
  RustSec advisory database and fails on known vulnerabilities. Informational
  warnings remain visible in its output.
- `cargo deny --locked check` checks advisories, licenses, bans, and sources for
  `x86_64-pc-windows-msvc`, including build and development dependencies. Known
  vulnerabilities, unsound or unmaintained dependencies, yanked versions,
  unapproved licenses, wildcard requirements, and unapproved registry/Git
  sources fail the check. Multiple crate versions produce warnings.

The scanners fetch current advisory data on each run. Fix a finding by updating
or replacing the dependency where possible. Any necessary exception must be
narrow and document the advisory, applicability, owner, and review date; do not
disable a category of checks to make CI pass. License exceptions in `deny.toml`
are limited to the existing bundled fonts.

### Current advisory exception

[`RUSTSEC-2026-0192`](https://rustsec.org/advisories/RUSTSEC-2026-0192.html)
reports that `ttf-parser` is unmaintained, with no patched release. Version
0.25.1 is pulled in by `oxifont-subset`/`oxifont-parser` 0.2.2, the latest
available releases when this policy was added. This is a build dependency:
`build.rs` subsets the Ubuntu and Lucide font bytes bundled by locked crates;
it does not parse user-provided fonts at runtime through this dependency.

`deny.toml` temporarily excepts only this advisory. `cargo audit` still displays
its warning, and future vulnerability advisories remain blocking. The repository
maintainers own reviewing this exception by **2026-10-23**, or sooner when
oxifont updates. Update or replace the subsetter to remove `ttf-parser`, then
remove the exception; cargo-deny rejects unused advisory exceptions. The review
date is a maintenance reminder, not an automatically enforced expiration.

## Run locally

Use the same scanner versions as `.github/workflows/dependency-security.yml`:

```powershell
cargo install --locked cargo-audit --version 0.22.2
cargo install --locked cargo-deny --version 0.20.2
./tests/Test-Dependencies.ps1
```

Run `Test-Dependencies.ps1` before each local release build, alongside the existing
Rust verification. It validates locked Cargo metadata, runs `cargo audit --file
Cargo.lock`, then `cargo deny --locked check`, and stops on any failure. A failed
check must be resolved before running `cargo build --release`. The script can be
called from any working directory and restores that directory when it finishes.

After a version or dependency change, first synchronize the lockfile with
`cargo metadata --format-version 1 > $null` and review its diff. The checks themselves
use the existing lockfile and do not update dependencies. Rerun them if manifests,
the lockfile, vendored dependencies, or dependency policy change. Direct Cargo
builds do not invoke this script automatically; it is a required release preparation
step, also recorded in the local agent instructions. Release CI already enforces
both scanner checks before its build job.

Scanner versions are pinned in the workflow and must be updated there and in
these commands together; Dependabot's Cargo updates cover application manifests,
not `cargo install` commands. The security workflow's GitHub Action revisions
are pinned to commits and covered by the GitHub Actions updater. Require both
scanner checks in branch protection if merges must be blocked; that repository
setting is separate from the workflow files.

## Vendored dependency

`vendor/egui-winit` is a local patch and is not automatically refreshed by
Dependabot. When the egui/eframe family changes, follow
[`vendor/egui-winit/PATCH.md`](../vendor/egui-winit/PATCH.md) to update the matching
upstream source, reapply the text-only clipboard changes, and verify the feature
graph. Review upstream security fixes for this copy explicitly: registry
advisories and source allowlists do not validate locally modified source code.
