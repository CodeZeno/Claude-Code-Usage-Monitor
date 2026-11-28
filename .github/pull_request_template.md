<!--
For suspected vulnerabilities, email contact@codezeno.com.au before opening a
public PR. See SECURITY.md. Never include credential files, OAuth tokens, API
keys, session cookies, or authorization headers in code, fixtures, or attachments.
Redact personal information from logs and screenshots.
-->

## Summary

<!-- Describe the problem and resulting behavior. Link related public issues. -->

-

## Testing

<!-- List checks performed and their results, or explain why checks were not run.
Include relevant manual verification and redacted screenshots for UI changes. -->

-

## Checklist

- [ ] I have read CONTRIBUTING.md and CODE_OF_CONDUCT.md.
- [ ] The change is focused and contains no credentials or sensitive data.
- [ ] I have added or updated relevant documentation and tests, where applicable.
- [ ] Theme changes: I have updated `tests/snapshots/themes` and checked the new images
      (see CONTRIBUTING.md, Theme changes).
- [ ] New providers: `every_provider_meets_the_contract` passes with a redacted fixture,
      and the provider reads only a sign-in that its own CLI, app or IDE already stored;
      it does not ask the user for a key, token or cookie (see CONTRIBUTING.md,
      Provider changes).
- [ ] New hosts, programs, native libraries or crates are explained above, and any
      footprint warning from CI is explained (see CONTRIBUTING.md, Policy tests and
      Footprint).
- [ ] I have left changelog and version changes to maintainers unless requested.
