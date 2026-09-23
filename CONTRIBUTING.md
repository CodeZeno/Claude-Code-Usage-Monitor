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
[updater documentation](docs/updater.md) for update verification changes.

## Checks

For Rust changes, run the checks relevant to your change on Windows:

```powershell
cargo fmt --all -- --check
cargo test --locked
cargo build --locked --release
```

Add regression coverage for behavior changes where practical, using dummy data
instead of real credentials or live provider requests. For UI changes, manually
check the affected dashboard or taskbar behavior and include redacted screenshots
when helpful. For documentation-only changes, check links and formatting;
compiling the application is not necessary.

## Pull requests

Keep each pull request focused on one problem and avoid unrelated formatting or
refactoring. Explain the problem, the resulting behavior, and how you verified
the change. List checks that could not be run and why.

Use a descriptive title, such as `fix(poller): handle expired credentials` or
`docs: clarify provider setup`, and complete the **Summary** and **Testing**
sections in the pull request template. Link related public issues when relevant.

Do not edit `CHANGELOG.md`, bump package versions, or create release tags as part
of a contribution unless a maintainer explicitly requests it. Release metadata
is handled separately. Change `Cargo.lock` only when the contribution requires a
dependency update.

Contributions are provided under the repository's [MIT License](LICENSE).
