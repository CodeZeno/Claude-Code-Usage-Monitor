# Security policy

## Report a vulnerability privately

Email [contact@codezeno.com.au](mailto:contact@codezeno.com.au) with the subject
`[Security] Claude Code Usage Monitor`. This is Code Zeno's contact address,
published on [codezeno.com.au](https://codezeno.com.au/).

Do **not** report suspected vulnerabilities in public issues, pull requests, or
discussions. If you are unsure whether a problem is security-related, use email
first. GitHub private vulnerability reporting is not currently enabled for this
repository; email is the private reporting route.

Include what you can safely share:

- A description of the vulnerability and its potential impact.
- The app version or commit, Windows version, and affected provider or feature.
- Reproduction steps or a minimal proof of concept using dummy credentials.
- Redacted logs or screenshots, and any suggested mitigation.
- How you would like to be contacted and whether you want credit for the report.

Never send live OAuth access or refresh tokens, API keys, session cookies,
authorization headers, or complete credential files, even in a private report.
Remove account identifiers and other personal information from diagnostic logs,
screenshots, and configuration excerpts. If credentials have already been
exposed, revoke or rotate them through the affected provider.

## Scope and supported versions

The monitor reads local credentials for enabled providers, including Claude
Code, Codex, Google Antigravity, OpenCode Go, Cursor, and Grok Build. Reports about
credential exposure, unsafe credential handling, unintended network requests,
update verification, or dependencies used by the app are welcome. See
[updater verification](docs/updater.md) for the updater's trust boundary.

Security fixes target the latest stable release. Older releases do not have
separate security maintenance branches; users should update to receive fixes.
Reports affecting older versions or unreleased code are still welcome. Include
the affected version, and check the latest release when it is safe to do so.

## Handling and disclosure

Maintainers review reports privately and may ask for additional reproduction
details. Response and fix times depend on severity and maintainer availability;
there is no guaranteed response deadline. Follow up on the same email thread
if you have not received a response.

Please coordinate public disclosure with the maintainers so a fix or mitigation
can be made available before publishing exploit details. Reporter credit should
be agreed during that discussion.
