# Feature map

The names below are the ones to use in issues, pull requests, commit messages and
agent prompts, so everyone means the same part of the app. Each entry lists where
the code lives. The [user guide](../USER_GUIDE.md) describes the behavior.

## Widget

The always-visible usage display drawn by the active theme. One process owns
one widget window.

- **Theme surface `main`**: the widget itself. A theme's other surfaces are
  flyouts or tray icons (below).
- **Host**: where the widget is placed. Theme Studio calls the choices Automatic,
  Taskbar, Tray Icon, Desktop and Floating (`SurfaceNest` in the theme format).
  Placement, taskbar fitting and window lifetime are in `src/window/` (`positioning.rs`,
  `host_geometry.rs`, `taskbar_collision.rs`, `lifecycle.rs`), and each surface's
  window is created in `surfaces.rs`; desktop hosting is `src/desktop_compositor.rs`.
- **Widget process**: `src/window.rs` holds the app state and start-up, and
  `src/window/` the rest: the message loop (`message_loop.rs`), polling providers
  (`polling.rs`), update checks (`updates.rs`), Start with Windows (`startup.rs`)
  and the watchdogs that relaunch a hung widget or re-register after an
  explorer.exe restart (`watchdog.rs`).
- **Widget tooltip**: the hover summary, shown when a surface sets
  `"tooltip": "usage_summary"` (`src/window/widget_tooltip.rs`).
- **Widget actions**: what clicks, double-clicks, right-clicks and hovering do, as
  set by the theme (show or toggle the dashboard, open a URL, show the context
  menu, change a theme variable), in `src/window/mouse.rs`. Left-drag moves the
  widget (`src/window/message_loop.rs`).
- **Flyout**: an extra surface a theme opens on hover, such as Stacked Rows'
  `claude-popup` and `codex-popup`.

## Tray icon

The notification-area icon (`src/tray_icon.rs`, `src/tray_icon/`). Left-click
shows or hides the widget; right-click opens the context menu. The app icon is the
fallback; a theme can draw per-provider tray icons from `<provider>-tray-icon`
surfaces (`provider_artwork.rs`). Its usage tooltip and icon updates are in
`src/window/tray_summary.rs`.

## Context menu

The right-click menu on the widget and tray icon. Menus are documents
(`src/context_menu.rs`) shown natively by `src/window/window_context_menu.rs`. The
built-in menus are:

- **Classic v1**: Refresh, Update frequency, Providers, Settings (Start with
  Windows, Lock in taskbar, Language, Check for updates), Show widget, Open
  Dashboard, Exit.
- **Dashboard v2**: Open Dashboard, Exit.

## Dashboard

The settings and editing window, a separate process started with
`--studio --owner <hwnd>` (`src/dashboard.rs`, `src/studio_app.rs`). Its pages,
in navigation order (`Page` in `src/studio_app.rs`):

| Page | Contents | Code |
| --- | --- | --- |
| Settings | General, Providers, Display and Appearance sections | `studio_app/studio_settings.rs` |
| Theme Studio | The theme editor (below) | `studio_app/studio_theme_workspace.rs` |
| Context Menus | Menu documents and their editor | `studio_app/studio_context_menus.rs`, `studio_context_menu_editor.rs`, `studio_native_menu.rs` |
| Assets | Images available to themes | `studio_app/studio_assets.rs` |
| Diagnostics | Diagnostic logging and the live event log | `studio_app/studio_diagnostics.rs` |

`--dashboard-warp` opens an alternative WARP-rendered dashboard
(`studio_app/warp.rs`); see [dashboard rendering](dashboard-rendering.md).

### Theme Studio

- **Toolbar**: theme picker, New, Save, Duplicate, Import, Export, Delete, Live
  apply, undo and redo.
- **Layers panel**: the surface and layer tree (`studio_scene_helpers.rs`).
- **Preview**: the rendered surface (`studio_theme_workspace.rs`).
- **Inspector**: properties of the selected surface or layer (`studio_inspectors.rs`).
- **Asset library**: images the theme can use.
- **Helper panel**: contextual help (`studio_helper.rs`, `studio_helper_panels.rs`).

## Themes

A theme is a JSON document of surfaces and layers. The engine is `src/theme_engine.rs`
(the document format) and `src/theme_engine/`: expressions (`theme_expression.rs`),
the values they read (`theme_data_context.rs`), mouse actions
(`theme_mouse_actions.rs`), validation (`theme_validation.rs`), rendering
(`theme_rendering.rs`) and saved themes (`theme_storage.rs`). Built-in themes are
in `src/themes/`: Classic v1, Classic Vertical, Compact Fluent Quad, Minecraft
(Codex) and Stacked Rows. Most fields are expressions (`"width": "canvas.width - 8"`) and text uses `{expression:format}`
templates; [theme-reference.md](theme-reference.md) lists every field, value,
function, format and mouse action, generated from `src/theme_engine/theme_language.rs`.
Render any theme to PNG without starting the app with `--render-theme`; see
[CONTRIBUTING.md](../CONTRIBUTING.md#theme-changes).

## Providers

A provider is a usage source: Claude Code, Codex, Antigravity, OpenCode, Cursor,
Grok and Copilot. Each has a descriptor in `src/providers.rs` (its key, as used
in theme expressions such as `codex.available`, its usage-cache key, setting
text and menu command id) and a poller in `src/poller/`. See
[CONTRIBUTING.md](../CONTRIBUTING.md#provider-changes) for what a new provider
needs.

## Other windows and modes

- **Message boxes**: update prompts, update failures, provider sign-in errors and
  dashboard start-up failures.
- **Updater**: `src/updater.rs`, started from `src/window/updates.rs`;
  `--apply-update` is the internal helper mode.
  See [updater](updater.md).
- **Command-line flags**:
  - `--dashboard` opens the dashboard on start.
  - `--studio --theme-studio` opens the dashboard on Theme Studio.
  - `--allow-multiple` skips the single-instance check.
  - `--no-poll` starts without polling providers.
  - `--diagnose` and `--diagnose-append` write a diagnostic log.
  - `--render-theme` renders a theme to PNG and exits.
  - `--theme-reference` prints [theme-reference.md](theme-reference.md) and exits.
