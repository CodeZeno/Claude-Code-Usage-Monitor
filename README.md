![Windows](https://img.shields.io/badge/platform-Windows-blue)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# Claude Code Usage Monitor

![Screenshot](.github/animation.gif)

A lightweight Windows taskbar widget for people already using Claude Code, with optional Codex, Google Antigravity, and GitHub Copilot usage display.

It sits in your taskbar and shows your configured providers' available quota windows without needing to open the terminal or provider site.

## What You Get

- A **5h** bar for your current 5-hour Claude usage window
- A **7d** bar for your current 7-day window
- Optional Codex usage bars alongside Claude Code
- Optional Antigravity quota items captured through its official statusline integration
- A live countdown until each limit resets
- A small native widget that lives directly in the Windows taskbar
- System tray icon badges showing your enabled model usage percentage
- Left-click the tray icon to toggle the taskbar widget on or off
- Right-click options for refresh, displayed models, update frequency, language, startup, widget visibility, and updates
- Multi-monitor taskbar placement, so the widget can live on the taskbar for the screen you prefer

## Who This Is For

This app is for Windows users who already have **Claude Code (CLI or App) installed and signed in**.

Codex support is optional. To show Codex usage, install and sign in to the Codex CLI, then enable Codex from the right-click **Models** menu.

Antigravity support is optional too. Enable it from the **Models** menu and use **Help > Antigravity Setup...** for the statusline bridge command.

It works best if you want a simple "how close am I to the limit?" display that is always visible.

## Requirements

- Windows 10 or Windows 11
- Claude Code (CLI or App) installed and authenticated
- Optional: Codex CLI installed and authenticated, if you want Codex usage
- Optional: Google Antigravity installed and authenticated, if you want Antigravity usage

If you use Claude Code through WSL, that is supported too. The monitor can read your Claude Code credentials from Windows or from your WSL environment.

## Install

Install the latest version from WinGet:

```powershell
winget install CodeZeno.ClaudeCodeUsageMonitor
```

If you prefer not to use WinGet, you can still download the latest `claude-code-usage-monitor.exe` from the [Releases](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/releases) page and run it directly.

## Use

After installing with WinGet, run:

```powershell
claude-code-usage-monitor
```

Once running, it will appear in your taskbar and as one or more tray icons in the notification area.

- Drag the left divider to move the taskbar widget
- On multi-monitor setups, drag the widget onto another Windows taskbar to move it to that screen
- Right-click the taskbar widget or tray icon for refresh, displayed models, update frequency, Start with Windows, reset position, language, updates, and exit
- Left-click the tray icon to toggle the taskbar widget on or off
- Enable `Start with Windows` from the right-click menu if you want it to launch automatically when you sign in

### Models

Use the right-click **Models** menu to choose what the widget displays:

- **Claude** is enabled by default
- **Codex** can be enabled alongside Claude or shown by itself
- **Antigravity** can be enabled alongside the other providers or shown by itself as its own model column

When multiple models are shown, each model has its own usage bar and matching usage text color.

The widget keeps its labels short. Here's what each one actually represents:

- **Claude** is the shared Claude / Claude Code usage window, not a Claude Code-specific metric
- **Codex** displays native rate-limit data through OpenAI's official `codex app-server`; the monitor does not read Codex credential files or call a ChatGPT/Codex backend endpoint directly
- When available, the Codex heading also shows the number of banked **Full resets** reported by the Codex CLI; an unavailable count is shown separately from zero
- **Antigravity** reads only a sanitized local cache populated from Antigravity's official `/statusline <command>` payload; the monitor does not call Google/Antigravity endpoints or read its OAuth credentials
- Antigravity quota items are carried through by quota key. A weekly-only payload is normal; the app does not fabricate a 5-hour value when none was observed
- `gemini-weekly` is treated as a Gemini-family weekly quota. Other keys such as `3p-weekly` are not renamed until their meaning is authoritatively documented
- These columns are a common display for whatever usage data is available per provider, not a separate UI design per provider

*Last verified: 2026-09-16*

### System Tray Icon

The tray icon shows your current 5-hour usage as a percentage badge.

If multiple providers are enabled, the app shows one tray icon per provider. If only one model is enabled, it shows one tray icon.

The Claude tray icon uses the same warm usage colors as the Claude bar. The Codex tray icon uses a black and white badge style. The Antigravity tray icon uses a blue badge style, and GitHub Copilot uses purple.

Hovering over a tray icon shows the usage values for that model.

## Diagnostics

If you need to troubleshoot startup or visibility issues, run:

```powershell
claude-code-usage-monitor --diagnose
```

This writes a log file to:

```text
%TEMP%\claude-code-usage-monitor.log
```

Settings are saved to:

```text
%APPDATA%\ClaudeCodeUsageMonitor\settings.json
```

## Account Support

This app works with the same account types that Claude Code itself supports.

As of **March 19, 2026**, Anthropic's Claude Code setup documentation says:

- **Supported:** Pro, Max, Teams, Enterprise, and Console accounts
- **Not supported:** the free Claude.ai plan

If Anthropic changes Claude Code availability in the future, this app should follow whatever Claude Code supports, as long as the usage data remains exposed through the same authenticated endpoints.

## Privacy And Security

This project is **open source**, so you can inspect exactly what it does.

What the app reads:

- Your local Claude Code OAuth credentials from `~/.claude/.credentials.json`
- If needed, the same credentials file inside an installed WSL distro
- If Codex is enabled, native rate-limit data from the local official `codex app-server`; authentication remains the Codex CLI/app-server's responsibility
- If Antigravity is enabled, a sanitized local cache file this app itself wrote from the Antigravity CLI's official `/statusline` feature — never an OAuth token or Windows Credential Manager entry

What the app sends over the network:

- Requests to Anthropic's Claude endpoints to read your usage and rate-limit information
- No direct HTTP request for Codex usage — the monitor talks locally to the official `codex app-server`, which handles the Codex CLI session and its own network activity
- Nothing to Google or Antigravity — if Antigravity is enabled, this app only reads its own local cache file; see `docs/quota-rules.md` for how that cache is populated
- Requests to GitHub only if you use the app's update check / self-update feature
- If proxy environment variables such as `HTTPS_PROXY`, `HTTP_PROXY`, or `ALL_PROXY` are set, those outbound requests may use that proxy

What the app stores locally:

- Widget position
- Selected taskbar / screen
- Widget visibility
- Polling frequency
- Language preference
- Last update check time
- Displayed model preferences
- A local, app-specific machine ID (a random identifier generated on this PC, not a hardware or OS ID)
- Usage snapshots for Claude Code, Codex, and Antigravity.

Usage snapshots and the machine ID are stored under:

```text
%LOCALAPPDATA%\ClaudeCodeUsageMonitor
```

- `machine_id.txt` — the app-specific machine ID
- `usage\<machine-id>\current.json` — the latest successfully saved snapshot
- `usage\<machine-id>\history.jsonl` — an append-only history of successfully saved snapshot entries, with no automatic size limit, rotation, or cleanup

When a snapshot is created successfully, the app attempts to update `current.json` and append to `history.jsonl` independently.

Each snapshot records session and weekly usage percentages, reset times, and provider statuses such as `success`, `error`, `stale`, or `disabled`. It does not include credentials, tokens, or raw provider responses.

What it does **not** do:

- It does not send your credentials to any other server
- It does not use a separate backend service
- It does not collect analytics or telemetry
- It does not upload your project files
- It does not read or edit your Codex credentials file
- It does not upload usage snapshots or the machine ID to any server

Notes:

- If your Claude Code token is expired, the app may ask the local Claude CLI to refresh it in the background
- If Codex usage is unavailable because the CLI/app-server session is not authenticated, sign in with the Codex CLI; the monitor does not manage Codex credentials itself
- If your Antigravity token is expired, open Antigravity and sign in again. The monitor does not write Windows Credential Manager entries itself.
- Portable installs can update themselves by downloading the latest release from this repository
- Proxies should be trusted because proxied usage requests include your OAuth bearer token inside the TLS connection

## How It Works

The monitor:

1. Uses each enabled provider's supported local integration or authenticated source
2. Reads current usage from Anthropic, the official Codex app-server, and/or the Antigravity statusline-derived cache
3. Shows the result directly in the Windows taskbar
4. Keeps the widget aligned with the selected taskbar and tray area
5. Refreshes periodically in the background

If the newer usage endpoint is unavailable, it can fall back to reading the rate-limit headers returned by Claude's Messages API.

## Open Source

This project is licensed under MIT.

If you want to inspect the behavior or audit the code, everything is in this repository.
