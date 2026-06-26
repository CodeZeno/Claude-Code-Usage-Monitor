# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A lightweight **Windows-only** native taskbar widget (Rust) that displays Claude Code, Codex, and Google Antigravity usage/quota directly in the Windows taskbar, plus system tray icon badges. No backend service; it reads local OAuth credentials and polls provider endpoints directly.

The project targets Windows exclusively via the `windows` crate (Win32 APIs). It will not build or run on Linux/macOS. The development environment here is WSL, so builds and tests must target Windows (e.g. `cargo build --target x86_64-pc-windows-msvc` or build on a Windows host).

## Commands

```bash
cargo build              # debug build (Windows target)
cargo build --release    # optimized release (opt-level=z, LTO, stripped, panic=abort)
cargo test               # run tests
cargo test <name>        # run a single test by name
cargo clippy             # lint
```

Runtime entry points (the produced exe):
- `claude-code-usage-monitor` — normal run, shows widget + tray icons
- `claude-code-usage-monitor --diagnose` — writes a startup/visibility log to `%TEMP%\claude-code-usage-monitor.log`
- `updater::handle_cli_mode` intercepts update-related CLI args before the window launches (see `main.rs`)

## Architecture

`main.rs` wires modules and dispatches: diagnose mode → updater CLI mode → `window::run()` (the main message loop). Modules:

- **`window.rs`** (largest, ~3300 lines) — the heart. Owns the Win32 window (a layered/child window embedded into the taskbar), the `wnd_proc` message handler, all painting, drag-to-move (restricted to the divider handle), DPI/display-change handling, the right-click context menu, settings persistence (`SettingsFile`), and the timers (`TIMER_POLL`, `TIMER_COUNTDOWN`, `TIMER_RESET_POLL`, `TIMER_UPDATE_CHECK`). Background work posts custom messages (`WM_APP_USAGE_UPDATED`, `WM_APP_UPDATE_CHECK_COMPLETE`, `WM_APP_TRAY`) back to the window thread.
- **`native_interop.rs`** — Win32 helpers and shared constants: enumerating taskbars (`find_taskbars`, multi-monitor), locating taskbar child windows, taskbar/window rects, window-style and timer/message constants. Multi-monitor placement is anchored by `taskbar_index` (settings) plus `tray_offset`.
- **`poller.rs`** (~1700 lines) — all network + credential logic. Reads credentials (Windows `~/.claude/.credentials.json`, WSL distro fallback, Codex `auth.json`, Antigravity token in Windows Credential Manager `gemini:antigravity`), polls Anthropic / ChatGPT Codex / Google Antigravity endpoints, refreshes expired tokens by shelling out to the provider CLI (`refresh_or_fallback`, `cli_refresh_*`), and falls back to rate-limit headers. `poll()` is the entry; `credential_watch_*` detects credential changes.
- **`models.rs`** — shared data types: `UsageSection`, `UsageData`, `AppUsageData` (per-provider results consumed by window/tray rendering).
- **`tray_icon.rs`** — system tray (notification area) icons, one per enabled provider, with percentage badges and hover tooltips; left-click toggles widget.
- **`theme.rs`** — colors and the per-provider usage color schemes (Claude warm, Codex black/white, Antigravity Google-blue).
- **`updater.rs`** — update check + self-update (WinGet path and portable self-download from GitHub releases).
- **`localization/`** — 10 languages. `mod.rs` defines `LanguageId`, the `Strings` struct, system-language detection, and language resolution; each `*.rs` returns that language's `Strings`. Add a language by adding a module, extending `LanguageId::ALL`, and the match arms in `mod.rs`.
- **`diagnose.rs`** — opt-in file logging used across modules when `--diagnose` is passed.
- **`build.rs`** — embeds the icon and PE version metadata (from `CARGO_PKG_VERSION`) via `winres`.

## Persistence

- Settings: `%APPDATA%\ClaudeCodeUsageMonitor\settings.json` — `SettingsFile` in `window.rs` (tray_offset, taskbar_index, poll_interval_ms, language, last_update_check_unix, widget_visible, show_claude_code, show_codex, show_antigravity). Fields use serde defaults so old files stay forward-compatible — preserve that when adding fields.
- Startup-with-Windows: registry Run key `ClaudeCodeUsageMonitor`.
- Single-instance: global mutex `Global\ClaudeCodeUsageMonitor`.

## Conventions

- Version lives only in `Cargo.toml`; `build.rs` packs it into the exe and `updater.rs` compares against it. Release commits are tagged like `v1.4.8`.
- The widget embeds into the real Windows taskbar window; there's a non-embedded fallback paint path (`WM_PAINT`) for when embedding fails. Multi-monitor moves happen by dragging the widget onto another taskbar, which updates `taskbar_index`.
