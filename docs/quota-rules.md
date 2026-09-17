# Quota Rules

`Quota rules revision: 2026-09-18-01`
`Last verified: 2026-09-18`

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
| Internal adapter | `poll_antigravity` (reads `antigravity_statusline`'s local cache only), gated behind the `antigravity` Cargo feature |
| Source | The Antigravity CLI's (`agy`) own official `/statusline <command>` feature — a small external command `agy` invokes with a JSON payload on stdin once per interactive-TUI refresh. This app never calls a Google/Antigravity endpoint, reads an OAuth token, reads Windows Credential Manager, or launches `agy` itself; production data comes only from a sanitized local cache this app's own `aum-quota antigravity-statusline-bridge` subcommand wrote from that payload. Packaged as a stable, update-surviving MSIX App Execution Alias (`aum-quota.exe`) — see `ANTIGRAVITY-PACKAGING-BRIDGE-FIX-01` |
| Quota scope | Independent — this is Antigravity's own quota system, not a standalone Gemini API/app quota |
| Quota items | One item per quota key present in the cache (e.g. `gemini-weekly`, `3p-weekly`) — a weekly-only payload is normal; a future 5h-scoped key is picked up automatically without a code change |
| Freshness semantics | **No fixed cache-age TTL.** The cache only updates on Antigravity-TUI activity, not a timer (live measurement showed multi-minute gaps even during active use), so cache age alone is not evidence the data is wrong. Each quota item is instead judged against its own `reset_time`: still in the future → usable as the current reading; already past → the item is marked `Stale` with no value shown, because the pre-reset number must never be displayed as current and the true post-reset value hasn't been observed yet. A family with at least one non-stale item is `Available`; a family where every item has passed its reset is `Stale` at the family level too. `captured_at_unix` is retained on the cache as "last observed" metadata only, not a staleness signal — see `ANTIGRAVITY-ROUTING-SWITCH-01` |
| Unavailable conditions | Cache file missing (bridge never run / not opted into), malformed, or an unsupported `schema_version` — all three map to the same `Unavailable` family status as any other polling error, and are never displayed as `0%` usage. There is no legacy network/credential path to fall back to on any of these |
| Minimum fetchable unit | One item per quota key from the cache, judged independently for staleness — not a single aggregate percentage |
| Display caveats | `gemini-weekly` is treated as a Gemini-family weekly quota. `3p-weekly`'s semantics are **not documented by Google** and are **not finalized by this app**; it is carried through under its raw key rather than renamed to a specific model family (e.g. "Claude weekly" or "GPT weekly") until that meaning is confirmed from an authoritative source. No 5h-scoped key has been observed live yet, so this provider's data is weekly-only until/unless one appears — the reader must not panic or fabricate a 5h value in that case |
| Opt-in, not automatic | Configuring `/statusline <command>` inside Antigravity replaces its built-in status line unless `stack_with_default: true` is also set. This app does not modify the user's Antigravity configuration on its own; any future setup flow that offers to configure it must be an explicit, reversible, user-initiated action, and must preserve the built-in status line via `stack_with_default: true` |
| Setup UX | `ANTIGRAVITY-SETUP-UX-01`: the right-click Help menu's "Antigravity Setup..." item (feature-gated) shows the exact `/statusline aum-quota.exe antigravity-statusline-bridge` command (copied to the clipboard when the item is clicked), the exact `/statusline delete` command to remove it, and the last-observed time read fresh from the cache file (never a fabricated timestamp — a missing/malformed/unsupported-schema cache shows "no usage observed yet" instead). This app never edits the Antigravity CLI's own `settings.json` on the user's behalf, in setup or in teardown; turning this app's own `show_antigravity` display toggle off is a separate action from the Antigravity CLI's own statusLine configuration, and the dialog text is explicit about that distinction |
| Distribution/Store permission | A separate, unresolved question from this technical implementation and not addressed by this document |
| Last verified | 2026-09-16 |
| Rule revision | 2026-09-16-03 |

### GitHub Copilot

| Field | Value |
|---|---|
| Display name | GitHub Copilot |
| Stable family ID | `github_copilot` |
| Internal adapter | `poll_github_copilot`, `github_copilot_usage_from_response` |
| Source | Official GitHub user billing AI-credit usage REST API, `/users/{username}/settings/billing/ai_credit/usage`, invoked through `gh api` |
| Quota scope | Independent monthly Copilot AI Credits from the personal-account user billing source. A 2026-09-18 live recheck on an account the user reports as Copilot Free returned a successful response, so Free compatibility is recorded as observed behavior rather than generalized from the API documentation |
| Quota items | `monthly_ai_credits`: summed Copilot `grossQuantity` as used AI Credits; optionally paired with the manually selected plan allowance; reset at the first day of the next calendar month at 00:00 UTC |
| Fallback behavior | None. GitHub CLI is the credential broker; the app does not read, refresh, or store a GitHub token itself |
| Unavailable conditions | `gh` missing or not logged in, insufficient API permission, command/API failure, malformed values, or a non-empty response with no recognizable Copilot AI-credit rows. An empty `usageItems` array is valid and is interpreted as zero observed AI-credit usage, not a fetch failure |
| Minimum fetchable unit | Aggregate Copilot AI-credit usage rows. The app sums `grossQuantity`; it deliberately does not use `netQuantity`, which may be zero after included-credit discounts |
| Display caveats | The app does not hard-code or infer a Free numeric allowance. The 2026-09-18 live Free-account recheck returned success with `usageItems: []`; under the current parser this is `0` observed AI Credits. Free remains on the `Unknown` plan setting, which shows gross usage only and does not invent a limit, remaining amount, or percentage. Paid manual settings remain `Pro` = 1,500, `Pro+` = 7,000, `Max` = 20,000 AI Credits. Student, Business, and Enterprise are not claimed supported by this personal-account path. |
| Last verified | 2026-09-18 |
| Rule revision | 2026-09-18-01 |

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
| 2026-09-18-01 | 2026-09-18 | `GITHUB-COPILOT-FREE-RECHECK-01`: live Copilot Free recheck returned success with empty `usageItems`; treat as zero observed usage while keeping Free as `Unknown` / usage-only because no authoritative numeric Free limit is established. |
| 2026-09-16-03 | 2026-09-16 | `ANTIGRAVITY-SETUP-UX-01`: adds a Help-menu "Antigravity Setup..." dialog (setup command, disable command, last-observed time — clipboard-copies the setup command on click) and, as a prerequisite fix, replaces Antigravity's fixed session/weekly display slots with a dynamic per-cache-item bar list (`gemini-weekly` keeps its own weekly slot with pace guidance; every other item, including unrecognized future keys, gets its own always-visible bar, never hidden in a details-only view) — the prior routing-switch commit had left these bars permanently showing "not available" regardless of real cache content, since `UsageData::from_quota_items` never sets the fixed session/weekly availability flags the display code checked. |
| 2026-09-16-02 | 2026-09-16 | `ANTIGRAVITY-ROUTING-SWITCH-01`: switches `poll_antigravity` from the legacy Google Cloud Code HTTP/OAuth path onto the official statusLine cache exclusively, and removes the legacy path's code entirely (credential read, Windows Credential Manager access, HTTP fetch/summary-parsing functions, and their dedicated `CredentialWatchMode::Antigravity` variant). Decides against a fixed cache-age TTL (measured update cadence is activity-driven and irregular); freshness is instead judged per quota item against that item's own `reset_time`, with items past their reset marked `Stale` (never shown as current, never assumed `0`) and independent per key. A family with no current item is `Stale` at the family level. Cache missing/malformed/unsupported-schema all map to the existing `Unavailable` status, never `0%`, with no legacy fallback. |
| 2026-09-16-01 | 2026-09-16 | `ANTIGRAVITY-STATUSLINE-BRIDGE-01`: adds a parallel, opt-in, not-yet-active Antigravity data path via the Antigravity CLI's official `/statusline <command>` feature — a sanitized local cache (`antigravity_statusline` module + `aum-quota antigravity-statusline-bridge`) that never touches Google/Antigravity credentials or backends directly. Documents the verified payload shape, the `gemini-weekly`/`3p-weekly` quota keys (`3p-weekly`'s semantics unconfirmed), and that this path is weekly-only so far (no 5h key observed). Does not change which path `poll_antigravity` actually uses at runtime. |
| 2026-09-08-01 | 2026-09-08 | `CODEX-OFFICIAL-PATH-IMPLEMENT-01`: unifies Codex quota onto the official `codex app-server` `account/rateLimits/read` method only. Removes the `~/.codex/auth.json` credential read, the Codex OAuth token extraction/refresh, and the direct `https://chatgpt.com/backend-api/wham/usage` HTTP fetch entirely — 5h/7d usage and the banked Full reset count now come from one cached app-server response, with no fallback to any legacy path. |
| 2026-08-10-01 | 2026-08-10 | Generalizes the runtime record to stable quota families with ordered quota items, restores the Codex display name, and adds GitHub Copilot Paid Individual monthly AI Credits with GitHub CLI authentication, gross-usage, manual-plan, reset, failure, and non-retention rules. |
| 2026-08-09-01 | 2026-08-09 | Adds the ChatGPT/Codex banked Full reset count from the stable app-server method, including zero-vs-unavailable semantics, lifecycle, failure isolation, refresh, and non-retention rules. |
| 2026-08-08-01 | 2026-08-08 | Initial version. Documents Claude, ChatGPT, and Antigravity as of the `Claude Code`→`Claude` and `Codex`→`ChatGPT` display-label changes, and the Antigravity release-build feature-flag fix. |
