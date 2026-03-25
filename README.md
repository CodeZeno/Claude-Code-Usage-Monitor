# Claude Code Usage Monitor

A lightweight Windows taskbar widget for people already using Claude Code.

It sits in your taskbar and shows how much of your Claude Code usage window you have left, without needing to open the terminal or the Claude site.

![Windows](https://img.shields.io/badge/platform-Windows-blue)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

![Screenshot](.github/screenshot.png)

## What You Get

- A **5h** bar for your current 5-hour Claude usage window
- A **7d** bar for your current 7-day window
- A live countdown until each limit resets
- A small native widget that lives directly in the Windows taskbar
- Right-click options for refresh, update frequency, language, startup, and updates

## Who This Is For

This app is for Windows users who already have **Claude Code (CLI or App) installed and signed in**.

It works best if you want a simple "how close am I to the limit?" display that is always visible.

## Requirements

- Windows 10 or Windows 11
- Claude Code (CLI or App) installed and authenticated

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

Once running, it will appear in your taskbar.

- Drag the left divider to move it
- Right-click for refresh, update frequency, Start with Windows, reset position, language, updates, and exit
- Enable `Start with Windows` from the right-click menu if you want it to launch automatically when you sign in

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

What the app sends over the network:

- Requests to Anthropic's Claude endpoints to read your usage and rate-limit information
- Requests to GitHub only if you use the app's update check / self-update feature

What the app stores locally:

- Widget position
- Polling frequency
- Language preference
- Last update check time

What it does **not** do:

- It does not send your credentials to any other server
- It does not use a separate backend service
- It does not collect analytics or telemetry
- It does not upload your project files

Notes:

- If your Claude Code token is expired, the app may ask the local Claude CLI to refresh it in the background
- Portable installs can update themselves by downloading the latest release from this repository

## How It Works

The monitor:

1. Finds your Claude Code login credentials
2. Reads your current usage from Anthropic
3. Shows the result directly in the Windows taskbar
4. Refreshes periodically in the background

If the newer usage endpoint is unavailable, it can fall back to reading the rate-limit headers returned by Claude's Messages API.

## Open Source

This project is licensed under MIT.

If you want to inspect the behavior or audit the code, everything is in this repository.
