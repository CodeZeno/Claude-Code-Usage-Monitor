# Contributing

Bug reports, fixes, documentation, translations, themes, and feature proposals
are welcome. Please follow our [Code of Conduct](CODE_OF_CONDUCT.md).

## Security reports

Report suspected vulnerabilities privately using [SECURITY.md](SECURITY.md).
Do not open a public issue or pull request containing vulnerability details or
credentials.

## Issues and proposals

Search existing issues and pull requests before opening a new one. Use the bug
report or feature request template. For a substantial change, discuss the
problem and proposed approach in an issue before investing in implementation.

For bugs, include the app version, Windows version, affected provider, steps to
reproduce, and expected and actual behavior. Include only relevant, redacted
diagnostics. Never attach credential files, OAuth tokens, API keys, session
cookies, or authorization headers. Check screenshots and logs for account
identifiers and other personal information before posting.

## Development setup

Build on Windows 10 or 11 with Rust 1.95 or later and the MSVC toolchain. Install
the Visual Studio Build Tools with the Desktop development with C++ workload
and a Windows SDK. Fork and clone the repository, then create a topic branch.

From the repository root:

```powershell
cargo build --locked
cargo run --locked -- --dashboard
```

Provider testing requires the corresponding provider to be installed and signed
in. Enable only the providers needed for your change and use your own test
accounts. Keep credentials and local configuration out of commits and fixtures.

See the [user guide](USER_GUIDE.md) for application behavior,
[localization guide](src/localization/README.md) for translations, and
[updater documentation](docs/updater.md) for update verification changes. The
[feature map](docs/feature-map.md) names each part of the app (widget, tray icon,
context menu, dashboard pages, Theme Studio panels) and where its code lives; use
those names in issues and pull requests.

## Checks

For Rust changes, run these checks on Windows. CI runs the same checks on every
pull request, and all of them must pass before merging:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
./tests/Test-Repository.ps1
```

Add regression coverage for behavior changes where practical, using dummy data
instead of real credentials or live provider requests. For UI changes, manually
check the affected dashboard or taskbar behavior and include redacted screenshots
when helpful. For documentation-only changes, check links and formatting;
compiling the application is not necessary.

### Theme changes

Theme fields are mostly expressions, not fixed numbers; see the generated
[theme reference](docs/theme-reference.md) for every field, value, function,
format and mouse action. The language is defined once in
`src/theme_engine/theme_language.rs`: the evaluator, Theme Studio and the
reference read it, and `cargo test` fails when the reference is out of date or a
built-in theme uses a field the engine does not know.

Every surface of every theme in `src/themes/` is rendered in dark and light mode
from the sample usage in `tests/fixtures/theme-usage.json`, with the clock,
time zone and appearance fixed, and compared with the reference images in
`tests/snapshots/themes/`. `cargo test` fails when a render changes, writes the
new renders to `target/theme-snapshots/` (with a `-diff.png` beside each changed
image), and CI uploads that folder as the `theme-snapshots` artifact, so a theme
pull request can be reviewed as pictures.

When the change is intended, update the references, look at them, and commit
them with the change:

```powershell
$env:CCUM_UPDATE_SNAPSHOTS = 1; cargo test --locked theme_render; Remove-Item Env:CCUM_UPDATE_SNAPSHOTS
```

To render any theme file or built-in theme id without starting the app, for
example to attach screenshots to a pull request:

```powershell
cargo build --locked
./target/debug/claude-code-usage-monitor.exe --render-theme my-theme.json --out renders | Out-Host
```

`--out` is a folder (one `<theme>-<surface>-<appearance>.png` per surface and
appearance) or a `.png` file when one image is rendered. Options:
`--surface <id | index | all>`, `--appearance dark | light | both`,
`--fixture <usage.json>`, `--now <unix seconds>`, `--utc-offset <minutes>` and
`--scale <factor>`. The executable is a Windows GUI program, so in PowerShell
pipe it to `Out-Host` (or use `Start-Process -Wait -NoNewWindow`) to wait for it
and see its output. It prints a warning for each unknown field and failing
expression.

### Provider changes

`every_provider_meets_the_contract` in `src/poller/tests.rs` checks every
provider in `ProviderId::ALL`. A new provider needs:

- a redacted response body in `src/poller/fixtures/<key>.json`, using
  placeholders such as `redacted`, `usr_example` and `someone@example.com`
  instead of tokens, cookies, account ids and email addresses. The test rejects
  values that look like credentials;
- a `usage_from_fixture` function in its poller module, listed in
  `poller::usage_from_fixture`, that parses the body the way a poll does and
  reports at least one available limit;
- usage that survives a save and load of the usage cache, and that themes see
  as `<key>.available`;
- its display name and setting description in every file in
  `src/localization/locales/`;
- a unique key, cache key and menu command id in `src/providers.rs`.

### Lint rules

`Cargo.toml`'s `[lints]` table and `clippy.toml` encode project rules, so Clippy
reports a broken rule and the fix:

- `unsafe` code is denied unless a file opts in with
  `#![allow(unsafe_code, reason = "...")]`. Keep native calls in files that
  already opt in where practical.
- Every unsafe block needs a `// SAFETY:` comment stating why each precondition
  of the unsafe calls holds at that point: which handle is valid and who owns
  it, what keeps a pointer alive, and why a buffer length matches.
- Production code must not use `unwrap()`. Return or handle the error, or use
  `expect("...")` stating why the value cannot be missing. Tests may unwrap.
- HTTP agents are created only by `poller::build_agent` and
  `updater::build_agent`, which apply the TLS and timeout settings.
- Do not change the process environment (`std::env::set_var` and
  `std::env::remove_var`); provider discovery and parallel tests share it.

Fix the code rather than adding an `allow`. When an exception is necessary,
scope it to the item that needs it and give a `reason`.

### Line endings

`.gitattributes` stores text files with LF line endings, whatever your
`core.autocrlf` setting; Windows checkouts still get CRLF. Keep a file's existing
line endings when you edit it, and do not reformat line endings in unrelated
files. `tests/Test-Repository.ps1` reports any text file stored with CRLF.

## Pull requests

Keep each pull request focused on one problem and avoid unrelated formatting or
refactoring. Explain the problem, the resulting behavior, and how you verified
the change. List checks that could not be run and why.

Use a descriptive title, such as `fix(poller): handle expired credentials` or
`docs: clarify provider setup`, and complete the **Summary** and **Testing**
sections in the pull request template. Link related public issues when relevant.

Do not edit `CHANGELOG.md`, bump package versions, or create release tags as part
of a contribution unless a maintainer explicitly requests it. Maintainers add
merged changes to the `[Unreleased]` changelog section and prepare releases
separately; see [releasing](docs/releasing.md). Change `Cargo.lock` only when the contribution requires a
dependency update.

Contributions are provided under the repository's [MIT License](LICENSE).
