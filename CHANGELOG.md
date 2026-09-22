# Changelog

Notable changes to Claude Code Usage Monitor are documented here, newest first.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), with
changes grouped into Added, Changed, Fixed, and Removed where applicable.

## [2.13.44] - 2026-09-22

### Changed

- Rendered the dashboard GitHub link from an icon font glyph instead of an embedded SVG, so it follows the surrounding menu text colour. The mark is a 1 KB single-glyph TrueType font that joins the Lucide icon family as a fallback.

### Removed

- Dropped the `egui_extras` dependency and its `svg` feature, which existed solely to draw the GitHub logo. This removes the resvg, usvg, tiny-skia, and XML/CSS parsing stack and reduces the release executable by roughly 549 KB (7.3%).

## [2.13.43] - 2026-09-21

### Added

- Exposed Claude usage API quotas and legacy optional limits to custom themes, including named model bindings, active scoped limits, reset timers, account-specific data, and discovered values under Claude Code in the Theme Studio text and expression editors. Built-in themes and headline values remain unchanged. ([#56](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/56), [#61](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/61))

### Fixed

- Explained login, network, HTTP, and response-format failures in tray tooltips and account status. Logged malformed Claude usage responses without silently invoking the Messages API fallback, and updated sign-in guidance for desktop and CLI users. ([#72](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/72))

## [2.12.42] - 2026-09-21

### Fixed

- Published usage as each provider or account responds, allowing faster providers to appear while others refresh credentials or wait on network requests. Pending readings are preserved, and the dashboard receives partial updates too.
- Moved paused credential checks onto the poll worker so local credential reads and WSL probes cannot block the widget's window procedure. Manual refreshes queued during a check are preserved.
- Isolated test settings, usage caches, themes, and context menus in temporary directories per test thread, preventing tests from changing the real application data or interfering with each other.
- Restored the dashboard GitHub link, which stopped opening after the eframe update in `2.8.16`, by using the Windows browser opener shared with widget links.
- Detected Claude desktop credentials from Microsoft Store installations and preferred tokens carrying both usage-related scopes. Preserved newer token-cache support, credential-change watching, and explicit account selection.

## [2.12.41] - 2026-09-21

### Fixed

- Reduced widget jitter during tray icon updates and shell animations by waiting 80 ms after the last tray location event before repositioning. ([#107](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/107))
- Routed watchdog tray geometry changes through the same delay so they cannot trigger an immediate reposition and cancel a pending tray update.

## [2.12.40] - 2026-09-21

### Fixed

- Preserved the taskbar's logical host dimensions when widgets are dragged or automatically undocked, preventing custom themes using `host.height` from expanding to the full screen height. Child layout uses the same dimensions, with floating-card padding kept outside the content.
- Saved floating host dimensions per theme and root across restarts and repeated drags, and used the destination monitor's DPI when sizing a dropped widget. Re-docking adopts the destination taskbar's dimensions; themes authored as floating retain monitor-based sizing.
- Retained the last valid taskbar geometry during auto-hide or temporary taskbar removal, with a compact fallback when no taskbar dimensions are available for undocking.
- Fixed missed automatic undocking on Windows 11 by measuring actual taskbar controls instead of legacy container bounds. Collision and return checks account for widgets on either side of centred buttons, separate button groups and rows, vertical taskbars, monitor offsets, and DPI scaling.
- Kept taskbar accessibility queries off the UI and Explorer watchdog threads, ignored unavailable or stale measurements, and rechecked queued transitions before moving the widget. Automatic return preserves the original position and waits for clearance from app buttons on both sides.
- Allowed docking into measured gaps on either side of taskbar buttons and corrected saved offsets for gaps left of centred buttons, preventing the widget from jumping toward the tray after a drop.

## [2.12.39] - 2026-09-19

### Added

- Dashboard version button with a refresh icon to check for updates, changing to a download icon and version tooltip when an update is available. Uses the existing update prompts and portable EXE or WinGet installation flow.

### Changed

- Synchronized dashboard update status with the running monitor, disabled the version button while updates are busy, and resolved unsaved theme edits before starting an update action.

### Fixed

- Prevented repeated WinGet update launches while an update is starting.

## [2.12.38] - 2026-09-19

### Added

- Project changelog covering version history from `1.0.0` onward, with GitHub comparison links.
- [User guide](USER_GUIDE.md) covering theme duplication and customisation, saving and Live apply, provider selection, refresh settings, and usage direction.

### Changed

- Linked the user guide and changelog from the README.

## [2.12.37] - 2026-09-19

### Fixed

- Prevented widget flicker during routine rendering and theme updates by preserving layered DWM surfaces.
- Restored correct taskbar visibility after re-docking by rebinding layered surfaces only after successful parent changes.

## [2.12.36] - 2026-09-19

### Changed

- Made diagnostics logging optional, stopping recording and closing log files when disabled.
- Simplified the Diagnostics page with settings-style sections, icon controls, and a follow toggle; moved the app version beside the GitHub link.
- Preserved raw log output while displaying local dates and times in the viewer.

## [2.12.35] - 2026-09-18

### Fixed

- Accepted OpenCode Go console-session cookies without incorrectly prefixing them with `auth=`; preserved full Cookie headers and legacy raw tokens. Updated setup instructions. ([#102](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/102))

## [2.12.34] - 2026-09-18

### Added

- Live Diagnostics page with app version, refresh tracing, and copy/pause controls.

### Fixed

- Preserved Claude usage and HTTP failure status across dashboard reloads by correcting cache validation.
- Reported refresh requests that could not reach the running monitor.

## [2.12.33] - 2026-09-18

### Fixed

- Displayed Claude HTTP failures in account status and cleared them after successful polling.
- Preserved cached status, stale usage, and authentication recovery, including when accounts are renamed.

## [2.12.32] - 2026-09-18

### Fixed

- Resolved taskbars to the correct monitors, kept restored floating widgets on screen, and checked the full saved docking rectangle before re-docking.
- Corrected vertical docking geometry and placement overrides for themes with custom anchors and offsets.
- Synchronized fullscreen visibility checks and isolated the primary widget's window state from mirrored widgets.

## [2.12.31] - 2026-09-18

### Fixed

- Restored Claude desktop usage with support for the desktop app's `oauth:tokenCacheV2` format. ([#104](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/104))
- Allowed default-path Claude profiles to fall back to desktop credentials and detect desktop login or token rotation, while keeping custom `CLAUDE_CONFIG_DIR` accounts isolated.

## [2.12.30] - 2026-09-18

### Added

- Free widget dragging, magnetic docking, and saved placement across monitors.
- Automatic floating when taskbar space runs out and re-docking when space returns, with padded floating cards.

### Fixed

- Repositioned the docked widget as notification icons appear or disappear.
- Corrected first-drag flashes, secondary-monitor and high-DPI placement, drag capture transitions, and false collision loops.
- Prevented left-edge clipping in automatically floating cards while preserving compact docked dimensions.

## [2.11.29] - 2026-09-17

### Fixed

- Restored OpenCode Go polling after its console migration by using the console JSON status API with workspace context. ([#102](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/102))
- Calculated rolling, weekly, and monthly usage from the new meters while preserving absolute reset times; updated provider setup instructions.

## [2.11.28] - 2026-09-16

### Added

- Multiple named Claude Code and Codex accounts with independent usage, credentials, status, and default-account selection.
- Account-specific theme bindings, custom credential paths, and `CLAUDE_CONFIG_DIR` support. ([#75](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/75))
- Custom refresh intervals in minutes, synchronized between Settings and context menus.

### Fixed

- Preserved unique account IDs after deletion and validated account theme bindings without requiring live data.
- Restored authentication recovery and notifications independently for each account.

## [2.10.27] - 2026-09-15

### Added

- Compact Fluent Quad built-in theme, with support for the selected usage direction.

## [2.10.26] - 2026-09-13

### Fixed

- Prevented taskbar-position queries from hanging when Explorer stops responding. ([#98](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/98))
- Improved taskbar recovery after Explorer restarts by checking surviving window parents, re-docking, and repainting without entering a relaunch loop.

## [2.10.25] - 2026-09-12

### Added

- Conditional context-menu `Render` expressions, Theme Studio controls, and live preview filtering. ([#96](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/96))

### Fixed

- Tracked whether providers actually report each usage window, even when usage is zero or reset times are absent, including cached readings.
- Preserved idle Codex session windows instead of replacing them with weekly usage.
- Made monthly theme variables and labels validate without live provider data.

## [2.10.24] - 2026-09-11

### Fixed

- Deferred tray callbacks and restored the dashboard asynchronously to prevent locking problems during tray interactions. ([#94](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/94))

## [2.10.23] - 2026-09-10

### Added

- **Settings > Display > Usage direction** option to show either used or remaining allowance, with Used as the default. ([#93](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/93))
- Explicit `.display` theme bindings and Theme Studio helpers; updated Classic bars, summaries, and tray badges to follow the setting.

### Fixed

- Kept existing percentage bindings, usage summaries, and warning thresholds backward compatible; custom themes opt in to the new display direction.
- Kept tray badge sizing and severity colors correct when counting down remaining usage.

## [2.10.22] - 2026-09-10

### Fixed

- Prevented tray context-menu deadlocks by caching theme host geometry outside the application state lock. ([#92](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/92))
- Refreshed cached layout after shell and display changes while preserving live Theme Studio sizing.

## [2.10.21] - 2026-09-09

### Fixed

- Kept the dashboard readable in Windows light app mode by consistently applying its dark theme. ([#91](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/91))

## [2.10.20] - 2026-09-08

### Changed

- Streamlined the README around installation, usage, provider setup, privacy, troubleshooting, and source builds.

## [2.10.19] - 2026-09-04

### Added

- Thai localization, including integration with the language catalogue. ([#88](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/88))

## [2.9.19] - 2026-09-04

### Fixed

- Preserved the last valid usage values during transient polling failures, including when every provider fails.
- Kept authentication failures and failures without cached data in an explicit error state.

## [2.9.18] - 2026-09-03

### Added

- Polish localization. ([#90](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/90))

## [2.8.18] - 2026-09-01

### Added

- Configurable HTTP(S) URL actions for theme-layer mouse events. ([#87](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/87))

## [2.8.17] - 2026-08-28

### Fixed

- Restored live provider usage in tray tooltips. ([#85](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/issues/85))

## [2.8.16] - 2026-08-28

### Changed

- Raised the minimum supported Rust version to **1.95**.
- Updated HTTP, Windows, UI, archive, TOML, icon, and other dependencies; migrated integrations to the updated APIs while retaining platform certificate verification.

## [2.8.15] - 2026-08-27

### Added

- `get(this, property)` theme-expression lookup, including the current layer's resolved gap, with a Theme Studio helper.

## [2.7.14] - 2026-08-27

### Added

- GitHub project link in the dashboard navigation footer.

## [2.7.13] - 2026-08-24

### Added

- Localized local and UTC date/time formats for Theme Studio timestamps.
- Live clock variables, with theme refreshes matched to the displayed time precision.

## [2.6.13] - 2026-08-24

### Added

- Quoted string results from `if()` in Theme Studio text expressions, while keeping geometry and other numeric fields numeric-only.

## [2.5.13] - 2026-08-21

### Changed

- Reused HTTP connections and polled providers concurrently with a bounded worker count and deterministic results.

## [2.5.12] - 2026-08-21

### Changed

- Skipped window-state polling for themes without floating surfaces and avoided redundant floating-window visibility updates.

## [2.5.11] - 2026-08-21

### Fixed

- Validated provider reset timestamps before conversion and correctly applied ISO 8601 timezone offsets.

## [2.5.10] - 2026-08-21

### Fixed

- Prevented stale reset data from triggering repeated polls and bounded refresh requests queued during an active poll.

## [2.5.9] - 2026-08-20

### Added

- OpenCode monthly usage-window bindings for custom themes. ([#79](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/79))

## [2.4.9] - 2026-08-20

### Fixed

- Restored backward-compatible Codex session variables for existing custom themes.
- Added exact Codex five-hour bindings for new and bundled themes.

## [2.4.8] - 2026-08-20

### Added

- Paid credit tracking for Claude and Codex, including credit balances in Classic theme bars and tray badges. ([#81](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/81))
- Theme bindings for credit usage, balances, headline usage, and stale provider data.
- Persisted Codex credit baselines per account, including detection of balance top-ups.

### Fixed

- Retained a provider's last reading when another provider succeeds but that provider fails.
- Treated rate limits, server errors, and transport failures as temporary Claude polling failures instead of falling back to a quota-consuming request.
- Classified Codex rate-limit windows by duration and preserved legacy window behavior.

## [2.3.8] - 2026-08-19

### Added

- Claude desktop token-cache support for installations without standalone CLI credentials, plus discovery of the desktop app's bundled CLI. ([#80](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/80))

### Fixed

- Ordered bundled Claude CLI versions correctly when selecting an executable.
- Avoided unnecessary WSL credential probes when local credentials were already available.

## [2.2.8] - 2026-08-17

### Changed

- Improved Theme Studio preview performance with asynchronous rendering scaled to the viewport and coalesced render requests.
- Reduced unnecessary countdown redraws and aligned previews with the physical-pixel grid.

## [2.2.7] - 2026-08-17

### Added

- Pixel-perfect Theme Studio preview zooming, pointer-anchored navigation, and bounded panning.

## [2.2.6] - 2026-08-17

### Fixed

- Preserved fully rounded outer edges when rendering segmented progress bars with gaps.

## [2.2.5] - 2026-08-17

### Added

- DPI-aware `host.width` and `host.height` theme expressions, resolved for each native surface before layout and rendering.

## [2.2.4] - 2026-08-17

### Changed

- Restructured localization so contributors can add a language through a single file.

## [2.2.3] - 2026-08-17

### Added

- Context Menus translations across all supported languages.

## [2.2.2] - 2026-08-17

### Added

- Turkish language support.

## [2.2.1] - 2026-08-16

### Added

- Cursor usage monitoring, with local session discovery through Windows' built-in SQLite library and a `CURSOR_SESSION_TOKEN` override. ([#68](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/68))
- Cursor Auto/API usage displays with dedicated tray handling and localized labels.

## [2.1.1] - 2026-08-16

### Added

- OpenCode Go provider support using dashboard credentials, with a rolling five-hour gauge and a longer-term gauge selecting the more-used weekly or monthly window.

### Changed

- Kept the integrated OpenCode provider dashboard-only, omitting the contribution's bundled SQLite fallback to avoid increasing executable size.

## [2.0.1] - 2026-08-15

### Fixed

- Corrected WinGet manifest parsing.

## [2.0.0] - 2026-08-15

### Added

- Native settings dashboard for providers, refresh frequency, language, startup, and updates, with a `--dashboard` launch option.
- Visual Theme Studio with live previews, editable JSON themes, duplication, undo/redo, and optional live apply.
- Theme layers with text, progress bars, images, gradients, nested layouts, expressions, and mouse actions across desktop, taskbar, and tray surfaces.
- Context-menu editor with reusable menus, nested submenus, actions, and dynamic text labels.
- Built-in Classic theme and an editable Minecraft starter theme.

### Changed

- Moved widget rendering to the theme engine, with Classic reproducing the original usage display.

## [1.4.9] - 2026-07-17

### Added

- Simplified Chinese localization, including Chinese locale detection and corrected Macau mapping to Traditional Chinese.

## [1.4.8] - 2026-06-20

### Changed

- Updated the README.

## [1.4.7] - 2026-06-20

### Added

- Google Antigravity usage monitoring and localized provider controls. ([#31](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/31))

### Changed

- Tightened multi-provider spacing, used four bar segments in the three-provider layout, and applied Google blue to Antigravity.
- Restricted widget dragging to its divider handle.

### Fixed

- Corrected a context-menu command ID collision in a follow-up commit carrying the same version number.

## [1.4.6] - 2026-06-20

### Added

- Moving the widget between taskbars on different monitors. ([#37](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/37))

### Fixed

- Preserved Explorer-restart recovery after introducing multiple-taskbar support.

## [1.4.5] - 2026-06-20

### Fixed

- Continued polling Codex when Claude fails, and Claude when Codex fails, instead of letting one provider block the other.
- Preserved the original polling error when neither enabled provider succeeds.

## [1.4.4] - 2026-06-20

### Fixed

- Recovered the widget and tray icon after Explorer restarts through a taskbar watchdog and controlled relaunch. ([#36](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/36))
- Made the replacement process wait correctly for the previous instance to release its single-instance lock.

## [1.4.3] - 2026-06-12

### Added

- Brazilian Portuguese localization, including language selection and automatic locale detection. ([#29](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/29))

## [1.4.2] - 2026-06-12

### Added

- Russian localization, with a working language-menu command. ([#28](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/28))

## [1.4.1] - 2026-05-13

### Fixed

- Corrected widget drag state.

## [1.4.0] - 2026-05-09

### Added

- Codex usage monitoring through the model menu, with either provider or both visible.
- Separate provider tray icons and colored text to distinguish providers in the combined display.

## [1.3.9] - 2026-05-05

### Fixed

- Tried other credential sources after a permanent credential failure.

## [1.3.8] - 2026-05-05

### Fixed

- Stopped stealing focus in response to tray-location events caused by the monitor's own icon updates.

## [1.3.7] - 2026-05-04

### Added

- Dutch localization.

## [1.3.6] - 2026-05-01

### Added

- Proxy environment-variable support for HTTP requests.

## [1.3.5] - 2026-04-20

### Fixed

- Aligned displayed usage percentages with tray icon numbers.

## [1.3.4] - 2026-04-19

### Added

- Tray notification when OAuth credentials expire. ([#15](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/15))

### Fixed

- Showed a consistent authentication warning for expired, invalid, or missing credentials, with updated translations.
- Paused API polling and countdowns while authentication was required; watched credential-file changes to recover automatically, while retaining manual refresh support.

## [1.3.3] - 2026-04-19

### Added

- Traditional Chinese localization. ([#13](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/13))

## [1.3.2] - 2026-04-19

### Fixed

- Resolved a taskbar-positioning deadlock.

## [1.3.1] - 2026-04-07

### Added

- Korean localization. ([#9](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/9))

### Fixed

- Localized the Show Widget command and session/weekly window labels.

## [1.3.0] - 2026-03-31

### Added

- Native system tray icon with a live five-hour usage badge and tooltips for both usage windows. ([#6](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/6))
- Left-click to toggle widget visibility and right-click for its context menu, with visibility remembered across launches.

### Changed

- Improved Windows app identity and embedded-icon handling.

### Fixed

- Refined taskbar anchoring and countdown formatting to avoid displaying values such as `61m`.

## [1.2.10] - 2026-03-30

### Fixed

- Anchored the widget to the bottom of the taskbar to improve positioning.

## [1.2.9] - 2026-03-27

### Fixed

- Improved the widget's vertical placement.

## [1.2.8] - 2026-03-26

### Fixed

- Corrected update handling for portable and WinGet installations.

## [1.2.7] - 2026-03-26

### Changed

- Updated the README with WinGet installation guidance.

## [1.2.6] - 2026-03-25

### Added

- `--diagnose` option for troubleshooting startup issues.

## [1.2.5] - 2026-03-23

### Added

- Update dialog when a manual update check finds a newer version.

## [1.2.4] - 2026-03-21

### Fixed

- Added a timeout to WSL credential lookups to prevent startup from hanging. ([#4](https://github.com/CodeZeno/Claude-Code-Usage-Monitor/pull/4))

## [1.2.3] - 2026-03-19

### Changed

- Updated the README.

## [1.2.2] - 2026-03-19

### Changed

- Split localization into separate files to simplify maintenance and translation contributions.

## [1.2.1] - 2026-03-18

### Added

- WinGet distribution support and installation-aware update handling.

## [1.2.0] - 2026-03-18

### Added

- Update checks and self-updating from GitHub releases.

## [1.1.0] - 2026-03-18

### Added

- Windows-aware localization with a manual language override, supporting English, Spanish, French, German, and Japanese.

## [1.0.15] - 2026-03-15

### Added

- WSL2 credential support.

## [1.0.14] - 2026-03-15

### Changed

- Added Windows executable metadata and synchronized embedded file-version information with the package version.

## [1.0.13] - 2026-03-15

### Fixed

- Prevented console-window flashes during token refresh.

## [1.0.12] - 2026-03-11

### Added

- Windows high-DPI scaling support.

## [1.0.11] - 2026-03-07

### Fixed

- Hid the Claude CLI window during background token refresh.

## [1.0.10] - 2026-03-06

### Added

- Embedded application icon.

## [1.0.9] - 2026-03-06

### Fixed

- Restored fallback to the previous rate-limit-header method when the usage endpoint fails.

## [1.0.8] - 2026-03-05

### Changed

- Switched Claude usage polling to `/api/oauth/usage`.

### Fixed

- Corrected the app's displayed version.

## [1.0.7] - 2026-03-05

### Added

- Dragging the widget to a preferred position.
- Saved widget position and refresh frequency in `%APPDATA%\ClaudeCodeUsageMonitor\settings.json`.

## [1.0.6] - 2026-03-05

### Fixed

- Improved Claude CLI discovery and used a CLI prompt to trigger token refresh when `claude auth status` did not refresh credentials.
- Corrected Start with Windows handling and widened usage text to avoid clipping.

## [1.0.5] - 2026-03-01

### Fixed

- Delegated token refresh to the Claude CLI and re-read its credentials, avoiding refresh-token rotation conflicts with Claude Code.

### Removed

- The monitor's direct OAuth refresh logic and in-memory refreshed-token cache.

## [1.0.4] - 2026-03-01

### Fixed

- Cached refreshed OAuth tokens in memory across polling cycles.

## [1.0.3] - 2026-02-27

### Fixed

- Balanced row spacing to center the widget vertically.

## [1.0.2] - 2026-02-27

### Added

- Second-by-second countdowns during the final minute before a reset.
- Temporary five-second polling after a reset, returning to the normal interval when fresh reset data arrives.

## [1.0.1] - 2026-02-27

### Added

- Automatic OAuth token refresh and exponential retry backoff after polling failures.

### Fixed

- Displayed `...` during polling errors and prevented countdown updates from replacing the error indicator with stale data.

## [1.0.0] - 2026-02-26

### Added

- Initial Windows taskbar widget for Claude Code, showing five-hour and weekly usage bars with reset countdowns.
- Local OAuth credential discovery, configurable polling frequency, manual refresh, and automatic Windows light/dark styling.
- Prebuilt Windows executable releases and MIT-licensed Rust source.


[1.0.0]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/commit/a43200468049fff7d56f5693a96191a2829c5b62
[1.0.1]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/a43200468049fff7d56f5693a96191a2829c5b62...d8d15dcdba6dfa76b6c3e890a3f9d349286b1008
[1.0.2]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/d8d15dcdba6dfa76b6c3e890a3f9d349286b1008...266b8d5fb56fce922f48581ba7a17c5742f4d178
[1.0.3]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/266b8d5fb56fce922f48581ba7a17c5742f4d178...187d12250ca7d85a4c4ba44e7856211ae7ff183d
[1.0.4]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/187d12250ca7d85a4c4ba44e7856211ae7ff183d...6a3275517e186f0f5bab72d6e6a29076786bbdc7
[1.0.5]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/6a3275517e186f0f5bab72d6e6a29076786bbdc7...fcad7fb848048e8e9e911a1df39e66bc0d30517c
[1.0.6]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/fcad7fb848048e8e9e911a1df39e66bc0d30517c...9941c9a46d2a1c9f2be691491db8d2d513d98ae5
[1.0.7]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/9941c9a46d2a1c9f2be691491db8d2d513d98ae5...55d3a88b1677ae6161c2f29f531ef8e00c9f8aee
[1.0.8]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/55d3a88b1677ae6161c2f29f531ef8e00c9f8aee...ad4467e81431055b61cb8e3c9d2d29428c2ef988
[1.0.9]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/ad4467e81431055b61cb8e3c9d2d29428c2ef988...c8bd9683dd8f11c529321740bb636b86379f714d
[1.0.10]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/c8bd9683dd8f11c529321740bb636b86379f714d...a68f80181273284a0fbe4f04d13c19393b3759f8
[1.0.11]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/a68f80181273284a0fbe4f04d13c19393b3759f8...4de95f0bde86ee98df91262bb08a679f2bc0ae62
[1.0.12]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/4de95f0bde86ee98df91262bb08a679f2bc0ae62...97ca8f8468092df1f57ca1870094239137cb2677
[1.0.13]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/97ca8f8468092df1f57ca1870094239137cb2677...50d656bd5349ea59877a201af3824e63f1c88062
[1.0.14]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/50d656bd5349ea59877a201af3824e63f1c88062...7960ae9218ba844bf44e0a8037736e3b3741542d
[1.0.15]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/7960ae9218ba844bf44e0a8037736e3b3741542d...ec4177f7f61aecb12b7aecf3cc8172fb4dc05123
[1.1.0]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/ec4177f7f61aecb12b7aecf3cc8172fb4dc05123...d6ea143f0566eaf80527e13afa4520fb08b69d08
[1.2.0]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/d6ea143f0566eaf80527e13afa4520fb08b69d08...832e70fd72a88f4b33481f447e2ffefae8b04077
[1.2.1]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/832e70fd72a88f4b33481f447e2ffefae8b04077...e28bc51ab1bc835e74a40e6f6e5c8618acf0c9be
[1.2.2]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/e28bc51ab1bc835e74a40e6f6e5c8618acf0c9be...6d03532d5072054293d7786e3c587ebb4b80795c
[1.2.3]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/6d03532d5072054293d7786e3c587ebb4b80795c...5e039fa7dc4b7871d58d718b600cd945ca926555
[1.2.4]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/5e039fa7dc4b7871d58d718b600cd945ca926555...81a26eaf613bf8322ee59983ef081c8f139da952
[1.2.5]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/81a26eaf613bf8322ee59983ef081c8f139da952...110ae4ac58734b193a9460353e5d901112750233
[1.2.6]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/110ae4ac58734b193a9460353e5d901112750233...f527e82928767bedb42c4525c646fc03b2f238fc
[1.2.7]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/f527e82928767bedb42c4525c646fc03b2f238fc...9c8f73cf69776359821247ebf05ba890d8d16078
[1.2.8]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/9c8f73cf69776359821247ebf05ba890d8d16078...65cf2b5d1678c52003fa3742c7fa36e997af950d
[1.2.9]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/65cf2b5d1678c52003fa3742c7fa36e997af950d...7ef75f4c0b185e744c26ae72d932123d6f6006a7
[1.2.10]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/7ef75f4c0b185e744c26ae72d932123d6f6006a7...ba84737ffbada4d1a3eca0bfe81603b5fea97a98
[1.3.0]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/ba84737ffbada4d1a3eca0bfe81603b5fea97a98...643cc0a8b680e07e2b55aadd222f01d4e05be05e
[1.3.1]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/643cc0a8b680e07e2b55aadd222f01d4e05be05e...a407f6945381302ae3bbdc0883543cb4bd4681ee
[1.3.2]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/a407f6945381302ae3bbdc0883543cb4bd4681ee...b0fcd92af4cb4fd51dd2ee25f90259b5877f6cae
[1.3.3]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/b0fcd92af4cb4fd51dd2ee25f90259b5877f6cae...3bc9be442e151c291f1fda1fc9f9ef2d3bf3795e
[1.3.4]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/3bc9be442e151c291f1fda1fc9f9ef2d3bf3795e...17c63517ca394977229160702a3f0dcc68f16033
[1.3.5]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/17c63517ca394977229160702a3f0dcc68f16033...35f945daa9938d684dca2a5ff9dc0286993622e5
[1.3.6]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/35f945daa9938d684dca2a5ff9dc0286993622e5...63806d5421e1b5e2f7e468624835fb123e87a4c1
[1.3.7]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/63806d5421e1b5e2f7e468624835fb123e87a4c1...ae9be6704a1d8e588c1db6ac24266ebe03139686
[1.3.8]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/ae9be6704a1d8e588c1db6ac24266ebe03139686...f2b378462d6916eaa6ef465e8afa325c95f8b75c
[1.3.9]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/f2b378462d6916eaa6ef465e8afa325c95f8b75c...d54588bf6e0937c9ea72100dfedf226f26331d1a
[1.4.0]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/d54588bf6e0937c9ea72100dfedf226f26331d1a...4676f6f72eadf6c9076629e07f904f4200ce1b6e
[1.4.1]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/4676f6f72eadf6c9076629e07f904f4200ce1b6e...b5f038dd3946278e47fad0899bbe890c675df5e0
[1.4.2]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/b5f038dd3946278e47fad0899bbe890c675df5e0...286dd89a67a154ee493441bbd86964c958610547
[1.4.3]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/286dd89a67a154ee493441bbd86964c958610547...7af314996d19265de3b93f7afc4b7a5c173c488c
[1.4.4]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/7af314996d19265de3b93f7afc4b7a5c173c488c...b46e11fbf86060788b545c379d5b66087c4d9d07
[1.4.5]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/b46e11fbf86060788b545c379d5b66087c4d9d07...c333fdfa14e78c71c7698021b02734193b0ad7ae
[1.4.6]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/c333fdfa14e78c71c7698021b02734193b0ad7ae...c4e9dcb1ef9ad5442d62346e7f673d1757e9549a
[1.4.7]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/c4e9dcb1ef9ad5442d62346e7f673d1757e9549a...c64de77d8b526a374942086ca3bf51f75d2fa416
[1.4.8]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/c64de77d8b526a374942086ca3bf51f75d2fa416...9b299725c62f51aff82577a7ec634a5fd14a3bd9
[1.4.9]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/9b299725c62f51aff82577a7ec634a5fd14a3bd9...7b108da813550fc9500a3d8843ed207ab55b07df
[2.0.0]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/7b108da813550fc9500a3d8843ed207ab55b07df...f7accd587f6c0c37e307b5784b67f361c0a7e3e8
[2.0.1]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/f7accd587f6c0c37e307b5784b67f361c0a7e3e8...f30b2f09d559d5b42a161e8493d9cff3fb53e4bd
[2.1.1]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/f30b2f09d559d5b42a161e8493d9cff3fb53e4bd...d6e7d0afb6f5fd5fefe67cfa7239e82dbfbac161
[2.2.1]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/d6e7d0afb6f5fd5fefe67cfa7239e82dbfbac161...39dcbdea73b958426e213435dd3203033f5369d1
[2.2.2]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/39dcbdea73b958426e213435dd3203033f5369d1...46b4e2272bded3b036b57b9d3b72e8b1e063445b
[2.2.3]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/46b4e2272bded3b036b57b9d3b72e8b1e063445b...0ee6c73b875a2dbdbb31b6e82b3952226e02075c
[2.2.4]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/0ee6c73b875a2dbdbb31b6e82b3952226e02075c...3e0e577f8112e902b6a7b4ab62ce974bc10591fb
[2.2.5]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/3e0e577f8112e902b6a7b4ab62ce974bc10591fb...ac8742a0596a11b3b8048e7e7c51d1d04b6aada9
[2.2.6]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/ac8742a0596a11b3b8048e7e7c51d1d04b6aada9...870e0abf4b1607a435ed387da32387c252d25b0a
[2.2.7]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/870e0abf4b1607a435ed387da32387c252d25b0a...1f9ed30c19b7756f72cc7fd9cd5480351e93d49a
[2.2.8]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/1f9ed30c19b7756f72cc7fd9cd5480351e93d49a...20ae4be022f31249e58b463ad22840a9a99d9781
[2.3.8]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/20ae4be022f31249e58b463ad22840a9a99d9781...5dc567882f63a1e12dfed6ca7bb069a2ea531a25
[2.4.8]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/5dc567882f63a1e12dfed6ca7bb069a2ea531a25...0e49ee5e610f9a1f266e755343a7317602e734db
[2.4.9]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/0e49ee5e610f9a1f266e755343a7317602e734db...cc565738032a17175b0da6c7540f899d57540ea3
[2.5.9]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/cc565738032a17175b0da6c7540f899d57540ea3...d45438b90ab5c0fcf44fb9608bdf647310a1819d
[2.5.10]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/d45438b90ab5c0fcf44fb9608bdf647310a1819d...fb74f9a72ecd0923c1c468a54f430b35e891dd55
[2.5.11]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/fb74f9a72ecd0923c1c468a54f430b35e891dd55...57485d420ae15e47b5bfbb596e758437b7fe28e8
[2.5.12]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/57485d420ae15e47b5bfbb596e758437b7fe28e8...abca61e0caac6acd57f84accc3808d591bd694b5
[2.5.13]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/abca61e0caac6acd57f84accc3808d591bd694b5...02a0f21a5143545b3109543065d13e022f8e677a
[2.6.13]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/02a0f21a5143545b3109543065d13e022f8e677a...a0e28cb3ab04fb2dd8a75f3179944731aa8e6400
[2.7.13]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/a0e28cb3ab04fb2dd8a75f3179944731aa8e6400...318a8b57ee40e4847125bae9d542482013c3b67e
[2.7.14]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/318a8b57ee40e4847125bae9d542482013c3b67e...12bc4eee5becd4270920e0dde98575e5e39f06cc
[2.8.15]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/12bc4eee5becd4270920e0dde98575e5e39f06cc...b8e16fd62e71e017c83b4de8c09ef1d451eb8f3b
[2.8.16]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/b8e16fd62e71e017c83b4de8c09ef1d451eb8f3b...9aa35e6a64dc3ed87fa5a8d9b11b08cd601f532b
[2.8.17]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/9aa35e6a64dc3ed87fa5a8d9b11b08cd601f532b...3c8263012d4ee8de8156205c8dd1b0c6f3b8de43
[2.8.18]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/3c8263012d4ee8de8156205c8dd1b0c6f3b8de43...63bf911d4ebc87e63235a7c1e1808716aceaf798
[2.9.18]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/63bf911d4ebc87e63235a7c1e1808716aceaf798...97f4fd9b4a26f722c4f0be78f01098e013b35552
[2.9.19]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/97f4fd9b4a26f722c4f0be78f01098e013b35552...97d870dc8693ec98ddf6f020ad88597ea462a03c
[2.10.19]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/97d870dc8693ec98ddf6f020ad88597ea462a03c...c02d04457acad549d0f2a939b4549698413f7867
[2.10.20]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/c02d04457acad549d0f2a939b4549698413f7867...ef171ac0d7b422f0446385513f908f79274d8547
[2.10.21]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/ef171ac0d7b422f0446385513f908f79274d8547...845454e6623197a5cf76dcc744fc4fc89e82019f
[2.10.22]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/845454e6623197a5cf76dcc744fc4fc89e82019f...1584e62b27520f4104df072582667ffa494d86c9
[2.10.23]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/1584e62b27520f4104df072582667ffa494d86c9...8ad64219d57c2c59b4318412950a42e43e2abc5a
[2.10.24]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/8ad64219d57c2c59b4318412950a42e43e2abc5a...8f9afef889e3a0e8a180214abcd119d2276f00c5
[2.10.25]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/8f9afef889e3a0e8a180214abcd119d2276f00c5...b4a4d0e91f95279ae08285baf843b624666d2db1
[2.10.26]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/b4a4d0e91f95279ae08285baf843b624666d2db1...de6b0d5facaa206704c0b3459bd51dabee8a556a
[2.10.27]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/de6b0d5facaa206704c0b3459bd51dabee8a556a...c0a486882671cf836e8f07afe63741432df52640
[2.11.28]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/c0a486882671cf836e8f07afe63741432df52640...314054f7e9713456d4f02d52440bdfda451e22a3
[2.11.29]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/314054f7e9713456d4f02d52440bdfda451e22a3...d113a08d35cf417008b206be7d67e2ddcddd1750
[2.12.30]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/d113a08d35cf417008b206be7d67e2ddcddd1750...793c289717a6e29753a2ec90f90c929669b0eff4
[2.12.31]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/793c289717a6e29753a2ec90f90c929669b0eff4...19a039d79b48aca2174765a96325bfa0c43493cb
[2.12.32]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/19a039d79b48aca2174765a96325bfa0c43493cb...cce396d5c0f168de89da459f320375641563505a
[2.12.33]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/cce396d5c0f168de89da459f320375641563505a...46d2c7c4df4acdaca8caf6fa93a056b3c991854e
[2.12.34]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/46d2c7c4df4acdaca8caf6fa93a056b3c991854e...71afcd3726e5aca62eb3e7fcd30f33ed6e48ada3
[2.12.35]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/71afcd3726e5aca62eb3e7fcd30f33ed6e48ada3...b1822e59cbcd3bfc53f1874da6e9ca26fe670bf6
[2.12.36]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/b1822e59cbcd3bfc53f1874da6e9ca26fe670bf6...840d5d57df51d0bb316fdcb9b4490f92656791be
[2.12.37]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/840d5d57df51d0bb316fdcb9b4490f92656791be...68690d86cca38a5cab75803fbd83ca1395c42d4f
[2.12.38]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/v2.12.37...v2.12.38
[2.12.39]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/v2.12.38...v2.12.39
[2.12.40]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/v2.12.39...v2.12.40
[2.12.41]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/v2.12.40...v2.12.41
[2.12.42]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/v2.12.41...v2.12.42
[2.13.43]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/v2.12.42...v2.13.43
[2.13.44]: https://github.com/CodeZeno/Claude-Code-Usage-Monitor/compare/v2.13.43...v2.13.44
