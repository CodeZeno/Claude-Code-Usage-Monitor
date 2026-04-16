# Development Guide

This document covers how to build and run **Claude Code Usage Monitor** locally.

## Prerequisites

- **Windows 10 or Windows 11** (the app uses Win32 APIs and can only be built and run on Windows)
- **Rust** (stable toolchain) — install via [rustup](https://rustup.rs/)
- **MSVC build tools** — required by the `windows` crate and `winres`
  - Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) and select the **Desktop development with C++** workload, or install a full Visual Studio edition

## Clone the Repository

```powershell
git clone https://github.com/CodeZeno/Claude-Code-Usage-Monitor.git
cd Claude-Code-Usage-Monitor
```

## Build

### Debug build

```powershell
`cargo build`
```

The binary is output to `target\debug\claude-code-usage-monitor.exe`.

### Release build

```powershell
cargo build --release
```

The binary is output to `target\release\claude-code-usage-monitor.exe`. The release profile applies aggressive size optimisations (`opt-level = "z"`, LTO, stripping, single codegen unit).

> **Note:** `build.rs` embeds the app icon and Windows PE version metadata during compilation using `winres`. This requires the MSVC toolchain — it will not work with the GNU (`*-pc-windows-gnu`) target.

## Run`

```powershell
cargo run
```

Or run the compiled binary directly:

```powershell
.\target\debug\claude-code-usage-monitor.exe
```

The app starts as a system tray icon. Left-click the tray icon to toggle the taskbar widget. Right-click for options such as refresh interval, language, launch-at-startup, and update checks.

## Project Structure

```
src/
  main.rs            # Entry point, message loop
  models.rs          # Data types and Claude API response structs
  poller.rs          # Background polling logic
  window.rs          # Win32 taskbar widget window
  tray_icon.rs       # System tray icon and context menu
  theme.rs           # Colour theming (light/dark mode)
  updater.rs         # Auto-update checker
  diagnose.rs        # Diagnostics / debug helpers
  native_interop.rs  # Low-level Win32 helpers
  icons/             # Embedded icon assets
  localization/      # Translated UI strings (en, fr, de, es, ja, ko)
build.rs             # Embeds icon and version info into the PE binary
Cargo.toml           # Manifest and dependencies
```

## Dependencies

| Crate | Purpose |
|-------|---------|
| `windows` | Win32 API bindings |
| `ureq` | HTTP client for Claude usage API (native-tls) |
| `serde` / `serde_json` | JSON serialisation |
| `dirs` | Locate user credential files |
| `winres` *(build)* | Embed icon and version info into the `.exe` |

## Useful Commands

```powershell
# Check for compile errors without producing a binary
cargo check

# Run Clippy lints
cargo clippy

# Format code
cargo fmt

# Run tests (if any)
cargo test
```
