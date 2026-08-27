![Windows](https://img.shields.io/badge/platform-Windows-blue)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# Claude Code Usage Monitor

![Screenshot](.github/animation.gif)

A lightweight Windows taskbar widget for people already using Claude Code, with optional Codex, Google Antigravity, OpenCode Go, and Cursor usage display.

It sits in your taskbar and shows how much of your Claude Code, Codex, Antigravity, OpenCode Go, and/or Cursor usage window you have left, without needing to open the terminal or the provider site.

## What You Get

- A **5h** bar for your current 5-hour Claude usage window
- A **7d** bar for your current 7-day window
- Optional Codex usage bars alongside Claude Code
- Optional Antigravity model usage bars for Google's 5-hour and weekly Gemini quota windows
- Optional OpenCode Go usage bars for its 5-hour and most-constrained weekly/monthly window
- Optional Cursor Auto and API plan usage bars
- A live countdown until each limit resets
- A small native widget that lives directly in the Windows taskbar
- A persistent application icon in the system tray
- In the default Classic theme, left-click or double-click a provider tray icon to show or hide the taskbar widget; right-click it for its context menu
- Dashboard controls for refresh, displayed providers, update frequency, language, startup, and updates
- Multi-monitor taskbar placement, so the widget can live on the taskbar for the screen you prefer
- A native settings dashboard and live theme studio
- Unified scene objects with solid-colour, gradient, or image backgrounds, borders, text, data bars, and children
- Combined monitor, taskbar, and system-tray targets for each display

## Who This Is For

This app is for Windows users who already have **Claude Code (CLI or App) installed and signed in**.

Codex support is optional. To show Codex usage, install and sign in to the Codex CLI, then enable Codex in the dashboard's **Providers** section.

Antigravity support is optional too. To show Antigravity usage, install and sign in to Google Antigravity, then enable it in the dashboard's **Providers** section.

OpenCode Go support is optional. Install OpenCode, connect an OpenCode Go account, and enable **OpenCode** in the dashboard's **Providers** section. The monitor reads the server-side 5-hour, weekly, and monthly usage windows from the OpenCode dashboard.

For server-side usage, set `OPENCODE_GO_WORKSPACE_ID` and `OPENCODE_GO_AUTH_COOKIE`, or create `%APPDATA%\opencode-go\config.json`:

```json
{
  "workspaceId": "wrk_01...",
  "authCookie": "your-opencode-auth-cookie"
}
```

The workspace ID comes from `https://opencode.ai/workspace/<workspace-id>/go`. The auth cookie comes from an authenticated `opencode.ai` browser session. `OPENCODE_GO_CONFIG_FILE` can point to a different config file. Compatible `opencode-bar` and `opencode-quota` config locations are also detected.

Cursor support is optional. Sign in to Cursor and enable **Cursor** in the dashboard's **Providers** section. The monitor reads `cursorAuth/accessToken` from Cursor's `%APPDATA%\Cursor\User\globalStorage\state.vscdb` using the SQLite library built into Windows 10 and 11, then requests the Cursor usage summary. No SQLite engine is bundled. `CURSOR_SESSION_TOKEN` can override the detected session when needed.

It works best if you want a simple "how close am I to the limit?" display that is always visible.

## Requirements

- Windows 10 or Windows 11
- Claude Code (CLI or App) installed and authenticated
- Optional: Codex CLI installed and authenticated, if you want Codex usage
- Optional: Google Antigravity installed and authenticated, if you want Antigravity usage
- Optional: OpenCode installed and connected to OpenCode Go, if you want OpenCode usage
- Optional: Cursor installed and authenticated, if you want Cursor usage

If you use Claude Code through WSL, that is supported too. The monitor can read your Claude Code credentials from Windows or from your WSL environment.

If you only ever run Claude Code inside the Claude desktop app, there is nothing else to install. The standalone CLI login writes `~/.claude/.credentials.json`, but the desktop app keeps its own encrypted token cache in `%APPDATA%\Claude\config.json`. The monitor reads that cache read-only, through the same Windows DPAPI and AES-GCM scheme the app itself uses, when no CLI credentials are available.

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

Once running, it will appear in your taskbar and as a persistent application icon in the notification area.

- In the default Classic theme, left-click or double-click a provider tray icon to show or hide the taskbar widget; right-click it for its context menu
- Use **Theme Studio** to position root objects against a monitor, taskbar, or system tray on any display
- Enable `Start with Windows` from **Settings** if you want it to launch automatically when you sign in
- Manage every setting or build a custom theme from the same dashboard

You can also open the dashboard directly:

```powershell
claude-code-usage-monitor --dashboard
```

### Custom themes

Theme Studio previews edits on its canvas and stores versioned JSON themes in:

```text
%APPDATA%\ClaudeCodeUsageMonitor\themes
```

Choose a theme from the Studio toolbar. The bundled Classic theme remains read-only so there is always a working design to return to. The bundled Minecraft theme is installed once as an editable starting point, keeps local changes, and stays removed if you delete it. Use **Duplicate** to name and create another editable copy. **Live apply** starts off; enable it when you want each edit saved and applied immediately. Unsaved edits are clearly marked, and closing the dashboard or replacing the current theme asks whether to save or discard them. Use **Undo**/**Redo** in the Studio toolbar (or `Ctrl+Z`/`Ctrl+Y`) to reverse theme edits, including position and size changes.

The Studio scene tree, canvas, and inspector always share the available window width. Drag either narrow divider beside the canvas to resize the adjacent panel. Every entry is the same scene-object type. Use **Add layer** to create a layer beside the selected one at the same hierarchy level, or at the top level when no selection is available, then drag it by the right-side grip in the scene tree to reorder it or make it a child. Dropping a child between root surfaces promotes it back to a surface. Clicking anywhere on a scene row selects it; the disclosure arrow and preview icon also expand or collapse it, while double-clicking its truncated name selects all of it for inline renaming. Each row previews the object's background, border, corner radius, and content type. The inspector groups fields into collapsible **Layer**, **Appearance**, and **Positioning** sections and uses the selected object's name as its title. Root objects become native desktop windows and therefore expose a combined reference target such as **Monitor (Display 1)** or **System Tray (Display 2)**; child objects expose a parent anchor instead. Hovering any object in the tree temporarily outlines its resolved preview bounds in magenta without adding persistent resize controls to the canvas.

The widget always uses a Theme Studio theme; the built-in **Classic v1** theme recreates the original `5h` and `7d` widget and is enabled by default. It adapts to the enabled providers with 10, 5, 4, 3, or 2 bar segments per provider. Themes are ordered object stacks. Create a surface, choose optional text or data-bar content, then independently configure its background as **None**, **Solid colour**, **Gradient**, or **Image**, along with its border, layout, and children. Gradients use Start and End colours; their angle runs clockwise, with `0` drawing left-to-right and `90` drawing top-to-bottom. Geometry, visibility, root size, progress values and segment counts, gradient angles, and paint opacity accept calculations with `+`, `-`, `*`, `/`, `%`, comparisons, `&&`, `||`, parentheses, and functions including `get`, `min`, `max`, `clamp`, `round`, `floor`, `ceil`, `abs`, `sqrt`, `pow`, `lerp`, and `if`. The code button converts a regular value to an expression and immediately opens the expression helper; use it again to reopen the helper. Hover an expression field to reveal its reset button.

Every object supports **Freeform**, **Row**, and **Column** child layouts, whether its content is empty, text, an image, a shape, or a data bar. Row and column objects position rendered direct children automatically in scene order with configurable gap and cross-axis alignment. A child's X/Y values are offsets from its anchor, or optional fine offsets when its parent manages layout. `Render` accepts a boolean expression and removes false objects from layout; `Visibility` accepts an expression from 0–100 and fades an object without collapsing its space. Children are always clipped to their parent's bounds.

Enable **Mouse events** on a layer to handle Click, Double click, Right click, Mouse enter, and Mouse leave. The action helper can insert `show_dashboard()`, `toggle_dashboard()`, `show_context_menu()`, `set(target, property, value)`, `increase(target, property, amount)`, `decrease(target, property, amount)`, `toggle(target, render)`, and `reset(target, property)`. Use `self` for the current layer or select another layer on any root surface; the helper stores its stable ID. Runtime actions can override Render, Visibility, X, Y, Width, Height, and Rotation without changing the saved expressions. Increase and decrease work with numeric properties and accumulate from the current effective value, while `reset` restores the saved expression. The context-menu action opens the same menu used by the tray icon at the current pointer position. When both Click and Double click are configured, Click waits for the Windows double-click interval and is suppressed by a double click. The canvas safely previews layer changes without opening dashboards or context menus. Tray Icon roots support the same events as one Explorer-owned hit target; their child layers cannot be hit-tested independently.

### Context Menus

The dashboard's **Context Menus** workspace manages reusable native Windows menus separately from themes. Create or duplicate a menu, add actions, separators, and nested submenus, reorder items, and save it under a stable id. Menu actions cover refresh, update frequency, providers, startup, language, updates, widget visibility, dashboard, and exit, along with the layer action language and `http` or `https` links. `set_update_frequency()` uses seconds, while older saved millisecond values are upgraded when loaded. URL actions only run after the user chooses the menu item. Context-menu files are stored under `%APPDATA%\ClaudeCodeUsageMonitor\context-menus`.

Use `show_context_menu("menu-id")` or `show_context_menu("Unique Menu Name")` in a layer event. The action helper lists the saved menus and inserts their stable id. `show_context_menu()` uses the built-in **Classic v1** menu, and a missing or deleted reference safely falls back to that built-in menu. Dashboard v2 uses the stable id `dashboard-v2`; the previous `dashboard-and-exit` id remains a compatibility alias, as does `classic-v1-4-9` for Classic v1.

Solid-colour and gradient background transparency is controlled directly by each RGBA colour. Use `#00000000` for fully transparent, or change the final two digits for partial opacity.

Data names use the same structure for `claude`, `codex`, `antigravity`, `opencode`, and `cursor`:

- `{provider}.session.percentage` and `{provider}.weekly.percentage`
- `{provider}.session.remaining` and `{provider}.weekly.remaining`
- `{provider}.{window}.reset.seconds`, `.minutes`, `.hours`, `.days`, and `.unix`
- `{provider}.available`
- `providers.count`
- `providers.claude.enabled`, `providers.codex.enabled`, `providers.antigravity.enabled`, `providers.opencode.enabled`, and `providers.cursor.enabled`

The OpenCode Go provider additionally exposes its monthly window while it is available: `{opencode.monthly.percentage}`, `{opencode.monthly.remaining}`, `{opencode.monthly.label}` (`30d`), `{opencode.monthly.reset.*}`, and `{opencode.monthly.available}` (1 when monthly data is present, otherwise 0). The `weekly` bar keeps its automatic 7d/30d selection unchanged; use `{provider}.monthly.available` in a render expression to show monthly elements only when the dashboard reports a monthly window.

The `providers.*.enabled` values reflect the dashboard settings rather than temporary polling availability, so a provider error does not unexpectedly reflow the widget. For example, `ceil(10 / max(1, providers.count))` produces the classic adaptive segment count.

For Codex, `codex.session.*` remains backward compatible with themes created before window-duration classification: when Codex omits its five-hour window, it falls back to the reported weekly window. Use `codex.five_hour.*` when a theme needs the exact five-hour window, `codex.weekly.*` for the exact weekly window, or `codex.headline.percentage` for whichever live allowance is most relevant.

Text templates insert expressions in braces. For example, `{claude.session.percentage:0.0}%` fixes one decimal place and `{codex.weekly.reset.seconds:duration}` produces a compact countdown. Text expressions can return quoted strings, including from conditionals such as `{if(system.dark, "Dark", "Light")}`; numeric-only fields continue to require a number. Canvas size is available as `canvas.width` and `canvas.height`. The selected native host is available as `host.width` and `host.height` in 96-DPI logical pixels, so a taskbar root can use an expression such as `min(46, host.height)`.

Date and time expressions use Unix timestamps in seconds. `time.now.unix` and `time.now.milliseconds` provide the live clock; numeric components are available under `time.local.*` and `time.utc.*` as `year`, `month`, `day`, `weekday`, `hour`, `minute`, and `second`. Weekdays use Monday `0` through Sunday `6`. Any Unix timestamp, including provider reset values, accepts `weekday_2`, `weekday_short`, `weekday_long`, `day`, `day_2`, `month`, `month_2`, `month_short`, `month_long`, `year_2`, `year`, `date_short`, `date_long`, `time_short`, `time_seconds`, `time_24`, `time_24_seconds`, `time_12`, `time_12_seconds`, `datetime_short`, `datetime_long`, `iso_date`, `iso_time`, and `iso_datetime`. Prefix a format with `utc_` for UTC; formats without that prefix use the computer's local time zone and the app language for names. For example, `{time.now.unix:time_24}`, `{codex.weekly.reset.unix:date_long}`, and `{codex.weekly.reset.unix - 604800:weekday_2}` show the current time, the weekly reset date, and the weekday on which that seven-day period began. Live clocks redraw at the precision they display: once per second for second-level values and once per minute otherwise.

The application version is available as the text value `{app.version}`. Numeric expressions can use `app.version.major`, `app.version.minor`, and `app.version.patch`.

Image backgrounds support PNG, JPEG, GIF, BMP, and WebP. Theme Studio imports them into the shared `themes/assets` library so layers can reuse managed copies instead of depending on files elsewhere on the computer. Alpha channels are preserved, and image backgrounds can be combined with a border, text or data-bar content, layout, and children.

Use **Import** in Theme Studio to open either a standalone theme JSON file or a Theme Studio ZIP package. **Export** creates a ZIP containing `theme.json`, `context-menu.json`, and every managed image used by the current theme under `assets/`, then opens a Save dialog for the destination. Imported package assets are copied into the managed asset library and renamed safely if a different image already uses the same name. Dropping a Theme Studio ZIP anywhere on the dashboard runs the same import flow. On the **Assets** page, dropping supported image files runs the same operation as **Add image**.

Each root object has a **Nest**, a combined **Reference** dropdown, and two compact inline nine-point placement controls. Nest and Reference are independent: Nest controls which native shell host owns the window, while Reference only controls the coordinates used to position it. **Taskbar** nests are native children of the selected display's taskbar, so they appear and hide with it instead of covering fullscreen applications. **Tray Icon** nests are registered as genuine Windows notification-area icons; Explorer controls their drag order, overflow placement, and final DPI-scaled size while the themed root supplies their artwork. **Desktop** nests sit behind application windows and desktop icons. **Floating** nests preserve always-on-top placement but automatically hide while a fullscreen window occupies their display. The Reference dropdown selects a monitor, taskbar, or system tray on a specific display. The **reference point** selects a point on that region, and the **surface point** selects the point on the widget that attaches there. Child objects instead use a nine-point parent anchor, with expressions controlling width and height relative to the parent. For example, reference centre + surface centre keeps a resizing widget centred, while system-tray bottom-left + surface bottom-right keeps the widget immediately to the tray's left. To show the same design on another display, duplicate the root object and select that display.

### Models

Use the dashboard's **Providers** section to choose what the widget displays:

- **Claude Code** is enabled by default
- **Codex** can be enabled alongside Claude Code or shown by itself
- **Antigravity** can be enabled alongside the other providers or shown by itself as its own model column
- **OpenCode** can be enabled alongside the other providers or shown by itself
- **Cursor** can be enabled alongside the other providers or shown by itself

When multiple models are shown, each model has its own usage bar and matching usage text color. Antigravity prefers Google's Gemini quota summary when available and falls back to model quota data when needed.

### System Tray Icons

The default Classic theme includes separate Claude Code, Codex, Antigravity, OpenCode, and Cursor tray-icon roots. Each root uses `providers.<provider>.enabled` as its Render expression, so only enabled providers are registered with Explorer. Clicking or double-clicking one toggles the `main` taskbar root's Render value, and right-clicking either a provider icon or the taskbar widget opens the built-in `classic-v1` context menu. A custom theme with no Tray Icon roots falls back to one persistent application icon using `src/icons/icon.ico`. When upgrading directly from v1.4.9, a saved `widget_visible: false` preference creates and selects a writable **Migrated Theme** copy with `main.render` set to `false`; user-selected custom themes are left unchanged.

Theme Studio's **Context Menus** page supports action items, submenus, separators, and non-clickable Text items. Labels on Text, action, and submenu items are live text templates, so they can show usage values, reset countdowns, provider state, or the app version. The **ƒx Values** helper inserts the same usage and formatting tokens available to theme text layers.

Context menus currently use the native Windows menu renderer. Windows therefore controls their light/dark appearance, font, DPI scaling, padding, selection highlight, disabled-text colour, separators, submenu arrows, and checkmarks. Per-menu colours, fonts, backgrounds, rounded corners, and other custom item styling are not exposed; supporting those consistently would require replacing or owner-drawing the native menu. Text items are intentionally rendered using Windows' disabled informational-row style.

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
- If Codex is enabled, your local Codex credentials from `$CODEX_HOME/auth.json` or `~/.codex/auth.json`
- If Antigravity is enabled, your local Antigravity OAuth token from Windows Credential Manager target `gemini:antigravity`
- If OpenCode is enabled, its dashboard workspace ID and auth cookie
- If Cursor is enabled, its access token from Cursor's local `state.vscdb`, or `CURSOR_SESSION_TOKEN` when set

OpenCode dashboard credentials supplied through a JSON config file are stored as plain text in that file. Protect it with the same care as a browser session cookie.

What the app sends over the network:

- Requests to Anthropic's Claude endpoints to read your usage and rate-limit information
- Requests to ChatGPT's Codex usage endpoint to read your Codex usage and rate-limit information, if Codex is enabled
- Requests to Google's Cloud Code / Antigravity endpoints to read your Antigravity quota information, if Antigravity is enabled
- Requests to the OpenCode workspace dashboard to read OpenCode Go usage, if OpenCode is enabled and dashboard credentials are configured
- Requests to `cursor.com/api/usage-summary` to read Cursor plan usage, if Cursor is enabled
- Requests to GitHub only if you use the app's update check / self-update feature
- If proxy environment variables such as `HTTPS_PROXY`, `HTTP_PROXY`, or `ALL_PROXY` are set, those outbound requests may use that proxy

What the app stores locally:

- Widget position
- Selected taskbar / screen
- Polling frequency
- Language preference
- Last update check time
- Displayed model preferences
- Active custom theme, canvas placement, and theme files

What it does **not** do:

- It does not send your credentials to any other server
- It does not use a separate backend service
- It does not collect analytics or telemetry
- It does not upload your project files
- It opens Cursor's SQLite database read-only and never modifies it
- It does not directly edit your Codex credentials file
- It opens the Claude desktop app's token cache read-only and never modifies it

Notes:

- If your Claude Code token is expired, the app may ask the local Claude CLI to refresh it in the background. When the token came from the Claude desktop app, the desktop app refreshes it instead, so open the app and sign in again if it has lapsed.
- If your Codex token is expired, the app may ask the local Codex CLI to refresh it in the background. The monitor does not write `auth.json` itself; any credential update is handled by the Codex CLI.
- If your Antigravity token is expired, open Antigravity and sign in again. The monitor does not write Windows Credential Manager entries itself.
- Portable installs can update themselves by downloading the latest release from this repository
- If the Claude usage endpoint is rate limited or briefly unavailable, the app keeps showing your last known figures and retries with a backoff, rather than spending quota on a request just to read its rate limit headers
- Proxies should be trusted because proxied usage requests include your OAuth bearer token inside the TLS connection

## How It Works

The monitor:

1. Finds your enabled model login credentials
2. Reads your current usage from Anthropic, ChatGPT, and/or Google's Antigravity endpoints
3. Shows the result directly in the Windows taskbar
4. Keeps the widget aligned with the selected taskbar and tray area
5. Refreshes periodically in the background

If the newer usage endpoint is unavailable, it can fall back to reading the rate-limit headers returned by Claude's Messages API.

## Open Source

This project is licensed under MIT.

If you want to inspect the behavior or audit the code, everything is in this repository.
