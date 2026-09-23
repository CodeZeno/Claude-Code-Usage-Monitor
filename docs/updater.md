# Updater verification

The portable updater accepts only the exact `claude-code-usage-monitor.exe`
asset from the configured repository's latest stable GitHub release. It rejects
duplicate assets, unexpected download URLs, invalid semantic versions, drafts,
and prereleases (including prerelease tags incorrectly marked as stable).
Version precedence follows SemVer: `2.14.0-beta1` is below `2.14.0`, and build
metadata does not affect ordering.

Every update requires a valid `sha256:` digest and a nonzero size of at most
100 MiB in the GitHub release asset metadata. The updater streams the download,
checks its actual byte count and SHA-256, and only promotes the temporary file
after both match. It stops reading after the expected size plus one byte,
regardless of Content-Length, and deletes its partial file on failure. Requests
and redirects require HTTPS.

The helper receives the expected size and digest, rechecks the staged file before
replacing the installed executable, and holds a Windows file handle that denies
writes and deletion through replacement. Missing or malformed verification
arguments are errors; the old helper invocation without a digest is rejected.

## Trust boundary

The digest comes from the authenticated GitHub API over TLS. This detects corrupt
or substituted downloads relative to that metadata. It is **not a publisher
signature** and does not protect against a compromised GitHub repository or
release account. There is no Authenticode or detached-signature verification in
this updater; adding it requires release signing and a trusted publisher key.
Code already running as the same Windows user is outside this trust boundary.

GitHub computes asset digests for uploaded release assets; see the
[GitHub release asset API](https://docs.github.com/en/rest/releases/assets).
Releases without a digest cannot be installed through the portable updater.
The release workflow does not need a separate checksum upload. WinGet continues
to perform installation through its own upgrade command.

## Tests

Run `cargo test --locked updater` on Windows. Tests exercise metadata selection,
SemVer precedence, streamed size limits, known SHA-256 vectors, corrupt and
truncated bodies, interrupted and failed I/O, partial-file cleanup, helper
arguments, and Windows file locking. Tests do not download or execute updates.
The release workflow runs these tests before building a release.
