# Claude Code Usage Monitor

A lightweight Windows taskbar widget that displays your Claude API rate limit usage in real time.

![Windows](https://img.shields.io/badge/platform-Windows-blue)
![Rust](https://img.shields.io/badge/language-Rust-orange)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

![Screenshot](.github/screenshot.png)

## What it does

Embeds directly into the Windows taskbar and shows two progress bars:

- **5h** — Session usage (5-hour rolling window)
- **7d** — Weekly usage (7-day rolling window)

Each bar shows the current utilization percentage and a countdown until the rate limit resets.

## How it works

1. Reads your Claude OAuth token from `~/.claude/.credentials.json` (automatically refreshes expired tokens via the Claude CLI)
2. Queries the dedicated Anthropic OAuth usage endpoint (`/api/oauth/usage`) for utilization data
3. Falls back to the Messages API with rate limit header parsing (`anthropic-ratelimit-unified-*`) if the usage endpoint is unavailable
4. Renders the widget using Win32 GDI, embedded as a child window of the taskbar
5. Polls every 15 minutes by default (adjustable via context menu) and updates countdown timers between polls

The widget automatically detects dark/light mode from Windows system settings. You can drag the left divider to reposition the widget along the taskbar. Settings (position and poll frequency) are persisted to `%APPDATA%\ClaudeCodeUsageMonitor\settings.json`.

## Requirements

- Windows 10/11
- [Rust toolchain](https://rustup.rs/) (MSVC target)
- An active Claude Pro/Team subscription with OAuth credentials stored by [Claude Code](https://docs.anthropic.com/en/docs/claude-code)

## Building

```bash
cargo build --release
```

The binary will be at `target/release/claude-code-usage-monitor.exe`.

## Usage

Run the executable — the widget appears in your taskbar.

- **Drag** the left divider to reposition the widget along the taskbar
- **Right-click** for a context menu with **Refresh**, **Update Frequency**, **Settings** (Start with Windows, Reset Position), and **Exit**

## Project structure

```
src/
├── main.rs            # Entry point
├── models.rs          # UsageData / UsageSection types
├── poller.rs          # API polling, header parsing, formatting
├── window.rs          # Win32 window, rendering, message loop
├── native_interop.rs  # Win32 helper functions (taskbar, colors, etc.)
└── theme.rs           # Dark/light mode detection via registry
```

## Releases

Pre-built Windows executables are available on the [Releases](../../releases) page. Download `claude-code-usage-monitor.exe` and run it directly — no Rust toolchain required.

New releases are published automatically when a version tag is pushed:

```bash
git tag v1.0.0
git push origin v1.0.0
```
