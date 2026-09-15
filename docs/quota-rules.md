# Quota Rules

`Quota rules revision: 2026-09-16-01`
`Last verified: 2026-09-16`

This is the specification of record for how this app interprets and labels
provider usage data. It is not user-facing copy — README stays short and
simple; this document is where the underlying meaning of each provider's
number is defined and kept up to date.

## 1. Common Policy

- This app displays whatever usage quota it can actually fetch, in parallel,
  per provider.
- The on-screen UI stays simple. Any nuance about what a number really means
  belongs here, not in the widget or in README.
- A provider's UI label and its underlying data source are not guaranteed to
  be a 1:1 match. Where they diverge, this document is authoritative — if the
  UI label and this document disagree, this document describes the intended
  and correct meaning; the UI label is the simplified surface of it.
- A provider is added only on the condition that it does not produce a
  misleading display (see [Section 4](#4-conditions-for-adding-a-future-provider)).

## 2. Per-Provider Record Schema

Every provider entry in [Section 3](#3-current-providers) must record the
following fields. When a new provider is added, copy this schema and fill it
in — do not invent a different shape per provider.

| Field | Meaning |
|---|---|
| Display name | The label shown in the widget UI and tray icon |
| Stable family ID | The durable quota-family identifier used in runtime data and snapshots |
| Internal adapter | The Rust-side provider-specific fetch/parse functions |
| Source | Where the data comes from: endpoint(s), credential/auth mechanism |
| Quota scope | Whether the fetched number is a **shared** quota (spans multiple products/tools) or an **independent** quota (specific to this one integration) |
| Quota items | The ordered items supplied by this family, including metric form, unit, and reset meaning |
| Fallback behavior | Whether the fetch path has a fallback when the preferred data isn't available, and what it falls back to |
| Unavailable conditions | When this provider's window(s) should be treated as not available (no data, not applicable, etc.) |
| Minimum fetchable unit | The smallest unit of data the source actually returns (e.g. a single aggregate percentage vs. per-model breakdown) |
| Display caveats | Anything a reader should know before trusting the on-screen number at face value |
| Last verified | Date this entry was last checked against the live implementation |
| Rule revision | The `Quota rules revision` value this entry was last updated under |

## 3. Current Providers

### Claude

| Field | Value |
|---|---|
| Display name | Claude |
| Stable family ID | `claude` |
| Internal adapter | `poll_claude_code`, `claude_usage_from_response` |
| Source | `https://api.anthropic.com/api/oauth/usage`, authenticated with the OAuth token from `~/.claude/.credentials.json` (the Claude Code CLI's own login session) |
| Quota scope | Shared — this is the Claude / Claude Code account-level usage window, not a Claude Code-specific metric |
| Quota items | `session`: percentage + reset from the `five_hour` bucket; `weekly`: percentage + reset from the `seven_day` bucket |
| Fallback behavior | None beyond the existing credential-source fallback (Windows / WSL) |
| Unavailable conditions | No credentials found; credentials expired (no automatic CLI refresh by default) |
| Minimum fetchable unit | A single aggregate percentage + reset time per window |
| Display caveats | None known — the label "Claude" is expected to match the fetched data without qualification |
| Last verified | 2026-08-08 |
| Rule revision | 2026-08-08-01 |

### Codex

| Field | Value |
|---|---|
| Display name | Codex |
| Stable family ID | `codex` |
| Internal adapter | `poll_codex`, `cached_codex_usage`, `fetch_codex_app_server_usage`, `fetch_codex_rate_limits_result`, `codex_native_usage_from_result` |
| Source | OpenAI's official `codex app-server` (spawned as `codex app-server --stdio`), method `account/rateLimits/read`. The app never reads `~/.codex/auth.json` or any other Codex credential file, never extracts, stores, or refreshes a Codex OAuth token, and never calls a ChatGPT/Codex backend endpoint (e.g. `https://chatgpt.com/backend-api/wham/usage`) directly. Authentication is entirely the app-server's (and thus the Codex CLI login's) responsibility |
| Quota scope | Independent — this is Codex's own native rate-limit read, not a generic ChatGPT backend scrape |
| Quota items | `session` (5h): percentage + reset for a window with `windowDurationMins == 300`; `weekly` (7d): percentage + reset for a window with `windowDurationMins == 10_080`. Classification always uses `windowDurationMins` and is independent of whether the window arrived as `rateLimits.primary` or `rateLimits.secondary` |
| Fallback behavior | None. On any app-server failure (CLI not found, spawn failure, `initialize` failure, timeout, malformed/protocol-breaking response), the family is reported unavailable — there is no fallback to a credential file, a direct HTTP endpoint, or any other legacy path |
| Unavailable conditions | `codex` executable not resolvable; the app-server process fails to start, times out, or exits abnormally; the JSON-RPC handshake or `account/rateLimits/read` call fails or returns no `result`; a window is missing, null, or has a `windowDurationMins` other than `300`/`10_080`. Each condition degrades that window (or the whole family, when neither window is usable) to unavailable — never to a fabricated `0%` |
| Minimum fetchable unit | A single aggregate percentage + reset time per usage window, plus `rateLimitResetCredits.availableCount` for the banked Full reset count — all three read from the one `account/rateLimits/read` response |
| Display caveats | None known — Codex's label matches its own native rate-limit data with no ChatGPT-backend indirection |
| Last verified | 2026-09-08 |
| Rule revision | 2026-09-08-01 |

Codex app-server protocol and lifecycle rules:

- Per poll (subject to the cache below), the monitor spawns `codex app-server
  --stdio` over stdio, sends `initialize`, then `initialized`, then
  `account/rateLimits/read`, reads the one matching JSON-RPC response, and
  closes the subprocess. It does not run as a resident daemon.
- 5h usage, 7d/weekly usage, and the banked Full reset count are all read from
  that single `account/rateLimits/read` response — there is no separate
  request for banked resets.
- The count authority for banked resets is
  `rateLimitResetCredits.availableCount`; the number of optional credit detail
  rows is never used as the count. When `rateLimitResetCredits` or
  `availableCount` is absent, the Full reset count is unavailable, not zero.
- Reset-credit consume/redeem operations are never invoked, and private
  ChatGPT/Codex backend endpoints are never called directly.
- The child process is always cleaned up — on a clean response, on JSON-RPC
  protocol errors, on malformed/unparseable output, on `initialize` failure,
  and on timeout (`CODEX_APP_SERVER_TIMEOUT`, 10s). `CodexAppServer`'s `Drop`
  implementation waits briefly for the child to exit on its own and force-kills
  it otherwise, so no zombie process is left behind in any of these cases.
- The result of `account/rateLimits/read` (both usage windows and the banked
  reset count together) is cached for `CODEX_RATE_LIMITS_CACHE_TTL` (45
  seconds) so the app's poll cycle does not spawn `codex app-server` on every
  poll tick; the cache is shared by the GUI and headless polling paths.
- Raw app-server responses, credit IDs, auth tokens, cookies, and credentials
  are not logged or displayed. Credit detail rows and expiry metadata are not
  retained by the UI data model.

### Antigravity

| Field | Value |
|---|---|
| Display name | Antigravity |
| Stable family ID | `antigravity` |
| Internal adapter | `poll_antigravity`, `antigravity_usage_from_summary`, gated behind the `antigravity` Cargo feature |
| Source | Google Cloud Code / Antigravity quota endpoints, authenticated with the OAuth token from Windows Credential Manager target `gemini:antigravity` |
| Quota scope | Independent — this is Antigravity's own quota system, not a standalone Gemini API/app quota |
| Quota items | `session`: percentage + reset from the selected group's `5h` bucket; `weekly`: percentage + reset from its `weekly` bucket |
| Fallback behavior | The quota summary can contain multiple model-family groups (Gemini, Claude, GPT, image models). The fetch prefers the group whose name/description/bucket IDs match "Gemini"; if no Gemini group is present, it falls back to the first group it was able to parse, and further falls back to a separate model-quota endpoint if the summary itself is unavailable |
| Unavailable conditions | No Antigravity credentials found; `antigravity` Cargo feature not compiled in (default release build did not include it until 2026-08-08 — see rule revision 2026-08-08-01 in the app's own change history) |
| Minimum fetchable unit | A single aggregate percentage + reset time per window, taken from one selected group — not a per-model breakdown |
| Display caveats | The number is **not guaranteed to be Gemini's quota specifically**. It is Gemini's quota when a Gemini group is present in the summary, and some other model's quota otherwise. This is why Gemini is not listed as its own separate provider in this document — see [Section 4](#4-conditions-for-adding-a-future-provider) |
| Last verified | 2026-08-08 |
| Rule revision | 2026-08-08-01 |

#### Official statusLine bridge (parallel path, not yet wired into runtime)

`ANTIGRAVITY-STATUSLINE-BRIDGE-01` added a second, not-yet-active data path
for this provider, built to eventually replace the credential/backend path
above:

- **Technical source**: the Antigravity CLI's (`agy`) own official
  `/statusline <command>` feature — a small external command the CLI
  invokes with a JSON payload on stdin once per interactive-TUI refresh,
  confirmed live against `agy 1.1.23`. This app never talks to Google or
  Antigravity directly through this path, never reads an OAuth token or
  any credential, and never launches `agy` itself; it only ever reads
  bytes handed to it on stdin by the CLI the *user* is already running.
- **Bridge/cache**: `antigravity_statusline` module + the
  `aum-quota antigravity-statusline-bridge` subcommand. The subcommand
  reads one statusLine JSON payload from stdin, keeps only an allowlisted
  set of fields (`quota.*.remaining_fraction`, `quota.*.reset_time`,
  `quota.*.reset_in_seconds`, `plan_tier`, and a CLI/version string if
  present), and atomically writes them to
  `%APPDATA%\ClaudeCodeUsageMonitor\antigravity_statusline_cache.json`.
  Every other field in the raw payload — including `email`, `session_id`,
  `conversation_id`, `transcript_path`, `cwd`, `workspace`, and any
  prompt/conversation content — is never read by this bridge, let alone
  persisted.
- **Verified payload fields** (live capture, 2026-09-16): a `quota` object
  keyed by an internal quota name, each entry carrying
  `remaining_fraction` (`0.0`-`1.0`), `reset_time` (RFC 3339 UTC, e.g.
  `2026-09-22T21:44:07Z`), and `reset_in_seconds`; plus a top-level
  `plan_tier` string (e.g. `"Google AI Plus"`). The only two quota keys
  observed live so far are `gemini-weekly` and `3p-weekly` — **no 5h-scoped
  quota key has been observed yet**, so this provider's official-path data
  is weekly-only until/unless one appears. The reader must not panic, and
  must not fabricate a 5h value, when a payload has only weekly keys, or
  when a future payload adds keys this app doesn't yet recognize.
- **`gemini-weekly`** may be treated as a Gemini-family weekly quota.
- **`3p-weekly`**'s semantics are **not documented by Google** and are
  **not finalized by this app**. It is carried through under its raw key
  rather than renamed to a specific model family (e.g. "Claude weekly" or
  "GPT weekly") until that meaning is confirmed from an authoritative
  source.
- **Opt-in, not automatic**: configuring `/statusline <command>` inside
  Antigravity replaces its built-in status line unless
  `stack_with_default: true` is also set. This app does not modify the
  user's Antigravity configuration on its own; any future setup flow that
  offers to configure it must be an explicit, reversible, user-initiated
  action, and must preserve the built-in status line via
  `stack_with_default: true`.
- **Runtime status**: as of `ANTIGRAVITY-STATUSLINE-BRIDGE-01`, this bridge
  and its cache reader exist and are tested, but `poll_antigravity` above
  is still the only path actually wired into the running app. Replacing it
  is a separate, later decision — see the revision log entry for this
  change for what is and isn't done yet.
- **Distribution/Store permission is a separate, unresolved question** from
  this technical implementation and is not addressed by this document.

### GitHub Copilot

| Field | Value |
|---|---|
| Display name | GitHub Copilot |
| Stable family ID | `github_copilot` |
| Internal adapter | `poll_github_copilot`, `github_copilot_usage_from_response` |
| Source | Official GitHub user billing AI-credit usage REST API, `/users/{username}/settings/billing/ai_credit/usage`, invoked through `gh api` |
| Quota scope | Independent monthly Copilot AI Credits for a paid individual account |
| Quota items | `monthly_ai_credits`: summed Copilot `grossQuantity` as used AI Credits; optionally paired with the manually selected plan allowance; reset at the first day of the next calendar month at 00:00 UTC |
| Fallback behavior | None. GitHub CLI is the credential broker; the app does not read, refresh, or store a GitHub token itself |
| Unavailable conditions | `gh` missing or not logged in, insufficient API permission, command/API failure, malformed values, or a non-empty response with no recognizable Copilot AI-credit rows. These conditions are never displayed as zero usage |
| Minimum fetchable unit | Aggregate Copilot AI-credit usage rows. The app sums `grossQuantity`; it deliberately does not use `netQuantity`, which may be zero after included-credit discounts |
| Display caveats | Initial scope is Paid Individual only. Free, Student, Business, and Enterprise are unsupported. Plan is a manual setting: `Pro` = 1,500, `Pro+` = 7,000, `Max` = 20,000 AI Credits as of 2026-08-10. These totals include a flex allotment and may change in GitHub's product specification. `Unknown` shows gross usage only and does not invent limit, remaining, or percentage. Subscription billing date is not treated as quota reset date |
| Last verified | 2026-08-10 |
| Rule revision | 2026-08-10-01 |

Security and retention rules:

- Raw API responses, GitHub tokens, credentials, opaque authentication data,
  per-model billing detail, and individual usage rows are not logged,
  displayed, or retained in settings/snapshots.
- Only the aggregate quota item needed by the UI is retained.
- The app never runs `gh auth refresh`, changes scopes, or mutates the user's
  GitHub authentication state. Authentication failures are surfaced as an
  unavailable/not-configured state.

## 4. Conditions for Adding a Future Provider

A new provider may be added only when all of the following hold:

1. **Independently fetchable.** Its usage data can be retrieved on its own,
   without depending on another provider's fallback chain.
2. **Label/source gap is explainable.** If the display name and the actual
   data source diverge (as with Antigravity's label not always meaning Gemini
   specifically — see the Antigravity entry), that gap must be written down in
   this document using the schema in
   [Section 2](#2-per-provider-record-schema) before the label ships.
3. **No double display.** The new provider must not show a number that
   already appears (in full or in part) under an existing provider's column.
   Example: Gemini is not added as its own provider today because its data
   already surfaces, conditionally, under Antigravity.
4. **Has a documentation home.** README, and any future Help/About surface,
   must be able to carry a short pointer to this document for the new
   provider — this document is not a substitute for that pointer, and that
   pointer is not a substitute for filling in this document.
5. **Unavailable state is defined.** The conditions under which the new
   provider's window(s) should be treated as unavailable (rather than shown
   with stale or wrong data) must be written into its
   [Section 2](#2-per-provider-record-schema) entry.

If any of these cannot be satisfied yet, the correct action is to document
the provider's current limitation here (or defer it) rather than add a UI
column that misrepresents what is actually being measured.

## 5. Revision Log

| Revision | Date | Change |
|---|---|---|
| 2026-09-16-01 | 2026-09-16 | `ANTIGRAVITY-STATUSLINE-BRIDGE-01`: adds a parallel, opt-in, not-yet-active Antigravity data path via the Antigravity CLI's official `/statusline <command>` feature — a sanitized local cache (`antigravity_statusline` module + `aum-quota antigravity-statusline-bridge`) that never touches Google/Antigravity credentials or backends directly. Documents the verified payload shape, the `gemini-weekly`/`3p-weekly` quota keys (`3p-weekly`'s semantics unconfirmed), and that this path is weekly-only so far (no 5h key observed). Does not change which path `poll_antigravity` actually uses at runtime. |
| 2026-09-08-01 | 2026-09-08 | `CODEX-OFFICIAL-PATH-IMPLEMENT-01`: unifies Codex quota onto the official `codex app-server` `account/rateLimits/read` method only. Removes the `~/.codex/auth.json` credential read, the Codex OAuth token extraction/refresh, and the direct `https://chatgpt.com/backend-api/wham/usage` HTTP fetch entirely — 5h/7d usage and the banked Full reset count now come from one cached app-server response, with no fallback to any legacy path. |
| 2026-08-10-01 | 2026-08-10 | Generalizes the runtime record to stable quota families with ordered quota items, restores the Codex display name, and adds GitHub Copilot Paid Individual monthly AI Credits with GitHub CLI authentication, gross-usage, manual-plan, reset, failure, and non-retention rules. |
| 2026-08-09-01 | 2026-08-09 | Adds the ChatGPT/Codex banked Full reset count from the stable app-server method, including zero-vs-unavailable semantics, lifecycle, failure isolation, refresh, and non-retention rules. |
| 2026-08-08-01 | 2026-08-08 | Initial version. Documents Claude, ChatGPT, and Antigravity as of the `Claude Code`→`Claude` and `Codex`→`ChatGPT` display-label changes, and the Antigravity release-build feature-flag fix. |
