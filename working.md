# Working State — Taskbar Side + Monitor Selection

**Session date:** 2026-07-28
**Repo:** `F:\Projects\Coding Projects\Claude-Code-Usage-Monitor`
**Branch:** `main` — HEAD is `7b108da v1.4.9`
**Status:** Two features implemented, compile-verified, **not committed**. A test exe was built for the user, who is currently testing it.

This file is a handoff for another agent. It describes what was asked, what was changed, how it was verified, the environment landmines on this machine, and what is left to do.

---

## 1. What the user asked for

Three requests, in order:

1. **Taskbar side option.** The widget only ever anchored to the right side of the taskbar (next to the tray), which on this user's machine overlapped their other app icons. They wanted an option to pick left or right so their widgets stack on the left. They run it with Claude Code and Codex usage both enabled.
2. **Monitor selection.** Multi-monitor setup, widget is on monitor 1. They wanted to select which monitor's taskbar hosts the widget (previously only possible by dragging the widget onto another taskbar).
3. **Build an exe** so they can test on Windows. Done — see §5.

A fourth request is **pending** (see §7): after testing, add per-provider "sign in" menu items to the widget/tray menus.

---

## 2. Architecture context (what you need to know before editing)

The app is a Rust Win32 program (no framework). Key files:

| File | Role |
|---|---|
| `src/window.rs` (~3300 lines) | Window creation, `AppState`, settings load/save, positioning, drag handling, `wnd_proc`, context menu |
| `src/native_interop.rs` | Thin Win32 wrappers — taskbar enumeration, window embedding, monitor lookup |
| `src/tray_icon.rs` | Notification-area icons. Owns `IDM_TOGGLE_WIDGET: u16 = 70` |
| `src/poller.rs` (~1735 lines) | Credential reading + usage polling for all three providers |
| `src/localization/` | `mod.rs` defines `struct Strings`; 11 per-language files each with a `STRINGS` const |

**How the widget positions itself:** It is reparented as a **child window of the taskbar** (`Shell_TrayWnd` / `Shell_SecondaryTrayWnd`) via `native_interop::embed_in_taskbar`. There is a fallback "topmost popup" mode using screen coordinates when embedding fails — **every positioning change must handle both modes.** `position_at_taskbar()` in `window.rs` is the single source of truth for placement; the drag handler in `wnd_proc` (`WM_MOUSEMOVE`) duplicates the math for live feedback, so the two must stay in sync.

**Important gotcha:** adding a field to `Strings` in `localization/mod.rs` breaks the build until **all 11** language files are updated, since each `STRINGS` is a struct literal.

---

## 3. Feature 1 — Taskbar Side (left/right)

### Design

New enum in `window.rs:51`:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TaskbarSide { Left, #[default] Right }
```

`Right` is the default, so **existing installs behave exactly as before**. The existing `tray_offset: i32` field was reinterpreted rather than replaced: it is now the distance **from the anchored side**, growing away from that side. In `Right` mode it is measured leftward from the tray area (original behavior, unchanged); in `Left` mode it is measured rightward from the taskbar's left edge.

### Changes in `src/window.rs`

- `window.rs:51` — `TaskbarSide` enum (above).
- `window.rs:98` — `taskbar_side: TaskbarSide` added to `AppState`.
- `window.rs:132-133` — `IDM_SIDE_LEFT: u16 = 32`, `IDM_SIDE_RIGHT: u16 = 33`.
- `window.rs:324, 346, 406, 1352` — `taskbar_side` threaded through `SettingsFile` (with `#[serde(default)]`), its `Default` impl, `save_state_settings()`, and `AppState` construction in `run()`.
- `window.rs:609-618` — `offset_for_drop_point()` gained a `taskbar_side` parameter; in `Left` mode the offset is simply `desired_left`, skipping the tray-relative math.
- `window.rs:2096-2124` — `position_at_taskbar()` now also reads `taskbar_side` out of the state lock.
- `window.rs:2168, 2179` — the two `x` computations (embedded child vs. fallback popup) became `match taskbar_side` blocks. Left mode uses `tray_offset` (child coords) or `taskbar_rect.left + tray_offset` (screen coords).
- `window.rs:2420` — drag delta is **mirrored** per side: `Right` uses `drag_start_mouse_x - pt.x`, `Left` uses `pt.x - drag_start_mouse_x`. Without this the widget would move opposite to the cursor in left mode.
- `window.rs:2461` — live-drag `x` computation became a nested `match` over side × embedded.
- `window.rs:2522-2539` — `WM_LBUTTONUP` drag_result tuple carries `taskbar_side` into `offset_for_drop_point`, so cross-monitor drops respect the side.
- `window.rs:2633-2657` — `IDM_SIDE_LEFT | IDM_SIDE_RIGHT` handler. On an actual change it sets the side, **resets `tray_offset` to 0** (so the widget snaps flush to the newly chosen side), saves, repositions, re-renders.
- `window.rs:2817, 2833` — `show_context_menu` reads `taskbar_side` for the checkmark.
- `window.rs:2966-2998` — builds the **Settings → Taskbar Side** submenu with `MF_CHECKED` on the active side.

### Known minor behavior

In Left mode, offset 0 sits flush against the far-left edge of the taskbar, which on Windows 11 is where the Widgets/weather button lives. The user was told to drag the widget right once if that's in the way; the offset persists.

---

## 4. Feature 2 — Monitor selection

### Design

The infrastructure already existed: `AppState.taskbar_index` / `SettingsFile.taskbar_index` were already persisted and already used by the drag-to-another-taskbar feature, and `attach_to_taskbar(hwnd, index)` already handled re-parenting, re-hooking the tray WinEvent hook, and updating state. **This feature is just a menu in front of the existing code path** — that is why it is small.

Menu IDs are a dynamic range so the item count can vary with the number of taskbars:

```rust
const IDM_MONITOR_BASE: u16 = 100;
const IDM_MONITOR_MAX: u16 = 131;   // 32 taskbars max
```

Range chosen to sit clear of all existing IDs (highest previously was `IDM_TOGGLE_WIDGET = 70` in `tray_icon.rs`).

### Changes in `src/native_interop.rs`

- Imports extended with `POINT`, and `Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST}`. (`Win32_Graphics_Gdi` was already in `Cargo.toml` features — no manifest change needed.)
- `native_interop.rs:70` — new `pub fn monitor_number_for_rect(rect: RECT) -> Option<u32>`. Takes the center point of a taskbar rect → `MonitorFromPoint(MONITOR_DEFAULTTONEAREST)` → `GetMonitorInfoW` into a `MONITORINFOEXW` → parses the trailing digits of the `\\.\DISPLAYn` device name. This gives labels that match what Windows display settings shows in typical setups.

### Changes in `src/window.rs`

- `window.rs:2755-2769` — handler for `id if (IDM_MONITOR_BASE..=IDM_MONITOR_MAX).contains(&id)`. Computes `target_index = id - IDM_MONITOR_BASE`, no-ops if it equals the current index, otherwise calls `attach_to_taskbar` → `save_state_settings` → `position_at_taskbar` → `render_layered`.
  **Placement matters:** this arm is a guard pattern and must stay **before** the `id if id == tray_icon::IDM_TOGGLE_WIDGET` arm and after the literal-ID arms.
- `window.rs:2817, 2833` — `show_context_menu` also reads `taskbar_index`.
- `window.rs:3000-3033` — builds **Settings → Monitor** submenu. Calls `native_interop::find_taskbars()` fresh each time the menu opens (monitors can be hot-plugged). **The submenu is only appended when `taskbars.len() > 1`**, so single-monitor users see no change. Items are capped at `max_items` to stay inside the ID range. Label is `format!("{} {}", strings.monitor, number)`, falling back to `index + 1` if the display number can't be read.

### Note for the user (already communicated)

Windows only creates taskbars on secondary monitors when *"Show my taskbar on all displays"* is enabled in Windows taskbar settings. If it's off there is genuinely only one taskbar and the Monitor submenu correctly stays hidden.

---

## 5. Localization

`localization/mod.rs` `struct Strings` gained **four** fields:

```rust
pub taskbar_side: &'static str,
pub taskbar_side_left: &'static str,
pub taskbar_side_right: &'static str,
pub monitor: &'static str,
```

All 11 language files were updated (en, nl, es, fr, de, ja, ko, zh-TW, zh-CN, ru, pt-BR). Sample values — English: `"Taskbar Side" / "Left" / "Right" / "Monitor"`; French uses `"Écran"` for monitor; Japanese `"モニター"`; Simplified Chinese `"显示器"`; Traditional Chinese `"顯示器"`; Korean `"모니터"`; Russian `"Монитор"`. Dutch/German/Spanish/Portuguese keep `"Monitor"`.

---

## 6. Verification — and the two environment landmines

### ⚠️ Landmine 1: Smart App Control blocks all native builds

**`cargo build` cannot run natively on this machine.** Smart App Control (SAC) is **enabled** (confirmed: `HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy` → `VerifiedAndReputablePolicyState = 1`). It blocks execution of freshly compiled unsigned binaries, including **cargo's own build-script executables**. A native `cargo check` fails with:

```
could not execute process .../build-script-build (never executed)
Caused by: An Application Control policy has blocked this file. (os error 4551)
```

Disabling SAC is **irreversible** (cannot be re-enabled without resetting Windows), so it was **left enabled** — that is the user's call, not an agent's. Do not disable it without explicit instruction.

Installed during this session anyway (harmless, may be useful if SAC is ever turned off): `Rustlang.Rustup` (rustc 1.97.1, msvc host) and `Microsoft.VisualStudio.2022.BuildTools` with the VC++ workload. Cargo lives at `C:\Users\kuu\.cargo\bin\cargo.exe` and is **not on PATH** — invoke by full path.

### The working build path: cross-compile from WSL

WSL Ubuntu is used as the build host, targeting `x86_64-pc-windows-gnu`. Set up during this session (persisted in the distro):

- `apt-get install gcc mingw-w64 libc6-dev` (as root)
- rustup installed for the WSL **root** user, plus target `x86_64-pc-windows-gnu`
- `ln -sf /usr/bin/x86_64-w64-mingw32-windres /usr/local/bin/windres` — the `winres` build-dep invokes the unprefixed name
- Capitalized→lowercase import-lib symlinks in `/usr/x86_64-w64-mingw32/lib` (e.g. `libAdvapi32.a → libadvapi32.a`). The `windows` crate emits `-lAdvapi32` etc., which fails on a case-sensitive filesystem. Script kept at:
  `C:\Users\kuu\AppData\Local\Temp\claude\F--Projects-Coding-Projects-Claude-Code-Usage-Monitor\fb1a92aa-896b-4516-9ca9-24859f4d1a06\scratchpad\fix-libs.sh`

**Build recipe** (copies the tree into the WSL filesystem — building directly on `/mnt/f` is slow):

```bash
wsl.exe -d Ubuntu -e bash -lc "rm -rf ~/ccum && mkdir -p ~/ccum && cd '/mnt/f/Projects/Coding Projects/Claude-Code-Usage-Monitor' && cp -r src build.rs Cargo.toml Cargo.lock ~/ccum/ && source ~/.cargo/env && cd ~/ccum && cargo build --release --target x86_64-pc-windows-gnu"
```

### ⚠️ Landmine 2: GNU ld drops the resource object

With the GNU toolchain the icon/version resources compiled by `winres` land in `libresource.a` but get **garbage-collected out of the final binary** — the exe ends up with no `.rsrc` section (no icon, no version metadata). Fix is to force-link the object file directly:

```bash
RUSTFLAGS='-C link-arg=/root/ccum/target/x86_64-pc-windows-gnu/release/build/claude-code-usage-monitor-<HASH>/out/resource.o' \
  cargo build --release --target x86_64-pc-windows-gnu
```

The `<HASH>` directory varies per build — locate it with `find ~/ccum/target -name resource.o`. Verify success with `objdump -h <exe> | grep -i rsrc` (expect a `.rsrc` section) and on the Windows side by reading `VersionInfo`.

**None of this affects official releases** — CI (`.github/workflows/release.yml`) builds with MSVC on GitHub runners, where `build.rs`/`winres` work normally. These workarounds exist solely to produce a local test binary on a SAC-locked machine.

### Verification actually performed

- `cargo check --target x86_64-pc-windows-gnu` on the full tree after **both** features: **compiles clean, zero errors, zero warnings.** (For type-checking only, `build.rs` was stubbed to `fn main() {}` in the throwaway copy; the real `build.rs` is untouched in the repo and was used for the actual exe build.)
- Release exe built, `.rsrc` section confirmed present, `FileVersion 1.4.9 / Product "Claude Code Usage Monitor" / Company "Code Zeno Pty Ltd"` read back on Windows.
- **No runtime testing has been done by an agent.** SAC very likely prevents the unsigned test exe from launching at all. Runtime behavior of both features is **unverified** and rests with the user's testing.

### Test exe location

```
F:\Projects\Coding Projects\Claude-Code-Usage-Monitor\target\claude-code-usage-monitor-test.exe
```

`target/` is gitignored, so it will not be committed. The user was told to exit the running instance (tray → Exit) before launching it, since two instances fight over the taskbar.

---

## 7. Outstanding work

### a) Commit the changes — **not yet done, awaiting user's word**

15 modified files, no commits made. The user was asked whether to use one commit or one per feature and has **not answered**. Do not commit without confirmation. Note `working.md` (this file) is a new untracked file and is presumably a scratch/handoff artifact — confirm whether the user wants it tracked.

### b) Pending feature request: in-menu auth shortcuts

The user enabled all three providers and could not find any sign-in option. Explained: **the app never authenticates**; it only reads credentials that each tool stores itself. Diagnostic run this session found **all three credentials already present**:

- Claude: `C:\Users\kuu\.claude\.credentials.json` ✔ (`claude` CLI on PATH ✔)
- Codex: `C:\Users\kuu\.codex\auth.json` ✔ — but **the `codex` CLI is NOT on PATH** ✖. This matters beyond login: `poller.rs:404 cli_refresh_codex_token()` shells out to `codex` to refresh an expired token, so auto-refresh is currently broken. Fix is `npm install -g @openai/codex`.
- Antigravity: Credential Manager target `gemini:antigravity` ✔ (sign-in happens in the Antigravity IDE; no CLI)

Manual commands given to the user: `claude` then `/login`; `codex login`; for Antigravity, sign in inside the IDE. Then **Refresh** from the right-click menu.

**The agreed next step**, to be done after the user finishes testing the current build: add per-provider "Sign in…" items to the widget right-click menu and the tray menus — launching a terminal running `claude` / `codex login`, and launching the Antigravity IDE for the third. Suggested implementation notes:
- Reuse the credential-path/CLI-resolution helpers already in `poller.rs` (`resolve_windows_codex_path`, `codex_auth_path`, `windows_credential_source`) rather than re-deriving paths.
- Menu IDs: pick a fresh block clear of `IDM_MONITOR_BASE..=IDM_MONITOR_MAX` (100–131) — e.g. 140+.
- Four new `Strings` fields will be needed (a submenu label plus three provider items) → **all 11 language files must be updated**.
- Codex's CLI may be absent; the menu item should degrade gracefully (surface a helpful message rather than silently failing).

### c) Optional cleanup

VS Build Tools (several GB) is unusable while SAC is on; the user may want to uninstall it. Rustup is small. The user's decision.

---

## 8. Quick reference

**Settings file:** `%APPDATA%\ClaudeCodeUsageMonitor\settings.json` — new key `taskbar_side` (`"left"` / `"right"`, defaults to `"right"`); `taskbar_index` (pre-existing) now also settable from the Monitor menu.

**Menu ID map after this session:**

| ID(s) | Meaning |
|---|---|
| 1, 2 | Refresh, Exit |
| 10–13 | Poll frequency |
| 20 | Start with Windows |
| 30, 31 | Reset Position, Version action |
| **32, 33** | **Taskbar Side left / right (new)** |
| 40–51 | Language |
| 60–62 | Models |
| 70 | Toggle widget (in `tray_icon.rs`) |
| **100–131** | **Monitor selection, dynamic (new)** |

**Diagnostics:** `claude-code-usage-monitor --diagnose` → log at `%TEMP%\claude-code-usage-monitor.log`. `position_at_taskbar()` logs the computed x/y/w/h every time, which is the fastest way to debug placement problems.
