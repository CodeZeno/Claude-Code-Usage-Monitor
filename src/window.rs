use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use windows::core::GUID;
use windows::core::{HSTRING, PCWSTR};
use windows::ApplicationModel::{StartupTask, StartupTaskState};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_SINGLETHREADED};
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::Shell::{ExtractIconExW, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::diagnose;
use crate::localization::{self, LanguageId, Strings};
#[cfg(test)]
use crate::models::UsageData;
use crate::models::GITHUB_COPILOT_MONTHLY_ITEM_ID;
use crate::models::{AppUsageData, BankedResetCount, QuotaFamilyId, QuotaMetric, UsageSection};
#[cfg(feature = "self-update")]
use crate::native_interop::TIMER_UPDATE_CHECK;
use crate::native_interop::{
    self, Color, TIMER_COUNTDOWN, TIMER_POLL, TIMER_RESET_POLL, TIMER_VIRTUAL_DESKTOP_EVENT_SETTLE,
    WM_APP_TRAY, WM_APP_USAGE_UPDATED,
};
use crate::poller;
use crate::snapshot_schema;
use crate::snapshot_store;
use crate::theme;
use crate::tray_icon;
#[cfg(feature = "self-update")]
use crate::updater::UpdateCheckResult;
use crate::updater::{self, InstallChannel, ReleaseDescriptor};
use crate::virtual_desktop::{self, VirtualDesktopScope};

/// Wrapper to make HWND sendable across threads (safe for PostMessage usage)
#[derive(Clone, Copy)]
struct SendHwnd(isize);

unsafe impl Send for SendHwnd {}

impl SendHwnd {
    fn from_hwnd(hwnd: HWND) -> Self {
        Self(hwnd.0 as isize)
    }
    fn to_hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HorizontalResizeEdge {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
struct HorizontalResizeSession {
    edge: HorizontalResizeEdge,
    start_cursor_screen_x: i32,
    start_window_rect: RECT,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointerInteractionTarget {
    HorizontalResize(HorizontalResizeEdge),
    HelpButton,
    HeaderDrag,
    AuthCta(AuthAction),
    None,
}

/// A provider's own official CLI login entry point, reached from this app's
/// auth-error status text — see AUTH-RECOVERY-CTA-IMPLEMENT-01. The app
/// never reads, stores, or refreshes any provider credential itself; it only
/// launches the provider's own command in a visible console and lets that
/// command's own (usually browser-based) OAuth flow run to completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthAction {
    ClaudeLogin,
    CodexLogin,
}

impl AuthAction {
    /// Fixed program + args for this action. Never built from runtime text,
    /// status wording, or any other non-constant input — see
    /// `launch_visible_auth_command`'s own doc comment for why that matters.
    const fn command(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Self::ClaudeLogin => ("claude", &["auth", "login"]),
            Self::CodexLogin => ("codex", &["login"]),
        }
    }

    fn launch(self, hwnd: HWND) {
        let (program, args) = self.command();
        if !launch_visible_auth_command(hwnd, program, args) {
            diagnose::log(format!("unable to launch auth command for {program}"));
        }
    }
}

/// Only a high-confidence auth-specific `CellState`, for a provider whose
/// official CLI has a documented, no-argument login entry point, becomes
/// clickable. `FetchFailed`/`NotAvailable`/`Disabled`/`Loading` — and every
/// state for GitHub Copilot/Antigravity — are never CTA-eligible:
/// GitHub Copilot's current failure signal can't distinguish "not logged
/// in" from "wrong token scope" from an unrelated API error (see
/// `run_gh_api`), and Antigravity's CLI has no login/auth subcommand at all
/// (confirmed against `agy --help` during AUTH-RECOVERY-CTA-DESIGN-01).
/// Generic failures are never guessed to be auth-related.
fn provider_auth_action(family: QuotaFamilyId, state: CellState) -> Option<AuthAction> {
    let is_auth_state = matches!(
        state,
        CellState::AuthenticationExpired
            | CellState::AuthenticationProblem
            | CellState::CredentialsUnavailable
    );
    if !is_auth_state {
        return None;
    }
    match family {
        QuotaFamilyId::Claude => Some(AuthAction::ClaudeLogin),
        QuotaFamilyId::Codex => Some(AuthAction::CodexLogin),
        QuotaFamilyId::Antigravity | QuotaFamilyId::GithubCopilot => None,
    }
}

/// One CTA-eligible status-text cell's screen (client-coordinate) rect from
/// the most recent paint, used only for pointer hit-testing/hover — rebuilt
/// from scratch every paint (see `paint_content`) so a stale rect never
/// outlives the layout it was measured from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AuthCtaHitTarget {
    rect: RECT,
    action: AuthAction,
}

/// Launches `program args...` in a new, visible `cmd.exe` console via
/// `ShellExecuteW` (the same pattern `open_readme` already uses for
/// browser links), and returns immediately without waiting for it —
/// `cmd.exe /k` leaves the console open so the user can see the provider
/// CLI's own login flow (typically opening a browser) and any error output,
/// and closes it manually when done. `program`/`args` must always be the
/// fixed, compile-time values from `AuthAction::command` — never built from
/// status text, provider display names, or any other runtime/user string —
/// so there is no shell-injection surface here.
///
/// A `true` return only means `cmd.exe` itself launched; it is NOT proof
/// that `program` exists. If `program` isn't on PATH, `cmd.exe` still opens
/// normally and simply prints its own "not recognized" error inside that
/// visible console — this app does not attempt to detect that case, and
/// does not fall back to reading any credential file if it happens.
fn launch_visible_auth_command(hwnd: HWND, program: &str, args: &[&str]) -> bool {
    let mut command_line = String::from("/k ");
    command_line.push_str(program);
    for arg in args {
        command_line.push(' ');
        command_line.push_str(arg);
    }
    unsafe {
        let operation = native_interop::wide_str("open");
        let target = native_interop::wide_str("cmd.exe");
        let parameters = native_interop::wide_str(&command_line);
        let result = ShellExecuteW(
            hwnd,
            PCWSTR::from_raw(operation.as_ptr()),
            PCWSTR::from_raw(target.as_ptr()),
            PCWSTR::from_raw(parameters.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        result.0 as isize > 32
    }
}

/// The `windows` crate only exposes this constant behind the
/// `Win32_UI_Controls` feature, which this crate doesn't otherwise need —
/// so it's inlined here as the documented, stable Win32 message value
/// (`WM_MOUSELEAVE` has been `0x02A3` since Windows 2000 and is part of the
/// public Win32 ABI) rather than adding a new crate feature for one constant.
const WM_MOUSELEAVE: u32 = 0x02A3;

fn track_mouse_leave(hwnd: HWND) {
    let mut tme = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe {
        let _ = TrackMouseEvent(&mut tme);
    }
}

/// Shared application state
struct AppState {
    hwnd: SendHwnd,
    taskbar_hwnd: Option<HWND>,
    tray_notify_hwnd: Option<HWND>,
    win_event_hook: Option<HWINEVENTHOOK>,
    is_dark: bool,
    embedded: bool,
    language_override: Option<LanguageId>,
    language: LanguageId,
    install_channel: InstallChannel,

    display_basis: DisplayBasis,
    reset_display_mode: ResetDisplayMode,
    display_density: DisplayDensity,
    short_window_visibility: ShortWindowVisibility,
    short_window_alert_sensitivity: ShortWindowAlertSensitivity,
    popup_layout: PopupLayout,
    app_theme: AppTheme,

    session_state: CellState,
    session_percent: Option<f64>,
    session_text: String,
    session_pace: Option<PaceGuidanceLines>,
    weekly_state: CellState,
    weekly_percent: Option<f64>,
    weekly_text: String,
    weekly_pace: Option<PaceGuidanceLines>,
    weekly_remaining_text: Option<String>,
    codex_session_state: CellState,
    codex_session_percent: Option<f64>,
    codex_session_text: String,
    codex_session_pace: Option<PaceGuidanceLines>,
    codex_weekly_state: CellState,
    codex_weekly_percent: Option<f64>,
    codex_weekly_text: String,
    codex_weekly_pace: Option<PaceGuidanceLines>,
    codex_weekly_remaining_text: Option<String>,
    codex_banked_reset_count: BankedResetCount,
    codex_banked_reset_text: String,
    antigravity_session_state: CellState,
    antigravity_session_percent: Option<f64>,
    antigravity_session_text: String,
    antigravity_session_pace: Option<PaceGuidanceLines>,
    antigravity_weekly_state: CellState,
    antigravity_weekly_percent: Option<f64>,
    antigravity_weekly_text: String,
    antigravity_weekly_pace: Option<PaceGuidanceLines>,
    antigravity_weekly_remaining_text: Option<String>,
    /// Every Antigravity cache item except `gemini-weekly` (which keeps its
    /// own dedicated weekly slot above, with pace guidance) — one persistent
    /// bar per item, so a cache key this build doesn't specifically
    /// recognize (e.g. `3p-weekly`, or any future key) still gets an
    /// always-visible row rather than being hidden in a details view.
    antigravity_extra_items: Vec<AntigravityItemDisplay>,
    github_copilot_state: CellState,
    github_copilot_percent: Option<f64>,
    github_copilot_text: String,
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    show_github_copilot: bool,
    github_copilot_plan: poller::GithubCopilotPlan,

    data: Option<AppUsageData>,

    poll_interval_ms: u32,
    retry_count: u32,
    force_notify_auth_error: bool,
    auth_error_paused_polling: bool,
    auth_watch_mode: poller::CredentialWatchMode,
    auth_watch_snapshot: poller::CredentialWatchSnapshot,
    last_poll_ok: bool,
    update_status: UpdateStatus,
    last_update_check_unix: Option<u64>,

    taskbar_index: usize,
    tray_offset: i32,
    dragging: bool,
    drag_start_mouse_x: i32,
    drag_start_mouse_y: i32,
    drag_start_window_x: i32,
    drag_start_window_y: i32,
    resize_session: Option<HorizontalResizeSession>,
    /// This paint's CTA-eligible status-text rects, rebuilt from scratch
    /// every `paint_content` call — see `AuthCtaHitTarget`'s own doc.
    auth_cta_hit_targets: Vec<AuthCtaHitTarget>,
    /// The exact rect (from `auth_cta_hit_targets`) currently under the
    /// cursor, if any — drives the hover-underline affordance. `None` means
    /// no CTA cell is hovered.
    hovered_auth_cta_rect: Option<RECT>,
    /// Saved free placement. `None` uses the selected monitor's bottom-right
    /// work-area corner; `Some((x, y))` is restored inside the nearest current
    /// work area. Cleared by position reset and by a taskbar drop.
    manual_position: Option<(i32, i32)>,
    /// User-selected width in 96-DPI logical pixels. `None` preserves the
    /// provider-count-based default until the user resizes.
    widget_width_logical: Option<i32>,

    widget_visible: bool,
    always_on_top: bool,

    /// Which virtual desktops the widget should be displayed on. See
    /// `virtual_desktop` for the public-API sensing mechanism behind
    /// `CurrentOnly` — app-level visibility synchronization, not native
    /// per-desktop `Show`/`Hide` enforcement (see that module's docs).
    virtual_desktop_scope: VirtualDesktopScope,
    /// The desktop `CurrentOnly` should show the widget on. `None` while
    /// `virtual_desktop_scope` is `All`.
    virtual_desktop_target: Option<GUID>,
    /// The last desktop id `window::resync_virtual_desktop` sensed and
    /// committed. `None` means unknown (sensor has not succeeded yet, the
    /// event hook failed to register, or scope is `All` and this is simply
    /// unused) — `effective_widget_visible` fails open to visible in that
    /// case, which is also this app's documented behavior when the
    /// system-wide cloak-event hook could not be registered at all (no
    /// polling fallback — see `virtual_desktop` module docs).
    virtual_desktop_current: Option<GUID>,
    /// The widget's actual, last-applied OS-level show/hide state driven by
    /// `effective_widget_visible` — kept separate from the `widget_visible`
    /// user preference so `sync_effective_widget_visibility` only calls
    /// `ShowWindow` when the effective state actually changes.
    virtual_desktop_applied_visible: bool,
    /// The system-wide `EVENT_OBJECT_CLOAKED`/`EVENT_OBJECT_UNCLOAKED` hook
    /// driving event-driven `CurrentOnly` sync — `None` whenever
    /// `CurrentOnly` is off, or registration failed (in which case this app
    /// never falls back to polling; see `virtual_desktop` module docs).
    virtual_desktop_event_hook: Option<HWINEVENTHOOK>,
}

#[derive(Clone, Debug)]
enum UpdateStatus {
    Idle,
    Checking,
    Applying,
    UpToDate,
    Available(ReleaseDescriptor),
}

/// User's chosen basis for the usage number and bar: how much of the quota
/// has been used, or how much is left. Only the AppState-level display copy
/// is affected — the internal `UsageSection::percentage` (always "used%")
/// from `poller`/`models`/`snapshot_schema` is never rewritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DisplayBasis {
    UsedPercentage,
    RemainingAllowance,
}

impl Default for DisplayBasis {
    fn default() -> Self {
        DisplayBasis::UsedPercentage
    }
}

/// Whether quota-window time is rendered relative to now or as the absolute
/// local reset date/time. The serialized names are retained for settings
/// compatibility even though the menu calls this "Time Display".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResetDisplayMode {
    Relative,
    AbsoluteDateTime,
}

impl Default for ResetDisplayMode {
    fn default() -> Self {
        Self::Relative
    }
}

/// How much pace/guidance detail the popup shows alongside each window's
/// percentage — see `paint_content`'s weekly secondary/detail lines
/// (AUM-PACE-GUIDANCE-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DisplayDensity {
    Compact,
    Standard,
    Detailed,
}

impl Default for DisplayDensity {
    fn default() -> Self {
        Self::Standard
    }
}

/// How much of the popup's row structure is shown at once — independent of
/// `DisplayDensity` (which only controls how much text each *shown* weekly
/// row carries). `Compact` keeps just the provider header and each shown
/// provider's weekly row/bar; `Standard` is the full existing layout (basis
/// label, weekly secondary/detail lines, 5h session row). Deliberately a
/// separate enum/type from `DisplayDensity` rather than reusing its
/// `Compact` variant, since the two are orthogonal settings that happen to
/// share a name in English.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PopupLayout {
    Compact,
    Standard,
}

impl Default for PopupLayout {
    fn default() -> Self {
        Self::Compact
    }
}

/// The popup's own color scheme — independent of `PopupLayout`/`DisplayDensity`
/// and, deliberately, of the OS light/dark setting (`AppState.is_dark`/
/// `theme::is_dark_mode`). See `popup_palette` for the actual color values;
/// this enum is just the user's selection. `HighVisibility` is this app's own
/// high-contrast palette, not an implementation of Windows' High
/// Contrast/Contrast Themes feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AppTheme {
    RecommendedDark,
    Light,
    HighVisibility,
}

impl Default for AppTheme {
    fn default() -> Self {
        Self::RecommendedDark
    }
}

/// Whether the 5h window is always shown, only shown while it's in an
/// overpacing warning state, or never shown — see `paint_content`'s
/// standalone 5h pace row (AUM-PACE-GUIDANCE-01).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ShortWindowVisibility {
    Always,
    WarningOnly,
    Hidden,
}

impl Default for ShortWindowVisibility {
    fn default() -> Self {
        Self::WarningOnly
    }
}

/// Availability/status of a single (provider, window) usage cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CellState {
    Loading,
    Ok,
    AuthenticationExpired,
    AuthenticationProblem,
    CredentialsUnavailable,
    FetchFailed,
    Disabled,
    NotAvailable,
}

/// What a single usage cell should render. `bar_percent` is `Some` only when
/// there is a real current value to draw — a normal 0% is `Some(0.0)`, while
/// loading/error/unconfigured/not-available states are `None` so the bar can
/// never show a stale or invented "current" fill. `text` always carries the
/// user-facing string (either the basis-formatted number, or a localized
/// status word).
struct CellDisplay {
    bar_percent: Option<f64>,
    text: String,
}

/// One persistent, always-visible row for an Antigravity quota item beyond
/// the dedicated `gemini-weekly` slot. `label` is whatever
/// `antigravity_statusline::quota_key_label` (or the raw cache key, for an
/// unrecognized one) produced for this item — never invented here, and
/// never panics on a key this build has never seen before.
#[derive(Clone, Debug, Default)]
struct AntigravityItemDisplay {
    label: String,
    percent: Option<f64>,
    text: String,
}

/// Convert an internal used-percentage into the value to show, for the
/// chosen basis. The same result feeds both the number and the bar fill
/// (via `CellDisplay::bar_percent`), so they can never disagree.
fn display_value(basis: DisplayBasis, used_percent: f64) -> f64 {
    let used = used_percent.clamp(0.0, 100.0);
    match basis {
        DisplayBasis::UsedPercentage => used,
        DisplayBasis::RemainingAllowance => (100.0 - used).clamp(0.0, 100.0),
    }
}

/// Countdown text for a reset time, computed directly from `resets_at`
/// rather than by reparsing `poller::format_line`'s output. This mirrors
/// `poller::format_countdown`'s day/hour/minute/second floor rounding
/// exactly (verified line-by-line against the current `src/poller.rs`; see
/// the completion report). That function is a private detail of
/// `poller::format_line` and isn't reachable from here without a
/// `src/poller.rs` visibility change, which is out of scope for this
/// change. `None` in, `None` out (no countdown shown). A past reset time
/// reuses the existing `strings.now` word, same as `poller.rs`.
fn countdown_text(resets_at: Option<SystemTime>, strings: Strings) -> Option<String> {
    let reset = resets_at?;
    let remaining = match reset.duration_since(SystemTime::now()) {
        Ok(d) => d,
        Err(_) => return Some(strings.now.to_string()),
    };

    let total_secs = remaining.as_secs();
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3600;
    let total_days = total_secs / 86400;

    Some(if total_days >= 1 {
        format!("{total_days}{}", strings.day_suffix)
    } else if total_hours >= 1 {
        format!("{total_hours}{}", strings.hour_suffix)
    } else if total_mins >= 1 {
        format!("{total_mins}{}", strings.minute_suffix)
    } else {
        format!("{total_secs}{}", strings.second_suffix)
    })
}

fn format_reset_time(duration: &str, strings: Strings) -> String {
    format!(
        "{}{}{}",
        strings.reset_in, strings.reset_in_separator, duration
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalDateTimeParts {
    year: u16,
    month: u16,
    day: u16,
    weekday: u16,
    hour: u16,
    minute: u16,
}

fn system_time_to_local_parts(time: SystemTime) -> Option<LocalDateTimeParts> {
    const WINDOWS_TO_UNIX_EPOCH_SECS: u64 = 11_644_473_600;
    const TICKS_PER_SEC: u64 = 10_000_000;

    let since_unix = time.duration_since(UNIX_EPOCH).ok()?;
    let ticks = since_unix
        .as_secs()
        .checked_add(WINDOWS_TO_UNIX_EPOCH_SECS)?
        .checked_mul(TICKS_PER_SEC)?
        .checked_add(u64::from(since_unix.subsec_nanos()) / 100)?;
    let file_time = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        FileTimeToSystemTime(&file_time, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some(LocalDateTimeParts {
        year: local.wYear,
        month: local.wMonth,
        day: local.wDay,
        weekday: local.wDayOfWeek,
        hour: local.wHour,
        minute: local.wMinute,
    })
}

fn fill_reset_datetime_template(
    template: &str,
    value: LocalDateTimeParts,
    time_text: &str,
    strings: Strings,
) -> String {
    let weekday = strings
        .weekday_short
        .get(value.weekday as usize)
        .copied()
        .unwrap_or("");
    template
        .replace("{year}", &value.year.to_string())
        .replace("{month}", &value.month.to_string())
        .replace("{day}", &value.day.to_string())
        .replace("{weekday}", weekday)
        .replace("{time}", time_text)
}

fn format_absolute_reset_parts(
    reset: LocalDateTimeParts,
    _now: LocalDateTimeParts,
    strings: Strings,
) -> String {
    let time_template = if reset.minute == 0 {
        strings.reset_time_hour
    } else {
        strings.reset_time_hour_minute
    };
    let time_text = time_template
        .replace("{hour}", &reset.hour.to_string())
        .replace("{minute}", &format!("{:02}", reset.minute));
    fill_reset_datetime_template(strings.reset_datetime_day, reset, &time_text, strings)
}

fn format_absolute_reset_time(
    resets_at: Option<SystemTime>,
    now: SystemTime,
    strings: Strings,
) -> Option<String> {
    let resets_at = resets_at?;
    if resets_at.duration_since(now).is_err() {
        return Some(strings.now.to_string());
    }
    let reset = system_time_to_local_parts(resets_at)?;
    let now = system_time_to_local_parts(now)?;
    Some(format_absolute_reset_parts(reset, now, strings))
}

fn format_reset_display(
    resets_at: Option<SystemTime>,
    now: SystemTime,
    mode: ResetDisplayMode,
    granularity: DurationGranularity,
    strings: Strings,
) -> Option<String> {
    match mode {
        ResetDisplayMode::Relative => {
            let remaining_secs = remaining_secs_at(resets_at, now)?;
            let duration = format_duration(remaining_secs, granularity, strings);
            Some(format_reset_time(&duration, strings))
        }
        ResetDisplayMode::AbsoluteDateTime => format_absolute_reset_time(resets_at, now, strings),
    }
}

/// The display-basis prefix ("Used"/"Remaining") is chosen once via the
/// settings menu (see `IDM_DISPLAY_BASIS_USED`/`IDM_DISPLAY_BASIS_REMAINING`)
/// and isn't echoed anywhere in the popup body (AUM-WINDOW-UI-01C-1 removed
/// the popup's own basis-label row), so this is just "<percent>%" optionally
/// followed by " · <reset-in word> <countdown>".
fn format_cell_text_with_reset_mode(
    basis: DisplayBasis,
    section: &UsageSection,
    reset_display_mode: ResetDisplayMode,
    strings: Strings,
) -> String {
    let pct = display_value(basis, section.percentage);
    let pct_text = format!("{pct:.0}%");
    let reset_text = match reset_display_mode {
        ResetDisplayMode::Relative => countdown_text(section.resets_at, strings)
            .map(|countdown| format_reset_time(&countdown, strings)),
        ResetDisplayMode::AbsoluteDateTime => {
            format_absolute_reset_time(section.resets_at, SystemTime::now(), strings)
        }
    };
    match reset_text {
        Some(reset_text) => format!("{pct_text} \u{00b7} {reset_text}"),
        None => pct_text,
    }
}

fn format_cell_text(basis: DisplayBasis, section: &UsageSection, strings: Strings) -> String {
    format_cell_text_with_reset_mode(basis, section, ResetDisplayMode::Relative, strings)
}

/// Localized status word for a non-`Ok` cell. `CellState::Ok` has no status
/// word of its own — a caller reaching this with `Ok` has already failed to
/// pair it with `Some(section)`, which is a caller bug, not a real "loading"
/// state; assert in debug builds and fail safe to "not available" text
/// rather than silently presenting it as ordinary loading.
fn status_text(state: CellState, strings: Strings) -> &'static str {
    match state {
        CellState::Loading => strings.loading,
        CellState::AuthenticationExpired => strings.authentication_expired,
        CellState::AuthenticationProblem => strings.authentication_problem,
        CellState::CredentialsUnavailable => strings.credentials_unavailable,
        CellState::FetchFailed => strings.fetch_failed,
        CellState::Disabled => strings.not_available,
        CellState::NotAvailable => strings.not_available,
        CellState::Ok => {
            debug_assert!(
                false,
                "status_text called with CellState::Ok (caller should pair Ok with Some(section))"
            );
            strings.not_available
        }
    }
}

fn format_banked_reset_text(count: BankedResetCount, strings: Strings) -> String {
    match count {
        BankedResetCount::Available(count) => format!("{}: {count}", strings.full_reset),
        BankedResetCount::Unavailable => {
            format!("{}: {}", strings.full_reset, strings.not_available)
        }
    }
}

fn render_cell_with_reset_mode(
    state: CellState,
    section: Option<&UsageSection>,
    basis: DisplayBasis,
    reset_display_mode: ResetDisplayMode,
    strings: Strings,
) -> CellDisplay {
    match (state, section) {
        (CellState::Ok, Some(section)) => CellDisplay {
            bar_percent: Some(display_value(basis, section.percentage)),
            text: format_cell_text_with_reset_mode(basis, section, reset_display_mode, strings),
        },
        _ => CellDisplay {
            bar_percent: None,
            text: status_text(state, strings).to_string(),
        },
    }
}

fn render_cell(
    state: CellState,
    section: Option<&UsageSection>,
    basis: DisplayBasis,
    strings: Strings,
) -> CellDisplay {
    render_cell_with_reset_mode(state, section, basis, ResetDisplayMode::Relative, strings)
}

fn quota_item_section(
    data: Option<&AppUsageData>,
    family_id: QuotaFamilyId,
    item_id: &str,
) -> Option<UsageSection> {
    let item = data?.family(family_id)?.item(item_id)?;
    Some(UsageSection {
        percentage: item.used_percentage()?,
        resets_at: item.resets_at,
    })
}

fn format_quota_number(value: f64) -> String {
    if (value - value.round()).abs() < 0.000_001 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

fn render_generic_quota_item_with_reset_mode(
    state: CellState,
    item: Option<&crate::models::QuotaItem>,
    basis: DisplayBasis,
    reset_display_mode: ResetDisplayMode,
    strings: Strings,
) -> CellDisplay {
    if state != CellState::Ok {
        return CellDisplay {
            bar_percent: None,
            text: status_text(state, strings).to_string(),
        };
    }
    let Some(item) = item else {
        return CellDisplay {
            bar_percent: None,
            text: strings.not_available.to_string(),
        };
    };
    if item.availability == crate::models::QuotaItemAvailability::Unavailable {
        return CellDisplay {
            bar_percent: None,
            text: strings.not_available.to_string(),
        };
    }

    let used_percent = item.used_percentage();
    let bar_percent = used_percent.map(|value| display_value(basis, value));
    let metric_text = match item.metric.as_ref() {
        Some(QuotaMetric::Percentage(value)) => {
            format!("{:.0}%", display_value(basis, *value))
        }
        Some(QuotaMetric::Used { used, limit }) => match (basis, limit) {
            (DisplayBasis::UsedPercentage, Some(limit)) => format!(
                "{} / {} {}",
                format_quota_number(*used),
                format_quota_number(*limit),
                item.unit.as_str()
            ),
            // The compact monthly row also carries reset time. Its bar
            // already communicates the value relative to the configured
            // plan limit, so keep the exact remaining amount and omit the
            // repeated denominator/unit from this fixed-width text cell.
            (DisplayBasis::RemainingAllowance, Some(limit)) => {
                format_quota_number((*limit - *used).max(0.0))
            }
            (_, None) => format!("{} {}", format_quota_number(*used), item.unit.as_str()),
        },
        Some(QuotaMetric::Remaining { remaining, limit }) => match (basis, limit) {
            (DisplayBasis::RemainingAllowance, Some(_)) => format_quota_number(*remaining),
            (DisplayBasis::UsedPercentage, Some(limit)) => format!(
                "{} / {} {}",
                format_quota_number((*limit - *remaining).max(0.0)),
                format_quota_number(*limit),
                item.unit.as_str()
            ),
            (_, None) => format!(
                "{} {} remaining",
                format_quota_number(*remaining),
                item.unit.as_str()
            ),
        },
        None => String::new(),
    };
    let time_text = match (reset_display_mode, basis) {
        (ResetDisplayMode::AbsoluteDateTime, _) => format_reset_display(
            item.resets_at,
            SystemTime::now(),
            reset_display_mode,
            DurationGranularity::LongWindow,
            strings,
        ),
        // Generic quota items do not carry a safely-derived window start.
        // Relative used-rate mode therefore has no safe time text.
        (ResetDisplayMode::Relative, DisplayBasis::UsedPercentage) => None,
        (ResetDisplayMode::Relative, DisplayBasis::RemainingAllowance) => format_reset_display(
            item.resets_at,
            SystemTime::now(),
            reset_display_mode,
            DurationGranularity::LongWindow,
            strings,
        ),
    };
    let text = match (metric_text.is_empty(), time_text) {
        (false, Some(time_text)) => format!("{metric_text} · {time_text}"),
        (false, None) => metric_text,
        (true, Some(time_text)) => time_text,
        (true, None) => strings.not_available.to_string(),
    };
    CellDisplay { bar_percent, text }
}

fn render_generic_quota_item(
    state: CellState,
    item: Option<&crate::models::QuotaItem>,
    basis: DisplayBasis,
    strings: Strings,
) -> CellDisplay {
    render_generic_quota_item_with_reset_mode(
        state,
        item,
        basis,
        ResetDisplayMode::Relative,
        strings,
    )
}

/// The `UsageSection` view of `gemini-weekly` used only for weekly pace
/// guidance and the compact "remaining" text — both of which are pure
/// `UsageSection`-shaped helpers that predate this provider's dynamic item
/// list. `None` whenever there is no `gemini-weekly` item, or it exists but
/// isn't `Available` (Stale — past its own reset boundary — or
/// Unavailable): pace guidance must never run against a pre-reset value
/// that `render_generic_quota_item_with_reset_mode` has already decided not
/// to show as current.
fn antigravity_weekly_pace_section(
    item: Option<&crate::models::QuotaItem>,
) -> Option<UsageSection> {
    let item = item?;
    if item.availability != crate::models::QuotaItemAvailability::Available {
        return None;
    }
    Some(UsageSection {
        percentage: item.used_percentage()?,
        resets_at: item.resets_at,
    })
}

/// Every Antigravity cache item except `gemini-weekly`, rendered via the
/// same generic per-item renderer GitHub Copilot's single custom item
/// already uses — safe for any `QuotaItemAvailability` (Available, Stale,
/// Unavailable) and any key this build doesn't specifically recognize,
/// since `item.label` was already resolved to a safe display label (or the
/// raw key) by `antigravity_statusline::quota_key_label` when the item was
/// built. `state` is the shared provider-level gate
/// (`antigravity_provider_cell_state`) — never per-item — since a
/// successful poll's per-item availability is what the renderer itself
/// checks.
fn antigravity_extra_items_from(
    items: &[crate::models::QuotaItem],
    state: CellState,
    basis: DisplayBasis,
    reset_display_mode: ResetDisplayMode,
    strings: Strings,
) -> Vec<AntigravityItemDisplay> {
    items
        .iter()
        .filter(|item| item.id != "gemini-weekly")
        .map(|item| {
            let display = render_generic_quota_item_with_reset_mode(
                state,
                Some(item),
                basis,
                reset_display_mode,
                strings,
            );
            AntigravityItemDisplay {
                label: item.label.clone(),
                percent: display.bar_percent,
                text: display.text,
            }
        })
        .collect()
}

/// Classify a just-completed provider poll into session/weekly cell states.
/// `Disabled` (provider not requested this poll) is not a normal render
/// target — it maps to `NotAvailable` rather than `Loading`, since it does
/// not mean "waiting for first data"; the genuine "never polled yet" state
/// is `CellState::Loading` set once at `AppState` construction and left
/// alone here.
fn provider_error_cell_state(provider: QuotaFamilyId, error: poller::PollError) -> CellState {
    match (provider, error) {
        (QuotaFamilyId::Claude, poller::PollError::TokenExpired) => {
            CellState::AuthenticationExpired
        }
        (
            QuotaFamilyId::Claude | QuotaFamilyId::Codex | QuotaFamilyId::Antigravity,
            poller::PollError::NoCredentials,
        ) => CellState::CredentialsUnavailable,
        (
            QuotaFamilyId::Claude | QuotaFamilyId::Codex | QuotaFamilyId::Antigravity,
            poller::PollError::AuthRequired,
        )
        | (QuotaFamilyId::Codex, poller::PollError::TokenExpired) => {
            CellState::AuthenticationProblem
        }
        _ => CellState::FetchFailed,
    }
}

fn poll_cell_states(
    provider: QuotaFamilyId,
    outcome: &poller::ProviderPollOutcome,
) -> (CellState, CellState) {
    match outcome {
        poller::ProviderPollOutcome::Success { usage, .. } => (
            if usage.session_available() {
                CellState::Ok
            } else {
                CellState::NotAvailable
            },
            if usage.weekly_available() {
                CellState::Ok
            } else {
                CellState::NotAvailable
            },
        ),
        poller::ProviderPollOutcome::Error { error, .. } => {
            let state = provider_error_cell_state(provider, *error);
            (state, state)
        }
        poller::ProviderPollOutcome::Disabled => (CellState::NotAvailable, CellState::NotAvailable),
    }
}

fn poll_quota_item_state(
    provider: QuotaFamilyId,
    outcome: &poller::ProviderPollOutcome,
    item_id: &str,
) -> CellState {
    match outcome {
        poller::ProviderPollOutcome::Success { usage, .. } => usage
            .quota_items()
            .into_iter()
            .find(|item| item.id == item_id)
            .filter(|item| item.availability != crate::models::QuotaItemAvailability::Unavailable)
            .map_or(CellState::NotAvailable, |_| CellState::Ok),
        poller::ProviderPollOutcome::Error { .. } => poll_cell_states(provider, outcome).0,
        poller::ProviderPollOutcome::Disabled => CellState::Disabled,
    }
}

/// The single provider-level gate used for every Antigravity display row
/// (the dedicated `gemini-weekly` slot and every extra-item row alike).
/// Antigravity's items are a dynamic, arbitrarily-keyed list rather than a
/// fixed session/weekly pair, so — unlike `poll_cell_states` — this never
/// inspects `UsageData::session_available`/`weekly_available` (always
/// `false` for `UsageData::from_quota_items`); a successful poll is always
/// `Ok` here, and each item's own `QuotaItemAvailability` (Available/
/// Stale/Unavailable) is what actually decides whether that specific row
/// shows a value, inside `render_generic_quota_item_with_reset_mode`.
fn antigravity_provider_cell_state(outcome: &poller::ProviderPollOutcome) -> CellState {
    match outcome {
        poller::ProviderPollOutcome::Success { .. } => CellState::Ok,
        poller::ProviderPollOutcome::Error { error, .. } => {
            provider_error_cell_state(QuotaFamilyId::Antigravity, *error)
        }
        poller::ProviderPollOutcome::Disabled => CellState::Disabled,
    }
}

fn banked_reset_count_for_poll(outcome: &poller::ProviderPollOutcome) -> BankedResetCount {
    match outcome {
        poller::ProviderPollOutcome::Success { usage, .. } => usage.banked_reset_count,
        poller::ProviderPollOutcome::Disabled | poller::ProviderPollOutcome::Error { .. } => {
            BankedResetCount::Unavailable
        }
    }
}

fn merge_successful_provider(
    data: &mut Option<AppUsageData>,
    provider: QuotaFamilyId,
    outcome: &poller::ProviderPollOutcome,
) {
    if let poller::ProviderPollOutcome::Success { usage, .. } = outcome {
        data.get_or_insert_with(AppUsageData::default)
            .upsert(usage.clone().into_quota_family(provider));
    }
}

/// Overwrite each provider's cached `UsageData` with this poll's result only
/// when that provider actually succeeded this round; a provider that didn't
/// succeed (error or disabled) keeps whatever was cached before. That old
/// value is never read once the corresponding `CellState` (set from the same
/// `report`, right alongside this call) is anything but `Ok` — see
/// `render_cell`. Called the same way from both the success and failure
/// branches of `do_poll` so "this provider is Ok" and "this provider's
/// displayed value is from an old poll" can never occur together, regardless
/// of ordering.
fn merge_successful_providers(data: &mut Option<AppUsageData>, report: &poller::PollReport) {
    // Preserve the existing final-report behavior where an all-error report
    // still initializes an empty cache before its error states are rendered.
    data.get_or_insert_with(AppUsageData::default);
    merge_successful_provider(data, QuotaFamilyId::Claude, &report.claude_code);
    merge_successful_provider(data, QuotaFamilyId::Codex, &report.codex);
    merge_successful_provider(data, QuotaFamilyId::Antigravity, &report.antigravity);
    merge_successful_provider(data, QuotaFamilyId::GithubCopilot, &report.github_copilot);
}

fn apply_provider_poll_update(
    state: &mut AppState,
    provider: QuotaFamilyId,
    outcome: &poller::ProviderPollOutcome,
) {
    match provider {
        QuotaFamilyId::Claude => {
            let (session_state, weekly_state) = poll_cell_states(provider, outcome);
            state.session_state = session_state;
            state.weekly_state = weekly_state;
        }
        QuotaFamilyId::Codex => {
            let (session_state, weekly_state) = poll_cell_states(provider, outcome);
            state.codex_session_state = session_state;
            state.codex_weekly_state = weekly_state;
            state.codex_banked_reset_count = banked_reset_count_for_poll(outcome);
        }
        QuotaFamilyId::Antigravity => {
            // `antigravity_session_state` has no real item to back it (no
            // 5h-scoped Antigravity quota key has ever been observed — see
            // `antigravity_statusline` module docs) and always renders
            // "not available"; kept only so that legacy slot doesn't get
            // stuck on its initial `Loading` value forever.
            state.antigravity_session_state = poll_cell_states(provider, outcome).0;
            state.antigravity_weekly_state = antigravity_provider_cell_state(outcome);
        }
        QuotaFamilyId::GithubCopilot => {
            state.github_copilot_state =
                poll_quota_item_state(provider, outcome, GITHUB_COPILOT_MONTHLY_ITEM_ID);
        }
    }
    merge_successful_provider(&mut state.data, provider, outcome);
    refresh_usage_texts(state);
}

// ── Weekly pace guidance (AUM-PACE-GUIDANCE-01) ───────────────────────────
//
// This section is pure logic only: no popup drawing, settings, or
// localization are wired to it yet.

/// Fixed window durations. `UsageSection` only carries `percentage` and an
/// absolute `resets_at`, never a window start time, so pace/guidance math
/// treats these as constants — the same assumption the existing "5h"/"7d"
/// row labels already make for every provider.
const SESSION_WINDOW_SECS: u64 = 5 * 3600;
const WEEKLY_WINDOW_SECS: u64 = 7 * 24 * 3600;

/// How far past a window's nominal length `resets_at` is still trusted as
/// "the window just started" (elapsed = 0) rather than rejected outright.
/// Covers small clock/poll skew between client and server without
/// pretending to know a real elapsed time beyond the window boundary.
const WINDOW_TIME_TOLERANCE_SECS: u64 = 5 * 60;

/// Below this much elapsed time into the window, pace is considered too
/// noisy to judge.
const PACE_JUDGING_MIN_ELAPSED_SECS: u64 = 6 * 3600;
/// pace_diff (used% − elapsed%) boundaries, in percentage points.
const PACE_UNDER_PACE_MAX_PT: f64 = -10.0;
const PACE_ON_TRACK_MAX_PT: f64 = 10.0;
const PACE_SLIGHTLY_OVER_MAX_PT: f64 = 25.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WeeklyPaceStatus {
    Judging,
    UnderPace,
    OnTrack,
    SlightlyOverpacing,
    Overpacing,
}

/// Seconds remaining until `resets_at`, or `None` if it's missing or already
/// past (can't safely derive elapsed/remaining from a stale or absent reset).
fn remaining_secs_at(resets_at: Option<SystemTime>, now: SystemTime) -> Option<u64> {
    let reset = resets_at?;
    reset.duration_since(now).ok().map(|d| d.as_secs())
}

/// Elapsed time into a fixed-length window, derived from time remaining
/// until reset:
/// - `remaining_secs <= window_secs`: the ordinary case.
/// - up to `WINDOW_TIME_TOLERANCE_SECS` past `window_secs`: treated as
///   elapsed = 0 (the window just started; small clock/poll skew).
/// - beyond that tolerance: `None` — not a value a real window can produce,
///   so no elapsed time is guessed.
fn elapsed_secs_in_window(remaining_secs: u64, window_secs: u64) -> Option<u64> {
    if remaining_secs <= window_secs {
        return Some(window_secs - remaining_secs);
    }
    let tolerated_max = window_secs.saturating_add(WINDOW_TIME_TOLERANCE_SECS);
    if remaining_secs <= tolerated_max {
        return Some(0);
    }
    None
}

/// `pace_diff = used% − elapsed%`, in percentage points. `0.0` when
/// `window_secs` is `0` (never a real window; callers that care already
/// guard on this before/alongside calling). Exposed separately from
/// `weekly_pace_status` so the "予定との差" (Detailed density) display can
/// show the raw point value without recomputing it.
fn weekly_pace_diff_pt(elapsed_secs: u64, window_secs: u64, used_percent: f64) -> f64 {
    if window_secs == 0 {
        return 0.0;
    }
    let elapsed_fraction = (elapsed_secs as f64 / window_secs as f64).clamp(0.0, 1.0);
    used_percent.clamp(0.0, 100.0) - elapsed_fraction * 100.0
}

/// Judged against fixed point-boundaries. Always resolves to a concrete
/// status (`Judging` included) given valid elapsed/window/used inputs;
/// callers gate on missing/unsafe reset data themselves (via
/// `remaining_secs_at`/`elapsed_secs_in_window`) before ever calling this.
fn weekly_pace_status(elapsed_secs: u64, window_secs: u64, used_percent: f64) -> WeeklyPaceStatus {
    if window_secs == 0 || elapsed_secs < PACE_JUDGING_MIN_ELAPSED_SECS {
        return WeeklyPaceStatus::Judging;
    }
    let pace_diff = weekly_pace_diff_pt(elapsed_secs, window_secs, used_percent);

    if pace_diff <= PACE_UNDER_PACE_MAX_PT {
        WeeklyPaceStatus::UnderPace
    } else if pace_diff <= PACE_ON_TRACK_MAX_PT {
        WeeklyPaceStatus::OnTrack
    } else if pace_diff <= PACE_SLIGHTLY_OVER_MAX_PT {
        WeeklyPaceStatus::SlightlyOverpacing
    } else {
        WeeklyPaceStatus::Overpacing
    }
}

/// Below this much time left, guidance switches from "%/day" to "%/hour".
/// Remaining time only ever counts down within a window (it jumps back up
/// once, at reset, to a fresh window) so this threshold is crossed at most
/// once per window — not something that flaps back and forth on its own.
const FUTURE_PACE_HOURLY_THRESHOLD_SECS: u64 = 24 * 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FuturePaceUnit {
    PerDay,
    PerHour,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FuturePaceGuidance {
    value: f64,
    unit: FuturePaceUnit,
}

/// "Remaining allowance ÷ time left", regardless of the user's chosen
/// display basis — this is a budget/burn-rate figure ("how much room is
/// left, spread over how much time is left"), not a restatement of the
/// current value, so it does not flip with `DisplayBasis`.
fn future_pace_guidance(used_percent: f64, remaining_secs: u64) -> Option<FuturePaceGuidance> {
    if remaining_secs == 0 {
        return None;
    }
    let remaining_percent = (100.0 - used_percent.clamp(0.0, 100.0)).max(0.0);
    if remaining_secs < FUTURE_PACE_HOURLY_THRESHOLD_SECS {
        let remaining_hours = remaining_secs as f64 / 3600.0;
        Some(FuturePaceGuidance {
            value: remaining_percent / remaining_hours,
            unit: FuturePaceUnit::PerHour,
        })
    } else {
        let remaining_days = remaining_secs as f64 / 86400.0;
        Some(FuturePaceGuidance {
            value: remaining_percent / remaining_days,
            unit: FuturePaceUnit::PerDay,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ShortWindowAlertSensitivity {
    Sensitive,
    Standard,
    Relaxed,
}

impl Default for ShortWindowAlertSensitivity {
    fn default() -> Self {
        ShortWindowAlertSensitivity::Standard
    }
}

struct ShortWindowAlertThresholds {
    grace_secs: u64,
    min_used_percent: f64,
    exhaustion_lead_secs: u64,
}

impl ShortWindowAlertSensitivity {
    fn thresholds(self) -> ShortWindowAlertThresholds {
        match self {
            ShortWindowAlertSensitivity::Sensitive => ShortWindowAlertThresholds {
                grace_secs: 20 * 60,
                min_used_percent: 30.0,
                exhaustion_lead_secs: 30 * 60,
            },
            ShortWindowAlertSensitivity::Standard => ShortWindowAlertThresholds {
                grace_secs: 30 * 60,
                min_used_percent: 40.0,
                exhaustion_lead_secs: 45 * 60,
            },
            ShortWindowAlertSensitivity::Relaxed => ShortWindowAlertThresholds {
                grace_secs: 45 * 60,
                min_used_percent: 50.0,
                exhaustion_lead_secs: 75 * 60,
            },
        }
    }
}

/// True only when all three conditions hold: past the grace period, past
/// the minimum used%, and — projecting the current average pace
/// (`used% / elapsed`) linearly forward — 100% is reached with at least
/// `exhaustion_lead_secs` to spare before the actual reset (boundary
/// inclusive throughout: `>=`, not `>`). Division is only reached once
/// `used > 0.0` is established; `used >= 100.0` is handled directly and
/// unconditionally (AUM-WINDOW-UI-01C-1-HF1: already-100%-used is a fact,
/// not a projection, so `exhaustion_lead_secs` — "warn this long *before*
/// the projected 100%" — has nothing left to gate on; gating it on
/// `remaining_secs` anyway meant a real 100%-used cell silently stopped
/// showing under `WarningOnly` once the window's reset drew within
/// `exhaustion_lead_secs`, even though 100% used must always be a warning
/// regardless of how much of the window is left) without going through the
/// projection at all.
/// Linear projection of "how many seconds from now until 100%, if the
/// current average pace (`used% / elapsed`) continues". `None` when the
/// projection isn't meaningful/safe: no usage yet or already at/over 100%
/// (`used` outside `(0, 100)`), no elapsed time to average over, or a
/// non-finite/negative result (clock skew, bad data). Shared by
/// `short_window_is_overpacing` (5h alert) and `weekly_exhaustion_lead_secs`
/// (7d Detailed-density display) so the projection math exists in one place.
fn projected_secs_to_exhaustion(elapsed_secs: u64, used_percent: f64) -> Option<f64> {
    let used = used_percent.clamp(0.0, 100.0);
    if used <= 0.0 || used >= 100.0 || elapsed_secs == 0 {
        return None;
    }
    let secs_to_exhaustion = (100.0 - used) * elapsed_secs as f64 / used;
    if !secs_to_exhaustion.is_finite() || secs_to_exhaustion < 0.0 {
        return None;
    }
    Some(secs_to_exhaustion)
}

fn short_window_is_overpacing(
    elapsed_secs: u64,
    remaining_secs: u64,
    used_percent: f64,
    sensitivity: ShortWindowAlertSensitivity,
) -> bool {
    let t = sensitivity.thresholds();

    if elapsed_secs < t.grace_secs {
        return false;
    }
    let used = used_percent.clamp(0.0, 100.0);
    if used < t.min_used_percent {
        return false;
    }
    if used >= 100.0 {
        return true;
    }

    let Some(secs_to_exhaustion) = projected_secs_to_exhaustion(elapsed_secs, used_percent) else {
        return false;
    };

    (remaining_secs as f64 - secs_to_exhaustion) >= t.exhaustion_lead_secs as f64
}

// ── Pace-guidance display model (AUM-PACE-GUIDANCE-01) ─────────────────────
//
// Pure text generation only: no Win32 types, no drawing calls, no
// `SystemTime::now()` (always taken as a `now` parameter). The resulting
// `PaceGuidanceLines` is drawn by `paint_content` (weekly secondary/detail
// lines and the standalone 5h pace row) — see the connection unit's
// completion report.

fn weekly_pace_status_text(status: WeeklyPaceStatus, strings: Strings) -> &'static str {
    match status {
        WeeklyPaceStatus::Judging => strings.weekly_pace_judging,
        WeeklyPaceStatus::UnderPace => strings.weekly_pace_under_pace,
        WeeklyPaceStatus::OnTrack => strings.weekly_pace_on_track,
        WeeklyPaceStatus::SlightlyOverpacing => strings.weekly_pace_slightly_overpacing,
        WeeklyPaceStatus::Overpacing => strings.weekly_pace_overpacing,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DurationGranularity {
    LongWindow,
    ShortWindow,
}

/// Window-aware duration text. Long windows use days + hours (or hours only),
/// while the 5h window uses hours + minutes (or minutes only). Seconds are
/// intentionally omitted from both directions of the time axis.
fn format_duration(total_secs: u64, granularity: DurationGranularity, strings: Strings) -> String {
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let minutes = (total_secs % 3600) / 60;
    match granularity {
        DurationGranularity::LongWindow if days >= 1 => {
            format!("{days}{}{hours}{}", strings.day_suffix, strings.hour_suffix)
        }
        DurationGranularity::LongWindow => format!("{hours}{}", strings.hour_suffix),
        DurationGranularity::ShortWindow if hours >= 1 => format!(
            "{hours}{}{minutes}{}",
            strings.hour_suffix, strings.minute_suffix
        ),
        DurationGranularity::ShortWindow => format!("{minutes}{}", strings.minute_suffix),
    }
}

/// Time-axis text for a fixed quota window. Relative mode follows the selected
/// quota basis (elapsed for used percentage, remaining until reset for
/// remaining allowance). Absolute mode replaces either relative form with the
/// local reset date/time. If the selected value cannot be derived, no time
/// text is shown.
fn format_window_time_with_reset_mode(
    basis: DisplayBasis,
    resets_at: Option<SystemTime>,
    now: SystemTime,
    remaining_secs: Option<u64>,
    elapsed_secs: Option<u64>,
    granularity: DurationGranularity,
    reset_display_mode: ResetDisplayMode,
    strings: Strings,
) -> Option<String> {
    match reset_display_mode {
        ResetDisplayMode::AbsoluteDateTime => format_absolute_reset_time(resets_at, now, strings),
        ResetDisplayMode::Relative => match basis {
            DisplayBasis::UsedPercentage => {
                let elapsed_secs = elapsed_secs?;
                let duration = format_duration(elapsed_secs, granularity, strings);
                Some(format!("{} {duration}", strings.elapsed))
            }
            DisplayBasis::RemainingAllowance => {
                let duration = format_duration(remaining_secs?, granularity, strings);
                Some(format_reset_time(&duration, strings))
            }
        },
    }
}

fn format_window_time(
    basis: DisplayBasis,
    remaining_secs: Option<u64>,
    elapsed_secs: Option<u64>,
    granularity: DurationGranularity,
    strings: Strings,
) -> Option<String> {
    format_window_time_with_reset_mode(
        basis,
        None,
        SystemTime::now(),
        remaining_secs,
        elapsed_secs,
        granularity,
        ResetDisplayMode::Relative,
        strings,
    )
}

/// 10+ as an integer, [1, 10) to one decimal, (0, 1) to two decimals —
/// keeps small rates (e.g. "0.35%/hour") from collapsing to "0" while
/// avoiding false precision on larger ones.
fn format_pace_rate_value(value: f64) -> String {
    if value >= 10.0 {
        format!("{value:.0}")
    } else if value >= 1.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

/// `None` for any non-finite or negative value — `future_pace_guidance`
/// should never actually produce one, but this is the last line of defense
/// against ever displaying "NaN%/day" or "-3%/day".
fn format_future_pace_guidance(guidance: &FuturePaceGuidance, strings: Strings) -> Option<String> {
    if !guidance.value.is_finite() || guidance.value < 0.0 {
        return None;
    }
    let unit_suffix = match guidance.unit {
        FuturePaceUnit::PerDay => strings.per_day_suffix,
        FuturePaceUnit::PerHour => strings.per_hour_suffix,
    };
    Some(format!(
        "{}%/{unit_suffix}",
        format_pace_rate_value(guidance.value)
    ))
}

/// "+19pt" / "-12pt" / "0pt". Rounds to whole points and normalizes a
/// rounded `-0.0` to `0.0` first so a near-zero negative `pace_diff` can
/// never print as "-0pt". `None` for non-finite input.
fn format_pace_diff_pt(pace_diff: f64) -> Option<String> {
    if !pace_diff.is_finite() {
        return None;
    }
    let mut rounded = pace_diff.round();
    if rounded == 0.0 {
        rounded = 0.0; // collapses -0.0 to +0.0 (IEEE 754 equality treats them equal)
    }
    Some(if rounded > 0.0 {
        format!("+{rounded:.0}pt")
    } else {
        format!("{rounded:.0}pt")
    })
}

/// For the Detailed-density weekly line: how long before the actual reset
/// the window is projected to hit 100%, if the current average pace
/// continues. `None` whenever this can't be shown safely — before the
/// pace-judging window opens, at 0% or 100%+ usage (nothing meaningful to
/// project, or already covered by the "使いすぎ" status word instead), or
/// when the projected exhaustion would land at/after the reset (not a
/// "before reset" warning scenario).
fn weekly_exhaustion_lead_secs(
    elapsed_secs: u64,
    remaining_secs: u64,
    used_percent: f64,
) -> Option<u64> {
    if elapsed_secs < PACE_JUDGING_MIN_ELAPSED_SECS {
        return None;
    }
    let secs_to_exhaustion = projected_secs_to_exhaustion(elapsed_secs, used_percent)?;
    let remaining = remaining_secs as f64;
    if secs_to_exhaustion >= remaining {
        return None;
    }
    Some((remaining - secs_to_exhaustion).round() as u64)
}

fn format_exhaustion_text(lead_secs: u64, strings: Strings) -> String {
    let duration = format_duration(lead_secs, DurationGranularity::LongWindow, strings);
    let before_reset = strings
        .exhaustion_before_reset
        .replace("{duration}", &duration);
    format!("{} {before_reset}", strings.exhaustion_label)
}

/// A ready-to-draw text block for one usage window's pace-guidance display.
/// Purely data — no Win32 types, no drawing. `is_warning` flags text that
/// should be visually distinguished later (currently only ever `true` for
/// the 5h window's overpacing line); it doesn't change what text is
/// produced here.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PaceGuidanceLines {
    primary: String,
    secondary: Option<String>,
    detail: Option<String>,
    is_warning: bool,
}

fn pace_basis_prefix(basis: DisplayBasis, strings: Strings) -> &'static str {
    match basis {
        DisplayBasis::UsedPercentage => strings.pace_used_prefix,
        DisplayBasis::RemainingAllowance => strings.pace_remaining_prefix,
    }
}

/// Builds the weekly (7d) window's pace-guidance display, or `None` when
/// `used_percent` itself is unknown or non-finite (nothing to show at all —
/// distinct from a known value with unusable reset data, which still shows
/// the current value alone). `now` is a parameter, never read internally.
fn weekly_pace_guidance_lines_with_reset_mode(
    used_percent: Option<f64>,
    resets_at: Option<SystemTime>,
    now: SystemTime,
    basis: DisplayBasis,
    density: DisplayDensity,
    reset_display_mode: ResetDisplayMode,
    strings: Strings,
) -> Option<PaceGuidanceLines> {
    let used_percent = used_percent?;
    if !used_percent.is_finite() {
        return None;
    }
    let used_percent = used_percent.clamp(0.0, 100.0);

    let display_pct = display_value(basis, used_percent);
    let pct_text = format!("{} {display_pct:.0}%", pace_basis_prefix(basis, strings));

    let remaining_secs = remaining_secs_at(resets_at, now);
    let elapsed_secs = remaining_secs.and_then(|r| elapsed_secs_in_window(r, WEEKLY_WINDOW_SECS));

    let (Some(remaining_secs), Some(elapsed_secs)) = (remaining_secs, elapsed_secs) else {
        // Missing/past/out-of-range reset data: current value only,
        // regardless of density.
        return Some(PaceGuidanceLines {
            primary: pct_text,
            secondary: None,
            detail: None,
            is_warning: false,
        });
    };

    let time_text = format_window_time_with_reset_mode(
        basis,
        resets_at,
        now,
        Some(remaining_secs),
        Some(elapsed_secs),
        DurationGranularity::LongWindow,
        reset_display_mode,
        strings,
    )?;

    if density == DisplayDensity::Compact {
        return Some(PaceGuidanceLines {
            primary: format!("{pct_text} \u{00b7} {time_text}"),
            secondary: None,
            detail: None,
            is_warning: false,
        });
    }

    let status = weekly_pace_status(elapsed_secs, WEEKLY_WINDOW_SECS, used_percent);
    let primary = if status == WeeklyPaceStatus::Judging {
        pct_text
    } else {
        format!("{pct_text} {}", weekly_pace_status_text(status, strings))
    };

    let future_pace_text = future_pace_guidance(used_percent, remaining_secs)
        .and_then(|guidance| format_future_pace_guidance(&guidance, strings));
    let secondary = Some(match future_pace_text {
        Some(future_text) => format!(
            "{time_text}\u{ff5c}{} {future_text}",
            strings.future_pace_label
        ),
        None => time_text,
    });

    if density == DisplayDensity::Standard {
        return Some(PaceGuidanceLines {
            primary,
            secondary,
            detail: None,
            is_warning: false,
        });
    }

    // Detailed: append the pace-diff and/or exhaustion projection, when
    // each can be computed safely; omit whichever can't rather than
    // guessing.
    let pace_diff = weekly_pace_diff_pt(elapsed_secs, WEEKLY_WINDOW_SECS, used_percent);
    let diff_text =
        format_pace_diff_pt(pace_diff).map(|d| format!("{} {d}", strings.pace_diff_label));
    // Keep calculating the projection for every status, but surface it only
    // once the existing weekly pace status is itself in a warning band. This
    // avoids presenting "On Track" or "Under Pace" alongside a projected
    // pre-reset exhaustion without changing either calculation.
    let exhaustion_lead_secs =
        weekly_exhaustion_lead_secs(elapsed_secs, remaining_secs, used_percent);
    let exhaustion_text = if matches!(
        status,
        WeeklyPaceStatus::SlightlyOverpacing | WeeklyPaceStatus::Overpacing
    ) {
        exhaustion_lead_secs.map(|lead_secs| format_exhaustion_text(lead_secs, strings))
    } else {
        None
    };

    let detail = match (diff_text, exhaustion_text) {
        (Some(d), Some(e)) => Some(format!("{d}\u{ff5c}{e}")),
        (Some(d), None) => Some(d),
        (None, Some(e)) => Some(e),
        (None, None) => None,
    };

    Some(PaceGuidanceLines {
        primary,
        secondary,
        detail,
        is_warning: false,
    })
}

fn weekly_pace_guidance_lines(
    used_percent: Option<f64>,
    resets_at: Option<SystemTime>,
    now: SystemTime,
    basis: DisplayBasis,
    density: DisplayDensity,
    strings: Strings,
) -> Option<PaceGuidanceLines> {
    weekly_pace_guidance_lines_with_reset_mode(
        used_percent,
        resets_at,
        now,
        basis,
        density,
        ResetDisplayMode::Relative,
        strings,
    )
}

/// Builds the short (5h) window's pace-guidance display, or `None` when it
/// should not be shown at all this poll — either `used_percent` is unknown,
/// `visibility` is `Hidden`, or `visibility` is `WarningOnly` and the window
/// isn't currently overpacing. Never shows `UnderPace`/`OnTrack`/
/// `SlightlyOverpacing` wording, matching the "5時間枠には...を表示しない"
/// requirement — only a plain value, or (when warranted) the same
/// "使いすぎ"/Overpacing word the weekly line uses.
fn short_window_pace_guidance_lines_with_reset_mode(
    used_percent: Option<f64>,
    resets_at: Option<SystemTime>,
    now: SystemTime,
    basis: DisplayBasis,
    visibility: ShortWindowVisibility,
    sensitivity: ShortWindowAlertSensitivity,
    reset_display_mode: ResetDisplayMode,
    strings: Strings,
) -> Option<PaceGuidanceLines> {
    if visibility == ShortWindowVisibility::Hidden {
        return None;
    }
    let used_percent = used_percent?;
    if !used_percent.is_finite() {
        return None;
    }
    let used_percent = used_percent.clamp(0.0, 100.0);

    let remaining_secs = remaining_secs_at(resets_at, now);
    let elapsed_secs = remaining_secs.and_then(|r| elapsed_secs_in_window(r, SESSION_WINDOW_SECS));

    let is_overpacing = match (elapsed_secs, remaining_secs) {
        (Some(elapsed), Some(remaining)) => {
            short_window_is_overpacing(elapsed, remaining, used_percent, sensitivity)
        }
        _ => false,
    };

    if visibility == ShortWindowVisibility::WarningOnly && !is_overpacing {
        return None;
    }

    let display_pct = display_value(basis, used_percent);
    let pct_text = format!("{} {display_pct:.0}%", pace_basis_prefix(basis, strings));
    let status_suffix = if is_overpacing {
        format!(" {}", strings.weekly_pace_overpacing)
    } else {
        String::new()
    };

    let time_text = format_window_time_with_reset_mode(
        basis,
        resets_at,
        now,
        remaining_secs,
        elapsed_secs,
        DurationGranularity::ShortWindow,
        reset_display_mode,
        strings,
    );
    let primary = match time_text {
        Some(time_text) => format!("{pct_text}{status_suffix} \u{00b7} {time_text}"),
        None => format!("{pct_text}{status_suffix}"),
    };

    Some(PaceGuidanceLines {
        primary,
        secondary: None,
        detail: None,
        is_warning: is_overpacing,
    })
}

fn short_window_pace_guidance_lines(
    used_percent: Option<f64>,
    resets_at: Option<SystemTime>,
    now: SystemTime,
    basis: DisplayBasis,
    visibility: ShortWindowVisibility,
    sensitivity: ShortWindowAlertSensitivity,
    strings: Strings,
) -> Option<PaceGuidanceLines> {
    short_window_pace_guidance_lines_with_reset_mode(
        used_percent,
        resets_at,
        now,
        basis,
        visibility,
        sensitivity,
        ResetDisplayMode::Relative,
        strings,
    )
}

/// Compact provider-header row's weekly-remaining text (AUM-WINDOW-UI-01C-1):
/// "<remaining prefix> <Xd Yh>", reusing the same `remaining_secs_at`/
/// `format_duration` primitives `weekly_pace_guidance_lines`
/// already uses for its own countdown text, and the existing
/// `pace_remaining_prefix` string rather than a new localization key — it
/// already reads naturally in front of a duration in every shipped
/// language. `None` whenever there's nothing safe to show: no reset time,
/// or the reset has already passed (both handled by `remaining_secs_at`
/// returning `None`). Takes `now` as a parameter rather than reading the
/// clock itself, same as `weekly_pace_guidance_lines`, so callers (and
/// tests) control "now" explicitly.
fn compact_weekly_remaining_text_with_reset_mode(
    resets_at: Option<SystemTime>,
    now: SystemTime,
    reset_display_mode: ResetDisplayMode,
    strings: Strings,
) -> Option<String> {
    match reset_display_mode {
        ResetDisplayMode::Relative => {
            let remaining_secs = remaining_secs_at(resets_at, now)?;
            Some(format!(
                "{} {}",
                strings.pace_remaining_prefix,
                format_duration(remaining_secs, DurationGranularity::LongWindow, strings)
            ))
        }
        ResetDisplayMode::AbsoluteDateTime => format_absolute_reset_time(resets_at, now, strings),
    }
}

fn compact_weekly_remaining_text(
    resets_at: Option<SystemTime>,
    now: SystemTime,
    strings: Strings,
) -> Option<String> {
    compact_weekly_remaining_text_with_reset_mode(
        resets_at,
        now,
        ResetDisplayMode::Relative,
        strings,
    )
}

/// One provider cell's Compact weekly-remaining text, gated the same way
/// `weekly_pace_for_cell` gates its own guidance text: only
/// `(CellState::Ok, Some(section))` produces anything. A cached `section`
/// surviving under a non-`Ok` state (loading/error/unconfigured/
/// not-available — see `merge_successful_providers`) must never reach the
/// popup, same as it never reaches the bar or the pace text.
fn compact_weekly_remaining_for_cell_with_reset_mode(
    state: CellState,
    section: Option<&UsageSection>,
    now: SystemTime,
    reset_display_mode: ResetDisplayMode,
    strings: Strings,
) -> Option<String> {
    match (state, section) {
        (CellState::Ok, Some(section)) => compact_weekly_remaining_text_with_reset_mode(
            section.resets_at,
            now,
            reset_display_mode,
            strings,
        ),
        _ => None,
    }
}

fn compact_weekly_remaining_for_cell(
    state: CellState,
    section: Option<&UsageSection>,
    now: SystemTime,
    strings: Strings,
) -> Option<String> {
    compact_weekly_remaining_for_cell_with_reset_mode(
        state,
        section,
        now,
        ResetDisplayMode::Relative,
        strings,
    )
}

/// Weekly pace guidance for one provider's cell, gated the same way
/// `render_cell` gates its bar: only `(CellState::Ok, Some(section))`
/// produces anything. A cached `section` surviving under a non-`Ok` state
/// (loading/error/unconfigured/not-available — see
/// `merge_successful_providers`) must never reach the guidance text, same as
/// it never reaches the bar.
fn weekly_pace_for_cell_with_reset_mode(
    state: CellState,
    section: Option<&UsageSection>,
    now: SystemTime,
    basis: DisplayBasis,
    density: DisplayDensity,
    reset_display_mode: ResetDisplayMode,
    strings: Strings,
) -> Option<PaceGuidanceLines> {
    match (state, section) {
        (CellState::Ok, Some(section)) => weekly_pace_guidance_lines_with_reset_mode(
            Some(section.percentage),
            section.resets_at,
            now,
            basis,
            density,
            reset_display_mode,
            strings,
        ),
        _ => None,
    }
}

fn weekly_pace_for_cell(
    state: CellState,
    section: Option<&UsageSection>,
    now: SystemTime,
    basis: DisplayBasis,
    density: DisplayDensity,
    strings: Strings,
) -> Option<PaceGuidanceLines> {
    weekly_pace_for_cell_with_reset_mode(
        state,
        section,
        now,
        basis,
        density,
        ResetDisplayMode::Relative,
        strings,
    )
}

/// Short (5h) window pace guidance for one provider's cell — same
/// `CellState::Ok`-gating as `weekly_pace_for_cell`.
fn session_pace_for_cell_with_reset_mode(
    state: CellState,
    section: Option<&UsageSection>,
    now: SystemTime,
    basis: DisplayBasis,
    visibility: ShortWindowVisibility,
    sensitivity: ShortWindowAlertSensitivity,
    reset_display_mode: ResetDisplayMode,
    strings: Strings,
) -> Option<PaceGuidanceLines> {
    match (state, section) {
        (CellState::Ok, Some(section)) => short_window_pace_guidance_lines_with_reset_mode(
            Some(section.percentage),
            section.resets_at,
            now,
            basis,
            visibility,
            sensitivity,
            reset_display_mode,
            strings,
        ),
        _ => None,
    }
}

fn session_pace_for_cell(
    state: CellState,
    section: Option<&UsageSection>,
    now: SystemTime,
    basis: DisplayBasis,
    visibility: ShortWindowVisibility,
    sensitivity: ShortWindowAlertSensitivity,
    strings: Strings,
) -> Option<PaceGuidanceLines> {
    session_pace_for_cell_with_reset_mode(
        state,
        section,
        now,
        basis,
        visibility,
        sensitivity,
        ResetDisplayMode::Relative,
        strings,
    )
}

const RETRY_BASE_MS: u32 = 30_000; // 30 seconds

const POLL_1_MIN: u32 = 60_000;
const POLL_5_MIN: u32 = 300_000;
const POLL_15_MIN: u32 = 900_000;
const POLL_1_HOUR: u32 = 3_600_000;

// Menu item IDs for update frequency
const IDM_FREQ_1MIN: u16 = 10;
const IDM_FREQ_5MIN: u16 = 11;
const IDM_FREQ_15MIN: u16 = 12;
const IDM_FREQ_1HOUR: u16 = 13;
const IDM_START_WITH_WINDOWS: u16 = 20;
const IDM_ALWAYS_ON_TOP: u16 = 32;
const IDM_RESET_POSITION: u16 = 30;
#[cfg(feature = "self-update")]
const IDM_VERSION_ACTION: u16 = 31;
const IDM_HELP_DISPLAY_GUIDE: u16 = 100;
const IDM_HELP_AI_QUOTAS: u16 = 101;
const IDM_HELP_README: u16 = 102;
const IDM_HELP_VERSION_INFORMATION: u16 = 103;
#[cfg(feature = "antigravity")]
const IDM_HELP_ANTIGRAVITY_SETUP: u16 = 104;
const IDM_LANG_SYSTEM: u16 = 40;
const IDM_LANG_ENGLISH: u16 = 41;
const IDM_LANG_DUTCH: u16 = 42;
const IDM_LANG_SPANISH: u16 = 43;
const IDM_LANG_FRENCH: u16 = 44;
const IDM_LANG_GERMAN: u16 = 45;
const IDM_LANG_JAPANESE: u16 = 46;
const IDM_LANG_KOREAN: u16 = 47;
const IDM_LANG_TRADITIONAL_CHINESE: u16 = 48;
const IDM_LANG_RUSSIAN: u16 = 49;
const IDM_LANG_PORTUGUESE_BRAZIL: u16 = 50;
const IDM_LANG_SIMPLIFIED_CHINESE: u16 = 51;
const IDM_MODEL_CLAUDE_CODE: u16 = 60;
const IDM_MODEL_CODEX: u16 = 61;
#[cfg(feature = "antigravity")]
const IDM_MODEL_ANTIGRAVITY: u16 = 62;
const IDM_MODEL_GITHUB_COPILOT: u16 = 63;
// 70 is `tray_icon::IDM_TOGGLE_WIDGET`, not redefined here — it shares the
// same `WM_COMMAND` id space as every constant in this block, so it must be
// treated as already taken (71 is skipped too, to leave no ambiguity next
// to it). See `wm_command_menu_ids_are_globally_unique` for the test that
// checks this across both files.
const IDM_DISPLAY_BASIS_USED: u16 = 72;
const IDM_DISPLAY_BASIS_REMAINING: u16 = 73;
// 74-79 intentionally left free.
const IDM_DISPLAY_DENSITY_COMPACT: u16 = 80;
const IDM_DISPLAY_DENSITY_STANDARD: u16 = 81;
const IDM_DISPLAY_DENSITY_DETAILED: u16 = 82;
const IDM_SHORT_WINDOW_VISIBILITY_ALWAYS: u16 = 83;
const IDM_SHORT_WINDOW_VISIBILITY_WARNING_ONLY: u16 = 84;
const IDM_SHORT_WINDOW_VISIBILITY_HIDDEN: u16 = 85;
const IDM_SHORT_WINDOW_ALERT_SENSITIVITY_SENSITIVE: u16 = 86;
const IDM_SHORT_WINDOW_ALERT_SENSITIVITY_STANDARD: u16 = 87;
const IDM_SHORT_WINDOW_ALERT_SENSITIVITY_RELAXED: u16 = 88;
const IDM_POPUP_LAYOUT_COMPACT: u16 = 89;
const IDM_POPUP_LAYOUT_STANDARD: u16 = 90;
const IDM_APP_THEME_RECOMMENDED_DARK: u16 = 91;
const IDM_APP_THEME_LIGHT: u16 = 92;
const IDM_APP_THEME_HIGH_VISIBILITY: u16 = 93;
const IDM_GITHUB_COPILOT_PLAN_UNKNOWN: u16 = 94;
const IDM_GITHUB_COPILOT_PLAN_PRO: u16 = 95;
const IDM_GITHUB_COPILOT_PLAN_PRO_PLUS: u16 = 96;
const IDM_GITHUB_COPILOT_PLAN_MAX: u16 = 97;
const IDM_RESET_DISPLAY_RELATIVE: u16 = 98;
const IDM_RESET_DISPLAY_ABSOLUTE: u16 = 99;
// 104-109 intentionally left free (100-103 are the Help submenu above).
const IDM_VIRTUAL_DESKTOP_SCOPE_ALL: u16 = 110;
const IDM_VIRTUAL_DESKTOP_SCOPE_CURRENT_ONLY: u16 = 111;

/// Pure `menu ID -> enum value` lookups, shared by `show_context_menu`
/// (which sets which item starts checked) and the `WM_COMMAND` handler
/// (which applies the selection). Kept separate from any Win32 call so both
/// directions of the ID/value mapping can be unit tested without a window.
fn display_density_for_menu_id(id: u16) -> Option<DisplayDensity> {
    match id {
        IDM_DISPLAY_DENSITY_COMPACT => Some(DisplayDensity::Compact),
        IDM_DISPLAY_DENSITY_STANDARD => Some(DisplayDensity::Standard),
        IDM_DISPLAY_DENSITY_DETAILED => Some(DisplayDensity::Detailed),
        _ => None,
    }
}

fn reset_display_mode_for_menu_id(id: u16) -> Option<ResetDisplayMode> {
    match id {
        IDM_RESET_DISPLAY_RELATIVE => Some(ResetDisplayMode::Relative),
        IDM_RESET_DISPLAY_ABSOLUTE => Some(ResetDisplayMode::AbsoluteDateTime),
        _ => None,
    }
}

fn short_window_visibility_for_menu_id(id: u16) -> Option<ShortWindowVisibility> {
    match id {
        IDM_SHORT_WINDOW_VISIBILITY_ALWAYS => Some(ShortWindowVisibility::Always),
        IDM_SHORT_WINDOW_VISIBILITY_WARNING_ONLY => Some(ShortWindowVisibility::WarningOnly),
        IDM_SHORT_WINDOW_VISIBILITY_HIDDEN => Some(ShortWindowVisibility::Hidden),
        _ => None,
    }
}

fn short_window_alert_sensitivity_for_menu_id(id: u16) -> Option<ShortWindowAlertSensitivity> {
    match id {
        IDM_SHORT_WINDOW_ALERT_SENSITIVITY_SENSITIVE => {
            Some(ShortWindowAlertSensitivity::Sensitive)
        }
        IDM_SHORT_WINDOW_ALERT_SENSITIVITY_STANDARD => Some(ShortWindowAlertSensitivity::Standard),
        IDM_SHORT_WINDOW_ALERT_SENSITIVITY_RELAXED => Some(ShortWindowAlertSensitivity::Relaxed),
        _ => None,
    }
}

fn popup_layout_for_menu_id(id: u16) -> Option<PopupLayout> {
    match id {
        IDM_POPUP_LAYOUT_COMPACT => Some(PopupLayout::Compact),
        IDM_POPUP_LAYOUT_STANDARD => Some(PopupLayout::Standard),
        _ => None,
    }
}

/// Same pure `menu ID -> enum value` mapping pattern as
/// `popup_layout_for_menu_id`, for `AppTheme`.
fn app_theme_for_menu_id(id: u16) -> Option<AppTheme> {
    match id {
        IDM_APP_THEME_RECOMMENDED_DARK => Some(AppTheme::RecommendedDark),
        IDM_APP_THEME_LIGHT => Some(AppTheme::Light),
        IDM_APP_THEME_HIGH_VISIBILITY => Some(AppTheme::HighVisibility),
        _ => None,
    }
}

/// Same pure `menu ID -> enum value` mapping pattern as
/// `popup_layout_for_menu_id`, for `VirtualDesktopScope`.
fn virtual_desktop_scope_for_menu_id(id: u16) -> Option<VirtualDesktopScope> {
    match id {
        IDM_VIRTUAL_DESKTOP_SCOPE_ALL => Some(VirtualDesktopScope::All),
        IDM_VIRTUAL_DESKTOP_SCOPE_CURRENT_ONLY => Some(VirtualDesktopScope::CurrentOnly),
        _ => None,
    }
}

fn github_copilot_plan_for_menu_id(id: u16) -> Option<poller::GithubCopilotPlan> {
    match id {
        IDM_GITHUB_COPILOT_PLAN_UNKNOWN => Some(poller::GithubCopilotPlan::Unknown),
        IDM_GITHUB_COPILOT_PLAN_PRO => Some(poller::GithubCopilotPlan::Pro),
        IDM_GITHUB_COPILOT_PLAN_PRO_PLUS => Some(poller::GithubCopilotPlan::ProPlus),
        IDM_GITHUB_COPILOT_PLAN_MAX => Some(poller::GithubCopilotPlan::Max),
        _ => None,
    }
}

fn apply_github_copilot_plan_selection(
    show_github_copilot: &mut bool,
    github_copilot_plan: &mut poller::GithubCopilotPlan,
    plan: poller::GithubCopilotPlan,
) {
    *show_github_copilot = true;
    *github_copilot_plan = plan;
}

const WM_DPICHANGED_MSG: u32 = 0x02E0;
#[cfg(feature = "self-update")]
const WM_APP_UPDATE_CHECK_COMPLETE: u32 = WM_APP + 2;
const WM_APP_DEFERRED_TRAY_TOGGLE: u32 = WM_APP + 4;
const TRAY_ICON_UPDATE_REPOSITION_SUPPRESS_MS: u64 = 750;

/// How often the watchdog thread polls for an explorer.exe restart (which
/// recreates the taskbar and wipes our tray-icon registration).
const TASKBAR_WATCH_INTERVAL_SECS: u64 = 2;

static SUPPRESS_TRAY_REPOSITION_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// Current system DPI (96 = 100% scaling, 144 = 150%, 192 = 200%, etc.)
static CURRENT_DPI: AtomicU32 = AtomicU32::new(96);

/// Scale a base pixel value (designed at 96 DPI) to the current DPI.
fn sc(px: i32) -> i32 {
    let dpi = CURRENT_DPI.load(Ordering::Relaxed);
    (px as f64 * dpi as f64 / 96.0).round() as i32
}

/// Re-query the monitor DPI for our window and update the cached value.
/// Uses GetDpiForWindow which returns the live DPI (unlike GetDpiForSystem
/// which is cached at process startup and never changes).
fn refresh_dpi() {
    let hwnd = {
        let state = lock_state();
        state.as_ref().map(|s| s.hwnd.to_hwnd())
    };
    if let Some(hwnd) = hwnd {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi > 0 {
            CURRENT_DPI.store(dpi, Ordering::Relaxed);
        }
    }
}

/// Spacing below which two relaunches are treated as a storm (e.g. explorer.exe
/// crash-looping); when detected we back off instead of spawning in a tight loop.
const RELAUNCH_THROTTLE_SECS: u64 = 10;
const RELAUNCH_BACKOFF_SECS: u64 = 30;
/// Environment flag set on a relaunched child so it waits for the previous
/// instance's single-instance mutex instead of exiting immediately.
const ENV_RELAUNCH: &str = "CCUM_RELAUNCH";
/// Unix timestamp (seconds) of the relaunch that spawned this process, passed to
/// the child so it can detect a relaunch storm.
const ENV_LAST_RELAUNCH_UNIX: &str = "CCUM_LAST_RELAUNCH_UNIX";

/// Relaunch the widget as a fresh process after explorer.exe has restarted.
///
/// The popup window itself is top-level and survives an explorer.exe
/// restart, but the taskbar/tray notification area it was tracking does
/// not: the system tray icon needs to be re-added, and the taskbar anchor
/// (used for tray-relative X positioning) needs to be re-resolved against
/// the freshly created taskbar. Spawning a clean new process - which
/// re-registers the tray icon and re-selects the taskbar anchor on startup
/// - is the simplest robust recovery. The child is flagged via
/// `ENV_RELAUNCH` so it waits for this instance's single-instance mutex to
/// be released before taking over (see the guard in `run`).
fn relaunch_self() {
    // Back off if we are relaunching very soon after the relaunch that spawned
    // us: that signals the shell is crash-looping, not a one-off restart.
    let now = now_unix_secs();
    let last = std::env::var(ENV_LAST_RELAUNCH_UNIX)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if last != 0 && now.saturating_sub(last) < RELAUNCH_THROTTLE_SECS {
        diagnose::log("relaunch storm detected; backing off before relaunching");
        std::thread::sleep(Duration::from_secs(RELAUNCH_BACKOFF_SECS));
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            diagnose::log_error("watchdog: unable to resolve current executable", error);
            return;
        }
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    match std::process::Command::new(exe)
        .args(&args)
        .env(ENV_RELAUNCH, "1")
        .env(ENV_LAST_RELAUNCH_UNIX, now.to_string())
        .spawn()
    {
        Ok(_) => {
            diagnose::log("watchdog: relaunched fresh instance, exiting old one");
            std::process::exit(0);
        }
        Err(error) => {
            diagnose::log_error("watchdog: unable to spawn relaunched instance", error);
        }
    }
}

/// Detect explorer.exe restarts and recover from them.
///
/// When explorer.exe restarts, the old taskbar HWND becomes invalid and a
/// new one is created; our tray icon registration and taskbar anchor go
/// stale along with it. This dedicated thread polls the taskbar handle and,
/// when it changes, relaunches the widget as a fresh process to re-register
/// the tray icon and re-select the taskbar anchor.
fn spawn_taskbar_watchdog() {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(TASKBAR_WATCH_INTERVAL_SECS));
        let stored = {
            let state = lock_state();
            state.as_ref().and_then(|s| s.taskbar_hwnd)
        };
        // Only relevant once we have selected a taskbar anchor at least once.
        let Some(old) = stored else {
            continue;
        };
        let taskbars = native_interop::find_taskbars();
        if !taskbars.is_empty() && !taskbars.iter().any(|taskbar| taskbar.hwnd == old) {
            let new = taskbars[0].hwnd;
            diagnose::log(format!(
                "watchdog: taskbar changed old={:?} new={:?} -> relaunching",
                old.0, new.0
            ));
            relaunch_self();
        }
    });
}

fn load_embedded_app_icons() -> (HICON, HICON) {
    unsafe {
        let mut exe_buf = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
        if len == 0 {
            return (HICON::default(), HICON::default());
        }

        let mut large_icon = HICON::default();
        let mut small_icon = HICON::default();
        let extracted = ExtractIconExW(
            PCWSTR::from_raw(exe_buf.as_ptr()),
            0,
            Some(&mut large_icon),
            Some(&mut small_icon),
            1,
        );

        if extracted == 0 {
            (HICON::default(), HICON::default())
        } else {
            (large_icon, small_icon)
        }
    }
}

unsafe impl Send for AppState {}

static STATE: Mutex<Option<AppState>> = Mutex::new(None);

/// Lock STATE safely, recovering from poisoned mutex
fn lock_state() -> MutexGuard<'static, Option<AppState>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn settings_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata)
        .join("ClaudeCodeUsageMonitor")
        .join("settings.json")
}

#[derive(Debug, Serialize, Deserialize)]
struct SettingsFile {
    #[serde(default)]
    tray_offset: i32,
    #[serde(default)]
    taskbar_index: usize,
    #[serde(default = "default_poll_interval")]
    poll_interval_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_update_check_unix: Option<u64>,
    #[serde(default = "default_widget_visible")]
    widget_visible: bool,
    #[serde(default)]
    always_on_top: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    widget_width_logical: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    manual_x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    manual_y: Option<i32>,
    #[serde(default = "default_show_claude_code")]
    show_claude_code: bool,
    #[serde(default = "default_show_codex")]
    show_codex: bool,
    #[serde(default = "default_show_antigravity")]
    show_antigravity: bool,
    #[serde(default)]
    show_github_copilot: bool,
    #[serde(default)]
    github_copilot_plan: poller::GithubCopilotPlan,
    #[serde(default, deserialize_with = "deserialize_display_basis")]
    display_basis: DisplayBasis,
    #[serde(default, deserialize_with = "deserialize_reset_display_mode")]
    reset_display_mode: ResetDisplayMode,
    #[serde(default, deserialize_with = "deserialize_display_density")]
    display_density: DisplayDensity,
    #[serde(default, deserialize_with = "deserialize_short_window_visibility")]
    short_window_visibility: ShortWindowVisibility,
    #[serde(
        default,
        deserialize_with = "deserialize_short_window_alert_sensitivity"
    )]
    short_window_alert_sensitivity: ShortWindowAlertSensitivity,
    #[serde(default, deserialize_with = "deserialize_popup_layout")]
    popup_layout: PopupLayout,
    #[serde(default, deserialize_with = "deserialize_app_theme")]
    app_theme: AppTheme,
    #[serde(default, deserialize_with = "deserialize_virtual_desktop_scope")]
    virtual_desktop_scope: VirtualDesktopScope,
    /// The virtual desktop id `virtual_desktop_scope` = `CurrentOnly` is
    /// pinned to, stored as `GUID::to_u128` for stable, serde-friendly
    /// round-tripping. `None` means `All`, or a `CurrentOnly` target that
    /// has not been resolved yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    virtual_desktop_target: Option<u128>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            tray_offset: 0,
            taskbar_index: 0,
            poll_interval_ms: default_poll_interval(),
            language: None,
            last_update_check_unix: None,
            widget_visible: true,
            always_on_top: false,
            widget_width_logical: None,
            manual_x: None,
            manual_y: None,
            show_claude_code: true,
            show_codex: false,
            show_antigravity: false,
            show_github_copilot: false,
            github_copilot_plan: poller::GithubCopilotPlan::Unknown,
            display_basis: DisplayBasis::default(),
            reset_display_mode: ResetDisplayMode::default(),
            display_density: DisplayDensity::default(),
            short_window_visibility: ShortWindowVisibility::default(),
            short_window_alert_sensitivity: ShortWindowAlertSensitivity::default(),
            popup_layout: PopupLayout::default(),
            app_theme: AppTheme::default(),
            virtual_desktop_scope: VirtualDesktopScope::default(),
            virtual_desktop_target: None,
        }
    }
}

/// Falls back to the default basis for any value this build doesn't
/// recognize (e.g. a newer settings.json written by a future version),
/// rather than letting one unrecognized field fail the whole `SettingsFile`
/// parse and reset every other saved preference back to default.
fn deserialize_display_basis<'de, D>(deserializer: D) -> Result<DisplayBasis, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(serde_json::Value::deserialize(deserializer)
        .ok()
        .and_then(|value| serde_json::from_value::<DisplayBasis>(value).ok())
        .unwrap_or_default())
}

fn deserialize_reset_display_mode<'de, D>(deserializer: D) -> Result<ResetDisplayMode, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(serde_json::Value::deserialize(deserializer)
        .ok()
        .and_then(|value| serde_json::from_value::<ResetDisplayMode>(value).ok())
        .unwrap_or_default())
}

/// Same lenient fallback as `deserialize_display_basis`, for `DisplayDensity`.
fn deserialize_display_density<'de, D>(deserializer: D) -> Result<DisplayDensity, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(serde_json::Value::deserialize(deserializer)
        .ok()
        .and_then(|value| serde_json::from_value::<DisplayDensity>(value).ok())
        .unwrap_or_default())
}

/// Same lenient fallback as `deserialize_display_basis`, for
/// `ShortWindowVisibility`.
fn deserialize_short_window_visibility<'de, D>(
    deserializer: D,
) -> Result<ShortWindowVisibility, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(serde_json::Value::deserialize(deserializer)
        .ok()
        .and_then(|value| serde_json::from_value::<ShortWindowVisibility>(value).ok())
        .unwrap_or_default())
}

/// Same lenient fallback as `deserialize_display_basis`, for
/// `ShortWindowAlertSensitivity`.
fn deserialize_short_window_alert_sensitivity<'de, D>(
    deserializer: D,
) -> Result<ShortWindowAlertSensitivity, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(serde_json::Value::deserialize(deserializer)
        .ok()
        .and_then(|value| serde_json::from_value::<ShortWindowAlertSensitivity>(value).ok())
        .unwrap_or_default())
}

/// Same lenient fallback as `deserialize_display_basis`, for `PopupLayout`.
fn deserialize_popup_layout<'de, D>(deserializer: D) -> Result<PopupLayout, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(serde_json::Value::deserialize(deserializer)
        .ok()
        .and_then(|value| serde_json::from_value::<PopupLayout>(value).ok())
        .unwrap_or_default())
}

/// Same lenient fallback as `deserialize_display_basis`, for `AppTheme`.
fn deserialize_app_theme<'de, D>(deserializer: D) -> Result<AppTheme, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(serde_json::Value::deserialize(deserializer)
        .ok()
        .and_then(|value| serde_json::from_value::<AppTheme>(value).ok())
        .unwrap_or_default())
}

/// Same lenient fallback as `deserialize_display_basis`, for
/// `VirtualDesktopScope`.
fn deserialize_virtual_desktop_scope<'de, D>(
    deserializer: D,
) -> Result<VirtualDesktopScope, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(serde_json::Value::deserialize(deserializer)
        .ok()
        .and_then(|value| serde_json::from_value::<VirtualDesktopScope>(value).ok())
        .unwrap_or_default())
}

fn default_poll_interval() -> u32 {
    POLL_15_MIN
}

fn default_widget_visible() -> bool {
    true
}

fn default_show_claude_code() -> bool {
    true
}

fn default_show_codex() -> bool {
    false
}

fn default_show_antigravity() -> bool {
    false
}

fn load_settings() -> SettingsFile {
    let content = match std::fs::read_to_string(settings_path()) {
        Ok(c) => c,
        Err(_) => return SettingsFile::default(),
    };
    let mut settings: SettingsFile = serde_json::from_str(&content).unwrap_or_default();
    settings.widget_width_logical = normalize_saved_widget_width(settings.widget_width_logical);
    normalize_github_copilot_settings(&mut settings);
    #[cfg(not(feature = "antigravity"))]
    {
        settings.show_antigravity = false;
    }
    if !settings.show_claude_code
        && !settings.show_codex
        && !settings.show_antigravity
        && !settings.show_github_copilot
    {
        settings.show_claude_code = true;
    }
    settings
}

fn normalize_saved_widget_width(width: Option<i32>) -> Option<i32> {
    width.map(|width| width.clamp(1, MAX_WIDGET_WIDTH_LOGICAL))
}

fn normalize_github_copilot_settings(settings: &mut SettingsFile) {
    if settings.github_copilot_plan != poller::GithubCopilotPlan::Unknown {
        settings.show_github_copilot = true;
    }
}

fn save_settings(settings: &SettingsFile) {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(path, json);
    }
}

fn save_state_settings() {
    let state = lock_state();
    if let Some(s) = state.as_ref() {
        save_settings(&SettingsFile {
            tray_offset: s.tray_offset,
            taskbar_index: s.taskbar_index,
            poll_interval_ms: s.poll_interval_ms,
            language: s
                .language_override
                .map(|language| language.code().to_string()),
            last_update_check_unix: s.last_update_check_unix,
            widget_visible: s.widget_visible,
            always_on_top: s.always_on_top,
            widget_width_logical: s.widget_width_logical,
            manual_x: s.manual_position.map(|position| position.0),
            manual_y: s.manual_position.map(|position| position.1),
            show_claude_code: s.show_claude_code,
            show_codex: s.show_codex,
            show_antigravity: s.show_antigravity,
            show_github_copilot: s.show_github_copilot,
            github_copilot_plan: s.github_copilot_plan,
            display_basis: s.display_basis,
            reset_display_mode: s.reset_display_mode,
            display_density: s.display_density,
            short_window_visibility: s.short_window_visibility,
            short_window_alert_sensitivity: s.short_window_alert_sensitivity,
            popup_layout: s.popup_layout,
            app_theme: s.app_theme,
            virtual_desktop_scope: s.virtual_desktop_scope,
            virtual_desktop_target: s.virtual_desktop_target.map(|guid| guid.to_u128()),
        });
    }
}

/// How long to wait, after the *last* `EVENT_OBJECT_CLOAKED`/`UNCLOAKED`
/// event of a desktop-switch burst, before doing the one-shot confirmation
/// resync — this is the *only* resync for the whole burst (see
/// `system_wide_cloak_event_proc`; there is deliberately no separate
/// immediate resync). A burst as a whole spans ~170-260ms (dozens of
/// events, every window on the system), but measured live, the
/// *adjacent-event* gap within one burst never exceeded 14ms — waiting for
/// the whole burst span before confirming (an earlier 250ms) was adding
/// real, felt latency on top of the burst itself for no benefit. 80ms is a
/// ~5.7x margin over that 14ms maximum. A live A/B check against 40ms (a
/// ~2.9x margin) found no perceptible difference in felt latency, so 80ms
/// is kept as the final value — the extra margin is free (no felt cost)
/// and safer against timing jitter this session's live data didn't cover.
const VIRTUAL_DESKTOP_EVENT_SETTLE_MS: u32 = 80;

/// Pure visibility rule combining the user's "Show Widget" preference with
/// the virtual-desktop scope — the single place both are reconciled, so
/// every caller (startup, the menu, the poll timer) applies the identical
/// rule. `current_desktop` being `None` (sensor has not yet succeeded, or
/// hasn't run) fails open to visible: a transient sensing gap must never
/// hide the widget on its own.
fn effective_widget_visible(
    widget_visible: bool,
    scope: VirtualDesktopScope,
    current_desktop: Option<GUID>,
    target_desktop: Option<GUID>,
) -> bool {
    if !widget_visible {
        return false;
    }
    match scope {
        VirtualDesktopScope::All => true,
        VirtualDesktopScope::CurrentOnly => match (current_desktop, target_desktop) {
            (Some(current), Some(target)) => current == target,
            _ => true,
        },
    }
}

/// Applies `effective_widget_visible`'s result to the real window, reusing
/// the same safe show/hide path as the user's own "Show Widget" toggle
/// (`toggle_widget_visibility`) — but never touches the `widget_visible`
/// preference itself. Only calls into Win32 when the effective state
/// actually changed since the last call (`virtual_desktop_applied_visible`
/// cache), so the poll timer does not re-issue `ShowWindow` every tick.
fn sync_effective_widget_visibility(hwnd: HWND) {
    let (should_show, already_applied, always_on_top) = {
        let state = lock_state();
        match state.as_ref() {
            Some(s) => {
                let should_show = effective_widget_visible(
                    s.widget_visible,
                    s.virtual_desktop_scope,
                    s.virtual_desktop_current,
                    s.virtual_desktop_target,
                );
                (
                    should_show,
                    s.virtual_desktop_applied_visible,
                    s.always_on_top,
                )
            }
            None => return,
        }
    };
    if should_show == already_applied {
        return;
    }
    unsafe {
        if should_show {
            position_at_taskbar();
            show_widget_without_activation(hwnd, always_on_top);
            render_layered();
            finalize_widget_show_z_order(hwnd, always_on_top);
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
    let mut state = lock_state();
    if let Some(s) = state.as_mut() {
        s.virtual_desktop_applied_visible = should_show;
    }
}

/// Re-senses the current desktop (see
/// `virtual_desktop::sense_current_desktop_id`) and applies effective
/// visibility immediately. Called only on demand — at startup, on a
/// `CurrentOnly`/`All` scope change, and by the event-driven trigger below
/// — never on a periodic timer. A sensor failure (`None`) leaves
/// `virtual_desktop_current` untouched entirely, never treated as "hide",
/// per `effective_widget_visible`'s fail-open rule.
fn resync_virtual_desktop(hwnd: HWND) {
    let Some(sensed) = virtual_desktop::sense_current_desktop_id(hwnd) else {
        return;
    };
    {
        let mut state = lock_state();
        let Some(s) = state.as_mut() else {
            return;
        };
        s.virtual_desktop_current = Some(sensed);
    }
    sync_effective_widget_visibility(hwnd);
}

/// `WinEventProc` for the system-wide `EVENT_OBJECT_CLOAKED`/`UNCLOAKED`
/// hook — see `virtual_desktop` module docs for why this is system-wide
/// rather than scoped to any one window (no sentinel window exists to
/// scope it to). One real desktop switch produces a burst of dozens of
/// these events (every window on the system, cloaked and uncloaked
/// together), so every event in a burst just (re-)arms the one-shot
/// `TIMER_VIRTUAL_DESKTOP_EVENT_SETTLE` timer — coalescing the whole burst
/// into exactly one confirmation resync once it settles, never one resync
/// per event and never a periodic poll. Deliberately does *not* resync
/// immediately on the burst's first event — an earlier version did, but
/// that could sense a transient, mid-switch-animation desktop id and
/// briefly show/hide the widget incorrectly before the settle
/// confirmation corrected it; confirmed live as a real, noticeable flash.
///
/// Filters to `idObject == OBJID_WINDOW`/`idChild == CHILDID_SELF`: the
/// whole-window cloak transition, not the flood of child/control-level
/// object events the same hook also delivers.
unsafe extern "system" fn system_wide_cloak_event_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _event_hwnd: HWND,
    idobject: i32,
    idchild: i32,
    _dweventthread: u32,
    _dwmseventtime: u32,
) {
    if idobject != OBJID_WINDOW.0 || idchild != CHILDID_SELF as i32 {
        return;
    }

    let main_hwnd = {
        let state = lock_state();
        state.as_ref().map(|s| s.hwnd.to_hwnd())
    };
    let Some(main_hwnd) = main_hwnd else {
        return;
    };

    // Every event in a burst only (re-)arms the one-shot settle timer —
    // deliberately no immediate resync here. An earlier version resynced
    // on the burst's first event too, but that could sense a transient,
    // mid-switch-animation desktop id and briefly apply the wrong
    // visibility before the settle confirmation corrected it a moment
    // later — confirmed live as a real, noticeable flash. Waiting for the
    // burst to settle is the only resync, at the cost of up to
    // `VIRTUAL_DESKTOP_EVENT_SETTLE_MS` of extra latency.
    unsafe {
        SetTimer(
            main_hwnd,
            TIMER_VIRTUAL_DESKTOP_EVENT_SETTLE,
            VIRTUAL_DESKTOP_EVENT_SETTLE_MS,
            None,
        );
    }
}

/// `WM_TIMER` handler for `TIMER_VIRTUAL_DESKTOP_EVENT_SETTLE`. Always
/// `KillTimer`'d immediately (this is a one-shot "burst has settled"
/// signal, re-armed by every event during a burst — see
/// `system_wide_cloak_event_proc` — never a repeating poll), then does the
/// final confirmation resync.
fn handle_virtual_desktop_event_settle(hwnd: HWND) {
    unsafe {
        let _ = KillTimer(hwnd, TIMER_VIRTUAL_DESKTOP_EVENT_SETTLE);
    }
    resync_virtual_desktop(hwnd);
}

/// Registers the system-wide cloak-event hook driving event-driven
/// `CurrentOnly` sync — see `virtual_desktop` module docs. On failure,
/// logs and leaves `virtual_desktop_event_hook` as `None`; this app never
/// falls back to polling in that case, relying instead on
/// `effective_widget_visible`'s existing fail-open rule (an unknown
/// current desktop always shows the widget).
fn start_virtual_desktop_event_sync() {
    let hook = native_interop::set_system_wide_cloak_event_hook(system_wide_cloak_event_proc);
    if hook.is_none() {
        diagnose::log(
            "virtual_desktop: system-wide cloak event hook registration failed; \
             CurrentOnly fails open to all-desktops visible (no polling fallback)",
        );
    }
    let mut state = lock_state();
    if let Some(s) = state.as_mut() {
        s.virtual_desktop_event_hook = hook;
    }
}

/// Tears down the event-driven sync hook and any pending settle timer —
/// safe to call unconditionally (a no-op when nothing is active).
fn stop_virtual_desktop_event_sync(hwnd: HWND) {
    let hook = {
        let mut state = lock_state();
        state
            .as_mut()
            .and_then(|s| s.virtual_desktop_event_hook.take())
    };
    if let Some(hook) = hook {
        native_interop::unhook_win_event(hook);
    }
    unsafe {
        let _ = KillTimer(hwnd, TIMER_VIRTUAL_DESKTOP_EVENT_SETTLE);
    }
}

/// Applied once at startup (`run`), after the main window and `AppState`
/// exist. If the saved scope is `CurrentOnly`, senses the current desktop
/// once (so the widget doesn't wait for the first cloak event to get its
/// first visibility decision), starts the event-driven sync hook, and
/// applies effective visibility immediately. Never touches the widget's
/// window style or calls `MoveWindowToDesktop` — see the `virtual_desktop`
/// module docs for why.
fn apply_virtual_desktop_scope_at_startup(hwnd: HWND) {
    let scope = {
        let state = lock_state();
        match state.as_ref() {
            Some(s) => s.virtual_desktop_scope,
            None => return,
        }
    };
    if scope != VirtualDesktopScope::CurrentOnly {
        return;
    }
    let sensed = virtual_desktop::sense_current_desktop_id(hwnd);
    {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            s.virtual_desktop_current = sensed;
        }
    }
    start_virtual_desktop_event_sync();
    sync_effective_widget_visibility(hwnd);
}

/// Handle a "Virtual desktop" submenu selection.
///
/// `CurrentOnly` senses the current desktop (via
/// `virtual_desktop::sense_current_desktop_id`, the same public-API-only
/// sensor `resync_virtual_desktop` uses — never `MoveWindowToDesktop`) and
/// saves it as the target. This also covers re-targeting: selecting
/// `CurrentOnly` again while already in that scope, from a different
/// desktop than the current target, simply re-senses and overwrites the
/// target with wherever the menu was actually opened from.
///
/// `All` stops the event-driven sync and clears the target; the widget's
/// window itself was never moved or restyled for `CurrentOnly`, so there
/// is nothing to undo and no process relaunch is needed.
fn apply_virtual_desktop_scope_from_menu(hwnd: HWND, new_scope: VirtualDesktopScope) {
    match new_scope {
        VirtualDesktopScope::CurrentOnly => {
            let Some(current) = virtual_desktop::sense_current_desktop_id(hwnd) else {
                diagnose::log(
                    "virtual_desktop: could not sense the current desktop; leaving scope unchanged",
                );
                return;
            };
            {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    s.virtual_desktop_scope = VirtualDesktopScope::CurrentOnly;
                    s.virtual_desktop_target = Some(current);
                    s.virtual_desktop_current = Some(current);
                }
            }
            save_state_settings();
            start_virtual_desktop_event_sync();
            sync_effective_widget_visibility(hwnd);
        }
        VirtualDesktopScope::All => {
            let already_all = {
                let state = lock_state();
                state
                    .as_ref()
                    .map(|s| s.virtual_desktop_scope == VirtualDesktopScope::All)
                    .unwrap_or(true)
            };
            if already_all {
                return;
            }
            stop_virtual_desktop_event_sync(hwnd);
            {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    s.virtual_desktop_scope = VirtualDesktopScope::All;
                    s.virtual_desktop_target = None;
                    s.virtual_desktop_current = None;
                }
            }
            save_state_settings();
            sync_effective_widget_visibility(hwnd);
        }
    }
}

fn app_tray_icon_data(strings: Strings) -> Vec<tray_icon::TrayIconData> {
    vec![tray_icon::TrayIconData {
        kind: tray_icon::TrayIconKind::App,
        percent: None,
        tooltip: strings.window_title.to_string(),
    }]
}

fn tray_icon_data_from_state() -> Vec<tray_icon::TrayIconData> {
    let state = lock_state();
    state
        .as_ref()
        .map(|s| app_tray_icon_data(s.language.strings()))
        .unwrap_or_default()
}

fn sync_tray_icons(hwnd: HWND) {
    let icons = tray_icon_data_from_state();
    tray_icon::sync(hwnd, &icons);
}

/// Apply (or remove) the topmost z-order per the "always on top" preference.
/// Always explicit about both directions (TOPMOST/NOTOPMOST) so an OFF
/// preference can't leave a stale topmost z-order in place.
fn apply_always_on_top(hwnd: HWND, always_on_top: bool) {
    let insert_after = if always_on_top {
        HWND_TOPMOST
    } else {
        HWND_NOTOPMOST
    };
    set_z_order_without_activation(hwnd, insert_after);
}

fn set_z_order_without_activation(hwnd: HWND, insert_after: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

fn finalize_widget_show_z_order(hwnd: HWND, always_on_top: bool) {
    if always_on_top {
        set_z_order_without_activation(hwnd, HWND_TOPMOST);
    } else {
        // A newly shown window needs to enter the topmost band once to become
        // visible, then immediately returns to the normal z-order band.
        set_z_order_without_activation(hwnd, HWND_TOPMOST);
        set_z_order_without_activation(hwnd, HWND_NOTOPMOST);
    }
}

fn show_widget_without_activation(hwnd: HWND, always_on_top: bool) {
    let insert_after = if always_on_top {
        HWND_TOPMOST
    } else {
        HWND_TOP
    };
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

/// Flips the "Show Widget" preference and applies its effect through
/// `effective_widget_visible`/`sync_effective_widget_visibility`, so a
/// `CurrentOnly` scope still keeps the widget hidden on a non-target
/// desktop even when the user turns this preference back on. Also starts
/// or stops the event-driven sync hook per the "no unnecessary event
/// subscription while the widget is off" rule — see the `virtual_desktop`
/// module docs — and, when turning back on, senses the current desktop
/// immediately rather than waiting for the first cloak event.
fn toggle_widget_visibility(hwnd: HWND) {
    let (new_visible, scope) = {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            s.widget_visible = !s.widget_visible;
            (s.widget_visible, s.virtual_desktop_scope)
        } else {
            return;
        }
    };
    save_state_settings();
    if scope == VirtualDesktopScope::CurrentOnly {
        if new_visible {
            if let Some(sensed) = virtual_desktop::sense_current_desktop_id(hwnd) {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    s.virtual_desktop_current = Some(sensed);
                }
            }
            start_virtual_desktop_event_sync();
        } else {
            stop_virtual_desktop_event_sync(hwnd);
        }
    }
    sync_effective_widget_visibility(hwnd);
}

/// Locate the taskbar to anchor the top-level popup against (tray-relative
/// X position, work-area-relative Y position). This does not reparent the
/// window or change its style - the popup always remains a top-level,
/// topmost window positioned above the taskbar.
fn select_taskbar_anchor(requested_index: usize) -> bool {
    let taskbars = native_interop::find_taskbars();
    if taskbars.is_empty() {
        diagnose::log("no taskbar found; popup will use its default position");
        return false;
    }

    let index = requested_index.min(taskbars.len().saturating_sub(1));
    let taskbar = taskbars[index];
    diagnose::log(format!(
        "taskbar selected index={index} count={} hwnd={:?} rect=({}, {}, {}, {})",
        taskbars.len(),
        taskbar.hwnd,
        taskbar.rect.left,
        taskbar.rect.top,
        taskbar.rect.right,
        taskbar.rect.bottom
    ));

    let old_hook = {
        let mut state = lock_state();
        state.as_mut().and_then(|s| s.win_event_hook.take())
    };
    if let Some(hook) = old_hook {
        native_interop::unhook_win_event(hook);
    }

    let tray_notify = native_interop::find_child_window(taskbar.hwnd, "TrayNotifyWnd");
    if tray_notify.is_some() {
        diagnose::log("TrayNotifyWnd found");
    } else {
        diagnose::log("TrayNotifyWnd not found");
    }

    let hook = tray_notify.and_then(|tray_hwnd| {
        let thread_id = native_interop::get_window_thread_id(tray_hwnd);
        native_interop::set_tray_event_hook(thread_id, on_tray_location_changed)
    });
    if hook.is_some() {
        diagnose::log("tray event hook installed");
    } else {
        diagnose::log("tray event hook could not be installed");
    }

    let mut state = lock_state();
    if let Some(s) = state.as_mut() {
        s.taskbar_hwnd = Some(taskbar.hwnd);
        s.tray_notify_hwnd = tray_notify;
        s.win_event_hook = hook;
        s.taskbar_index = index;
    }
    true
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(feature = "self-update")]
fn update_check_interval() -> Duration {
    Duration::from_secs(24 * 60 * 60)
}

#[cfg(feature = "self-update")]
fn auto_update_check_due(last_update_check_unix: Option<u64>) -> bool {
    let Some(last_update_check_unix) = last_update_check_unix else {
        return true;
    };

    now_unix_secs().saturating_sub(last_update_check_unix) >= update_check_interval().as_secs()
}

#[cfg(feature = "self-update")]
fn schedule_auto_update_check(hwnd: HWND) {
    let delay_ms = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };

        if auto_update_check_due(s.last_update_check_unix) {
            None
        } else {
            let elapsed = now_unix_secs().saturating_sub(s.last_update_check_unix.unwrap_or(0));
            let remaining_secs = update_check_interval().as_secs().saturating_sub(elapsed);
            Some((remaining_secs.saturating_mul(1000)).min(u32::MAX as u64) as u32)
        }
    };

    unsafe {
        let _ = KillTimer(hwnd, TIMER_UPDATE_CHECK);
        if let Some(delay_ms) = delay_ms {
            SetTimer(hwnd, TIMER_UPDATE_CHECK, delay_ms.max(1), None);
        }
    }
}

/// Recompute the bar-fill value and display text for every cell from the
/// currently cached poll data (`state.data`), each cell's already-determined
/// `CellState` (set only by `do_poll`, untouched here), the current display
/// basis, and the current language. Safe to call on a countdown tick, a
/// display-basis change, or a language change — none of those change what
/// data is available, only how it should be shown right now. A non-`Ok`
/// cell's `section` lookup is irrelevant (`render_cell` ignores it), which
/// is what keeps a stale cached percentage from ever being drawn once a
/// poll has marked that cell as failed/loading/unavailable.
fn refresh_usage_texts(state: &mut AppState) {
    let strings = state.language.strings();
    let basis = state.display_basis;
    let reset_display_mode = state.reset_display_mode;
    let density = state.display_density;
    let visibility = state.short_window_visibility;
    let sensitivity = state.short_window_alert_sensitivity;
    // Captured once so the weekly/5h pace guidance for every provider in
    // this refresh agrees on "now" — see AUM-PACE-GUIDANCE-01's
    // `weekly_pace_guidance_lines`/`short_window_pace_guidance_lines`, which
    // deliberately take `now` as a parameter rather than reading the clock
    // themselves.
    let now = SystemTime::now();
    let data = state.data.as_ref();

    let claude_session = quota_item_section(data, QuotaFamilyId::Claude, "session");
    let claude_weekly = quota_item_section(data, QuotaFamilyId::Claude, "weekly");
    let session = render_cell_with_reset_mode(
        state.session_state,
        claude_session.as_ref(),
        basis,
        reset_display_mode,
        strings,
    );
    state.session_percent = session.bar_percent;
    state.session_text = session.text;
    state.session_pace = session_pace_for_cell_with_reset_mode(
        state.session_state,
        claude_session.as_ref(),
        now,
        basis,
        visibility,
        sensitivity,
        reset_display_mode,
        strings,
    );
    let weekly = render_cell_with_reset_mode(
        state.weekly_state,
        claude_weekly.as_ref(),
        basis,
        reset_display_mode,
        strings,
    );
    state.weekly_percent = weekly.bar_percent;
    state.weekly_text = weekly.text;
    state.weekly_pace = weekly_pace_for_cell_with_reset_mode(
        state.weekly_state,
        claude_weekly.as_ref(),
        now,
        basis,
        density,
        reset_display_mode,
        strings,
    );
    state.weekly_remaining_text = compact_weekly_remaining_for_cell_with_reset_mode(
        state.weekly_state,
        claude_weekly.as_ref(),
        now,
        reset_display_mode,
        strings,
    );

    let codex_session_section = quota_item_section(data, QuotaFamilyId::Codex, "session");
    let codex_weekly_section = quota_item_section(data, QuotaFamilyId::Codex, "weekly");
    let codex_session = render_cell_with_reset_mode(
        state.codex_session_state,
        codex_session_section.as_ref(),
        basis,
        reset_display_mode,
        strings,
    );
    state.codex_session_percent = codex_session.bar_percent;
    state.codex_session_text = codex_session.text;
    state.codex_session_pace = session_pace_for_cell_with_reset_mode(
        state.codex_session_state,
        codex_session_section.as_ref(),
        now,
        basis,
        visibility,
        sensitivity,
        reset_display_mode,
        strings,
    );
    let codex_weekly = render_cell_with_reset_mode(
        state.codex_weekly_state,
        codex_weekly_section.as_ref(),
        basis,
        reset_display_mode,
        strings,
    );
    state.codex_weekly_percent = codex_weekly.bar_percent;
    state.codex_weekly_text = codex_weekly.text;
    state.codex_weekly_pace = weekly_pace_for_cell_with_reset_mode(
        state.codex_weekly_state,
        codex_weekly_section.as_ref(),
        now,
        basis,
        density,
        reset_display_mode,
        strings,
    );
    state.codex_weekly_remaining_text = compact_weekly_remaining_for_cell_with_reset_mode(
        state.codex_weekly_state,
        codex_weekly_section.as_ref(),
        now,
        reset_display_mode,
        strings,
    );
    state.codex_banked_reset_text =
        format_banked_reset_text(state.codex_banked_reset_count, strings);

    // Antigravity's items are a dynamic, arbitrarily-keyed list (from the
    // official statusLine cache — see `antigravity_statusline` module docs),
    // not a fixed session/weekly pair, so `state.antigravity_weekly_state`
    // (really: the provider-level Ok/error/disabled gate, computed by
    // `antigravity_provider_cell_state`) is reused for every item's `state`
    // argument below; each item's own `QuotaItemAvailability` is what
    // decides whether that specific row shows a value.
    let antigravity_session = render_cell_with_reset_mode(
        state.antigravity_session_state,
        None,
        basis,
        reset_display_mode,
        strings,
    );
    state.antigravity_session_percent = antigravity_session.bar_percent;
    state.antigravity_session_text = antigravity_session.text;
    state.antigravity_session_pace = None;

    let antigravity_family = data.and_then(|d| d.family(QuotaFamilyId::Antigravity));
    let antigravity_weekly_item = antigravity_family.and_then(|f| f.item("gemini-weekly"));
    let antigravity_weekly = render_generic_quota_item_with_reset_mode(
        state.antigravity_weekly_state,
        antigravity_weekly_item,
        basis,
        reset_display_mode,
        strings,
    );
    state.antigravity_weekly_percent = antigravity_weekly.bar_percent;
    state.antigravity_weekly_text = antigravity_weekly.text;
    let antigravity_weekly_section = antigravity_weekly_pace_section(antigravity_weekly_item);
    state.antigravity_weekly_pace = weekly_pace_for_cell_with_reset_mode(
        state.antigravity_weekly_state,
        antigravity_weekly_section.as_ref(),
        now,
        basis,
        density,
        reset_display_mode,
        strings,
    );
    state.antigravity_weekly_remaining_text = compact_weekly_remaining_for_cell_with_reset_mode(
        state.antigravity_weekly_state,
        antigravity_weekly_section.as_ref(),
        now,
        reset_display_mode,
        strings,
    );
    state.antigravity_extra_items = antigravity_extra_items_from(
        antigravity_family
            .map(|f| f.items.as_slice())
            .unwrap_or(&[]),
        state.antigravity_weekly_state,
        basis,
        reset_display_mode,
        strings,
    );

    let github_copilot_item = data
        .and_then(|data| data.family(QuotaFamilyId::GithubCopilot))
        .and_then(|family| family.item(GITHUB_COPILOT_MONTHLY_ITEM_ID));
    let github_copilot = render_generic_quota_item_with_reset_mode(
        state.github_copilot_state,
        github_copilot_item,
        basis,
        reset_display_mode,
        strings,
    );
    state.github_copilot_percent = github_copilot.bar_percent;
    state.github_copilot_text = github_copilot.text;
}

fn set_window_title(hwnd: HWND, strings: Strings) {
    unsafe {
        let title = native_interop::wide_str(strings.window_title);
        let _ = SetWindowTextW(hwnd, PCWSTR::from_raw(title.as_ptr()));
    }
}

fn show_info_message(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let title_wide = native_interop::wide_str(title);
        let message_wide = native_interop::wide_str(message);
        let _ = MessageBoxW(
            hwnd,
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

const README_URL: &str = "https://github.com/ysawase/ai-usage-monitor#readme";

/// The exact `/statusline <command>` this app's own packaged App Execution
/// Alias needs, verified against a real installed MSIX — see
/// `ANTIGRAVITY-PACKAGING-BRIDGE-FIX-01` and `docs/quota-rules.md`. Not
/// localized: it is a literal command, not user-facing prose.
const ANTIGRAVITY_SETUP_COMMAND: &str = "/statusline aum-quota.exe antigravity-statusline-bridge";
/// Restores Antigravity's built-in status line. Distinct from this app's
/// own `show_antigravity` display toggle — turning Antigravity display off
/// in this app never runs this on the user's behalf.
const ANTIGRAVITY_DISABLE_COMMAND: &str = "/statusline delete";

/// Copies `text` to the clipboard as plain Unicode text. Best-effort: a
/// failure at any step (open, alloc, lock, set) is reported back as `false`
/// so the caller can still show the command as plain text for the user to
/// select manually, but never panics and never leaves the clipboard
/// partially open.
fn copy_text_to_clipboard(hwnd: HWND, text: &str) -> bool {
    const CF_UNICODETEXT: u32 = 13;

    unsafe {
        if OpenClipboard(hwnd).is_err() {
            return false;
        }

        let copied = (|| -> Option<()> {
            EmptyClipboard().ok()?;
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let byte_len = wide.len() * std::mem::size_of::<u16>();
            let hglobal = GlobalAlloc(GMEM_MOVEABLE, byte_len).ok()?;
            let ptr = GlobalLock(hglobal);
            if ptr.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), ptr.cast::<u8>(), byte_len);
            let _ = GlobalUnlock(hglobal);
            SetClipboardData(CF_UNICODETEXT, HANDLE(hglobal.0)).ok()?;
            Some(())
        })()
        .is_some();

        let _ = CloseClipboard();
        copied
    }
}

fn current_strings() -> Strings {
    let state = lock_state();
    state
        .as_ref()
        .map(|s| s.language.strings())
        .unwrap_or(LanguageId::English.strings())
}

fn build_provider_help_text(strings: Strings) -> String {
    let mut message = String::from(strings.help_ai_quotas_intro);
    for provider in strings.help_provider_notes {
        message.push_str("\n\n");
        message.push_str(provider.name);
        message.push_str("\n");
        message.push_str(provider.description);
    }
    message
}

fn show_display_guide(hwnd: HWND) {
    let strings = current_strings();
    show_info_message(
        hwnd,
        strings.help_display_guide,
        strings.help_display_guide_body,
    );
}

/// Builds the full setup/disable/last-observed message body shown by the
/// "Antigravity Setup..." menu item. Always shows the setup and disable
/// commands (this app's own display toggle for Antigravity is a separate
/// thing from the Antigravity CLI's own statusLine configuration, and a
/// user who wants to reconfigure or remove it needs both regardless of
/// which state the cache is currently in). The last-observed line reflects
/// only what `antigravity_statusline::read_cache` actually finds on disk —
/// a missing, malformed, or unsupported-schema cache all show
/// `antigravity_never_observed` rather than a fabricated timestamp.
fn build_antigravity_setup_text(strings: Strings) -> String {
    let mut message = String::from(strings.antigravity_setup_intro);
    message.push_str("\n\n");
    message.push_str(strings.antigravity_setup_command_label);
    message.push('\n');
    message.push_str(ANTIGRAVITY_SETUP_COMMAND);
    message.push_str("\n\n");
    message.push_str(strings.antigravity_setup_disable_label);
    message.push('\n');
    message.push_str(ANTIGRAVITY_DISABLE_COMMAND);
    message.push_str("\n\n");
    message.push_str(strings.antigravity_last_observed_label);
    message.push_str(&antigravity_last_observed_text(strings));
    message
}

/// "Last observed: HH:MM" (local time) for a valid cache, or
/// `antigravity_never_observed` for anything else — a missing file is
/// exactly as "never observed" as a malformed or unsupported-schema one;
/// none of them are a real captured reading.
fn antigravity_last_observed_text(strings: Strings) -> String {
    let path = crate::antigravity_statusline::default_cache_path();
    let crate::antigravity_statusline::CacheReadResult::Valid(cache) =
        crate::antigravity_statusline::read_cache(&path)
    else {
        return strings.antigravity_never_observed.to_string();
    };
    let captured_at = UNIX_EPOCH + Duration::from_secs(cache.captured_at_unix);
    match system_time_to_local_parts(captured_at) {
        Some(parts) => format!("{:02}:{:02}", parts.hour, parts.minute),
        None => strings.antigravity_never_observed.to_string(),
    }
}

/// Copies `ANTIGRAVITY_SETUP_COMMAND` to the clipboard (best-effort — the
/// command is also shown as plain text in the dialog either way, so a
/// clipboard failure never blocks the user from copying it manually) and
/// shows the full setup/disable/last-observed dialog.
fn show_antigravity_setup(hwnd: HWND) {
    let _ = copy_text_to_clipboard(hwnd, ANTIGRAVITY_SETUP_COMMAND);
    let strings = current_strings();
    show_info_message(
        hwnd,
        strings.antigravity_setup_title,
        &build_antigravity_setup_text(strings),
    );
}

fn show_ai_quota_help(hwnd: HWND) {
    let strings = current_strings();
    show_info_message(
        hwnd,
        strings.help_ai_quotas,
        &build_provider_help_text(strings),
    );
}

fn version_information_text(strings: Strings) -> String {
    format!("{}\n\nv{}", strings.window_title, env!("CARGO_PKG_VERSION"))
}

fn show_version_information(hwnd: HWND) {
    let strings = current_strings();
    show_info_message(
        hwnd,
        strings.help_version_information,
        &version_information_text(strings),
    );
}

fn open_readme(hwnd: HWND) -> bool {
    unsafe {
        let operation = native_interop::wide_str("open");
        let url = native_interop::wide_str(README_URL);
        let result = ShellExecuteW(
            hwnd,
            PCWSTR::from_raw(operation.as_ptr()),
            PCWSTR::from_raw(url.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        result.0 as isize > 32
    }
}

#[cfg(feature = "self-update")]
fn show_error_message(hwnd: HWND, title: &str, message: &str) {
    unsafe {
        let title_wide = native_interop::wide_str(title);
        let message_wide = native_interop::wide_str(message);
        let _ = MessageBoxW(
            hwnd,
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(feature = "self-update")]
fn show_update_prompt(hwnd: HWND, strings: Strings, release: &ReleaseDescriptor) -> bool {
    let message = strings
        .update_prompt_now
        .replace("{version}", &release.latest_version);

    unsafe {
        let title_wide = native_interop::wide_str(strings.update_available);
        let message_wide = native_interop::wide_str(&message);
        MessageBoxW(
            hwnd,
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_YESNO | MB_ICONQUESTION,
        ) == IDYES
    }
}

fn apply_language_to_state(state: &mut AppState, language_override: Option<LanguageId>) {
    state.language_override = language_override;
    state.language = localization::resolve_language(language_override);
    set_window_title(state.hwnd.to_hwnd(), state.language.strings());
    refresh_usage_texts(state);
}

fn update_language_change() -> bool {
    let mut state = lock_state();
    let Some(app_state) = state.as_mut() else {
        return false;
    };

    if app_state.language_override.is_some() {
        return false;
    }

    let new_language = localization::detect_system_language();
    if new_language == app_state.language {
        return false;
    }

    apply_language_to_state(app_state, None);
    true
}

#[cfg(feature = "self-update")]
fn begin_update_check(hwnd: HWND, interactive: bool) {
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    let (strings, install_channel) = {
        let mut state = lock_state();
        let Some(app_state) = state.as_mut() else {
            return;
        };

        if matches!(
            app_state.update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            if interactive {
                show_info_message(
                    hwnd,
                    app_state.language.strings().updates,
                    app_state.language.strings().update_in_progress,
                );
            }
            return;
        }

        app_state.update_status = UpdateStatus::Checking;
        (app_state.language.strings(), app_state.install_channel)
    };

    std::thread::spawn(move || {
        let hwnd = send_hwnd.to_hwnd();
        let checked_at = now_unix_secs();
        match updater::check_for_updates() {
            Ok(UpdateCheckResult::UpToDate) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::UpToDate;
                        s.last_update_check_unix = Some(checked_at);
                    }
                }
                save_state_settings();
                if interactive {
                    show_info_message(hwnd, strings.updates, strings.up_to_date);
                }
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
            Ok(UpdateCheckResult::Available(release)) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Available(release.clone());
                        s.last_update_check_unix = Some(checked_at);
                    }
                }
                save_state_settings();
                if interactive && show_update_prompt(hwnd, strings, &release) {
                    match install_channel {
                        InstallChannel::Portable => begin_update_apply(hwnd, release),
                        InstallChannel::Winget => begin_winget_update(hwnd),
                    }
                }
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
            Err(error) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Idle;
                        s.last_update_check_unix = Some(checked_at);
                    }
                }
                save_state_settings();
                if interactive {
                    let message = format!("{}.\n\n{}", strings.update_failed, error);
                    show_error_message(hwnd, strings.updates, &message);
                }
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
        }
    });
}

#[cfg(feature = "self-update")]
fn begin_update_apply(hwnd: HWND, release: ReleaseDescriptor) {
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    let strings = {
        let mut state = lock_state();
        let Some(app_state) = state.as_mut() else {
            return;
        };

        if matches!(
            app_state.update_status,
            UpdateStatus::Checking | UpdateStatus::Applying
        ) {
            show_info_message(
                hwnd,
                app_state.language.strings().updates,
                app_state.language.strings().update_in_progress,
            );
            return;
        }

        app_state.update_status = UpdateStatus::Applying;
        app_state.language.strings()
    };

    std::thread::spawn(move || {
        let hwnd = send_hwnd.to_hwnd();
        match updater::begin_self_update(&release) {
            Ok(()) => unsafe {
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Err(error) => {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.update_status = UpdateStatus::Available(release);
                    }
                }
                let message = format!("{}.\n\n{}", strings.update_failed, error);
                show_error_message(hwnd, strings.updates, &message);
                unsafe {
                    let _ = PostMessageW(hwnd, WM_APP_UPDATE_CHECK_COMPLETE, WPARAM(0), LPARAM(0));
                }
            }
        }
    });
}

#[cfg(feature = "self-update")]
fn begin_winget_update(hwnd: HWND) {
    let strings = {
        let state = lock_state();
        state.as_ref().map(|s| s.language.strings())
    }
    .unwrap_or(LanguageId::English.strings());

    match updater::begin_winget_update() {
        Ok(()) => unsafe {
            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        Err(error) => {
            let message = format!("{}.\n\n{}", strings.update_failed, error);
            show_error_message(hwnd, strings.updates, &message);
        }
    }
}

const STARTUP_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_REGISTRY_KEY: &str = "ClaudeCodeUsageMonitor";
const STARTUP_TASK_ID: &str = "AIUsageMonitorStartup";

fn is_packaged() -> bool {
    let mut package_name_length = 0;
    unsafe {
        GetCurrentPackageFullName(&mut package_name_length, windows::core::PWSTR::null())
            == ERROR_INSUFFICIENT_BUFFER
    }
}

fn packaged_startup_task() -> windows::core::Result<StartupTask> {
    StartupTask::GetAsync(&HSTRING::from(STARTUP_TASK_ID))?.get()
}

fn is_packaged_startup_enabled() -> bool {
    let result = packaged_startup_task().and_then(|task| task.State());
    match result {
        Ok(state) => {
            state == StartupTaskState::Enabled || state == StartupTaskState::EnabledByPolicy
        }
        Err(error) => {
            diagnose::log_error("unable to read packaged startup task state", error);
            false
        }
    }
}

fn set_packaged_startup_enabled(enable: bool) {
    let result = packaged_startup_task().and_then(|task| {
        if enable {
            task.RequestEnableAsync()?.get().map(|_| ())
        } else {
            task.Disable()
        }
    });
    if let Err(error) = result {
        diagnose::log_error("unable to update packaged startup task state", error);
    }
}

/// Returns true only if the startup registry value points to this executable.
fn is_startup_enabled() -> bool {
    if is_packaged() {
        return is_packaged_startup_enabled();
    }

    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);
        let key_name = native_interop::wide_str(STARTUP_REGISTRY_KEY);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        );
        if result.is_err() {
            return false;
        }

        // Query the size of the value
        let mut data_size: u32 = 0;
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            None,
            Some(&mut data_size),
        );
        if result.is_err() || data_size == 0 {
            let _ = RegCloseKey(hkey);
            return false;
        }

        // Read the value
        let mut buf = vec![0u8; data_size as usize];
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr()),
            Some(&mut data_size),
        );
        let _ = RegCloseKey(hkey);
        if result.is_err() {
            return false;
        }

        // Convert the registry value (UTF-16) to a string
        let wide_slice =
            std::slice::from_raw_parts(buf.as_ptr() as *const u16, data_size as usize / 2);
        let reg_value = String::from_utf16_lossy(wide_slice)
            .trim_end_matches('\0')
            .to_string();

        // Get the current executable path
        let mut exe_buf = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
        if len == 0 {
            return false;
        }
        let current_exe = String::from_utf16_lossy(&exe_buf[..len]);

        // Case-insensitive comparison (Windows paths are case-insensitive)
        reg_value.eq_ignore_ascii_case(&current_exe)
    }
}

fn set_startup_enabled(enable: bool) {
    if is_packaged() {
        set_packaged_startup_enabled(enable);
        return;
    }

    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if result.is_err() {
            return;
        }

        let key_name = native_interop::wide_str(STARTUP_REGISTRY_KEY);

        if enable {
            let mut exe_buf = [0u16; 260];
            let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
            if len > 0 {
                // Write the wide string including null terminator
                let byte_len = ((len + 1) * 2) as u32;
                let _ = RegSetValueExW(
                    hkey,
                    PCWSTR::from_raw(key_name.as_ptr()),
                    0,
                    REG_SZ,
                    Some(std::slice::from_raw_parts(
                        exe_buf.as_ptr() as *const u8,
                        byte_len as usize,
                    )),
                );
            }
        } else {
            let _ = RegDeleteValueW(hkey, PCWSTR::from_raw(key_name.as_ptr()));
        }

        let _ = RegCloseKey(hkey);
    }
}

// Dimensions matching the C# version
const SEGMENT_W: i32 = 10;
const SEGMENT_H: i32 = 13;
const SEGMENT_GAP: i32 = 1;
const SEGMENT_COUNT: i32 = 10;
const CORNER_RADIUS: i32 = 2;

const LEFT_DIVIDER_W: i32 = 3;
const DIVIDER_RIGHT_MARGIN: i32 = 10;
const LABEL_WIDTH: i32 = 18;
const LABEL_RIGHT_MARGIN: i32 = 10;
const BAR_RIGHT_MARGIN: i32 = 4;
/// Wide enough for the longest expected per-cell value+reset text — 3-digit
/// percent + " · " + reset-in word + countdown, no display-basis prefix
/// (that now lives once in the header row instead of on every cell). Worst
/// case measured against the actual localized `reset_in`/`day_suffix`
/// strings shipped in `src/localization/*.rs`: Japanese
/// "100% · リセットまで 7日" ≈ "100%"(~28px) + " · "(~14px) +
/// "リセットまで"(6 full-width glyphs ≈78px) + " "(~4px) + "7日"(~20px) ≈ 144px
/// at Segoe UI 12px (≈7px/Latin glyph, ≈13px/CJK full-width glyph). 160px
/// leaves a ~16px margin for other languages. This is a character-count
/// calculation, not a live GDI measurement (see completion report for why);
/// it has not been visually verified on this machine (no runtime access
/// here) and needs a home-PC check across all 11 languages.
const TEXT_WIDTH: i32 = 160;
/// Minimum text budget per provider when two or more columns share the
/// widget. Combined with the existing fixed bar widths, this yields the
/// practical 353/477/623 logical-pixel minima for 2/3/4 providers while the
/// single-provider minimum remains the legacy 315 pixels.
const MIN_MULTI_PROVIDER_TEXT_WIDTH: i32 = 96;
const MAX_WIDGET_WIDTH_LOGICAL: i32 = 1200;
const RESIZE_EDGE_LOGICAL: i32 = 6;
const HELP_BUTTON_SIZE_LOGICAL: i32 = 14;
const HELP_BUTTON_TOP_LOGICAL: i32 = 3;
const HELP_BUTTON_RIGHT_MARGIN_LOGICAL: i32 = 4;
const HELP_BUTTON_CONTENT_GAP_LOGICAL: i32 = 4;
const MODEL_RIGHT_MARGIN: i32 = 3;
const RIGHT_MARGIN: i32 = 1;
/// Height of each of the two header text rows (display-basis label, then
/// provider names) added above the existing 5h/7d bar rows.
const HEADER_ROW_H: i32 = 14;
/// 3px top margin + HEADER_ROW_H (basis label) + 2px + HEADER_ROW_H
/// (provider names) + 4px + SEGMENT_H (weekly row) + 10px + SEGMENT_H (5h
/// row) + 5px bottom margin = 3+14+2+14+4+13+10+13+5 = 78. The bar row
/// height and the 10px gap between them (`ROW_GAP_H`) are unchanged from
/// before this feature; only the two header rows and their margins are new.
/// The weekly row is drawn above the 5h row (see `pace_row_layout`) —
/// visual order doesn't change this total. Not visually verified on this
/// machine — see completion report.
///
/// AUM-WINDOW-UI-01C-1: the basis-label row this breakdown includes is no
/// longer drawn at all (removed from the popup body for every
/// `PopupLayout`) — `popup_height_logical` now always subtracts
/// `BASIS_LABEL_ROW_H` from this constant rather than conditionally, so the
/// breakdown above is historical (this constant's *value* is unchanged;
/// only how much of it actually reaches the screen has).
const WIDGET_HEIGHT: i32 = 78;

/// Gap the basis-label row used to keep above the provider-header row below
/// it (see `WIDGET_HEIGHT`'s breakdown: "...HEADER_ROW_H (basis label) +
/// 2px..."). Only remaining use is `BASIS_LABEL_ROW_H`'s own definition,
/// now that the basis-label row itself is never drawn — see
/// `BASIS_LABEL_ROW_H`'s doc.
const BASIS_LABEL_GAP_H: i32 = 2;
/// Logical budget the basis-label row ("Used %" / "Remaining Allowance")
/// used to occupy at the top of the `Standard`-layout popup: its own
/// `HEADER_ROW_H` plus `BASIS_LABEL_GAP_H`. AUM-WINDOW-UI-01C-1 removed that
/// row from the popup body entirely (it's still available via the settings
/// menu — see `IDM_DISPLAY_BASIS_USED`/`IDM_DISPLAY_BASIS_REMAINING`), so
/// `popup_height_logical` now always subtracts this budget rather than only
/// when `PopupLayout::Compact` (which already omitted it) — kept as a named
/// constant so that unconditional subtraction stays self-documenting instead
/// of a bare magic number.
const BASIS_LABEL_ROW_H: i32 = HEADER_ROW_H + BASIS_LABEL_GAP_H;

/// Height of one additional pace-guidance text line (the weekly row's
/// secondary/detail lines), reusing the same line-height already used for
/// the header rows above the bars — see `HEADER_ROW_H`. Not visually
/// verified on this machine — see completion report.
const PACE_LINE_H: i32 = HEADER_ROW_H;

/// Logical (pre-DPI-scale) gap between the popup's two main bar rows —
/// weekly and 5h — reusing the same `10` this widget always used between
/// them (see `WIDGET_HEIGHT`'s breakdown). Named so `popup_height_logical`
/// and `pace_row_layout` can each add or remove exactly this much depending
/// on whether the 5h row has anything to show this poll.
const ROW_GAP_H: i32 = 10;

/// How many extra lines below the weekly bar this one provider's
/// pace-guidance block actually needs: `secondary` present adds one,
/// `detail` present (only ever populated alongside `secondary` — see
/// `weekly_pace_guidance_lines`) adds another. `None` — not
/// `CellState::Ok`, or no usable pace data (e.g. reset time unknown) —
/// needs zero, regardless of `DisplayDensity`.
fn weekly_pace_extra_lines_for(pace: Option<&PaceGuidanceLines>) -> i32 {
    match pace {
        Some(lines) => i32::from(lines.secondary.is_some()) + i32::from(lines.detail.is_some()),
        None => 0,
    }
}

/// Max extra weekly-guidance lines the popup must reserve: the max actually
/// needed across only the *currently-shown* providers. A hidden provider's
/// stale pace data (if any lingers in `AppState`) must never affect the
/// shared popup height, and providers that are shown but not
/// `CellState::Ok` (loading/error/unconfigured/not-available) contribute
/// zero via `weekly_pace_extra_lines_for`'s `None` case. Flattened (no
/// `&AppState`) so both the state-holding call sites below and
/// `paint_content`'s own already-flattened params share one implementation
/// instead of two copies of the same predicate, and so this is directly
/// testable without constructing an `AppState`.
fn weekly_pace_extra_lines_shown(
    show_claude_code: bool,
    weekly_pace: Option<&PaceGuidanceLines>,
    show_codex: bool,
    codex_weekly_pace: Option<&PaceGuidanceLines>,
    show_antigravity: bool,
    antigravity_weekly_pace: Option<&PaceGuidanceLines>,
) -> i32 {
    let mut max_lines = 0;
    if show_claude_code {
        max_lines = max_lines.max(weekly_pace_extra_lines_for(weekly_pace));
    }
    if show_codex {
        max_lines = max_lines.max(weekly_pace_extra_lines_for(codex_weekly_pace));
    }
    if show_antigravity {
        max_lines = max_lines.max(weekly_pace_extra_lines_for(antigravity_weekly_pace));
    }
    max_lines
}

fn weekly_pace_extra_lines(state: &AppState) -> i32 {
    weekly_pace_extra_lines_shown(
        state.show_claude_code,
        state.weekly_pace.as_ref(),
        state.show_codex,
        state.codex_weekly_pace.as_ref(),
        state.show_antigravity,
        state.antigravity_weekly_pace.as_ref(),
    )
}

/// One provider's decision for the (repurposed, `ShortWindowVisibility`-
/// gated) 5h bar row: whether this cell shows anything at all this poll,
/// and if so, what `percent`/`text` to draw. `text` is `render_cell`'s
/// output for this cell — the same loading, provider-error, or
/// `NotAvailable` status word, or plain percent+reset text
/// the cell would have shown before pace guidance existed. Branches on
/// `state` itself (never on `text`'s content) so a status word never has to
/// be pattern-matched or string-compared to be recognized as one.
///
/// - `Hidden`: never shows anything, regardless of `state` — the user chose
///   to hide the 5h window entirely, including its error/loading states.
/// - `pace` is `Some` (`CellState::Ok`, and `Always`/overpacing-`WarningOnly`
///   warrants a line — see `short_window_pace_guidance_lines`): shows the
///   pace-guidance `primary` line at the original `percent`.
/// - `state` is `Ok` with a real current value (`percent` is `Some`, per
///   `CellDisplay::bar_percent`'s contract) but no `pace` this poll
///   (`WarningOnly` + not overpacing, or an `Always` non-finite-percent
///   edge): nothing to show — the row is pace-driven once a cell has a
///   real value, not a duplicate of the plain number.
/// - Anything else — `state` is not `Ok`, or is `Ok` but paired with a
///   missing value (the `render_cell`/`status_text` caller-bug fail-safe,
///   where `status_text` fails safe to "not available" text) — keeps the
///   existing status `text`: a suppressed *pace* line must never suppress
///   the provider's actual poll status.
/// The 4th element (`is_warning`) mirrors `PaceGuidanceLines::is_warning`
/// when `pace` supplies the shown text, `false` otherwise — it's the signal
/// `draw_row` uses to color that cell's *value text* with the palette's
/// warning color (see `PopupPalette::warning`), never the bar segments
/// themselves (those always keep the provider's own accent color).
fn session_cell_decision<'a>(
    state: CellState,
    percent: Option<f64>,
    text: &'a str,
    pace: Option<&'a PaceGuidanceLines>,
    visibility: ShortWindowVisibility,
) -> (bool, Option<f64>, &'a str, bool) {
    if visibility == ShortWindowVisibility::Hidden {
        return (false, None, "", false);
    }
    if let Some(lines) = pace {
        return (true, percent, lines.primary.as_str(), lines.is_warning);
    }
    if state == CellState::Ok && percent.is_some() {
        return (false, None, "", false);
    }
    (true, percent, text, false)
}

/// Whether this provider alone should keep the 5h row visible under
/// `WarningOnly`. Only a real pace-driven warning justifies the row;
/// loading/error/not-available status text may still be drawn when another
/// provider's warning keeps the row alive, but status text alone must not
/// leave a status-only row behind.
fn session_cell_justifies_warning_only_row(
    _state: CellState,
    pace: Option<&PaceGuidanceLines>,
) -> bool {
    pace.is_some()
}

/// One provider's contribution to the 5h row's existence — shared by both
/// `needs_session_row` (popup sizing) and `paint_content` (drawing), which
/// must never disagree about which providers keep the row alive (see
/// `session_row_visible`'s own doc). Identical to `session_cell_decision`'s
/// `shows` for `Always`/`Hidden`; for `WarningOnly`, defers to
/// `session_cell_justifies_warning_only_row` instead, so status-only cells
/// cannot keep the row alive on their own.
fn session_row_cell_shows(
    state: CellState,
    percent: Option<f64>,
    text: &str,
    pace: Option<&PaceGuidanceLines>,
    visibility: ShortWindowVisibility,
) -> bool {
    if visibility == ShortWindowVisibility::WarningOnly {
        return session_cell_justifies_warning_only_row(state, pace);
    }
    session_cell_decision(state, percent, text, pace, visibility).0
}

/// Whether the 5h bar row has anything to draw at all this poll: at least
/// one *currently-shown* provider's `session_cell_decision` says to show
/// something. The `show_*` checks here are the authority — a hidden
/// provider's decision must never keep the row alive. This is the single
/// predicate popup height and the row's own draw call must agree on (both
/// derive their `*_shows` inputs from the same `session_cell_decision`
/// calls); when `false` the row (and its connecting `ROW_GAP_H`) are
/// removed from the layout entirely rather than left blank.
fn session_row_visible(
    show_claude_code: bool,
    claude_shows: bool,
    show_codex: bool,
    codex_shows: bool,
    show_antigravity: bool,
    antigravity_shows: bool,
) -> bool {
    (show_claude_code && claude_shows)
        || (show_codex && codex_shows)
        || (show_antigravity && antigravity_shows)
}

/// A quota row exists only when at least one currently shown provider has
/// that row enabled for display. A provider that is merely hidden by a user
/// setting contributes `false`; a displayed provider whose applicable quota
/// is currently unavailable still contributes `true` and renders its normal
/// localized unavailable/error state.
fn quota_row_visible(provider_cells: &[(bool, bool)]) -> bool {
    provider_cells
        .iter()
        .any(|(provider_is_shown, cell_is_enabled)| *provider_is_shown && *cell_is_enabled)
}

fn needs_weekly_row(state: &AppState) -> bool {
    quota_row_visible(&[
        (state.show_claude_code, true),
        (state.show_codex, true),
        (state.show_antigravity, true),
        // GitHub Copilot has a monthly quota row, not a weekly one.
        (state.show_github_copilot, false),
    ])
}

fn needs_monthly_row(state: &AppState) -> bool {
    quota_row_visible(&[
        (state.show_claude_code, false),
        (state.show_codex, false),
        (state.show_antigravity, false),
        (state.show_github_copilot, true),
    ])
}

fn needs_session_row(state: &AppState) -> bool {
    let visibility = state.short_window_visibility;
    let claude_shows = session_row_cell_shows(
        state.session_state,
        state.session_percent,
        &state.session_text,
        state.session_pace.as_ref(),
        visibility,
    );
    let codex_shows = session_row_cell_shows(
        state.codex_session_state,
        state.codex_session_percent,
        &state.codex_session_text,
        state.codex_session_pace.as_ref(),
        visibility,
    );
    let antigravity_shows = session_row_cell_shows(
        state.antigravity_session_state,
        state.antigravity_session_percent,
        &state.antigravity_session_text,
        state.antigravity_session_pace.as_ref(),
        visibility,
    );
    session_row_visible(
        state.show_claude_code,
        claude_shows,
        state.show_codex,
        codex_shows,
        state.show_antigravity,
        antigravity_shows,
    )
}

/// Which of the popup's optional rows/lines are actually shown this frame —
/// the single row-selection judgment shared verbatim by `paint_content`
/// (what to draw) and `popup_height_logical`/`pace_row_layout` (how tall to
/// make the popup), so drawing and sizing can never disagree about which
/// rows exist. `PopupLayout::Compact` suppresses pace detail and the 5h row,
/// while retaining whichever primary quota rows (weekly/monthly) actually
/// have an enabled provider cell. `PopupLayout::Standard` passes all row
/// decisions through. A provider-specific row with no enabled cell is
/// removed by the same decision used by painting and height calculation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VisibleRows {
    weekly_row: bool,
    weekly_extra_lines: i32,
    session_row: bool,
    monthly_row: bool,
    /// Count of Antigravity extra-item rows (every cache item besides
    /// `gemini-weekly`) — always shown regardless of `PopupLayout`, the same
    /// treatment `monthly_row` gets, since these are current quota data, not
    /// an optional detail line.
    antigravity_extra_rows: i32,
}

fn visible_rows(
    layout: PopupLayout,
    weekly_extra_lines: i32,
    needs_session_row: bool,
) -> VisibleRows {
    visible_rows_for_quota(
        layout,
        true,
        weekly_extra_lines,
        needs_session_row,
        false,
        0,
    )
}

fn visible_rows_for_quota(
    layout: PopupLayout,
    weekly_row: bool,
    weekly_extra_lines: i32,
    session_row: bool,
    monthly_row: bool,
    antigravity_extra_rows: i32,
) -> VisibleRows {
    match layout {
        PopupLayout::Compact => VisibleRows {
            weekly_row,
            weekly_extra_lines: 0,
            session_row: false,
            monthly_row,
            antigravity_extra_rows,
        },
        PopupLayout::Standard => VisibleRows {
            weekly_row,
            weekly_extra_lines: if weekly_row { weekly_extra_lines } else { 0 },
            session_row,
            monthly_row,
            antigravity_extra_rows,
        },
    }
}

/// Popup height (logical, pre-DPI-scale px) for the exact visible row set.
/// The header and outer margins are fixed; each visible quota row contributes
/// one segment height, gaps exist only between visible rows, and weekly pace
/// detail contributes its actual line count. Callers apply `sc(...)` once.
fn popup_height_logical(rows: VisibleRows) -> i32 {
    const HEADER_ONLY_HEIGHT: i32 = 3 + HEADER_ROW_H + 4 + 5;
    let row_count = i32::from(rows.weekly_row)
        + i32::from(rows.session_row)
        + i32::from(rows.monthly_row)
        + rows.antigravity_extra_rows;
    let gaps = (row_count - 1).max(0);
    HEADER_ONLY_HEIGHT
        + row_count * SEGMENT_H
        + gaps * ROW_GAP_H
        + rows.weekly_extra_lines * PACE_LINE_H
}

/// Popup height for the current state: the base widget height plus
/// whatever the pace-guidance block currently needs, filtered through the
/// current `PopupLayout`. Mirrors `total_widget_width_for_state`'s pattern
/// of a `&AppState`-taking variant (used where a lock is already held)
/// alongside a self-locking `widget_height()` convenience wrapper below.
fn widget_height_for_state(state: &AppState) -> i32 {
    let rows = visible_rows_for_quota(
        state.popup_layout,
        needs_weekly_row(state),
        weekly_pace_extra_lines(state),
        needs_session_row(state),
        needs_monthly_row(state),
        state.antigravity_extra_items.len() as i32,
    );
    widget_height_for_rows(rows)
}

fn popup_height_device_for_dpi(rows: VisibleRows, dpi: u32) -> i32 {
    let row_count = i32::from(rows.weekly_row)
        + i32::from(rows.session_row)
        + i32::from(rows.monthly_row)
        + rows.antigravity_extra_rows;
    let gaps = (row_count - 1).max(0);

    scaled_for_dpi(3, dpi)
        + scaled_for_dpi(HEADER_ROW_H, dpi)
        + scaled_for_dpi(4, dpi)
        + row_count * scaled_for_dpi(SEGMENT_H, dpi)
        + gaps * scaled_for_dpi(ROW_GAP_H, dpi)
        + rows.weekly_extra_lines * scaled_for_dpi(PACE_LINE_H, dpi)
        + scaled_for_dpi(5, dpi)
}

fn widget_height_for_rows(rows: VisibleRows) -> i32 {
    popup_height_device_for_dpi(rows, CURRENT_DPI.load(Ordering::Relaxed))
}

fn widget_height() -> i32 {
    let state = lock_state();
    match state.as_ref() {
        Some(s) => widget_height_for_state(s),
        None => sc(WIDGET_HEIGHT),
    }
}

/// Logical y-coordinates (already DPI-scaled, same convention `paint_content`
/// uses throughout) for every row in the popup's header + weekly/5h block,
/// given the popup's total scaled `height` and the same `rows`
/// (`VisibleRows`) input `popup_height_logical` used to size that `height`
/// in the first place — the two must always agree, which is why this is the
/// one place either `paint_content` or a test computes these positions.
/// Order top to bottom: provider header, optional weekly bar and its pace
/// detail, optional 5h bar, optional monthly bar. Gaps exist only between
/// rows that are actually present, so a hidden row leaves neither its label
/// nor vertical whitespace behind.
struct PaceRowLayout {
    provider_header_y: i32,
    weekly_row_y: Option<i32>,
    weekly_secondary_y: Option<i32>,
    session_row_y: Option<i32>,
    monthly_row_y: Option<i32>,
    /// One Y per Antigravity extra-item row, in the same order as
    /// `AppState::antigravity_extra_items` — empty when there are none.
    antigravity_extra_rows_y: Vec<i32>,
}

fn pace_row_layout(height: i32, rows: VisibleRows) -> PaceRowLayout {
    let provider_header_y = sc(3);
    let mut y = provider_header_y + sc(HEADER_ROW_H) + sc(4);
    let mut rows_left = i32::from(rows.weekly_row)
        + i32::from(rows.session_row)
        + i32::from(rows.monthly_row)
        + rows.antigravity_extra_rows;

    let weekly_row_y = rows.weekly_row.then_some(y);
    let weekly_secondary_y = if rows.weekly_row && rows.weekly_extra_lines >= 1 {
        Some(y + sc(SEGMENT_H))
    } else {
        None
    };
    if rows.weekly_row {
        y += sc(SEGMENT_H) + rows.weekly_extra_lines * sc(PACE_LINE_H);
        rows_left -= 1;
        if rows_left > 0 {
            y += sc(ROW_GAP_H);
        }
    }

    let session_row_y = rows.session_row.then_some(y);
    if rows.session_row {
        y += sc(SEGMENT_H);
        rows_left -= 1;
        if rows_left > 0 {
            y += sc(ROW_GAP_H);
        }
    }

    let monthly_row_y = rows.monthly_row.then_some(y);
    if rows.monthly_row {
        y += sc(SEGMENT_H);
        rows_left -= 1;
        if rows_left > 0 {
            y += sc(ROW_GAP_H);
        }
    }

    let mut antigravity_extra_rows_y =
        Vec::with_capacity(rows.antigravity_extra_rows.max(0) as usize);
    for _ in 0..rows.antigravity_extra_rows {
        antigravity_extra_rows_y.push(y);
        y += sc(SEGMENT_H);
        rows_left -= 1;
        if rows_left > 0 {
            y += sc(ROW_GAP_H);
        }
    }

    debug_assert_eq!(y + sc(5), height);
    PaceRowLayout {
        provider_header_y,
        weekly_row_y,
        weekly_secondary_y,
        session_row_y,
        monthly_row_y,
        antigravity_extra_rows_y,
    }
}

/// AUM-WINDOW-UI-01C-2-STEP2 (drag UX): the fixed y-boundary immediately
/// below the provider header. Optional quota rows begin at this boundary;
/// hiding any of them changes only the content below, never the established
/// header drag target.
fn header_band_bottom(state: &AppState) -> i32 {
    let _ = state;
    sc(3 + HEADER_ROW_H + 4)
}

/// Whether `(client_x, client_y)` falls within the popup's draggable header
/// band — the full-width strip from the top edge down to (not including)
/// `header_band_bottom`. Replaces the old narrow left-edge handle
/// (AUM-WINDOW-UI-01C-2-STEP2): the header band already shows the provider
/// names, making it a far more discoverable drag target than a 10px-wide
/// strip ever was.
fn is_drag_region_point(client_x: i32, client_y: i32, width: i32, header_band_bottom: i32) -> bool {
    client_x >= 0 && client_x < width && client_y >= 0 && client_y < header_band_bottom
}

fn horizontal_resize_edge_width_for_dpi(dpi: u32) -> i32 {
    scaled_for_dpi(RESIZE_EDGE_LOGICAL, dpi).max(4)
}

fn help_button_rect_for_client_width(client_width: i32, dpi: u32) -> RECT {
    let size = scaled_for_dpi(HELP_BUTTON_SIZE_LOGICAL, dpi);
    let right = (client_width
        - horizontal_resize_edge_width_for_dpi(dpi)
        - scaled_for_dpi(HELP_BUTTON_RIGHT_MARGIN_LOGICAL, dpi))
    .max(size);
    let left = right - size;
    let top = scaled_for_dpi(HELP_BUTTON_TOP_LOGICAL, dpi);
    RECT {
        left,
        top,
        right,
        bottom: top + size,
    }
}

fn header_column_width_avoiding_help(
    column_x: i32,
    column_width: i32,
    client_width: i32,
    dpi: u32,
    is_last_provider: bool,
) -> i32 {
    if !is_last_provider {
        return column_width;
    }

    let help_left = help_button_rect_for_client_width(client_width, dpi).left;
    let available = help_left - scaled_for_dpi(HELP_BUTTON_CONTENT_GAP_LOGICAL, dpi) - column_x;
    column_width.min(available.max(0))
}

fn point_is_in_rect(client_x: i32, client_y: i32, rect: RECT) -> bool {
    client_x >= rect.left && client_x < rect.right && client_y >= rect.top && client_y < rect.bottom
}

fn horizontal_resize_edge_at(
    client_x: i32,
    client_width: i32,
    edge_width: i32,
) -> Option<HorizontalResizeEdge> {
    if client_x < 0 || client_x >= client_width || client_width <= 0 {
        return None;
    }

    let edge_width = edge_width.max(1).min(client_width);
    if client_x < edge_width {
        Some(HorizontalResizeEdge::Left)
    } else if client_x >= client_width - edge_width {
        Some(HorizontalResizeEdge::Right)
    } else {
        None
    }
}

fn pointer_interaction_target(
    client_x: i32,
    client_y: i32,
    client_width: i32,
    resize_edge_width: i32,
    header_bottom: i32,
    help_button_rect: RECT,
    auth_cta_hit_targets: &[AuthCtaHitTarget],
) -> PointerInteractionTarget {
    if let Some(edge) = horizontal_resize_edge_at(client_x, client_width, resize_edge_width) {
        PointerInteractionTarget::HorizontalResize(edge)
    } else if point_is_in_rect(client_x, client_y, help_button_rect) {
        PointerInteractionTarget::HelpButton
    } else if let Some(hit) = auth_cta_hit_targets
        .iter()
        .find(|hit| point_is_in_rect(client_x, client_y, hit.rect))
    {
        PointerInteractionTarget::AuthCta(hit.action)
    } else if is_drag_region_point(client_x, client_y, client_width, header_bottom) {
        PointerInteractionTarget::HeaderDrag
    } else {
        PointerInteractionTarget::None
    }
}

fn window_dpi(hwnd: HWND) -> u32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        CURRENT_DPI.load(Ordering::Relaxed).max(1)
    } else {
        dpi
    }
}

fn pointer_interaction_under_cursor(hwnd: HWND) -> PointerInteractionTarget {
    let mut point = POINT::default();
    let mut client_rect = RECT::default();
    unsafe {
        if GetCursorPos(&mut point).is_err()
            || !ScreenToClient(hwnd, &mut point).as_bool()
            || GetClientRect(hwnd, &mut client_rect).is_err()
        {
            return PointerInteractionTarget::None;
        }
    }
    let state = lock_state();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return PointerInteractionTarget::None,
    };
    let dpi = window_dpi(hwnd);
    pointer_interaction_target(
        point.x,
        point.y,
        client_rect.right - client_rect.left,
        horizontal_resize_edge_width_for_dpi(dpi),
        header_band_bottom(s),
        help_button_rect_for_client_width(client_rect.right - client_rect.left, dpi),
        &s.auth_cta_hit_targets,
    )
}

/// AUM-WINDOW-UI-01C-2-STEP2: the popup's screen position while free-dragging
/// — the window's position at drag-start, shifted by how far the cursor has
/// moved since. Shared by `WM_MOUSEMOVE` (live follow) and `WM_LBUTTONUP`
/// (final drop position), so the two can never compute a different answer
/// for the same cursor position. No clamping here — off-screen recovery is
/// a separate, later step.
fn drag_follow_position(
    start_window: (i32, i32),
    start_mouse: (i32, i32),
    current_mouse: (i32, i32),
) -> (i32, i32) {
    (
        start_window.0 + (current_mouse.0 - start_mouse.0),
        start_window.1 + (current_mouse.1 - start_mouse.1),
    )
}

fn horizontal_resize_rect(
    session: HorizontalResizeSession,
    cursor_screen_x: i32,
    minimum_width: i32,
    maximum_width: i32,
    required_height: i32,
) -> RECT {
    let start_width = session
        .start_window_rect
        .right
        .saturating_sub(session.start_window_rect.left);
    let delta = cursor_screen_x.saturating_sub(session.start_cursor_screen_x);
    let desired_width = match session.edge {
        HorizontalResizeEdge::Left => start_width.saturating_sub(delta),
        HorizontalResizeEdge::Right => start_width.saturating_add(delta),
    };
    let width = desired_width.clamp(minimum_width, maximum_width);
    let (left, right) = match session.edge {
        HorizontalResizeEdge::Left => (
            session.start_window_rect.right.saturating_sub(width),
            session.start_window_rect.right,
        ),
        HorizontalResizeEdge::Right => (
            session.start_window_rect.left,
            session.start_window_rect.left.saturating_add(width),
        ),
    };

    RECT {
        left,
        top: session.start_window_rect.top,
        right,
        bottom: session
            .start_window_rect
            .top
            .saturating_add(required_height),
    }
}

fn clear_horizontal_resize_session(session: &mut Option<HorizontalResizeSession>) -> bool {
    session.take().is_some()
}

fn active_family_count(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    show_github_copilot: bool,
) -> i32 {
    (show_claude_code as i32
        + show_codex as i32
        + show_antigravity as i32
        + show_github_copilot as i32)
        .max(1)
}

fn scaled_for_dpi(px: i32, dpi: u32) -> i32 {
    let dpi = dpi.max(1);
    (px as f64 * dpi as f64 / 96.0).round() as i32
}

fn logical_from_device(px: i32, dpi: u32) -> i32 {
    let dpi = dpi.max(1);
    (px as f64 * 96.0 / dpi as f64).round() as i32
}

fn completed_resize_settings(
    rect: RECT,
    dpi: u32,
    manual_position: Option<(i32, i32)>,
) -> (i32, Option<(i32, i32)>) {
    (
        logical_from_device(rect.right - rect.left, dpi),
        manual_position.map(|_| (rect.left, rect.top)),
    )
}

fn usage_bar_logical_width(segment_count: i32) -> i32 {
    (SEGMENT_W + SEGMENT_GAP) * segment_count - SEGMENT_GAP + BAR_RIGHT_MARGIN
}

fn usage_bar_device_width(segment_count: i32, dpi: u32) -> i32 {
    (scaled_for_dpi(SEGMENT_W, dpi) + scaled_for_dpi(SEGMENT_GAP, dpi)) * segment_count
        - scaled_for_dpi(SEGMENT_GAP, dpi)
        + scaled_for_dpi(BAR_RIGHT_MARGIN, dpi)
}

fn fixed_widget_logical_width(active_families: i32) -> i32 {
    LEFT_DIVIDER_W
        + DIVIDER_RIGHT_MARGIN
        + LABEL_WIDTH
        + LABEL_RIGHT_MARGIN
        + MODEL_RIGHT_MARGIN * (active_families - 1)
        + RIGHT_MARGIN
}

fn fixed_widget_device_width(active_families: i32, dpi: u32) -> i32 {
    scaled_for_dpi(LEFT_DIVIDER_W, dpi)
        + scaled_for_dpi(DIVIDER_RIGHT_MARGIN, dpi)
        + scaled_for_dpi(LABEL_WIDTH, dpi)
        + scaled_for_dpi(LABEL_RIGHT_MARGIN, dpi)
        + scaled_for_dpi(MODEL_RIGHT_MARGIN, dpi) * (active_families - 1)
        + scaled_for_dpi(RIGHT_MARGIN, dpi)
}

fn default_widget_width_logical_for(active_families: i32) -> i32 {
    let active_families = active_families.max(1);
    let column_width = usage_bar_logical_width(row_bar_segment_count(active_families)) + TEXT_WIDTH;
    fixed_widget_logical_width(active_families) + column_width * active_families
}

fn minimum_widget_width_logical_for(active_families: i32) -> i32 {
    let active_families = active_families.max(1);
    let text_width = if active_families == 1 {
        TEXT_WIDTH
    } else {
        MIN_MULTI_PROVIDER_TEXT_WIDTH
    };
    let column_width = usage_bar_logical_width(row_bar_segment_count(active_families)) + text_width;
    fixed_widget_logical_width(active_families) + column_width * active_families
}

fn widget_width_limits_device(active_families: i32, dpi: u32, work_area_width: i32) -> (i32, i32) {
    let work_area_width = work_area_width.max(1);
    let active_families = active_families.max(1);
    let minimum_text_width = if active_families == 1 {
        TEXT_WIDTH
    } else {
        MIN_MULTI_PROVIDER_TEXT_WIDTH
    };
    let minimum_column_width = usage_bar_device_width(row_bar_segment_count(active_families), dpi)
        + scaled_for_dpi(minimum_text_width, dpi);
    let minimum = (fixed_widget_device_width(active_families, dpi)
        + minimum_column_width * active_families)
        .min(work_area_width);
    let maximum = scaled_for_dpi(MAX_WIDGET_WIDTH_LOGICAL, dpi)
        .min(work_area_width)
        .max(minimum);
    (minimum, maximum)
}

fn resolved_widget_width_device(
    saved_logical_width: Option<i32>,
    active_families: i32,
    dpi: u32,
    work_area_width: i32,
) -> i32 {
    let desired = saved_logical_width.map_or_else(
        || {
            fixed_widget_device_width(active_families, dpi)
                + (usage_bar_device_width(row_bar_segment_count(active_families), dpi)
                    + scaled_for_dpi(TEXT_WIDTH, dpi))
                    * active_families
        },
        |logical_width| scaled_for_dpi(logical_width, dpi),
    );
    let (minimum, maximum) = widget_width_limits_device(active_families, dpi, work_area_width);
    desired.clamp(minimum, maximum)
}

fn resolved_widget_size_device(
    saved_logical_width: Option<i32>,
    active_families: i32,
    dpi: u32,
    work_area_width: i32,
    required_height: i32,
) -> (i32, i32) {
    (
        resolved_widget_width_device(saved_logical_width, active_families, dpi, work_area_width),
        required_height,
    )
}

fn provider_column_width_for_client_at_dpi(
    client_width: i32,
    active_families: i32,
    dpi: u32,
) -> i32 {
    let active_families = active_families.max(1);
    let fixed = fixed_widget_device_width(active_families, dpi);
    ((client_width - fixed) / active_families).max(0)
}

fn provider_column_width_for_client(client_width: i32, active_families: i32) -> i32 {
    provider_column_width_for_client_at_dpi(
        client_width,
        active_families,
        CURRENT_DPI.load(Ordering::Relaxed),
    )
}

fn clamp_position_to_work_area(
    work_area: RECT,
    width: i32,
    height: i32,
    desired_x: i32,
    desired_y: i32,
) -> (i32, i32) {
    let max_x = (work_area.right - width).max(work_area.left);
    let max_y = (work_area.bottom - height).max(work_area.top);
    (
        desired_x.clamp(work_area.left, max_x),
        desired_y.clamp(work_area.top, max_y),
    )
}

fn default_popup_position(work_area: RECT, width: i32, height: i32) -> (i32, i32) {
    clamp_position_to_work_area(
        work_area,
        width,
        height,
        work_area.right - width,
        work_area.bottom - height,
    )
}

fn reset_saved_position(tray_offset: &mut i32, manual_position: &mut Option<(i32, i32)>) {
    *tray_offset = 0;
    *manual_position = None;
}

fn row_bar_segment_count(active_models: i32) -> i32 {
    match active_models {
        1 => SEGMENT_COUNT,
        2 => 5,
        _ => 4,
    }
}

fn total_widget_width_for(active_models: i32) -> i32 {
    sc(default_widget_width_logical_for(active_models))
}

fn total_widget_width_for_state(state: &AppState) -> i32 {
    total_widget_width_for(active_family_count_for_state(state))
}

fn active_family_count_for_state(state: &AppState) -> i32 {
    active_family_count(
        state.show_claude_code,
        state.show_codex,
        state.show_antigravity,
        state.show_github_copilot,
    )
}

fn total_widget_width() -> i32 {
    let active_models = {
        let state = lock_state();
        state
            .as_ref()
            .map(|s| {
                active_family_count(
                    s.show_claude_code,
                    s.show_codex,
                    s.show_antigravity,
                    s.show_github_copilot,
                )
            })
            .unwrap_or(1)
    };
    total_widget_width_for(active_models)
}

/// Solid identification color for each provider's usage bar — a brand-image
/// color, not an assertion of the exact official brand color. Always the
/// same regardless of `AppTheme`/warning state; see `PopupPalette` for the
/// theme-driven background/text/track/border colors these bars sit on top
/// of.
fn claude_accent_color() -> Color {
    Color::from_hex("#D97757")
}

fn codex_accent_color() -> Color {
    Color::from_hex("#7477E8")
}

fn antigravity_accent_color() -> Color {
    Color::from_hex("#4285F4")
}

fn github_copilot_accent_color() -> Color {
    Color::from_hex("#8250DF")
}

/// The popup's canonical color source (AUM-WINDOW-UI-01B). Both draw paths —
/// `render_layered`'s embedded/layered path and `paint`'s non-embedded
/// `WM_PAINT` fallback — build their colors by calling this function with
/// the user's chosen `AppTheme`, instead of each computing its own
/// light/dark color set. Provider accent colors (`claude_accent_color` and
/// friends) are intentionally not part of this palette: they identify a
/// provider, not the theme, and stay constant across all three themes.
struct PopupPalette {
    background: Color,
    /// Bar track / panel fill.
    track: Color,
    primary_text: Color,
    /// Used for the weekly pace guidance's secondary/detail lines (see
    /// `draw_weekly_pace_extra_lines`), which are already visually
    /// subordinate to the primary row text.
    secondary_text: Color,
    /// Divider/separator color (see `paint_content`'s left divider).
    border: Color,
    /// Value-text color for a session-row cell whose pace guidance flags a
    /// warning (currently only the 5h window's overpacing case — see
    /// `PaceGuidanceLines::is_warning`). Never applied to bar segments
    /// themselves, which always keep the provider's own accent color.
    warning: Color,
    /// Provider-name header row, including its Compact-only weekly-remaining
    /// text (see `draw_provider_header_row`). Equal to `primary_text` for
    /// RecommendedDark/Light (no visible change there); HighVisibility gives
    /// it its own color to separate section headings from ordinary body
    /// text.
    heading_text: Color,
}

fn popup_palette(theme: AppTheme) -> PopupPalette {
    match theme {
        AppTheme::RecommendedDark => PopupPalette {
            background: Color::from_hex("#11171D"),
            track: Color::from_hex("#18222B"),
            primary_text: Color::from_hex("#F4F8FB"),
            secondary_text: Color::from_hex("#AAB8C3"),
            border: Color::from_hex("#31424F"),
            warning: Color::from_hex("#F2B84B"),
            heading_text: Color::from_hex("#F4F8FB"),
        },
        AppTheme::Light => PopupPalette {
            background: Color::from_hex("#F4F7FA"),
            // A near-white-but-not-quite gray, not pure white, so the track
            // stays visible against the light background instead of
            // vanishing into it.
            track: Color::from_hex("#E3E9EF"),
            primary_text: Color::from_hex("#17212B"),
            secondary_text: Color::from_hex("#5D6C78"),
            border: Color::from_hex("#BDCCD7"),
            warning: Color::from_hex("#B15C00"),
            heading_text: Color::from_hex("#17212B"),
        },
        AppTheme::HighVisibility => PopupPalette {
            background: Color::from_hex("#000000"),
            track: Color::from_hex("#484848"),
            primary_text: Color::from_hex("#FFFFFF"),
            secondary_text: Color::from_hex("#00E5FF"),
            border: Color::from_hex("#FFFFFF"),
            warning: Color::from_hex("#FF4D4D"),
            heading_text: Color::from_hex("#FFFF00"),
        },
    }
}

/// Whether `theme` shows a 1px vertical divider between adjacent provider
/// columns (Claude Code/Codex, Codex/Antigravity). Only HighVisibility —
/// RecommendedDark/Light rely on the provider accent colors and spacing
/// alone to separate columns.
fn theme_shows_column_dividers(theme: AppTheme) -> bool {
    matches!(theme, AppTheme::HighVisibility)
}

/// Whether `theme` draws a 1px outline around each usage bar segment's
/// unfilled (track-colored) area. Only HighVisibility — never drawn over a
/// segment's provider-accent fill, see `draw_usage_bar`.
fn theme_outlines_usage_track(theme: AppTheme) -> bool {
    matches!(theme, AppTheme::HighVisibility)
}

/// Whether `theme` should use the "dark" variant of the per-provider value-
/// text tint (`claude_usage_text_color` and friends) when more than one
/// model is shown. Independent of `PopupPalette` — this only selects between
/// each function's two hardcoded tint variants, keyed off overall theme
/// darkness rather than a fourth copy of the theme's own colors.
fn theme_is_dark_variant(theme: AppTheme) -> bool {
    !matches!(theme, AppTheme::Light)
}

fn claude_usage_text_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_hex("#F09A7A")
    } else {
        Color::from_hex("#A94F32")
    }
}

fn codex_usage_text_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_hex("#F5F5F5")
    } else {
        Color::from_hex("#1F1F1F")
    }
}

fn antigravity_usage_text_color(is_dark: bool) -> Color {
    if is_dark {
        Color::from_hex("#8AB4F8")
    } else {
        Color::from_hex("#1967D2")
    }
}

pub fn run() {
    let winrt_initialized =
        is_packaged() && unsafe { RoInitialize(RO_INIT_SINGLETHREADED).is_ok() };

    // Enable Per-Monitor DPI Awareness V2 for crisp rendering at any scale factor
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CURRENT_DPI.store(GetDpiForSystem(), Ordering::Relaxed);
    }
    diagnose::log("window::run started");

    // Initializes COM (STA) on this, the UI thread, for the Virtual Desktop
    // scope feature. Safe to call even when `winrt_initialized` already set
    // up the same apartment type. Every `virtual_desktop` call happens on
    // this thread from here on.
    virtual_desktop::ensure_com_initialized();

    // Single-instance guard: silently exit if another instance is running.
    // Exception: when relaunched after an explorer restart (ENV_RELAUNCH set),
    // wait for the previous instance to release the mutex, then take over.
    let is_relaunch = std::env::var(ENV_RELAUNCH).is_ok();
    let mutex_name = native_interop::wide_str("Global\\ClaudeCodeUsageMonitor");
    let _mutex = unsafe {
        let handle = CreateMutexW(None, true, PCWSTR::from_raw(mutex_name.as_ptr()));
        match handle {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    if is_relaunch {
                        diagnose::log("relaunch: waiting for previous instance to exit");
                        let wait_result = WaitForSingleObject(h, 10_000);
                        if wait_result != WAIT_OBJECT_0 && wait_result != WAIT_ABANDONED {
                            diagnose::log(format!(
                                "startup aborted: previous instance did not exit cleanly ({wait_result:?})"
                            ));
                            return;
                        }
                    } else {
                        diagnose::log("startup aborted: another instance is already running");
                        return;
                    }
                }
                h
            }
            Err(error) => {
                diagnose::log_error(
                    "startup aborted: unable to create single-instance mutex",
                    error,
                );
                return;
            }
        }
    };

    let class_name = native_interop::wide_str("ClaudeCodeUsageMonitor");

    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap();
        let (large_icon, small_icon) = load_embedded_app_icons();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(hinstance.0),
            hIcon: large_icon,
            hIconSm: small_icon,
            hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };

        let atom = RegisterClassExW(&wc);
        if atom == 0 {
            diagnose::log("RegisterClassExW returned 0");
        }

        let settings = load_settings();
        let language_override = settings.language.as_deref().and_then(LanguageId::from_code);
        let language = localization::resolve_language(language_override);
        let install_channel = updater::current_install_channel();

        // Create as a top-level layered popup, anchored above the taskbar.
        let title = native_interop::wide_str(language.strings().window_title);
        let initial_model_count = active_family_count(
            settings.show_claude_code,
            settings.show_codex,
            settings.show_antigravity,
            settings.show_github_copilot,
        );
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            resolved_widget_width_device(
                settings.widget_width_logical,
                initial_model_count,
                96,
                i32::MAX / 4,
            ),
            sc(WIDGET_HEIGHT),
            HWND::default(),
            HMENU::default(),
            hinstance,
            None,
        )
        .unwrap();

        if !large_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_BIG as usize),
                LPARAM(large_icon.0 as isize),
            );
        }
        if !small_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_SMALL as usize),
                LPARAM(small_icon.0 as isize),
            );
        }

        diagnose::log(format!("main window created hwnd={:?}", hwnd));

        let is_dark = theme::is_dark_mode();

        {
            let mut state = lock_state();
            *state = Some(AppState {
                hwnd: SendHwnd::from_hwnd(hwnd),
                taskbar_hwnd: None,
                tray_notify_hwnd: None,
                win_event_hook: None,
                is_dark,
                embedded: false,
                language_override,
                language,
                install_channel,
                display_basis: settings.display_basis,
                reset_display_mode: settings.reset_display_mode,
                display_density: settings.display_density,
                short_window_visibility: settings.short_window_visibility,
                short_window_alert_sensitivity: settings.short_window_alert_sensitivity,
                popup_layout: settings.popup_layout,
                app_theme: settings.app_theme,
                session_state: CellState::Loading,
                session_percent: None,
                session_text: String::new(),
                session_pace: None,
                weekly_state: CellState::Loading,
                weekly_percent: None,
                weekly_text: String::new(),
                weekly_pace: None,
                weekly_remaining_text: None,
                codex_session_state: CellState::Loading,
                codex_session_percent: None,
                codex_session_text: String::new(),
                codex_session_pace: None,
                codex_weekly_state: CellState::Loading,
                codex_weekly_percent: None,
                codex_weekly_text: String::new(),
                codex_weekly_pace: None,
                codex_weekly_remaining_text: None,
                codex_banked_reset_count: BankedResetCount::Unavailable,
                codex_banked_reset_text: String::new(),
                antigravity_session_state: CellState::Loading,
                antigravity_session_percent: None,
                antigravity_session_text: String::new(),
                antigravity_session_pace: None,
                antigravity_weekly_state: CellState::Loading,
                antigravity_weekly_percent: None,
                antigravity_weekly_text: String::new(),
                antigravity_weekly_pace: None,
                antigravity_weekly_remaining_text: None,
                antigravity_extra_items: Vec::new(),
                github_copilot_state: CellState::Loading,
                github_copilot_percent: None,
                github_copilot_text: String::new(),
                show_claude_code: settings.show_claude_code,
                show_codex: settings.show_codex,
                show_antigravity: settings.show_antigravity,
                show_github_copilot: settings.show_github_copilot,
                github_copilot_plan: settings.github_copilot_plan,
                data: None,
                poll_interval_ms: settings.poll_interval_ms,
                retry_count: 0,
                force_notify_auth_error: false,
                auth_error_paused_polling: false,
                auth_watch_mode: poller::CredentialWatchMode::ActiveSource,
                auth_watch_snapshot: Vec::new(),
                last_poll_ok: false,
                update_status: UpdateStatus::Idle,
                last_update_check_unix: settings.last_update_check_unix,
                taskbar_index: settings.taskbar_index,
                tray_offset: settings.tray_offset,
                dragging: false,
                drag_start_mouse_x: 0,
                drag_start_mouse_y: 0,
                drag_start_window_x: 0,
                drag_start_window_y: 0,
                resize_session: None,
                auth_cta_hit_targets: Vec::new(),
                hovered_auth_cta_rect: None,
                manual_position: settings.manual_x.zip(settings.manual_y),
                widget_width_logical: settings.widget_width_logical,
                widget_visible: settings.widget_visible,
                always_on_top: settings.always_on_top,
                virtual_desktop_scope: settings.virtual_desktop_scope,
                virtual_desktop_target: settings.virtual_desktop_target.map(GUID::from_u128),
                virtual_desktop_current: None,
                // The window has not been shown yet at all at this point,
                // regardless of the `widget_visible` preference — matches
                // reality so the first real show/hide decision below (or
                // inside `apply_virtual_desktop_scope_at_startup` for
                // `CurrentOnly`) is not skipped as a false no-op.
                virtual_desktop_applied_visible: false,
                virtual_desktop_event_hook: None,
            });
            if let Some(s) = state.as_mut() {
                refresh_usage_texts(s);
            }
        }

        // Locate the taskbar to anchor the popup against; this does not
        // reparent the window. Regardless of whether a taskbar is found,
        // the window is always initialized as a top-level popup.
        select_taskbar_anchor(settings.taskbar_index);

        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        // Register the application-wide system tray icon.
        sync_tray_icons(hwnd);

        // CreateWindowExW used the legacy fixed height, which can exceed the
        // height of the rows actually visible. Size the hidden window before
        // the virtual-desktop COM call, which can dispatch a reentrant WM_PAINT.
        position_at_taskbar();

        // Resolve the saved virtual-desktop scope before the window is
        // shown, so a `CurrentOnly` widget never flashes visible first and
        // only then gets hidden. For `CurrentOnly` this is also the
        // window's actual first show/hide (via
        // `sync_effective_widget_visibility`'s `SetWindowPos`-based path,
        // not `ShowWindow`, which has first-call `nCmdShow` quirks this
        // avoids); for `All` it is a no-op and the plain `ShowWindow` below
        // remains the first show, unchanged from before this feature
        // existed.
        apply_virtual_desktop_scope_at_startup(hwnd);

        // Show only if widget_visible preference is true. Scope
        // `CurrentOnly` was already fully handled above.
        if settings.virtual_desktop_scope == VirtualDesktopScope::All {
            if settings.widget_visible {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                s.virtual_desktop_applied_visible = settings.widget_visible;
            }
        }
        diagnose::log("window shown");

        // Initial render via UpdateLayeredWindow (for embedded) or InvalidateRect (fallback)
        render_layered();
        // Apply the saved z-order after the initial show and layered render.
        // Explicitly applying NOTOPMOST when disabled also prevents a stale
        // topmost state from lingering.
        apply_always_on_top(hwnd, settings.always_on_top);

        // Poll timer: 15 minutes
        let initial_poll_ms = {
            let state = lock_state();
            state
                .as_ref()
                .map(|s| s.poll_interval_ms)
                .unwrap_or(POLL_15_MIN)
        };
        SetTimer(hwnd, TIMER_POLL, initial_poll_ms, None);

        // Watch for explorer.exe restarts so we can re-add the tray icon and
        // re-select the taskbar anchor (the shell discards tray registrations
        // when it restarts). Runs on a dedicated thread, independent of the
        // window's own message loop.
        spawn_taskbar_watchdog();

        // Initial poll
        let send_hwnd = SendHwnd::from_hwnd(hwnd);
        std::thread::spawn(move || {
            diagnose::log("initial poll thread started");
            do_poll(send_hwnd);
        });

        #[cfg(feature = "self-update")]
        {
            schedule_auto_update_check(hwnd);
            let should_check_updates = {
                let state = lock_state();
                state
                    .as_ref()
                    .map(|s| auto_update_check_due(s.last_update_check_unix))
                    .unwrap_or(false)
            };
            if should_check_updates {
                begin_update_check(hwnd, false);
            }
        }

        // Initial theme check
        check_theme_change();

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    if winrt_initialized {
        unsafe { RoUninitialize() };
    }
}

/// Render widget content and push to the layered window via UpdateLayeredWindow.
/// Renders fully opaque with the actual taskbar background colour so that
/// ClearType sub-pixel font rendering can be used for crisp, OS-native text.
fn render_layered() {
    refresh_dpi();
    let (
        hwnd_val,
        app_theme,
        embedded,
        strings,
        short_window_visibility,
        popup_layout,
        session_state,
        session_pct,
        session_text,
        session_pace,
        weekly_state,
        weekly_pct,
        weekly_text,
        weekly_pace,
        weekly_remaining_text,
        codex_session_state,
        codex_session_pct,
        codex_session_text,
        codex_session_pace,
        codex_weekly_state,
        codex_weekly_pct,
        codex_weekly_text,
        codex_weekly_pace,
        codex_weekly_remaining_text,
        codex_banked_reset_text,
        antigravity_session_state,
        antigravity_session_pct,
        antigravity_session_text,
        antigravity_session_pace,
        antigravity_weekly_pct,
        antigravity_weekly_text,
        antigravity_weekly_pace,
        antigravity_weekly_remaining_text,
        antigravity_extra_items,
        github_copilot_percent,
        github_copilot_text,
        show_claude_code,
        show_codex,
        show_antigravity,
        show_github_copilot,
        height,
        hovered_auth_cta_rect,
    ) = {
        let state = lock_state();
        match state.as_ref() {
            Some(s) => (
                s.hwnd,
                s.app_theme,
                s.embedded,
                s.language.strings(),
                s.short_window_visibility,
                s.popup_layout,
                s.session_state,
                s.session_percent,
                s.session_text.clone(),
                s.session_pace.clone(),
                s.weekly_state,
                s.weekly_percent,
                s.weekly_text.clone(),
                s.weekly_pace.clone(),
                s.weekly_remaining_text.clone(),
                s.codex_session_state,
                s.codex_session_percent,
                s.codex_session_text.clone(),
                s.codex_session_pace.clone(),
                s.codex_weekly_state,
                s.codex_weekly_percent,
                s.codex_weekly_text.clone(),
                s.codex_weekly_pace.clone(),
                s.codex_weekly_remaining_text.clone(),
                s.codex_banked_reset_text.clone(),
                s.antigravity_session_state,
                s.antigravity_session_percent,
                s.antigravity_session_text.clone(),
                s.antigravity_session_pace.clone(),
                s.antigravity_weekly_percent,
                s.antigravity_weekly_text.clone(),
                s.antigravity_weekly_pace.clone(),
                s.antigravity_weekly_remaining_text.clone(),
                s.antigravity_extra_items.clone(),
                s.github_copilot_percent,
                s.github_copilot_text.clone(),
                s.show_claude_code,
                s.show_codex,
                s.show_antigravity,
                s.show_github_copilot,
                widget_height_for_state(s),
                s.hovered_auth_cta_rect,
            ),
            None => return,
        }
    };

    let hwnd = hwnd_val.to_hwnd();

    // For non-embedded fallback, just invalidate and let WM_PAINT handle it
    if !embedded {
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
        }
        return;
    }

    let width = {
        let mut rect = RECT::default();
        unsafe {
            if GetClientRect(hwnd, &mut rect).is_ok() && rect.right > rect.left {
                rect.right - rect.left
            } else {
                total_widget_width()
            }
        }
    };

    let palette = popup_palette(app_theme);
    let provider_tint_dark = theme_is_dark_variant(app_theme);
    let show_column_dividers = theme_shows_column_dividers(app_theme);
    let outline_usage_track = theme_outlines_usage_track(app_theme);
    let accent = claude_accent_color();
    let codex_accent = codex_accent_color();
    let antigravity_accent = antigravity_accent_color();
    let mut auth_cta_hits: Vec<AuthCtaHitTarget> = Vec::new();

    unsafe {
        let screen_dc = GetDC(hwnd);

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let mem_dc = CreateCompatibleDC(screen_dc);
        let dib =
            CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();

        if dib.is_invalid() || bits.is_null() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(hwnd, screen_dc);
            return;
        }

        let old_bmp = SelectObject(mem_dc, dib);
        let pixel_count = (width * height) as usize;

        // Render once with the actual taskbar background colour.
        // Using an opaque background lets us use CLEARTYPE_QUALITY for
        // sub-pixel font rendering that matches the rest of the OS.
        paint_content(
            mem_dc,
            width,
            height,
            provider_tint_dark,
            &palette.background,
            &palette.primary_text,
            &palette.secondary_text,
            &accent,
            &palette.track,
            &palette.border,
            &palette.warning,
            &palette.heading_text,
            strings,
            short_window_visibility,
            session_state,
            session_pct,
            &session_text,
            session_pace.as_ref(),
            weekly_state,
            weekly_pct,
            &weekly_text,
            weekly_pace.as_ref(),
            weekly_remaining_text.as_deref(),
            codex_session_state,
            codex_session_pct,
            &codex_session_text,
            codex_session_pace.as_ref(),
            codex_weekly_state,
            codex_weekly_pct,
            &codex_weekly_text,
            codex_weekly_pace.as_ref(),
            codex_weekly_remaining_text.as_deref(),
            &codex_banked_reset_text,
            antigravity_session_state,
            antigravity_session_pct,
            &antigravity_session_text,
            antigravity_session_pace.as_ref(),
            antigravity_weekly_pct,
            &antigravity_weekly_text,
            antigravity_weekly_pace.as_ref(),
            antigravity_weekly_remaining_text.as_deref(),
            &antigravity_extra_items,
            github_copilot_percent,
            &github_copilot_text,
            show_claude_code,
            show_codex,
            show_antigravity,
            show_github_copilot,
            &codex_accent,
            &antigravity_accent,
            popup_layout,
            show_column_dividers,
            outline_usage_track,
            hovered_auth_cta_rect,
            &mut auth_cta_hits,
        );

        // Background pixels → alpha 1 (nearly invisible but still hittable for right-click).
        // Content pixels → fully opaque (preserves ClearType sub-pixel rendering).
        let bg_bgr = palette.background.to_colorref();
        let pixel_data = std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count);
        for px in pixel_data.iter_mut() {
            let rgb = *px & 0x00FFFFFF;
            if rgb == bg_bgr {
                *px = 0x01000000;
            } else {
                *px = rgb | 0xFF000000;
            }
        }

        // Push to window via UpdateLayeredWindow
        let pt_src = POINT { x: 0, y: 0 };
        let sz = SIZE {
            cx: width,
            cy: height,
        };
        let blend = BLENDFUNCTION {
            BlendOp: 0, // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1, // AC_SRC_ALPHA
        };

        let _ = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            None,
            Some(&sz),
            mem_dc,
            Some(&pt_src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );

        // Cleanup
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(hwnd, screen_dc);
    }

    let mut state = lock_state();
    if let Some(s) = state.as_mut() {
        s.auth_cta_hit_targets = auth_cta_hits;
    }
}

/// Paint all widget content onto a DC with a given background color.
fn paint_content(
    hdc: HDC,
    width: i32,
    height: i32,
    provider_tint_dark: bool,
    bg: &Color,
    text_color: &Color,
    secondary_text: &Color,
    accent: &Color,
    track: &Color,
    border: &Color,
    warning: &Color,
    heading_text: &Color,
    strings: Strings,
    short_window_visibility: ShortWindowVisibility,
    session_state: CellState,
    session_pct: Option<f64>,
    session_text: &str,
    session_pace: Option<&PaceGuidanceLines>,
    weekly_state: CellState,
    weekly_pct: Option<f64>,
    weekly_text: &str,
    weekly_pace: Option<&PaceGuidanceLines>,
    weekly_remaining_text: Option<&str>,
    codex_session_state: CellState,
    codex_session_pct: Option<f64>,
    codex_session_text: &str,
    codex_session_pace: Option<&PaceGuidanceLines>,
    codex_weekly_state: CellState,
    codex_weekly_pct: Option<f64>,
    codex_weekly_text: &str,
    codex_weekly_pace: Option<&PaceGuidanceLines>,
    codex_weekly_remaining_text: Option<&str>,
    codex_banked_reset_text: &str,
    antigravity_session_state: CellState,
    antigravity_session_pct: Option<f64>,
    antigravity_session_text: &str,
    antigravity_session_pace: Option<&PaceGuidanceLines>,
    antigravity_weekly_pct: Option<f64>,
    antigravity_weekly_text: &str,
    antigravity_weekly_pace: Option<&PaceGuidanceLines>,
    antigravity_weekly_remaining_text: Option<&str>,
    antigravity_extra_items: &[AntigravityItemDisplay],
    github_copilot_percent: Option<f64>,
    github_copilot_text: &str,
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    show_github_copilot: bool,
    codex_accent: &Color,
    antigravity_accent: &Color,
    popup_layout: PopupLayout,
    show_column_dividers: bool,
    outline_usage_track: bool,
    hovered_auth_cta_rect: Option<RECT>,
    auth_cta_hits: &mut Vec<AuthCtaHitTarget>,
) {
    unsafe {
        let client_rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };

        let bg_brush = CreateSolidBrush(COLORREF(bg.to_colorref()));
        FillRect(hdc, &client_rect, bg_brush);
        let _ = DeleteObject(bg_brush);

        // Left divider — a single solid color from the palette's `border`
        // (previously two hand-picked is_dark/light RGB tuples forming a
        // bevel; now sourced from the same theme-driven color for both
        // halves, consistent with the "single canonical border color"
        // consolidation).
        let divider_h = sc(25);
        let divider_top = (height - divider_h) / 2;
        let divider_bottom = divider_top + divider_h;

        let divider_brush = CreateSolidBrush(COLORREF(border.to_colorref()));
        let left_rect = RECT {
            left: 0,
            top: divider_top,
            right: sc(2),
            bottom: divider_bottom,
        };
        FillRect(hdc, &left_rect, divider_brush);

        let right_rect = RECT {
            left: sc(2),
            top: divider_top,
            right: sc(3),
            bottom: divider_bottom,
        };
        FillRect(hdc, &right_rect, divider_brush);
        let _ = DeleteObject(divider_brush);

        let content_x = sc(LEFT_DIVIDER_W) + sc(DIVIDER_RIGHT_MARGIN);

        // AUM-WINDOW-UI-01B: HighVisibility-only outline traced around each
        // usage bar segment's unfilled (track-colored) area — see
        // `draw_usage_bar`, which never draws it over the provider-accent
        // fill. `None` for every other theme, so `draw_row`/`draw_usage_bar`
        // skip the outline entirely.
        let track_outline: Option<&Color> = if outline_usage_track {
            Some(border)
        } else {
            None
        };

        // AUM-PACE-GUIDANCE-01: same predicates as the `&AppState`-based
        // `weekly_pace_extra_lines`/`needs_session_row` (used for popup
        // sizing), applied to this function's own flattened params, so the
        // layout computed here always agrees with what
        // `widget_height_for_state` sized the popup to.
        let weekly_lines = weekly_pace_extra_lines_shown(
            show_claude_code,
            weekly_pace,
            show_codex,
            codex_weekly_pace,
            show_antigravity,
            antigravity_weekly_pace,
        );
        let claude_session_decision = session_cell_decision(
            session_state,
            session_pct,
            session_text,
            session_pace,
            short_window_visibility,
        );
        let codex_session_decision = session_cell_decision(
            codex_session_state,
            codex_session_pct,
            codex_session_text,
            codex_session_pace,
            short_window_visibility,
        );
        let claude_session_auth_action = provider_auth_action(QuotaFamilyId::Claude, session_state);
        let codex_session_auth_action =
            provider_auth_action(QuotaFamilyId::Codex, codex_session_state);
        let claude_weekly_auth_action = provider_auth_action(QuotaFamilyId::Claude, weekly_state);
        let codex_weekly_auth_action =
            provider_auth_action(QuotaFamilyId::Codex, codex_weekly_state);
        let antigravity_session_decision = session_cell_decision(
            antigravity_session_state,
            antigravity_session_pct,
            antigravity_session_text,
            antigravity_session_pace,
            short_window_visibility,
        );
        let needs_session_row = session_row_visible(
            show_claude_code,
            session_row_cell_shows(
                session_state,
                session_pct,
                session_text,
                session_pace,
                short_window_visibility,
            ),
            show_codex,
            session_row_cell_shows(
                codex_session_state,
                codex_session_pct,
                codex_session_text,
                codex_session_pace,
                short_window_visibility,
            ),
            show_antigravity,
            session_row_cell_shows(
                antigravity_session_state,
                antigravity_session_pct,
                antigravity_session_text,
                antigravity_session_pace,
                short_window_visibility,
            ),
        );
        let needs_weekly_row = quota_row_visible(&[
            (show_claude_code, true),
            (show_codex, true),
            (show_antigravity, true),
            (show_github_copilot, false),
        ]);
        let needs_monthly_row = quota_row_visible(&[
            (show_claude_code, false),
            (show_codex, false),
            (show_antigravity, false),
            (show_github_copilot, true),
        ]);
        let rows = visible_rows_for_quota(
            popup_layout,
            needs_weekly_row,
            weekly_lines,
            needs_session_row,
            needs_monthly_row,
            antigravity_extra_items.len() as i32,
        );
        let layout = pace_row_layout(height, rows);

        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(text_color.to_colorref()));

        let font_name = native_interop::wide_str("Segoe UI");
        let font = CreateFontW(
            sc(-12),
            0,
            0,
            0,
            FW_MEDIUM.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            CLEARTYPE_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR::from_raw(font_name.as_ptr()),
        );
        let old_font = SelectObject(hdc, font);

        let help_rect =
            help_button_rect_for_client_width(width, CURRENT_DPI.load(Ordering::Relaxed));
        let help_border_brush = CreateSolidBrush(COLORREF(border.to_colorref()));
        FrameRect(hdc, &help_rect, help_border_brush);
        let _ = DeleteObject(help_border_brush);
        let _ = SetTextColor(hdc, COLORREF(heading_text.to_colorref()));
        let mut help_text_rect = help_rect;
        let mut help_text = native_interop::wide_str("?");
        let _ = DrawTextW(
            hdc,
            &mut help_text,
            &mut help_text_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = SetTextColor(hdc, COLORREF(text_color.to_colorref()));

        draw_provider_header_row(
            hdc,
            width,
            content_x,
            layout.provider_header_y,
            heading_text,
            strings,
            show_claude_code,
            show_codex,
            show_antigravity,
            show_github_copilot,
            popup_layout == PopupLayout::Compact,
            weekly_remaining_text,
            codex_weekly_remaining_text,
            antigravity_weekly_remaining_text,
            codex_banked_reset_text,
        );

        // AUM-PACE-GUIDANCE-01: when a provider has usable weekly pace
        // guidance (`CellState::Ok` with a real percentage/reset — see
        // `weekly_pace_for_cell`), its bar-row text becomes the guidance
        // `primary` line (percent + basis prefix + pace-status word)
        // instead of the plain percent+reset text; otherwise the existing
        // status/plain text is unchanged.
        let weekly_row_text = weekly_pace
            .map(|l| l.primary.as_str())
            .unwrap_or(weekly_text);
        let codex_weekly_row_text = codex_weekly_pace
            .map(|l| l.primary.as_str())
            .unwrap_or(codex_weekly_text);
        let antigravity_weekly_row_text = antigravity_weekly_pace
            .map(|l| l.primary.as_str())
            .unwrap_or(antigravity_weekly_text);

        let github_copilot_accent = github_copilot_accent_color();
        let mut weekly_cells = Vec::new();
        if show_claude_code {
            weekly_cells.push(RowCell {
                percent: weekly_pct,
                text: weekly_row_text,
                accent,
                provider_text_color: claude_usage_text_color(provider_tint_dark),
                is_warning: false,
                auth_action: claude_weekly_auth_action,
            });
        }
        if show_codex {
            weekly_cells.push(RowCell {
                percent: codex_weekly_pct,
                text: codex_weekly_row_text,
                accent: codex_accent,
                provider_text_color: codex_usage_text_color(provider_tint_dark),
                is_warning: false,
                auth_action: codex_weekly_auth_action,
            });
        }
        if show_antigravity {
            weekly_cells.push(RowCell {
                percent: antigravity_weekly_pct,
                text: antigravity_weekly_row_text,
                accent: antigravity_accent,
                provider_text_color: antigravity_usage_text_color(provider_tint_dark),
                is_warning: false,
                auth_action: None,
            });
        }
        if show_github_copilot {
            weekly_cells.push(RowCell {
                percent: None,
                text: strings.not_available,
                accent: &github_copilot_accent,
                provider_text_color: github_copilot_accent,
                is_warning: false,
                auth_action: None,
            });
        }

        if let Some(weekly_row_y) = layout.weekly_row_y {
            draw_row(
                hdc,
                width,
                content_x,
                weekly_row_y,
                text_color,
                strings.weekly_window,
                &weekly_cells,
                track,
                warning,
                // The weekly row never carries a warning flag of its own — see
                // `weekly_pace_guidance_lines`, which always sets
                // `PaceGuidanceLines::is_warning` to `false`.
                track_outline,
                hovered_auth_cta_rect,
                auth_cta_hits,
            );
        }

        // AUM-PACE-GUIDANCE-01: weekly secondary/detail lines, one column
        // per shown provider, aligned under that provider's own bar (same
        // column x positions `draw_row`/`draw_provider_header_row` use).
        // `None` (not `CellState::Ok`, or no usable pace data) leaves that
        // provider's column blank for this block rather than showing stale
        // text.
        if let Some(secondary_y) = layout.weekly_secondary_y {
            let (claude_col_x, codex_col_x, antigravity_col_x) = provider_column_x_positions(
                width,
                content_x,
                show_claude_code,
                show_codex,
                show_antigravity,
                show_github_copilot,
            );
            let pace_column_width = provider_column_width_for_client(
                width,
                active_family_count(
                    show_claude_code,
                    show_codex,
                    show_antigravity,
                    show_github_copilot,
                ),
            );

            if show_claude_code {
                draw_weekly_pace_extra_lines(
                    hdc,
                    claude_col_x,
                    secondary_y,
                    pace_column_width,
                    weekly_pace,
                    secondary_text,
                );
            }
            if show_codex {
                draw_weekly_pace_extra_lines(
                    hdc,
                    codex_col_x,
                    secondary_y,
                    pace_column_width,
                    codex_weekly_pace,
                    secondary_text,
                );
            }
            if show_antigravity {
                draw_weekly_pace_extra_lines(
                    hdc,
                    antigravity_col_x,
                    secondary_y,
                    pace_column_width,
                    antigravity_weekly_pace,
                    secondary_text,
                );
            }
        }

        // AUM-PACE-GUIDANCE-01: the 5h bar row is now the sole
        // `ShortWindowVisibility`-gated 5h display — see
        // `session_cell_decision`. A provider whose decision says not to
        // show (suppressed by `Hidden`, or `WarningOnly` with nothing to
        // warn about) shows a fully blank cell in this row; a provider
        // that's not `CellState::Ok` keeps its existing status text
        // instead of going blank. The row itself is skipped entirely (see
        // `layout.session_row_y`/`needs_session_row`) when no shown
        // provider has an overuse warning; status-only cells do not keep
        // the row alive under `WarningOnly`.
        if let Some(session_row_y) = layout.session_row_y {
            let mut session_cells = Vec::new();
            if show_claude_code {
                session_cells.push(RowCell {
                    percent: claude_session_decision.1,
                    text: claude_session_decision.2,
                    accent,
                    provider_text_color: claude_usage_text_color(provider_tint_dark),
                    is_warning: claude_session_decision.3,
                    auth_action: claude_session_auth_action,
                });
            }
            if show_codex {
                session_cells.push(RowCell {
                    percent: codex_session_decision.1,
                    text: codex_session_decision.2,
                    accent: codex_accent,
                    provider_text_color: codex_usage_text_color(provider_tint_dark),
                    is_warning: codex_session_decision.3,
                    auth_action: codex_session_auth_action,
                });
            }
            if show_antigravity {
                session_cells.push(RowCell {
                    percent: antigravity_session_decision.1,
                    text: antigravity_session_decision.2,
                    accent: antigravity_accent,
                    provider_text_color: antigravity_usage_text_color(provider_tint_dark),
                    is_warning: antigravity_session_decision.3,
                    auth_action: None,
                });
            }
            if show_github_copilot {
                session_cells.push(RowCell {
                    percent: None,
                    text: strings.not_available,
                    accent: &github_copilot_accent,
                    provider_text_color: github_copilot_accent,
                    is_warning: false,
                    auth_action: None,
                });
            }
            draw_row(
                hdc,
                width,
                content_x,
                session_row_y,
                text_color,
                strings.session_window,
                &session_cells,
                track,
                warning,
                track_outline,
                hovered_auth_cta_rect,
                auth_cta_hits,
            );
        }

        if let Some(monthly_row_y) = layout.monthly_row_y {
            let mut monthly_cells = Vec::new();
            if show_claude_code {
                monthly_cells.push(RowCell {
                    percent: None,
                    text: strings.not_available,
                    accent,
                    provider_text_color: claude_usage_text_color(provider_tint_dark),
                    is_warning: false,
                    auth_action: None,
                });
            }
            if show_codex {
                monthly_cells.push(RowCell {
                    percent: None,
                    text: strings.not_available,
                    accent: codex_accent,
                    provider_text_color: codex_usage_text_color(provider_tint_dark),
                    is_warning: false,
                    auth_action: None,
                });
            }
            if show_antigravity {
                monthly_cells.push(RowCell {
                    percent: None,
                    text: strings.not_available,
                    accent: antigravity_accent,
                    provider_text_color: antigravity_usage_text_color(provider_tint_dark),
                    is_warning: false,
                    auth_action: None,
                });
            }
            monthly_cells.push(RowCell {
                percent: github_copilot_percent,
                text: github_copilot_text,
                accent: &github_copilot_accent,
                provider_text_color: github_copilot_accent,
                is_warning: false,
                auth_action: None,
            });
            draw_row(
                hdc,
                width,
                content_x,
                monthly_row_y,
                text_color,
                strings.monthly_window,
                &monthly_cells,
                track,
                warning,
                track_outline,
                hovered_auth_cta_rect,
                auth_cta_hits,
            );
        }

        // Antigravity's cache can carry any number of quota keys beyond
        // `gemini-weekly` (already drawn in the weekly row above) — every
        // one of them gets its own persistent row here, labeled with
        // whatever safe display label `antigravity_statusline` gave it
        // (a raw, un-renamed key for anything this build doesn't
        // specifically recognize), never hidden in a details-only view.
        for (item, &row_y) in antigravity_extra_items
            .iter()
            .zip(layout.antigravity_extra_rows_y.iter())
        {
            draw_row(
                hdc,
                width,
                content_x,
                row_y,
                text_color,
                &item.label,
                &[RowCell {
                    percent: item.percent,
                    text: &item.text,
                    accent: antigravity_accent,
                    provider_text_color: antigravity_usage_text_color(provider_tint_dark),
                    is_warning: false,
                    auth_action: None,
                }],
                track,
                warning,
                track_outline,
                hovered_auth_cta_rect,
                auth_cta_hits,
            );
        }

        // AUM-WINDOW-UI-01B: outer 1px frame in the palette's border color,
        // drawn last (on top of the rows/divider) and inset within the
        // existing client rect so it never expands the popup's bounds.
        // Shared by both draw paths since both call this function.
        let outline_w = sc(1).max(1);
        let outline_brush = CreateSolidBrush(COLORREF(border.to_colorref()));
        FillRect(
            hdc,
            &RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: outline_w,
            },
            outline_brush,
        );
        FillRect(
            hdc,
            &RECT {
                left: 0,
                top: height - outline_w,
                right: width,
                bottom: height,
            },
            outline_brush,
        );
        FillRect(
            hdc,
            &RECT {
                left: 0,
                top: 0,
                right: outline_w,
                bottom: height,
            },
            outline_brush,
        );
        FillRect(
            hdc,
            &RECT {
                left: width - outline_w,
                top: 0,
                right: width,
                bottom: height,
            },
            outline_brush,
        );
        let _ = DeleteObject(outline_brush);

        // AUM-WINDOW-UI-01B: HighVisibility-only 1px column dividers between
        // adjacent shown provider columns, in the same border color as the
        // outer frame above. Positioned inside the existing
        // `MODEL_RIGHT_MARGIN` gap `draw_row`/`provider_column_x_positions`
        // already leave between columns, so it never overlaps a column's
        // text or bar.
        if show_column_dividers {
            let active_families = active_family_count(
                show_claude_code,
                show_codex,
                show_antigravity,
                show_github_copilot,
            );
            let divider_column_width = provider_column_width_for_client(width, active_families);
            let column_divider_w = sc(1).max(1);
            let column_divider_brush = CreateSolidBrush(COLORREF(border.to_colorref()));
            let first_column_x = content_x + sc(LABEL_WIDTH) + sc(LABEL_RIGHT_MARGIN);
            for index in 1..active_families {
                let boundary_x = first_column_x
                    + index * divider_column_width
                    + (index - 1) * sc(MODEL_RIGHT_MARGIN)
                    + sc(MODEL_RIGHT_MARGIN) / 2;
                FillRect(
                    hdc,
                    &RECT {
                        left: boundary_x,
                        top: 0,
                        right: boundary_x + column_divider_w,
                        bottom: height,
                    },
                    column_divider_brush,
                );
            }
            let _ = DeleteObject(column_divider_brush);
        }

        SelectObject(hdc, old_font);
        let _ = DeleteObject(font);
    }
}

fn do_poll(send_hwnd: SendHwnd) {
    let hwnd = send_hwnd.to_hwnd();
    let (show_claude_code, show_codex, show_antigravity, show_github_copilot, github_copilot_plan) = {
        let state = lock_state();
        state
            .as_ref()
            .map(|s| {
                (
                    s.show_claude_code,
                    s.show_codex,
                    s.show_antigravity,
                    s.show_github_copilot,
                    s.github_copilot_plan,
                )
            })
            .unwrap_or((
                true,
                false,
                false,
                false,
                poller::GithubCopilotPlan::Unknown,
            ))
    };

    let report = poller::poll_report_with_github_copilot_updates(
        show_claude_code,
        show_codex,
        show_antigravity,
        show_github_copilot,
        github_copilot_plan,
        |provider, outcome| {
            {
                let mut state = lock_state();
                if let Some(state) = state.as_mut() {
                    apply_provider_poll_update(state, provider, outcome);
                }
            }
            unsafe {
                let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        },
    );

    match report.clone().into_app_usage_data() {
        Ok(data) => {
            persist_poll_snapshot(&report);

            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                // Classify availability straight from this poll's outcome for
                // every provider (not just the ones that succeeded): a
                // provider that failed this round while others succeeded
                // must not keep showing its old percentage as current.
                let (session_state, weekly_state) =
                    poll_cell_states(QuotaFamilyId::Claude, &report.claude_code);
                s.session_state = session_state;
                s.weekly_state = weekly_state;
                let (codex_session_state, codex_weekly_state) =
                    poll_cell_states(QuotaFamilyId::Codex, &report.codex);
                s.codex_session_state = codex_session_state;
                s.codex_weekly_state = codex_weekly_state;
                s.codex_banked_reset_count = banked_reset_count_for_poll(&report.codex);
                s.antigravity_session_state =
                    poll_cell_states(QuotaFamilyId::Antigravity, &report.antigravity).0;
                s.antigravity_weekly_state = antigravity_provider_cell_state(&report.antigravity);
                s.github_copilot_state = poll_quota_item_state(
                    QuotaFamilyId::GithubCopilot,
                    &report.github_copilot,
                    GITHUB_COPILOT_MONTHLY_ITEM_ID,
                );

                // Stop fast-poll if reset data is now fresh
                if !poller::app_is_past_reset(&data) {
                    unsafe {
                        let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                    }
                }

                merge_successful_providers(&mut s.data, &report);
                s.last_poll_ok = true;
                refresh_usage_texts(s);

                // Recovered from errors — restore normal poll interval
                if s.retry_count > 0 {
                    s.retry_count = 0;
                    let interval = s.poll_interval_ms;
                    unsafe {
                        SetTimer(hwnd, TIMER_POLL, interval, None);
                    }
                }
                s.force_notify_auth_error = false;
                s.auth_error_paused_polling = false;
                s.auth_watch_mode = poller::CredentialWatchMode::ActiveSource;
                s.auth_watch_snapshot.clear();
            }

            unsafe {
                let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
        Err(e) => {
            let auth_watch = match e {
                poller::PollError::AuthRequired
                | poller::PollError::TokenExpired
                | poller::PollError::NoCredentials
                    if show_github_copilot
                        && !show_claude_code
                        && !show_codex
                        && !show_antigravity =>
                {
                    None
                }
                poller::PollError::AuthRequired | poller::PollError::TokenExpired => Some((
                    poller::CredentialWatchMode::ActiveSource,
                    poller::credential_watch_snapshot(poller::CredentialWatchMode::ActiveSource),
                )),
                poller::PollError::NoCredentials => Some((
                    poller::CredentialWatchMode::AllSources,
                    poller::credential_watch_snapshot(poller::CredentialWatchMode::AllSources),
                )),
                poller::PollError::RequestFailed => None,
            };
            // Distinguish auth-required errors from transient errors.
            let notify_auth_error = {
                let mut state = lock_state();
                let mut should_notify = false;
                if let Some(s) = state.as_mut() {
                    s.last_poll_ok = false;

                    // The whole poll failed, but classify per-provider from
                    // `report` anyway (Disabled/Error only here): this
                    // distinguishes provider-specific error states instead
                    // of collapsing everything into one generic error word,
                    // and `refresh_usage_texts` below
                    // reads these states to keep the bar unfilled rather
                    // than leaving the last successful percentage on screen.
                    let (session_state, weekly_state) =
                        poll_cell_states(QuotaFamilyId::Claude, &report.claude_code);
                    s.session_state = session_state;
                    s.weekly_state = weekly_state;
                    let (codex_session_state, codex_weekly_state) =
                        poll_cell_states(QuotaFamilyId::Codex, &report.codex);
                    s.codex_session_state = codex_session_state;
                    s.codex_weekly_state = codex_weekly_state;
                    s.codex_banked_reset_count = banked_reset_count_for_poll(&report.codex);
                    s.antigravity_session_state =
                        poll_cell_states(QuotaFamilyId::Antigravity, &report.antigravity).0;
                    s.antigravity_weekly_state =
                        antigravity_provider_cell_state(&report.antigravity);
                    s.github_copilot_state = poll_quota_item_state(
                        QuotaFamilyId::GithubCopilot,
                        &report.github_copilot,
                        GITHUB_COPILOT_MONTHLY_ITEM_ID,
                    );
                    // No-op in practice today (a total-failure `report` never
                    // contains a `Success` outcome), but keeps this branch
                    // using the exact same update path as the success branch
                    // instead of relying on that invariant.
                    merge_successful_providers(&mut s.data, &report);
                    refresh_usage_texts(s);

                    match auth_watch {
                        Some((watch_mode, watch_snapshot)) => {
                            // Only show the balloon on the first failure so it doesn't spam.
                            if s.retry_count == 0 || s.force_notify_auth_error {
                                should_notify = true;
                            }
                            s.force_notify_auth_error = false;
                            s.auth_error_paused_polling = true;
                            s.auth_watch_mode = watch_mode;
                            s.auth_watch_snapshot = watch_snapshot;
                            s.retry_count = s.retry_count.saturating_add(1);
                            unsafe {
                                let _ = KillTimer(hwnd, TIMER_POLL);
                                let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                                let _ = KillTimer(hwnd, TIMER_COUNTDOWN);
                                SetTimer(hwnd, TIMER_POLL, s.poll_interval_ms, None);
                            }
                        }
                        _ => {
                            // Transient network / credential-missing errors: exponential backoff.
                            s.force_notify_auth_error = false;
                            s.auth_error_paused_polling = false;
                            s.auth_watch_mode = poller::CredentialWatchMode::ActiveSource;
                            s.auth_watch_snapshot.clear();
                            s.retry_count = s.retry_count.saturating_add(1);
                            let backoff = RETRY_BASE_MS.saturating_mul(
                                1u32.checked_shl(s.retry_count - 1).unwrap_or(u32::MAX),
                            );
                            let retry_ms = backoff.min(s.poll_interval_ms);
                            unsafe {
                                let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                                SetTimer(hwnd, TIMER_POLL, retry_ms, None);
                            }
                        }
                    }
                }
                should_notify
            };

            if notify_auth_error {
                let balloon = {
                    let state = lock_state();
                    state.as_ref().map(|s| {
                        if s.show_claude_code {
                            (
                                s.language.strings().token_expired_title,
                                s.language.strings().token_expired_body,
                            )
                        } else if s.show_codex {
                            (
                                s.language.strings().codex_token_expired_title,
                                s.language.strings().codex_token_expired_body,
                            )
                        } else {
                            (
                                s.language.strings().antigravity_token_expired_title,
                                s.language.strings().antigravity_token_expired_body,
                            )
                        }
                    })
                };
                if let Some((title, body)) = balloon {
                    tray_icon::notify_balloon(hwnd, title, body);
                }
            }

            unsafe {
                let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// Persist a snapshot of a successful poll. Failures here are logged as fixed,
/// non-identifying warnings and never affect the already-completed poll result
/// or the next poll.
fn persist_poll_snapshot(report: &poller::PollReport) {
    let Some(local_data_root) = snapshot_store::local_data_root() else {
        diagnose::log("snapshot persistence skipped: local data root unavailable");
        return;
    };

    let machine_id = match snapshot_store::ensure_machine_id(&local_data_root) {
        Ok(machine_id) => machine_id,
        Err(_) => {
            diagnose::log("snapshot persistence skipped: machine id unavailable");
            return;
        }
    };

    let snapshot =
        match snapshot_schema::snapshot_from_poll_report(&machine_id, report, SystemTime::now()) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                diagnose::log("snapshot persistence skipped: unable to build snapshot");
                return;
            }
        };

    let paths = snapshot_store::SnapshotPaths::new(&local_data_root, &machine_id);
    let outcome = snapshot_store::persist_snapshot(&paths, &snapshot);
    match (outcome.current, outcome.history) {
        (snapshot_store::PersistResult::Saved, snapshot_store::PersistResult::Saved) => {}
        (snapshot_store::PersistResult::Failed, snapshot_store::PersistResult::Saved) => {
            diagnose::log("snapshot persistence warning: current snapshot save failed");
        }
        (snapshot_store::PersistResult::Saved, snapshot_store::PersistResult::Failed) => {
            diagnose::log("snapshot persistence warning: history snapshot save failed");
        }
        (snapshot_store::PersistResult::Failed, snapshot_store::PersistResult::Failed) => {
            diagnose::log("snapshot persistence warning: current and history snapshot save failed");
        }
    }
}

fn schedule_countdown_timer() {
    let state = lock_state();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return,
    };

    let hwnd = s.hwnd.to_hwnd();
    if !s.last_poll_ok {
        unsafe {
            let _ = KillTimer(hwnd, TIMER_COUNTDOWN);
            let _ = KillTimer(hwnd, TIMER_RESET_POLL);
        }
        return;
    }

    let data = match &s.data {
        Some(d) => d,
        None => return,
    };

    // If a reset time has passed, poll every 5s to pick up fresh data
    if poller::app_is_past_reset(data) {
        unsafe {
            SetTimer(hwnd, TIMER_RESET_POLL, 5_000, None);
        }
    }

    let min_delay = data
        .families
        .iter()
        .flat_map(|family| family.items.iter())
        .filter_map(|item| poller::time_until_display_change(item.resets_at))
        .min();

    let ms = min_delay
        .unwrap_or(Duration::from_secs(60))
        .as_millis()
        .max(1000) as u32;

    unsafe {
        SetTimer(hwnd, TIMER_COUNTDOWN, ms, None);
    }
}

fn check_theme_change() {
    let new_dark = theme::is_dark_mode();
    let changed = {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            if s.is_dark != new_dark {
                s.is_dark = new_dark;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if changed {
        render_layered();
    }
}

fn check_language_change() {
    if update_language_change() {
        render_layered();
    }
}

fn update_display() {
    let mut state = lock_state();
    let s = match state.as_mut() {
        Some(s) => s,
        None => return,
    };

    // Don't overwrite error text with stale cached data
    if !s.last_poll_ok {
        return;
    }

    refresh_usage_texts(s);
}

fn suppress_tray_reposition_for(duration: Duration) {
    let mut until = SUPPRESS_TRAY_REPOSITION_UNTIL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *until = Some(Instant::now() + duration);
}

fn tray_reposition_is_suppressed() -> bool {
    let now = Instant::now();
    let mut until = SUPPRESS_TRAY_REPOSITION_UNTIL
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    match *until {
        Some(deadline) if now < deadline => true,
        Some(_) => {
            *until = None;
            false
        }
        None => false,
    }
}

fn position_at_taskbar() {
    refresh_dpi();
    // Drop the app-state lock before any Win32 call that may synchronously
    // re-enter our window procedure.
    let (hwnd, taskbar_hwnd, manual_position, saved_width, active_families) = {
        let state = lock_state();
        let s = match state.as_ref() {
            Some(s) => s,
            None => return,
        };

        // Don't fight the user's drag
        if s.dragging {
            return;
        }

        (
            s.hwnd.to_hwnd(),
            s.taskbar_hwnd,
            s.manual_position,
            s.widget_width_logical,
            active_family_count_for_state(s),
        )
    };

    let widget_height = widget_height();
    let work_area = manual_position
        .and_then(|(x, y)| native_interop::get_monitor_work_area_for_point(POINT { x, y }))
        .or_else(|| native_interop::get_monitor_work_area(hwnd))
        .or_else(|| taskbar_hwnd.and_then(native_interop::get_monitor_work_area));
    let Some(work_area) = work_area else {
        diagnose::log("position_at_taskbar skipped: unable to query monitor work area");
        return;
    };
    let widget_width = resolved_widget_width_device(
        saved_width,
        active_families,
        CURRENT_DPI.load(Ordering::Relaxed),
        work_area.right - work_area.left,
    );
    let (x, y) = match manual_position {
        Some((x, y)) => clamp_position_to_work_area(work_area, widget_width, widget_height, x, y),
        None => default_popup_position(work_area, widget_width, widget_height),
    };
    if manual_position.is_some() && manual_position != Some((x, y)) {
        {
            let mut state = lock_state();
            if let Some(state) = state.as_mut() {
                state.manual_position = Some((x, y));
            }
        }
        save_state_settings();
    }
    native_interop::move_window(hwnd, x, y, widget_width, widget_height);
    diagnose::log(format!(
        "positioned popup at x={x} y={y} w={widget_width} h={widget_height}"
    ));
}

fn resize_limits_for_window(hwnd: HWND) -> (i32, i32, i32) {
    let (active_families, required_height) = {
        let state = lock_state();
        state
            .as_ref()
            .map(|s| (active_family_count_for_state(s), widget_height_for_state(s)))
            .unwrap_or((1, sc(WIDGET_HEIGHT)))
    };
    let dpi = window_dpi(hwnd);
    let work_area_width = native_interop::get_monitor_work_area(hwnd)
        .map(|area| area.right - area.left)
        .unwrap_or_else(|| scaled_for_dpi(MAX_WIDGET_WIDTH_LOGICAL, dpi));
    let (minimum, maximum) = widget_width_limits_device(active_families, dpi, work_area_width);
    (minimum, maximum, required_height)
}

fn finish_horizontal_resize(hwnd: HWND) {
    let rect = native_interop::get_window_rect_safe(hwnd);
    let dpi = window_dpi(hwnd);
    {
        let mut state = lock_state();
        if let (Some(state), Some(rect)) = (state.as_mut(), rect) {
            let (logical_width, manual_position) =
                completed_resize_settings(rect, dpi, state.manual_position);
            state.widget_width_logical = Some(logical_width);
            state.manual_position = manual_position;
        }
    }
    save_state_settings();
    position_at_taskbar();
    render_layered();
}

fn window_size_needs_sync(
    current_rect: Option<RECT>,
    required_width: i32,
    required_height: i32,
) -> bool {
    current_rect.map_or(true, |rect| {
        rect.right - rect.left != required_width || rect.bottom - rect.top != required_height
    })
}

fn sync_usage_geometry_if_needed(hwnd: HWND) {
    let required = {
        let state = lock_state();
        state.as_ref().map(|s| {
            (
                s.resize_session.is_some(),
                s.widget_width_logical,
                active_family_count_for_state(s),
                widget_height_for_state(s),
            )
        })
    };
    let Some((resizing, saved_width, active_families, required_height)) = required else {
        return;
    };
    if resizing {
        return;
    }
    let work_area_width = native_interop::get_monitor_work_area(hwnd)
        .map(|area| area.right - area.left)
        .unwrap_or(i32::MAX / 4);
    let (required_width, required_height) = resolved_widget_size_device(
        saved_width,
        active_families,
        CURRENT_DPI.load(Ordering::Relaxed),
        work_area_width,
        required_height,
    );

    if window_size_needs_sync(
        native_interop::get_window_rect_safe(hwnd),
        required_width,
        required_height,
    ) {
        position_at_taskbar();
    }
}

/// Compute the popup's top-left Y so its bottom edge sits flush with
/// `work_area_bottom` (the taskbar's top edge, for a bottom-docked
/// taskbar), extending upward into the work area. If the popup is taller
/// than the work area itself, `work_area_top` wins (the popup may then
/// extend past `work_area_bottom` as an unavoidable last resort - there is
/// no space above the taskbar tall enough to fit it).
fn compute_popup_y(work_area_top: i32, work_area_bottom: i32, popup_height: i32) -> i32 {
    (work_area_bottom - popup_height).max(work_area_top)
}

/// Clamp the popup's desired X so it stays within the work area
/// horizontally. If the popup is wider than the work area itself,
/// `work_area_left` wins (mirrors `compute_popup_y`'s last-resort rule).
fn clamp_popup_x(
    desired_x: i32,
    work_area_left: i32,
    work_area_right: i32,
    popup_width: i32,
) -> i32 {
    let max_x = (work_area_right - popup_width).max(work_area_left);
    desired_x.clamp(work_area_left, max_x)
}

/// AUM-WINDOW-UI-01C-2-STEP1: the popup's automatic (taskbar-anchored) x/y,
/// extracted verbatim from `position_at_taskbar`'s own calculation so the
/// "where does auto-placement put the popup" arithmetic is a pure function
/// separate from the Win32 plumbing (DPI refresh, taskbar/tray/work-area
/// queries, `tray_offset` clamping, `MoveWindow`) that `position_at_taskbar`
/// still owns. Composes the existing `compute_popup_y`/`clamp_popup_x`
/// rather than duplicating their logic — `y` is independent of `tray_left`/
/// `tray_offset`, and `x` is `tray_left - popup_width - tray_offset` clamped
/// into `work_area` exactly as before this extraction. This is purely a
/// responsibility split: given the same inputs, it returns the same `(x, y)`
/// `position_at_taskbar` computed inline previously — no behavior change,
/// and no new placement concept (manual/auto mode, saved coordinates, etc.)
/// is introduced here.
fn compute_auto_popup_position(
    work_area: RECT,
    tray_left: i32,
    popup_width: i32,
    popup_height: i32,
    tray_offset: i32,
) -> (i32, i32) {
    let y = compute_popup_y(work_area.top, work_area.bottom, popup_height);
    let desired_x = tray_left - popup_width - tray_offset;
    let x = clamp_popup_x(desired_x, work_area.left, work_area.right, popup_width);
    (x, y)
}

/// WinEvent callback for tray icon location changes
unsafe extern "system" fn on_tray_location_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    static LAST_REPOSITION: Mutex<Option<std::time::Instant>> = Mutex::new(None);

    let is_tray = {
        let state = lock_state();
        state
            .as_ref()
            .and_then(|s| s.tray_notify_hwnd)
            .map(|h| h == hwnd)
            .unwrap_or(false)
    };

    if is_tray {
        if tray_reposition_is_suppressed() {
            return;
        }

        let should_reposition = {
            let mut last = LAST_REPOSITION.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            if last
                .map(|t| now.duration_since(t).as_millis() > 500)
                .unwrap_or(true)
            {
                *last = Some(now);
                true
            } else {
                false
            }
        };
        if should_reposition {
            position_at_taskbar();
            render_layered();
        }
    }
}

/// Main window procedure
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // The popup has no native sizing frame. Keep the entire surface in
        // the client area so edge presses are handled by our capture-based
        // horizontal resize path below rather than DefWindowProc's sizing loop.
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_SIZE => {
            let resizing = {
                let state = lock_state();
                state
                    .as_ref()
                    .map(|state| state.resize_session.is_some())
                    .unwrap_or(false)
            };
            if resizing {
                render_layered();
            }
            LRESULT(0)
        }
        WM_PAINT => {
            // For non-embedded fallback, paint normally
            let embedded = {
                let state = lock_state();
                state.as_ref().map(|s| s.embedded).unwrap_or(false)
            };
            if embedded {
                // Layered windows don't use WM_PAINT; just validate the region
                let mut ps = PAINTSTRUCT::default();
                let _ = BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
            } else {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                paint(hdc, hwnd);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DISPLAYCHANGE | WM_DPICHANGED_MSG | WM_SETTINGCHANGE => {
            if msg == WM_DPICHANGED_MSG {
                let new_dpi = (wparam.0 & 0xFFFF) as u32;
                CURRENT_DPI.store(new_dpi, Ordering::Relaxed);
            }
            if msg == WM_SETTINGCHANGE {
                check_theme_change();
                check_language_change();
            }
            refresh_dpi();
            position_at_taskbar();
            render_layered();
            LRESULT(0)
        }
        WM_TIMER => {
            let timer_id = wparam.0;
            match timer_id {
                TIMER_POLL => {
                    let auth_watch = {
                        let state = lock_state();
                        state.as_ref().map(|s| {
                            (
                                s.auth_error_paused_polling,
                                s.auth_watch_mode,
                                s.auth_watch_snapshot.clone(),
                            )
                        })
                    };
                    match auth_watch {
                        Some((true, watch_mode, previous_snapshot)) => {
                            let current_snapshot = poller::credential_watch_snapshot(watch_mode);
                            if current_snapshot != previous_snapshot {
                                let mut state = lock_state();
                                if let Some(s) = state.as_mut() {
                                    if s.auth_error_paused_polling
                                        && s.auth_watch_mode == watch_mode
                                    {
                                        s.auth_watch_snapshot = current_snapshot;
                                    }
                                }
                                drop(state);
                                let sh = SendHwnd::from_hwnd(hwnd);
                                std::thread::spawn(move || {
                                    do_poll(sh);
                                });
                            }
                        }
                        Some((false, _, _)) => {
                            let sh = SendHwnd::from_hwnd(hwnd);
                            std::thread::spawn(move || {
                                do_poll(sh);
                            });
                        }
                        None => {}
                    }
                }
                TIMER_COUNTDOWN => {
                    update_display();
                    render_layered();
                    schedule_countdown_timer();
                }
                TIMER_RESET_POLL => {
                    let should_poll = {
                        let state = lock_state();
                        state
                            .as_ref()
                            .map(|s| !s.auth_error_paused_polling)
                            .unwrap_or(false)
                    };
                    if should_poll {
                        let sh = SendHwnd::from_hwnd(hwnd);
                        std::thread::spawn(move || {
                            do_poll(sh);
                        });
                    }
                }
                #[cfg(feature = "self-update")]
                TIMER_UPDATE_CHECK => {
                    begin_update_check(hwnd, false);
                }
                TIMER_VIRTUAL_DESKTOP_EVENT_SETTLE => {
                    handle_virtual_desktop_event_settle(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_USAGE_UPDATED => {
            check_theme_change();
            check_language_change();
            sync_usage_geometry_if_needed(hwnd);
            render_layered();
            schedule_countdown_timer();
            suppress_tray_reposition_for(Duration::from_millis(
                TRAY_ICON_UPDATE_REPOSITION_SUPPRESS_MS,
            ));
            sync_tray_icons(hwnd);
            LRESULT(0)
        }
        #[cfg(feature = "self-update")]
        WM_APP_UPDATE_CHECK_COMPLETE => {
            schedule_auto_update_check(hwnd);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let (is_resizing, is_dragging) = {
                let state = lock_state();
                state
                    .as_ref()
                    .map(|s| (s.resize_session.is_some(), s.dragging))
                    .unwrap_or((false, false))
            };
            let pointer_target = pointer_interaction_under_cursor(hwnd);
            if is_resizing
                || matches!(
                    pointer_target,
                    PointerInteractionTarget::HorizontalResize(_)
                )
            {
                let cursor = LoadCursorW(HINSTANCE::default(), IDC_SIZEWE).unwrap_or_default();
                SetCursor(cursor);
                return LRESULT(1);
            }
            if is_dragging {
                let cursor = LoadCursorW(HINSTANCE::default(), IDC_SIZEALL).unwrap_or_default();
                SetCursor(cursor);
                return LRESULT(1);
            }
            match pointer_target {
                PointerInteractionTarget::HelpButton | PointerInteractionTarget::AuthCta(_) => {
                    let cursor = LoadCursorW(HINSTANCE::default(), IDC_HAND).unwrap_or_default();
                    SetCursor(cursor);
                    return LRESULT(1);
                }
                PointerInteractionTarget::HeaderDrag => {
                    let cursor = LoadCursorW(HINSTANCE::default(), IDC_SIZEALL).unwrap_or_default();
                    SetCursor(cursor);
                    return LRESULT(1);
                }
                _ => {}
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDOWN => {
            let client_x = (lparam.0 & 0xFFFF) as i16 as i32;
            let client_y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut client_rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut client_rect);
            let client_width = client_rect.right - client_rect.left;
            let resize_edge_width = horizontal_resize_edge_width_for_dpi(window_dpi(hwnd));
            let interaction = {
                let state = lock_state();
                match state.as_ref() {
                    Some(s) => pointer_interaction_target(
                        client_x,
                        client_y,
                        client_width,
                        resize_edge_width,
                        header_band_bottom(s),
                        help_button_rect_for_client_width(client_width, window_dpi(hwnd)),
                        &s.auth_cta_hit_targets,
                    ),
                    None => PointerInteractionTarget::None,
                }
            };

            if interaction == PointerInteractionTarget::HelpButton {
                show_display_guide(hwnd);
                return LRESULT(0);
            }

            if let PointerInteractionTarget::AuthCta(action) = interaction {
                action.launch(hwnd);
                return LRESULT(0);
            }

            let mut pt = POINT::default();
            if GetCursorPos(&mut pt).is_err() {
                return LRESULT(0);
            }
            let window_rect = native_interop::get_window_rect_safe(hwnd);

            if let PointerInteractionTarget::HorizontalResize(edge) = interaction {
                let Some(start_window_rect) = window_rect else {
                    return LRESULT(0);
                };
                let mut state = lock_state();
                if let Some(state) = state.as_mut() {
                    state.resize_session = Some(HorizontalResizeSession {
                        edge,
                        start_cursor_screen_x: pt.x,
                        start_window_rect,
                    });
                }
                drop(state);
                SetCapture(hwnd);
                if GetCapture() != hwnd {
                    let mut state = lock_state();
                    if let Some(state) = state.as_mut() {
                        clear_horizontal_resize_session(&mut state.resize_session);
                    }
                }
                return LRESULT(0);
            }

            if interaction != PointerInteractionTarget::HeaderDrag {
                return LRESULT(0);
            }

            // AUM-WINDOW-UI-01C-2-STEP2: the window's current screen
            // position is the reference point free-drag deltas are applied
            // to in `WM_MOUSEMOVE` — captured once here rather than derived
            // from any taskbar/tray formula, since a free drag no longer
            // assumes the popup starts taskbar-anchored.
            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                s.dragging = true;
                s.drag_start_mouse_x = pt.x;
                s.drag_start_mouse_y = pt.y;
                if let Some(rect) = window_rect {
                    s.drag_start_window_x = rect.left;
                    s.drag_start_window_y = rect.top;
                }
            }
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let resize_session = {
                let state = lock_state();
                state.as_ref().and_then(|state| state.resize_session)
            };
            if let Some(resize_session) = resize_session {
                let mut point = POINT::default();
                if GetCursorPos(&mut point).is_ok() {
                    let (minimum, maximum, required_height) = resize_limits_for_window(hwnd);
                    let rect = horizontal_resize_rect(
                        resize_session,
                        point.x,
                        minimum,
                        maximum,
                        required_height,
                    );
                    native_interop::move_window(
                        hwnd,
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                    );
                }
                return LRESULT(0);
            }

            let is_dragging = {
                let state = lock_state();
                state.as_ref().map(|s| s.dragging).unwrap_or(false)
            };
            if is_dragging {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let current_width =
                    native_interop::get_window_rect_safe(hwnd).map(|rect| rect.right - rect.left);
                // AUM-WINDOW-UI-01C-2-STEP2: free 2-axis follow — the popup
                // tracks the cursor directly (window start + cursor delta),
                // no taskbar/tray/work-area math and no clamping here (see
                // STEP2's design: off-screen recovery is a separate,
                // later step).
                let move_target = {
                    let state = lock_state();
                    let s = match state.as_ref() {
                        Some(s) => s,
                        None => return LRESULT(0),
                    };
                    let (x, y) = drag_follow_position(
                        (s.drag_start_window_x, s.drag_start_window_y),
                        (s.drag_start_mouse_x, s.drag_start_mouse_y),
                        (pt.x, pt.y),
                    );
                    let hwnd_val = s.hwnd.to_hwnd();
                    let width = current_width.unwrap_or_else(|| total_widget_width_for_state(s));
                    let height = widget_height_for_state(s);
                    (hwnd_val, x, y, width, height)
                };
                let (hwnd_val, x, y, width, height) = move_target;
                native_interop::move_window(hwnd_val, x, y, width, height);
                return LRESULT(0);
            }

            // AUTH-RECOVERY-CTA-IMPLEMENT-01: only redraw on an actual
            // hover-target change (entering/leaving/switching an auth CTA
            // cell), never unconditionally on every mousemove — this fires
            // very frequently and a full-window repaint per move would be
            // wasteful for a change that usually isn't happening.
            let mut point = POINT::default();
            if GetCursorPos(&mut point).is_ok() && ScreenToClient(hwnd, &mut point).as_bool() {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    let new_hover = s
                        .auth_cta_hit_targets
                        .iter()
                        .find(|hit| point_is_in_rect(point.x, point.y, hit.rect))
                        .map(|hit| hit.rect);
                    if new_hover != s.hovered_auth_cta_rect {
                        s.hovered_auth_cta_rect = new_hover;
                        let hwnd_val = s.hwnd.to_hwnd();
                        let should_track = new_hover.is_some();
                        drop(state);
                        if should_track {
                            track_mouse_leave(hwnd_val);
                        }
                        let _ = InvalidateRect(hwnd_val, None, false);
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                if s.hovered_auth_cta_rect.take().is_some() {
                    let hwnd_val = s.hwnd.to_hwnd();
                    drop(state);
                    let _ = InvalidateRect(hwnd_val, None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let finished_resize = {
                let mut state = lock_state();
                state
                    .as_mut()
                    .map(|state| clear_horizontal_resize_session(&mut state.resize_session))
                    .unwrap_or(false)
            };
            if finished_resize {
                let _ = ReleaseCapture();
                finish_horizontal_resize(hwnd);
                return LRESULT(0);
            }

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let drag_result = {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    if s.dragging {
                        s.dragging = false;
                        Some((
                            s.drag_start_window_x,
                            s.drag_start_window_y,
                            s.drag_start_mouse_x,
                            s.drag_start_mouse_y,
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if let Some((start_window_x, start_window_y, start_mouse_x, start_mouse_y)) =
                drag_result
            {
                let _ = ReleaseCapture();
                let (final_x, final_y) = drag_follow_position(
                    (start_window_x, start_window_y),
                    (start_mouse_x, start_mouse_y),
                    (pt.x, pt.y),
                );
                // Persist every completed header drag, clamped to the nearest
                // current monitor's work area so the widget never overlaps a
                // taskbar or becomes stranded after monitor topology changes.
                let window_rect = native_interop::get_window_rect_safe(hwnd);
                let work_area = native_interop::get_monitor_work_area_for_point(POINT {
                    x: final_x,
                    y: final_y,
                });
                let (clamped_x, clamped_y) = match (window_rect, work_area) {
                    (Some(rect), Some(area)) => clamp_position_to_work_area(
                        area,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        final_x,
                        final_y,
                    ),
                    _ => (final_x, final_y),
                };
                if let Some(rect) = window_rect {
                    native_interop::move_window(
                        hwnd,
                        clamped_x,
                        clamped_y,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                    );
                }
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    s.manual_position = Some((clamped_x, clamped_y));
                }
                drop(state);
                save_state_settings();
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            let cancelled_resize = {
                let mut state = lock_state();
                state
                    .as_mut()
                    .map(|state| clear_horizontal_resize_session(&mut state.resize_session))
                    .unwrap_or(false)
            };
            if cancelled_resize {
                if msg == WM_CANCELMODE {
                    let _ = ReleaseCapture();
                }
                finish_horizontal_resize(hwnd);
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_RBUTTONUP => {
            show_context_menu(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 as u16;
            match id {
                1 => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.session_state = CellState::Loading;
                            s.weekly_state = CellState::Loading;
                            s.codex_session_state = CellState::Loading;
                            s.codex_weekly_state = CellState::Loading;
                            s.antigravity_session_state = CellState::Loading;
                            s.antigravity_weekly_state = CellState::Loading;
                            s.github_copilot_state = CellState::Loading;
                            refresh_usage_texts(s);
                            s.force_notify_auth_error = true;
                        }
                    }
                    render_layered();
                    let sh = SendHwnd::from_hwnd(hwnd);
                    std::thread::spawn(move || {
                        do_poll(sh);
                    });
                }
                IDM_HELP_DISPLAY_GUIDE => show_display_guide(hwnd),
                IDM_HELP_AI_QUOTAS => show_ai_quota_help(hwnd),
                IDM_HELP_README => {
                    if !open_readme(hwnd) {
                        let strings = current_strings();
                        show_info_message(hwnd, strings.help_readme, README_URL);
                    }
                }
                IDM_HELP_VERSION_INFORMATION => show_version_information(hwnd),
                #[cfg(feature = "antigravity")]
                IDM_HELP_ANTIGRAVITY_SETUP => show_antigravity_setup(hwnd),
                #[cfg(feature = "self-update")]
                IDM_VERSION_ACTION => {
                    let (install_channel, release) = {
                        let state = lock_state();
                        match state.as_ref() {
                            Some(s) => (
                                s.install_channel,
                                match &s.update_status {
                                    UpdateStatus::Available(release) => Some(release.clone()),
                                    _ => None,
                                },
                            ),
                            None => (InstallChannel::Portable, None),
                        }
                    };

                    match install_channel {
                        InstallChannel::Winget => {
                            if release.is_some() {
                                begin_winget_update(hwnd);
                            } else {
                                begin_update_check(hwnd, true);
                            }
                        }
                        InstallChannel::Portable => {
                            if let Some(release) = release {
                                begin_update_apply(hwnd, release);
                            } else {
                                begin_update_check(hwnd, true);
                            }
                        }
                    }
                }
                2 => {
                    let hook = {
                        let state = lock_state();
                        state.as_ref().and_then(|s| s.win_event_hook)
                    };
                    if let Some(h) = hook {
                        native_interop::unhook_win_event(h);
                    }
                    PostQuitMessage(0);
                }
                IDM_RESET_POSITION => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            reset_saved_position(&mut s.tray_offset, &mut s.manual_position);
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                }
                IDM_START_WITH_WINDOWS => {
                    set_startup_enabled(!is_startup_enabled());
                }
                IDM_ALWAYS_ON_TOP => {
                    let always_on_top = {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.always_on_top = !s.always_on_top;
                            Some(s.always_on_top)
                        } else {
                            None
                        }
                    };
                    if let Some(always_on_top) = always_on_top {
                        save_state_settings();
                        apply_always_on_top(hwnd, always_on_top);
                    }
                }
                IDM_DISPLAY_BASIS_USED | IDM_DISPLAY_BASIS_REMAINING => {
                    let new_basis = if id == IDM_DISPLAY_BASIS_USED {
                        DisplayBasis::UsedPercentage
                    } else {
                        DisplayBasis::RemainingAllowance
                    };
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.display_basis = new_basis;
                            refresh_usage_texts(s);
                        }
                    }
                    save_state_settings();
                    render_layered();
                    sync_tray_icons(hwnd);
                }
                IDM_RESET_DISPLAY_RELATIVE | IDM_RESET_DISPLAY_ABSOLUTE => {
                    if let Some(new_mode) = reset_display_mode_for_menu_id(id) {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.reset_display_mode = new_mode;
                            refresh_usage_texts(s);
                        }
                    }
                    save_state_settings();
                    render_layered();
                }
                IDM_DISPLAY_DENSITY_COMPACT
                | IDM_DISPLAY_DENSITY_STANDARD
                | IDM_DISPLAY_DENSITY_DETAILED => {
                    // Now consumed by the pace-guidance popup drawing (see
                    // AUM-PACE-GUIDANCE-01's connection unit), so — same as
                    // the display-basis handler above — recompute the cached
                    // pace text and reposition/redraw immediately; density
                    // can change the popup's required height.
                    if let Some(new_density) = display_density_for_menu_id(id) {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.display_density = new_density;
                            refresh_usage_texts(s);
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                    render_layered();
                }
                IDM_POPUP_LAYOUT_COMPACT | IDM_POPUP_LAYOUT_STANDARD => {
                    // Changes which rows the popup shows at all, so — same
                    // as the display-density handler above — recompute the
                    // popup's required height and reposition/redraw
                    // immediately.
                    if let Some(new_layout) = popup_layout_for_menu_id(id) {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.popup_layout = new_layout;
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                    render_layered();
                }
                IDM_APP_THEME_RECOMMENDED_DARK
                | IDM_APP_THEME_LIGHT
                | IDM_APP_THEME_HIGH_VISIBILITY => {
                    // Only the popup's colors change here — no row/height
                    // change like `PopupLayout` above, so no
                    // `position_at_taskbar()` call.
                    if let Some(new_theme) = app_theme_for_menu_id(id) {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.app_theme = new_theme;
                        }
                    }
                    save_state_settings();
                    render_layered();
                }
                IDM_VIRTUAL_DESKTOP_SCOPE_ALL | IDM_VIRTUAL_DESKTOP_SCOPE_CURRENT_ONLY => {
                    // Unlike the settings above, this mutates the live
                    // window's virtual-desktop assignment (and, for a
                    // switch back to `All`, relaunches the process) rather
                    // than only touching `AppState` — see
                    // `apply_virtual_desktop_scope_from_menu`, which also
                    // owns its own `save_state_settings()` call.
                    if let Some(new_scope) = virtual_desktop_scope_for_menu_id(id) {
                        apply_virtual_desktop_scope_from_menu(hwnd, new_scope);
                    }
                }
                IDM_SHORT_WINDOW_VISIBILITY_ALWAYS
                | IDM_SHORT_WINDOW_VISIBILITY_WARNING_ONLY
                | IDM_SHORT_WINDOW_VISIBILITY_HIDDEN => {
                    if let Some(new_visibility) = short_window_visibility_for_menu_id(id) {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.short_window_visibility = new_visibility;
                            refresh_usage_texts(s);
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                    render_layered();
                }
                IDM_SHORT_WINDOW_ALERT_SENSITIVITY_SENSITIVE
                | IDM_SHORT_WINDOW_ALERT_SENSITIVITY_STANDARD
                | IDM_SHORT_WINDOW_ALERT_SENSITIVITY_RELAXED => {
                    if let Some(new_sensitivity) = short_window_alert_sensitivity_for_menu_id(id) {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.short_window_alert_sensitivity = new_sensitivity;
                            refresh_usage_texts(s);
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                    render_layered();
                }
                IDM_FREQ_1MIN | IDM_FREQ_5MIN | IDM_FREQ_15MIN | IDM_FREQ_1HOUR => {
                    let new_interval = match id {
                        IDM_FREQ_1MIN => POLL_1_MIN,
                        IDM_FREQ_5MIN => POLL_5_MIN,
                        IDM_FREQ_15MIN => POLL_15_MIN,
                        IDM_FREQ_1HOUR => POLL_1_HOUR,
                        _ => POLL_15_MIN,
                    };
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.poll_interval_ms = new_interval;
                        }
                    }
                    save_state_settings();
                    // Reset the poll timer with the new interval
                    SetTimer(hwnd, TIMER_POLL, new_interval, None);
                }
                IDM_MODEL_CLAUDE_CODE | IDM_MODEL_CODEX | IDM_MODEL_GITHUB_COPILOT => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            match id {
                                IDM_MODEL_CLAUDE_CODE => {
                                    if s.show_codex
                                        || s.show_antigravity
                                        || s.show_github_copilot
                                        || !s.show_claude_code
                                    {
                                        s.show_claude_code = !s.show_claude_code;
                                    }
                                }
                                IDM_MODEL_CODEX => {
                                    if s.show_claude_code
                                        || s.show_antigravity
                                        || s.show_github_copilot
                                        || !s.show_codex
                                    {
                                        s.show_codex = !s.show_codex;
                                    }
                                }
                                IDM_MODEL_GITHUB_COPILOT => {
                                    if s.show_claude_code
                                        || s.show_codex
                                        || s.show_antigravity
                                        || !s.show_github_copilot
                                    {
                                        s.show_github_copilot = !s.show_github_copilot;
                                        if !s.show_github_copilot {
                                            s.github_copilot_plan =
                                                poller::GithubCopilotPlan::Unknown;
                                        }
                                    }
                                }
                                _ => {}
                            }
                            s.session_state = CellState::Loading;
                            s.weekly_state = CellState::Loading;
                            s.codex_session_state = CellState::Loading;
                            s.codex_weekly_state = CellState::Loading;
                            s.antigravity_session_state = CellState::Loading;
                            s.antigravity_weekly_state = CellState::Loading;
                            s.github_copilot_state = CellState::Loading;
                            refresh_usage_texts(s);
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                    render_layered();
                    sync_tray_icons(hwnd);
                    let sh = SendHwnd::from_hwnd(hwnd);
                    std::thread::spawn(move || {
                        do_poll(sh);
                    });
                }
                #[cfg(feature = "antigravity")]
                IDM_MODEL_ANTIGRAVITY => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            if s.show_claude_code
                                || s.show_codex
                                || s.show_github_copilot
                                || !s.show_antigravity
                            {
                                s.show_antigravity = !s.show_antigravity;
                            }
                            s.session_state = CellState::Loading;
                            s.weekly_state = CellState::Loading;
                            s.codex_session_state = CellState::Loading;
                            s.codex_weekly_state = CellState::Loading;
                            s.antigravity_session_state = CellState::Loading;
                            s.antigravity_weekly_state = CellState::Loading;
                            s.github_copilot_state = CellState::Loading;
                            refresh_usage_texts(s);
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                    render_layered();
                    sync_tray_icons(hwnd);
                    let sh = SendHwnd::from_hwnd(hwnd);
                    std::thread::spawn(move || {
                        do_poll(sh);
                    });
                }
                IDM_GITHUB_COPILOT_PLAN_UNKNOWN
                | IDM_GITHUB_COPILOT_PLAN_PRO
                | IDM_GITHUB_COPILOT_PLAN_PRO_PLUS
                | IDM_GITHUB_COPILOT_PLAN_MAX => {
                    let plan = github_copilot_plan_for_menu_id(id)
                        .expect("matched GitHub Copilot plan menu ID");
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            apply_github_copilot_plan_selection(
                                &mut s.show_github_copilot,
                                &mut s.github_copilot_plan,
                                plan,
                            );
                            s.github_copilot_state = CellState::Loading;
                            refresh_usage_texts(s);
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                    render_layered();
                    sync_tray_icons(hwnd);
                    let sh = SendHwnd::from_hwnd(hwnd);
                    std::thread::spawn(move || do_poll(sh));
                }
                IDM_LANG_SYSTEM
                | IDM_LANG_ENGLISH
                | IDM_LANG_DUTCH
                | IDM_LANG_SPANISH
                | IDM_LANG_FRENCH
                | IDM_LANG_GERMAN
                | IDM_LANG_JAPANESE
                | IDM_LANG_KOREAN
                | IDM_LANG_TRADITIONAL_CHINESE
                | IDM_LANG_SIMPLIFIED_CHINESE
                | IDM_LANG_RUSSIAN
                | IDM_LANG_PORTUGUESE_BRAZIL => {
                    let language_override = match id {
                        IDM_LANG_SYSTEM => None,
                        IDM_LANG_ENGLISH => Some(LanguageId::English),
                        IDM_LANG_DUTCH => Some(LanguageId::Dutch),
                        IDM_LANG_SPANISH => Some(LanguageId::Spanish),
                        IDM_LANG_FRENCH => Some(LanguageId::French),
                        IDM_LANG_GERMAN => Some(LanguageId::German),
                        IDM_LANG_JAPANESE => Some(LanguageId::Japanese),
                        IDM_LANG_KOREAN => Some(LanguageId::Korean),
                        IDM_LANG_TRADITIONAL_CHINESE => Some(LanguageId::TraditionalChinese),
                        IDM_LANG_SIMPLIFIED_CHINESE => Some(LanguageId::SimplifiedChinese),
                        IDM_LANG_RUSSIAN => Some(LanguageId::Russian),
                        IDM_LANG_PORTUGUESE_BRAZIL => Some(LanguageId::PortugueseBrazil),
                        _ => None,
                    };
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            apply_language_to_state(s, language_override);
                        }
                    }
                    save_state_settings();
                    render_layered();
                }
                id if id == tray_icon::IDM_TOGGLE_WIDGET => {
                    toggle_widget_visibility(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_DEFERRED_TRAY_TOGGLE => {
            toggle_widget_visibility(hwnd);
            LRESULT(0)
        }
        _ if msg == WM_APP_TRAY => {
            match tray_icon::handle_message(lparam) {
                tray_icon::TrayAction::ToggleWidget => {
                    let _ = PostMessageW(hwnd, WM_APP_DEFERRED_TRAY_TOGGLE, WPARAM(0), LPARAM(0));
                }
                tray_icon::TrayAction::ShowContextMenu => {
                    show_context_menu(hwnd);
                }
                tray_icon::TrayAction::None => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let hook = {
                let state = lock_state();
                state.as_ref().and_then(|s| s.win_event_hook)
            };
            if let Some(h) = hook {
                native_interop::unhook_win_event(h);
            }
            stop_virtual_desktop_event_sync(hwnd);
            tray_icon::remove_all(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn show_context_menu(hwnd: HWND) {
    unsafe {
        let (
            current_interval,
            strings,
            language_override,
            widget_visible,
            always_on_top,
            show_claude_code,
            show_codex,
            show_antigravity,
            show_github_copilot,
            github_copilot_plan,
            display_basis,
            reset_display_mode,
            display_density,
            short_window_visibility,
            short_window_alert_sensitivity,
            popup_layout,
            app_theme,
            virtual_desktop_scope,
        ) = {
            let state = lock_state();
            match state.as_ref() {
                Some(s) => (
                    s.poll_interval_ms,
                    s.language.strings(),
                    s.language_override,
                    s.widget_visible,
                    s.always_on_top,
                    s.show_claude_code,
                    s.show_codex,
                    s.show_antigravity,
                    s.show_github_copilot,
                    s.github_copilot_plan,
                    s.display_basis,
                    s.reset_display_mode,
                    s.display_density,
                    s.short_window_visibility,
                    s.short_window_alert_sensitivity,
                    s.popup_layout,
                    s.app_theme,
                    s.virtual_desktop_scope,
                ),
                None => (
                    POLL_15_MIN,
                    LanguageId::English.strings(),
                    None,
                    true,
                    false,
                    true,
                    false,
                    false,
                    false,
                    poller::GithubCopilotPlan::Unknown,
                    DisplayBasis::default(),
                    ResetDisplayMode::default(),
                    DisplayDensity::default(),
                    ShortWindowVisibility::default(),
                    ShortWindowAlertSensitivity::default(),
                    PopupLayout::default(),
                    AppTheme::default(),
                    VirtualDesktopScope::default(),
                ),
            }
        };

        let menu = CreatePopupMenu().unwrap();

        let refresh_str = native_interop::wide_str(strings.refresh);
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            1,
            PCWSTR::from_raw(refresh_str.as_ptr()),
        );

        let reset_pos_str = native_interop::wide_str(strings.reset_position);
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            IDM_RESET_POSITION as usize,
            PCWSTR::from_raw(reset_pos_str.as_ptr()),
        );

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let widget_label = native_interop::wide_str(strings.show_widget);
        let widget_flags = if widget_visible {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            menu,
            widget_flags,
            tray_icon::IDM_TOGGLE_WIDGET as usize,
            PCWSTR::from_raw(widget_label.as_ptr()),
        );

        let always_on_top_str = native_interop::wide_str(strings.always_on_top);
        let always_on_top_flags = if always_on_top {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            menu,
            always_on_top_flags,
            IDM_ALWAYS_ON_TOP as usize,
            PCWSTR::from_raw(always_on_top_str.as_ptr()),
        );

        let startup_str = native_interop::wide_str(strings.start_with_windows);
        let startup_flags = if is_startup_enabled() {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            menu,
            startup_flags,
            IDM_START_WITH_WINDOWS as usize,
            PCWSTR::from_raw(startup_str.as_ptr()),
        );

        // Virtual desktop scope submenu: mutually exclusive, radio-style,
        // same pattern as the other settings submenus below.
        let virtual_desktop_scope_menu = CreatePopupMenu().unwrap();
        let virtual_desktop_scope_items: [(u16, VirtualDesktopScope, &str); 2] = [
            (
                IDM_VIRTUAL_DESKTOP_SCOPE_ALL,
                VirtualDesktopScope::All,
                strings.virtual_desktop_scope_all,
            ),
            (
                IDM_VIRTUAL_DESKTOP_SCOPE_CURRENT_ONLY,
                VirtualDesktopScope::CurrentOnly,
                strings.virtual_desktop_scope_current_only,
            ),
        ];
        for (id, value, label) in virtual_desktop_scope_items {
            let label_str = native_interop::wide_str(label);
            let flags = if value == virtual_desktop_scope {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                virtual_desktop_scope_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let selected_virtual_desktop_scope_id = match virtual_desktop_scope {
            VirtualDesktopScope::All => IDM_VIRTUAL_DESKTOP_SCOPE_ALL,
            VirtualDesktopScope::CurrentOnly => IDM_VIRTUAL_DESKTOP_SCOPE_CURRENT_ONLY,
        };
        let _ = CheckMenuRadioItem(
            virtual_desktop_scope_menu,
            IDM_VIRTUAL_DESKTOP_SCOPE_ALL as u32,
            IDM_VIRTUAL_DESKTOP_SCOPE_CURRENT_ONLY as u32,
            selected_virtual_desktop_scope_id as u32,
            MF_BYCOMMAND.0,
        );
        let virtual_desktop_scope_label = native_interop::wide_str(strings.virtual_desktop_scope);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            virtual_desktop_scope_menu.0 as usize,
            PCWSTR::from_raw(virtual_desktop_scope_label.as_ptr()),
        );

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // Update Frequency submenu
        let freq_menu = CreatePopupMenu().unwrap();
        let freq_items: [(u16, u32, &str); 4] = [
            (IDM_FREQ_1MIN, POLL_1_MIN, strings.one_minute),
            (IDM_FREQ_5MIN, POLL_5_MIN, strings.five_minutes),
            (IDM_FREQ_15MIN, POLL_15_MIN, strings.fifteen_minutes),
            (IDM_FREQ_1HOUR, POLL_1_HOUR, strings.one_hour),
        ];
        for (id, interval, label) in freq_items {
            let label_str = native_interop::wide_str(label);
            let flags = if interval == current_interval {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                freq_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }

        let selected_frequency_id = match current_interval {
            POLL_1_MIN => IDM_FREQ_1MIN,
            POLL_5_MIN => IDM_FREQ_5MIN,
            POLL_1_HOUR => IDM_FREQ_1HOUR,
            _ => IDM_FREQ_15MIN,
        };
        let _ = CheckMenuRadioItem(
            freq_menu,
            IDM_FREQ_1MIN as u32,
            IDM_FREQ_1HOUR as u32,
            selected_frequency_id as u32,
            MF_BYCOMMAND.0,
        );

        let freq_label = native_interop::wide_str(strings.update_frequency);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            freq_menu.0 as usize,
            PCWSTR::from_raw(freq_label.as_ptr()),
        );

        // AI visibility submenu
        let displayed_ai_menu = CreatePopupMenu().unwrap();
        let claude_model = native_interop::wide_str(strings.claude_code_model);
        let claude_flags = if show_claude_code {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            displayed_ai_menu,
            claude_flags,
            IDM_MODEL_CLAUDE_CODE as usize,
            PCWSTR::from_raw(claude_model.as_ptr()),
        );

        let codex_model = native_interop::wide_str(strings.codex_model);
        let codex_flags = if show_codex {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            displayed_ai_menu,
            codex_flags,
            IDM_MODEL_CODEX as usize,
            PCWSTR::from_raw(codex_model.as_ptr()),
        );

        #[cfg(feature = "antigravity")]
        {
            let antigravity_model = native_interop::wide_str(strings.antigravity_model);
            let antigravity_flags = if show_antigravity {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                displayed_ai_menu,
                antigravity_flags,
                IDM_MODEL_ANTIGRAVITY as usize,
                PCWSTR::from_raw(antigravity_model.as_ptr()),
            );
        }

        let github_copilot_model = native_interop::wide_str(strings.github_copilot);
        let github_copilot_flags = if show_github_copilot {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            displayed_ai_menu,
            github_copilot_flags,
            IDM_MODEL_GITHUB_COPILOT as usize,
            PCWSTR::from_raw(github_copilot_model.as_ptr()),
        );

        let displayed_ai_label = native_interop::wide_str(strings.displayed_ai);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            displayed_ai_menu.0 as usize,
            PCWSTR::from_raw(displayed_ai_label.as_ptr()),
        );

        let copilot_plan_menu = CreatePopupMenu().unwrap();
        for (id, plan, label) in [
            (
                IDM_GITHUB_COPILOT_PLAN_UNKNOWN,
                poller::GithubCopilotPlan::Unknown,
                strings.github_copilot_plan_unknown,
            ),
            (
                IDM_GITHUB_COPILOT_PLAN_PRO,
                poller::GithubCopilotPlan::Pro,
                strings.github_copilot_plan_pro,
            ),
            (
                IDM_GITHUB_COPILOT_PLAN_PRO_PLUS,
                poller::GithubCopilotPlan::ProPlus,
                strings.github_copilot_plan_pro_plus,
            ),
            (
                IDM_GITHUB_COPILOT_PLAN_MAX,
                poller::GithubCopilotPlan::Max,
                strings.github_copilot_plan_max,
            ),
        ] {
            let label = native_interop::wide_str(label);
            let flags = if plan == github_copilot_plan {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                copilot_plan_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label.as_ptr()),
            );
        }
        let selected_copilot_plan_id = match github_copilot_plan {
            poller::GithubCopilotPlan::Unknown => IDM_GITHUB_COPILOT_PLAN_UNKNOWN,
            poller::GithubCopilotPlan::Pro => IDM_GITHUB_COPILOT_PLAN_PRO,
            poller::GithubCopilotPlan::ProPlus => IDM_GITHUB_COPILOT_PLAN_PRO_PLUS,
            poller::GithubCopilotPlan::Max => IDM_GITHUB_COPILOT_PLAN_MAX,
        };
        let _ = CheckMenuRadioItem(
            copilot_plan_menu,
            IDM_GITHUB_COPILOT_PLAN_UNKNOWN as u32,
            IDM_GITHUB_COPILOT_PLAN_MAX as u32,
            selected_copilot_plan_id as u32,
            MF_BYCOMMAND.0,
        );

        let language_menu = CreatePopupMenu().unwrap();
        let system_label = native_interop::wide_str(strings.system_default);
        let system_flags = if language_override.is_none() {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            language_menu,
            system_flags,
            IDM_LANG_SYSTEM as usize,
            PCWSTR::from_raw(system_label.as_ptr()),
        );

        for language in LanguageId::ALL {
            let id = match language {
                LanguageId::English => IDM_LANG_ENGLISH,
                LanguageId::Dutch => IDM_LANG_DUTCH,
                LanguageId::Spanish => IDM_LANG_SPANISH,
                LanguageId::French => IDM_LANG_FRENCH,
                LanguageId::German => IDM_LANG_GERMAN,
                LanguageId::Japanese => IDM_LANG_JAPANESE,
                LanguageId::Korean => IDM_LANG_KOREAN,
                LanguageId::TraditionalChinese => IDM_LANG_TRADITIONAL_CHINESE,
                LanguageId::SimplifiedChinese => IDM_LANG_SIMPLIFIED_CHINESE,
                LanguageId::Russian => IDM_LANG_RUSSIAN,
                LanguageId::PortugueseBrazil => IDM_LANG_PORTUGUESE_BRAZIL,
            };
            let label_str = native_interop::wide_str(language.native_name());
            let flags = if language_override == Some(language) {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                language_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let selected_language_id = match language_override {
            None => IDM_LANG_SYSTEM,
            Some(LanguageId::English) => IDM_LANG_ENGLISH,
            Some(LanguageId::Dutch) => IDM_LANG_DUTCH,
            Some(LanguageId::Spanish) => IDM_LANG_SPANISH,
            Some(LanguageId::French) => IDM_LANG_FRENCH,
            Some(LanguageId::German) => IDM_LANG_GERMAN,
            Some(LanguageId::Japanese) => IDM_LANG_JAPANESE,
            Some(LanguageId::Korean) => IDM_LANG_KOREAN,
            Some(LanguageId::TraditionalChinese) => IDM_LANG_TRADITIONAL_CHINESE,
            Some(LanguageId::SimplifiedChinese) => IDM_LANG_SIMPLIFIED_CHINESE,
            Some(LanguageId::Russian) => IDM_LANG_RUSSIAN,
            Some(LanguageId::PortugueseBrazil) => IDM_LANG_PORTUGUESE_BRAZIL,
        };
        let _ = CheckMenuRadioItem(
            language_menu,
            IDM_LANG_SYSTEM as u32,
            IDM_LANG_SIMPLIFIED_CHINESE as u32,
            selected_language_id as u32,
            MF_BYCOMMAND.0,
        );

        let display_settings_menu = CreatePopupMenu().unwrap();

        // Display basis submenu: mutually exclusive, radio-style.
        let display_basis_menu = CreatePopupMenu().unwrap();

        let used_percentage_str = native_interop::wide_str(strings.used_percentage);
        let used_percentage_flags = if display_basis == DisplayBasis::UsedPercentage {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            display_basis_menu,
            used_percentage_flags,
            IDM_DISPLAY_BASIS_USED as usize,
            PCWSTR::from_raw(used_percentage_str.as_ptr()),
        );

        let remaining_allowance_str = native_interop::wide_str(strings.remaining_allowance);
        let remaining_allowance_flags = if display_basis == DisplayBasis::RemainingAllowance {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            display_basis_menu,
            remaining_allowance_flags,
            IDM_DISPLAY_BASIS_REMAINING as usize,
            PCWSTR::from_raw(remaining_allowance_str.as_ptr()),
        );
        let selected_display_basis_id = match display_basis {
            DisplayBasis::UsedPercentage => IDM_DISPLAY_BASIS_USED,
            DisplayBasis::RemainingAllowance => IDM_DISPLAY_BASIS_REMAINING,
        };
        let _ = CheckMenuRadioItem(
            display_basis_menu,
            IDM_DISPLAY_BASIS_USED as u32,
            IDM_DISPLAY_BASIS_REMAINING as u32,
            selected_display_basis_id as u32,
            MF_BYCOMMAND.0,
        );

        let reset_display_menu = CreatePopupMenu().unwrap();
        for (id, value, label) in [
            (
                IDM_RESET_DISPLAY_RELATIVE,
                ResetDisplayMode::Relative,
                strings.reset_display_relative,
            ),
            (
                IDM_RESET_DISPLAY_ABSOLUTE,
                ResetDisplayMode::AbsoluteDateTime,
                strings.reset_display_datetime,
            ),
        ] {
            let label_str = native_interop::wide_str(label);
            let flags = if value == reset_display_mode {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                reset_display_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let selected_reset_display_id = match reset_display_mode {
            ResetDisplayMode::Relative => IDM_RESET_DISPLAY_RELATIVE,
            ResetDisplayMode::AbsoluteDateTime => IDM_RESET_DISPLAY_ABSOLUTE,
        };
        let _ = CheckMenuRadioItem(
            reset_display_menu,
            IDM_RESET_DISPLAY_RELATIVE as u32,
            IDM_RESET_DISPLAY_ABSOLUTE as u32,
            selected_reset_display_id as u32,
            MF_BYCOMMAND.0,
        );

        // Display density submenu: mutually exclusive, radio-style, same
        // pattern as the display-basis submenu above. Not yet connected to
        // any drawing code — see AUM-PACE-GUIDANCE-01's later units.
        let display_density_menu = CreatePopupMenu().unwrap();
        let display_density_items: [(u16, DisplayDensity, &str); 3] = [
            (
                IDM_DISPLAY_DENSITY_COMPACT,
                DisplayDensity::Compact,
                strings.display_density_compact,
            ),
            (
                IDM_DISPLAY_DENSITY_STANDARD,
                DisplayDensity::Standard,
                strings.standard_level,
            ),
            (
                IDM_DISPLAY_DENSITY_DETAILED,
                DisplayDensity::Detailed,
                strings.display_density_detailed,
            ),
        ];
        for (id, value, label) in display_density_items {
            let label_str = native_interop::wide_str(label);
            let flags = if value == display_density {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                display_density_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let selected_display_density_id = match display_density {
            DisplayDensity::Compact => IDM_DISPLAY_DENSITY_COMPACT,
            DisplayDensity::Standard => IDM_DISPLAY_DENSITY_STANDARD,
            DisplayDensity::Detailed => IDM_DISPLAY_DENSITY_DETAILED,
        };
        let _ = CheckMenuRadioItem(
            display_density_menu,
            IDM_DISPLAY_DENSITY_COMPACT as u32,
            IDM_DISPLAY_DENSITY_DETAILED as u32,
            selected_display_density_id as u32,
            MF_BYCOMMAND.0,
        );
        // Popup layout submenu: mutually exclusive, radio-style, same
        // pattern as the display-density submenu above. A separate setting
        // from `DisplayDensity` (which only controls how much text each
        // shown weekly row carries) — this controls which rows the popup
        // shows at all.
        let popup_layout_menu = CreatePopupMenu().unwrap();
        let popup_layout_items: [(u16, PopupLayout, &str); 2] = [
            (
                IDM_POPUP_LAYOUT_COMPACT,
                PopupLayout::Compact,
                strings.popup_layout_compact,
            ),
            (
                IDM_POPUP_LAYOUT_STANDARD,
                PopupLayout::Standard,
                strings.popup_layout_standard,
            ),
        ];
        for (id, value, label) in popup_layout_items {
            let label_str = native_interop::wide_str(label);
            let flags = if value == popup_layout {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                popup_layout_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let selected_popup_layout_id = match popup_layout {
            PopupLayout::Compact => IDM_POPUP_LAYOUT_COMPACT,
            PopupLayout::Standard => IDM_POPUP_LAYOUT_STANDARD,
        };
        let _ = CheckMenuRadioItem(
            popup_layout_menu,
            IDM_POPUP_LAYOUT_COMPACT as u32,
            IDM_POPUP_LAYOUT_STANDARD as u32,
            selected_popup_layout_id as u32,
            MF_BYCOMMAND.0,
        );
        // App theme submenu: mutually exclusive, radio-style, same pattern
        // as the popup-layout submenu above. Only affects popup colors
        // (`PopupPalette`/`popup_palette`) — never row count or height.
        let app_theme_menu = CreatePopupMenu().unwrap();
        let app_theme_items: [(u16, AppTheme, &str); 3] = [
            (
                IDM_APP_THEME_RECOMMENDED_DARK,
                AppTheme::RecommendedDark,
                strings.app_theme_recommended_dark,
            ),
            (
                IDM_APP_THEME_LIGHT,
                AppTheme::Light,
                strings.app_theme_light,
            ),
            (
                IDM_APP_THEME_HIGH_VISIBILITY,
                AppTheme::HighVisibility,
                strings.app_theme_high_visibility,
            ),
        ];
        for (id, value, label) in app_theme_items {
            let label_str = native_interop::wide_str(label);
            let flags = if value == app_theme {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                app_theme_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let selected_app_theme_id = match app_theme {
            AppTheme::RecommendedDark => IDM_APP_THEME_RECOMMENDED_DARK,
            AppTheme::Light => IDM_APP_THEME_LIGHT,
            AppTheme::HighVisibility => IDM_APP_THEME_HIGH_VISIBILITY,
        };
        let _ = CheckMenuRadioItem(
            app_theme_menu,
            IDM_APP_THEME_RECOMMENDED_DARK as u32,
            IDM_APP_THEME_HIGH_VISIBILITY as u32,
            selected_app_theme_id as u32,
            MF_BYCOMMAND.0,
        );
        // Short-window (5h) visibility submenu.
        let short_window_visibility_menu = CreatePopupMenu().unwrap();
        let short_window_visibility_items: [(u16, ShortWindowVisibility, &str); 3] = [
            (
                IDM_SHORT_WINDOW_VISIBILITY_ALWAYS,
                ShortWindowVisibility::Always,
                strings.short_window_visibility_always,
            ),
            (
                IDM_SHORT_WINDOW_VISIBILITY_WARNING_ONLY,
                ShortWindowVisibility::WarningOnly,
                strings.short_window_visibility_warning_only,
            ),
            (
                IDM_SHORT_WINDOW_VISIBILITY_HIDDEN,
                ShortWindowVisibility::Hidden,
                strings.short_window_visibility_hidden,
            ),
        ];
        for (id, value, label) in short_window_visibility_items {
            let label_str = native_interop::wide_str(label);
            let flags = if value == short_window_visibility {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                short_window_visibility_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let selected_short_window_visibility_id = match short_window_visibility {
            ShortWindowVisibility::Always => IDM_SHORT_WINDOW_VISIBILITY_ALWAYS,
            ShortWindowVisibility::WarningOnly => IDM_SHORT_WINDOW_VISIBILITY_WARNING_ONLY,
            ShortWindowVisibility::Hidden => IDM_SHORT_WINDOW_VISIBILITY_HIDDEN,
        };
        let _ = CheckMenuRadioItem(
            short_window_visibility_menu,
            IDM_SHORT_WINDOW_VISIBILITY_ALWAYS as u32,
            IDM_SHORT_WINDOW_VISIBILITY_HIDDEN as u32,
            selected_short_window_visibility_id as u32,
            MF_BYCOMMAND.0,
        );
        // Short-window (5h) alert sensitivity submenu.
        let short_window_alert_sensitivity_menu = CreatePopupMenu().unwrap();
        let short_window_alert_sensitivity_items: [(u16, ShortWindowAlertSensitivity, &str); 3] = [
            (
                IDM_SHORT_WINDOW_ALERT_SENSITIVITY_SENSITIVE,
                ShortWindowAlertSensitivity::Sensitive,
                strings.short_window_alert_sensitivity_sensitive,
            ),
            (
                IDM_SHORT_WINDOW_ALERT_SENSITIVITY_STANDARD,
                ShortWindowAlertSensitivity::Standard,
                strings.standard_level,
            ),
            (
                IDM_SHORT_WINDOW_ALERT_SENSITIVITY_RELAXED,
                ShortWindowAlertSensitivity::Relaxed,
                strings.short_window_alert_sensitivity_relaxed,
            ),
        ];
        for (id, value, label) in short_window_alert_sensitivity_items {
            let label_str = native_interop::wide_str(label);
            let flags = if value == short_window_alert_sensitivity {
                MF_CHECKED
            } else {
                MENU_ITEM_FLAGS(0)
            };
            let _ = AppendMenuW(
                short_window_alert_sensitivity_menu,
                flags,
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        let selected_short_window_alert_sensitivity_id = match short_window_alert_sensitivity {
            ShortWindowAlertSensitivity::Sensitive => IDM_SHORT_WINDOW_ALERT_SENSITIVITY_SENSITIVE,
            ShortWindowAlertSensitivity::Standard => IDM_SHORT_WINDOW_ALERT_SENSITIVITY_STANDARD,
            ShortWindowAlertSensitivity::Relaxed => IDM_SHORT_WINDOW_ALERT_SENSITIVITY_RELAXED,
        };
        let _ = CheckMenuRadioItem(
            short_window_alert_sensitivity_menu,
            IDM_SHORT_WINDOW_ALERT_SENSITIVITY_SENSITIVE as u32,
            IDM_SHORT_WINDOW_ALERT_SENSITIVITY_RELAXED as u32,
            selected_short_window_alert_sensitivity_id as u32,
            MF_BYCOMMAND.0,
        );
        // Assemble Display Settings in semantic groups: content/judgment,
        // then visual density/layout/theme.
        let display_basis_label = native_interop::wide_str(strings.usage_display_basis);
        let _ = AppendMenuW(
            display_settings_menu,
            MF_POPUP,
            display_basis_menu.0 as usize,
            PCWSTR::from_raw(display_basis_label.as_ptr()),
        );
        let reset_display_label = native_interop::wide_str(strings.reset_display);
        let _ = AppendMenuW(
            display_settings_menu,
            MF_POPUP,
            reset_display_menu.0 as usize,
            PCWSTR::from_raw(reset_display_label.as_ptr()),
        );

        let short_window_menu = CreatePopupMenu().unwrap();
        let short_window_visibility_label =
            native_interop::wide_str(strings.short_window_visibility);
        let _ = AppendMenuW(
            short_window_menu,
            MF_POPUP,
            short_window_visibility_menu.0 as usize,
            PCWSTR::from_raw(short_window_visibility_label.as_ptr()),
        );
        let short_window_alert_sensitivity_label =
            native_interop::wide_str(strings.short_window_alert_sensitivity);
        let _ = AppendMenuW(
            short_window_menu,
            MF_POPUP,
            short_window_alert_sensitivity_menu.0 as usize,
            PCWSTR::from_raw(short_window_alert_sensitivity_label.as_ptr()),
        );
        let short_window_label = native_interop::wide_str(strings.session_window_label);
        let _ = AppendMenuW(
            display_settings_menu,
            MF_POPUP,
            short_window_menu.0 as usize,
            PCWSTR::from_raw(short_window_label.as_ptr()),
        );

        let _ = AppendMenuW(display_settings_menu, MF_SEPARATOR, 0, PCWSTR::null());

        let display_density_label = native_interop::wide_str(strings.display_density);
        let _ = AppendMenuW(
            display_settings_menu,
            MF_POPUP,
            display_density_menu.0 as usize,
            PCWSTR::from_raw(display_density_label.as_ptr()),
        );
        let popup_layout_label = native_interop::wide_str(strings.popup_layout);
        let _ = AppendMenuW(
            display_settings_menu,
            MF_POPUP,
            popup_layout_menu.0 as usize,
            PCWSTR::from_raw(popup_layout_label.as_ptr()),
        );
        let app_theme_label = native_interop::wide_str(strings.app_theme);
        let _ = AppendMenuW(
            display_settings_menu,
            MF_POPUP,
            app_theme_menu.0 as usize,
            PCWSTR::from_raw(app_theme_label.as_ptr()),
        );

        let display_settings_label = native_interop::wide_str(strings.display_settings);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            display_settings_menu.0 as usize,
            PCWSTR::from_raw(display_settings_label.as_ptr()),
        );

        let language_label = native_interop::wide_str(strings.language);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            language_menu.0 as usize,
            PCWSTR::from_raw(language_label.as_ptr()),
        );

        let github_copilot_label = native_interop::wide_str(strings.github_copilot);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            copilot_plan_menu.0 as usize,
            PCWSTR::from_raw(github_copilot_label.as_ptr()),
        );

        let help_menu = CreatePopupMenu().unwrap();
        for (id, label) in [
            (IDM_HELP_DISPLAY_GUIDE, strings.help_display_guide),
            (IDM_HELP_AI_QUOTAS, strings.help_ai_quotas),
        ] {
            let label_str = native_interop::wide_str(label);
            let _ = AppendMenuW(
                help_menu,
                MENU_ITEM_FLAGS(0),
                id as usize,
                PCWSTR::from_raw(label_str.as_ptr()),
            );
        }
        #[cfg(feature = "antigravity")]
        {
            let antigravity_setup_label =
                native_interop::wide_str(strings.antigravity_setup_menu_item);
            let _ = AppendMenuW(
                help_menu,
                MENU_ITEM_FLAGS(0),
                IDM_HELP_ANTIGRAVITY_SETUP as usize,
                PCWSTR::from_raw(antigravity_setup_label.as_ptr()),
            );
        }
        let _ = AppendMenuW(help_menu, MF_SEPARATOR, 0, PCWSTR::null());

        let readme_label = native_interop::wide_str(strings.help_readme);
        let _ = AppendMenuW(
            help_menu,
            MENU_ITEM_FLAGS(0),
            IDM_HELP_README as usize,
            PCWSTR::from_raw(readme_label.as_ptr()),
        );

        let update_label = native_interop::wide_str(strings.check_for_updates);
        #[cfg(feature = "self-update")]
        {
            let _ = AppendMenuW(
                help_menu,
                MENU_ITEM_FLAGS(0),
                IDM_VERSION_ACTION as usize,
                PCWSTR::from_raw(update_label.as_ptr()),
            );
        }
        #[cfg(not(feature = "self-update"))]
        {
            let _ = AppendMenuW(
                help_menu,
                MF_GRAYED,
                0,
                PCWSTR::from_raw(update_label.as_ptr()),
            );
        }

        let version_label = native_interop::wide_str(strings.help_version_information);
        let _ = AppendMenuW(
            help_menu,
            MENU_ITEM_FLAGS(0),
            IDM_HELP_VERSION_INFORMATION as usize,
            PCWSTR::from_raw(version_label.as_ptr()),
        );

        let help_label = native_interop::wide_str(strings.help);
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            help_menu.0 as usize,
            PCWSTR::from_raw(help_label.as_ptr()),
        );

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let exit_str = native_interop::wide_str(strings.exit);
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            2,
            PCWSTR::from_raw(exit_str.as_ptr()),
        );

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);

        if virtual_desktop_scope == VirtualDesktopScope::CurrentOnly {
            // `hwnd` may currently be assigned to a different virtual
            // desktop than the one the menu was opened from. Bringing it to
            // the foreground here would make Windows switch the active
            // desktop out from under the user just to open a menu, so route
            // the menu through a throwaway tool window that always lives on
            // the current desktop instead, and pick the command up via
            // `TPM_RETURNCMD` rather than the normal `WM_COMMAND` routing
            // (which would otherwise go to that throwaway window, not the
            // real one).
            if let Some(owner) = virtual_desktop::create_probe_window() {
                let _ = SetForegroundWindow(owner);
                let cmd = TrackPopupMenu(
                    menu,
                    TPM_RIGHTBUTTON | TPM_RETURNCMD,
                    pt.x,
                    pt.y,
                    0,
                    owner,
                    None,
                );
                let _ = DestroyWindow(owner);
                if cmd.0 != 0 {
                    let _ = PostMessageW(hwnd, WM_COMMAND, WPARAM(cmd.0 as usize), LPARAM(0));
                }
            } else {
                // No public API path to a safe menu owner (COM/probe window
                // creation failed) — fail open to the previous behavior
                // rather than not showing the menu at all.
                let _ = SetForegroundWindow(hwnd);
                let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
            }
        } else {
            let _ = SetForegroundWindow(hwnd);
            let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
        }
        let _ = DestroyMenu(menu);
    }
}

/// Paint for non-embedded fallback (normal WM_PAINT path)
fn paint(hdc: HDC, hwnd: HWND) {
    let (
        app_theme,
        strings,
        short_window_visibility,
        popup_layout,
        session_state,
        session_pct,
        session_text,
        session_pace,
        weekly_state,
        weekly_pct,
        weekly_text,
        weekly_pace,
        weekly_remaining_text,
        codex_session_state,
        codex_session_pct,
        codex_session_text,
        codex_session_pace,
        codex_weekly_state,
        codex_weekly_pct,
        codex_weekly_text,
        codex_weekly_pace,
        codex_weekly_remaining_text,
        codex_banked_reset_text,
        antigravity_session_state,
        antigravity_session_pct,
        antigravity_session_text,
        antigravity_session_pace,
        antigravity_weekly_pct,
        antigravity_weekly_text,
        antigravity_weekly_pace,
        antigravity_weekly_remaining_text,
        antigravity_extra_items,
        github_copilot_percent,
        github_copilot_text,
        show_claude_code,
        show_codex,
        show_antigravity,
        show_github_copilot,
        hovered_auth_cta_rect,
    ) = {
        let state = lock_state();
        match state.as_ref() {
            Some(s) => (
                s.app_theme,
                s.language.strings(),
                s.short_window_visibility,
                s.popup_layout,
                s.session_state,
                s.session_percent,
                s.session_text.clone(),
                s.session_pace.clone(),
                s.weekly_state,
                s.weekly_percent,
                s.weekly_text.clone(),
                s.weekly_pace.clone(),
                s.weekly_remaining_text.clone(),
                s.codex_session_state,
                s.codex_session_percent,
                s.codex_session_text.clone(),
                s.codex_session_pace.clone(),
                s.codex_weekly_state,
                s.codex_weekly_percent,
                s.codex_weekly_text.clone(),
                s.codex_weekly_pace.clone(),
                s.codex_weekly_remaining_text.clone(),
                s.codex_banked_reset_text.clone(),
                s.antigravity_session_state,
                s.antigravity_session_percent,
                s.antigravity_session_text.clone(),
                s.antigravity_session_pace.clone(),
                s.antigravity_weekly_percent,
                s.antigravity_weekly_text.clone(),
                s.antigravity_weekly_pace.clone(),
                s.antigravity_weekly_remaining_text.clone(),
                s.antigravity_extra_items.clone(),
                s.github_copilot_percent,
                s.github_copilot_text.clone(),
                s.show_claude_code,
                s.show_codex,
                s.show_antigravity,
                s.show_github_copilot,
                s.hovered_auth_cta_rect,
            ),
            None => return,
        }
    };

    let palette = popup_palette(app_theme);
    let provider_tint_dark = theme_is_dark_variant(app_theme);
    let show_column_dividers = theme_shows_column_dividers(app_theme);
    let outline_usage_track = theme_outlines_usage_track(app_theme);
    let accent = claude_accent_color();
    let codex_accent = codex_accent_color();
    let antigravity_accent = antigravity_accent_color();
    let mut auth_cta_hits: Vec<AuthCtaHitTarget> = Vec::new();

    unsafe {
        let mut client_rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut client_rect);
        let width = client_rect.right - client_rect.left;
        let height = client_rect.bottom - client_rect.top;

        if width <= 0 || height <= 0 {
            return;
        }

        let mem_dc = CreateCompatibleDC(hdc);
        let mem_bmp = CreateCompatibleBitmap(hdc, width, height);
        let old_bmp = SelectObject(mem_dc, mem_bmp);

        paint_content(
            mem_dc,
            width,
            height,
            provider_tint_dark,
            &palette.background,
            &palette.primary_text,
            &palette.secondary_text,
            &accent,
            &palette.track,
            &palette.border,
            &palette.warning,
            &palette.heading_text,
            strings,
            short_window_visibility,
            session_state,
            session_pct,
            &session_text,
            session_pace.as_ref(),
            weekly_state,
            weekly_pct,
            &weekly_text,
            weekly_pace.as_ref(),
            weekly_remaining_text.as_deref(),
            codex_session_state,
            codex_session_pct,
            &codex_session_text,
            codex_session_pace.as_ref(),
            codex_weekly_state,
            codex_weekly_pct,
            &codex_weekly_text,
            codex_weekly_pace.as_ref(),
            codex_weekly_remaining_text.as_deref(),
            &codex_banked_reset_text,
            antigravity_session_state,
            antigravity_session_pct,
            &antigravity_session_text,
            antigravity_session_pace.as_ref(),
            antigravity_weekly_pct,
            &antigravity_weekly_text,
            antigravity_weekly_pace.as_ref(),
            antigravity_weekly_remaining_text.as_deref(),
            &antigravity_extra_items,
            github_copilot_percent,
            &github_copilot_text,
            show_claude_code,
            show_codex,
            show_antigravity,
            show_github_copilot,
            &codex_accent,
            &antigravity_accent,
            popup_layout,
            show_column_dividers,
            outline_usage_track,
            hovered_auth_cta_rect,
            &mut auth_cta_hits,
        );

        let _ = BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);

        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(mem_bmp);
        let _ = DeleteDC(mem_dc);
    }

    let mut state = lock_state();
    if let Some(s) = state.as_mut() {
        s.auth_cta_hit_targets = auth_cta_hits;
    }
}

/// Draws the provider-name header row: one text label per active provider
/// column, aligned with the same `model_x` positions `draw_row` uses for its
/// bars, so a provider is identifiable by its full localized name rather
/// than by accent color or an invented abbreviation.
///
/// `show_weekly_remaining` gates the Compact-only weekly-remaining text
/// (AUM-WINDOW-UI-01C-1): when `true`, each provider's own already-resolved
/// remaining-time string (`None` when there's nothing safe to show — see
/// `compact_weekly_remaining_for_cell`) is right-aligned in that same
/// provider's column, next to its name — never drawn at all, `Standard`
/// never passes `true` here.
fn draw_provider_header_row(
    hdc: HDC,
    client_width: i32,
    x: i32,
    y: i32,
    text_color: &Color,
    strings: Strings,
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    show_github_copilot: bool,
    show_weekly_remaining: bool,
    claude_weekly_remaining: Option<&str>,
    codex_weekly_remaining: Option<&str>,
    antigravity_weekly_remaining: Option<&str>,
    codex_banked_reset_text: &str,
) {
    let active_models = active_family_count(
        show_claude_code,
        show_codex,
        show_antigravity,
        show_github_copilot,
    );
    let column_width = provider_column_width_for_client(client_width, active_models);
    let dpi = CURRENT_DPI.load(Ordering::Relaxed);

    unsafe {
        let _ = SetTextColor(hdc, COLORREF(text_color.to_colorref()));
        let mut model_x = x + sc(LABEL_WIDTH) + sc(LABEL_RIGHT_MARGIN);
        if show_claude_code {
            let header_width = header_column_width_avoiding_help(
                model_x,
                column_width,
                client_width,
                dpi,
                !show_codex && !show_antigravity && !show_github_copilot,
            );
            draw_header_label(hdc, model_x, y, header_width, strings.claude_code_model);
            if show_weekly_remaining {
                draw_header_remaining_if_fits(
                    hdc,
                    model_x,
                    y,
                    header_width,
                    !show_codex && !show_antigravity,
                    strings.claude_code_model,
                    claude_weekly_remaining,
                );
            }
            model_x += column_width + sc(MODEL_RIGHT_MARGIN);
        }
        if show_codex {
            let codex_header = format!("{} · {}", strings.codex_model, codex_banked_reset_text);
            let header_width = header_column_width_avoiding_help(
                model_x,
                column_width,
                client_width,
                dpi,
                !show_antigravity && !show_github_copilot,
            );
            draw_header_label(hdc, model_x, y, header_width, &codex_header);
            if show_weekly_remaining {
                draw_header_remaining_if_fits(
                    hdc,
                    model_x,
                    y,
                    header_width,
                    !show_antigravity,
                    &codex_header,
                    codex_weekly_remaining,
                );
            }
            model_x += column_width + sc(MODEL_RIGHT_MARGIN);
        }
        if show_antigravity {
            let header_width = header_column_width_avoiding_help(
                model_x,
                column_width,
                client_width,
                dpi,
                !show_github_copilot,
            );
            draw_header_label(hdc, model_x, y, header_width, strings.antigravity_model);
            if show_weekly_remaining {
                draw_header_remaining_if_fits(
                    hdc,
                    model_x,
                    y,
                    header_width,
                    true,
                    strings.antigravity_model,
                    antigravity_weekly_remaining,
                );
            }
            model_x += column_width + sc(MODEL_RIGHT_MARGIN);
        }
        if show_github_copilot {
            let header_width =
                header_column_width_avoiding_help(model_x, column_width, client_width, dpi, true);
            draw_header_label(hdc, model_x, y, header_width, "GitHub Copilot");
        }
    }
}

fn draw_header_label(hdc: HDC, x: i32, y: i32, width: i32, label: &str) {
    unsafe {
        let mut label_wide: Vec<u16> = label.encode_utf16().collect();
        let mut label_rect = RECT {
            left: x,
            top: y,
            right: x + width,
            bottom: y + sc(HEADER_ROW_H),
        };
        let _ = DrawTextW(
            hdc,
            &mut label_wide,
            &mut label_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );
    }
}

/// Logical gap kept clear between a provider's name and its right-aligned
/// weekly-remaining text (see `draw_header_remaining_if_fits`) so the two
/// never visually run together even when both are close to the column's
/// measured width.
const HEADER_REMAINING_GAP_W: i32 = 8;

/// Logical margin reserved at a non-final provider column's right edge
/// (AUM-WINDOW-UI-01C-1 visual-review follow-up: without this, a
/// full-width remaining-time string sits flush against the next provider's
/// name — e.g. "残り5日13時間Codex" reading as one run-on string, even
/// though neither string is clipped or literally overlapping). Not applied
/// to the last shown column, which has no next-provider name after it and
/// already keeps the popup's own `RIGHT_MARGIN` beyond `column_width` —
/// see `draw_header_remaining_if_fits`'s `is_last_column`.
const COLUMN_BOUNDARY_GAP_W: i32 = 8;

/// Live-measured width (in the font currently selected into `hdc`) of
/// `text`, via `GetTextExtentPoint32W` — the same GDI primitive
/// `draw_header_remaining_if_fits` uses to decide whether the Compact
/// weekly-remaining text actually fits next to a provider name, rather than
/// assuming it fits from a fixed character count (see `TEXT_WIDTH`'s own doc
/// for why that assumption doesn't hold across 11 languages).
fn text_extent_width(hdc: HDC, text: &str) -> i32 {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE::default();
    unsafe {
        let _ = GetTextExtentPoint32W(hdc, &wide, &mut size);
    }
    size.cx
}

/// Draws `remaining` right-aligned in the provider header row within
/// `model_x..model_x + column_width`, but only when `name` (already drawn
/// left-aligned by the caller), the `HEADER_REMAINING_GAP_W` gap, and
/// `remaining` all actually fit — measured live via `text_extent_width`
/// against the font already selected into `hdc`. Draws nothing at all when
/// they don't fit: the provider name always wins, and a clipped or
/// overlapping remaining-time string would be worse than omitting it.
///
/// `is_last_column` reserves `COLUMN_BOUNDARY_GAP_W` at the column's right
/// edge for every column except the last shown one — both from the fit
/// check and from the drawn rect's right edge — so a non-final column's
/// remaining-time text keeps clear air before the next provider's name
/// instead of sitting flush against it. The last shown column has no next
/// provider to guard against, so it keeps using the full `column_width`
/// exactly as before this margin was added.
fn draw_header_remaining_if_fits(
    hdc: HDC,
    model_x: i32,
    y: i32,
    column_width: i32,
    is_last_column: bool,
    name: &str,
    remaining: Option<&str>,
) {
    let Some(remaining) = remaining else {
        return;
    };
    if remaining.is_empty() {
        return;
    }
    let boundary_margin = if is_last_column {
        0
    } else {
        sc(COLUMN_BOUNDARY_GAP_W)
    };
    let available_width = column_width - boundary_margin;
    let name_w = text_extent_width(hdc, name);
    let remaining_w = text_extent_width(hdc, remaining);
    if name_w + sc(HEADER_REMAINING_GAP_W) + remaining_w > available_width {
        return;
    }
    unsafe {
        let mut remaining_wide: Vec<u16> = remaining.encode_utf16().collect();
        let mut rect = RECT {
            left: model_x,
            top: y,
            right: model_x + available_width,
            bottom: y + sc(HEADER_ROW_H),
        };
        let _ = DrawTextW(
            hdc,
            &mut remaining_wide,
            &mut rect,
            DT_RIGHT | DT_VCENTER | DT_SINGLELINE,
        );
    }
}

/// The left-edge x position of each shown provider's bar/value column,
/// mirroring `draw_row`'s own internal `model_x` walk exactly (same
/// constants, same order) so pace-guidance text drawn separately still
/// lines up under the right bar. A provider's slot is `0` when it isn't
/// shown — harmless, since every caller here gates on the matching
/// `show_*` flag before using it.
fn provider_column_x_positions(
    client_width: i32,
    content_x: i32,
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    show_github_copilot: bool,
) -> (i32, i32, i32) {
    let active_families = active_family_count(
        show_claude_code,
        show_codex,
        show_antigravity,
        show_github_copilot,
    );
    let column_width = provider_column_width_for_client(client_width, active_families);

    let mut model_x = content_x + sc(LABEL_WIDTH) + sc(LABEL_RIGHT_MARGIN);
    let mut claude_x = 0;
    let mut codex_x = 0;
    let mut antigravity_x = 0;
    if show_claude_code {
        claude_x = model_x;
        model_x += column_width + sc(MODEL_RIGHT_MARGIN);
    }
    if show_codex {
        codex_x = model_x;
        model_x += column_width + sc(MODEL_RIGHT_MARGIN);
    }
    if show_antigravity {
        antigravity_x = model_x;
    }
    (claude_x, codex_x, antigravity_x)
}

/// One line of pace-guidance text under a provider's column, sized to that
/// provider's dynamically allocated bar+value column width — comfortable
/// for the common single-provider popup; a 2-3 provider layout may clip a
/// long localized string. Not visually verified on this machine — see
/// completion report.
fn draw_pace_text_line(hdc: HDC, x: i32, y: i32, width: i32, text: &str, color: &Color) {
    unsafe {
        let _ = SetTextColor(hdc, COLORREF(color.to_colorref()));
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            left: x,
            top: y,
            right: x + width,
            bottom: y + sc(PACE_LINE_H),
        };
        // DT_END_ELLIPSIS: a too-long localized string truncates with a
        // visible "…" instead of silently clipping mid-glyph at the column
        // edge — see AUM-PACE-GUIDANCE-01's popup-connection completion
        // report on `TEXT_WIDTH`'s worst-case sizing not being live-measured.
        let _ = DrawTextW(
            hdc,
            &mut wide,
            &mut rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );
    }
}

/// Draws one provider's weekly secondary/detail lines (Standard/Detailed
/// density only — see `weekly_pace_guidance_lines`), stacked below its bar.
/// `None` (not `CellState::Ok`, or no usable pace data) draws nothing,
/// leaving the reserved space blank rather than showing stale text.
fn draw_weekly_pace_extra_lines(
    hdc: HDC,
    x: i32,
    y: i32,
    width: i32,
    pace: Option<&PaceGuidanceLines>,
    text_color: &Color,
) {
    let Some(lines) = pace else {
        return;
    };
    let mut line_y = y;
    if let Some(secondary) = &lines.secondary {
        draw_pace_text_line(hdc, x, line_y, width, secondary, text_color);
        line_y += sc(PACE_LINE_H);
    }
    if let Some(detail) = &lines.detail {
        draw_pace_text_line(hdc, x, line_y, width, detail, text_color);
    }
}

struct RowCell<'a> {
    percent: Option<f64>,
    text: &'a str,
    accent: &'a Color,
    provider_text_color: Color,
    is_warning: bool,
    /// `Some` only for a Claude/Codex cell currently showing a high-confidence
    /// auth-required status (see `provider_auth_action`) — makes this cell's
    /// status text a click target for that action. `None` for every other
    /// cell, including every GitHub Copilot/Antigravity cell regardless of
    /// state.
    auth_action: Option<AuthAction>,
}

fn draw_row(
    hdc: HDC,
    client_width: i32,
    x: i32,
    y: i32,
    text_color: &Color,
    label: &str,
    cells: &[RowCell<'_>],
    track: &Color,
    warning: &Color,
    track_outline: Option<&Color>,
    hovered_auth_cta_rect: Option<RECT>,
    auth_cta_hits: &mut Vec<AuthCtaHitTarget>,
) {
    let seg_h = sc(SEGMENT_H);
    let active_models = (cells.len() as i32).max(1);
    let segment_count = row_bar_segment_count(active_models);
    let column_width = provider_column_width_for_client(client_width, active_models);
    let text_width = (column_width
        - usage_bar_device_width(segment_count, CURRENT_DPI.load(Ordering::Relaxed)))
    .max(0);
    let use_model_text_colors = active_models > 1;
    // `is_warning` always wins the *value text* color, regardless of
    // `use_model_text_colors` — but never touches the bar segments below
    // (`draw_usage_bar`'s `accent` argument, passed separately), which stay
    // the provider's own identification color even while warning.
    unsafe {
        let _ = SetTextColor(hdc, COLORREF(text_color.to_colorref()));
        let mut label_wide: Vec<u16> = label.encode_utf16().collect();
        let mut label_rect = RECT {
            left: x,
            top: y,
            right: x + sc(LABEL_WIDTH),
            bottom: y + seg_h,
        };
        let _ = DrawTextW(
            hdc,
            &mut label_wide,
            &mut label_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );

        let mut model_x = x + sc(LABEL_WIDTH) + sc(LABEL_RIGHT_MARGIN);
        for (index, cell) in cells.iter().enumerate() {
            let value_color = if cell.is_warning {
                *warning
            } else if use_model_text_colors {
                cell.provider_text_color
            } else {
                *text_color
            };
            let drawn_text_rect = draw_usage_bar(
                hdc,
                model_x,
                y,
                segment_count,
                text_width,
                cell.percent,
                cell.text,
                cell.accent,
                track,
                &value_color,
                track_outline,
                cell.auth_action.is_some(),
                hovered_auth_cta_rect,
            );
            if let (Some(action), Some(rect)) = (cell.auth_action, drawn_text_rect) {
                auth_cta_hits.push(AuthCtaHitTarget { rect, action });
            }
            if index + 1 < cells.len() {
                model_x += column_width + sc(MODEL_RIGHT_MARGIN);
            }
        }
    }
}

/// A usage-bar cell has nothing to draw when there's no percentage *and* no
/// status text either. Before `session_cell_decision` (AUM-PACE-GUIDANCE-01)
/// existed, `percent = None` always came with a non-empty status word (e.g.
/// "Loading"), so `draw_usage_bar` never saw `(None, "")`. `session_cell_decision`
/// can now return exactly that combination for a provider whose session row
/// is suppressed this poll, while the provider's column is still shown —
/// `draw_row` has no per-cell "skip" signal, so it calls `draw_usage_bar`
/// regardless. An empty `&str` turned into a `Vec<u16>` via
/// `encode_utf16().collect()` is a zero-length allocation with no real
/// backing memory (Rust gives it a dangling, merely-aligned pointer); handing
/// that pointer+len to `DrawTextW` is what crashed here.
fn usage_bar_has_content(percent: Option<f64>, text: &str) -> bool {
    percent.is_some() || !text.is_empty()
}

/// `percent` is `None` for any cell without a real current value (loading,
/// error, unconfigured, not-available). In that case no segment — neither
/// filled nor empty track — is drawn, so the bar area is left blank rather
/// than rendering what would look like an ordinary 0% bar; the status word
/// in `text` is the only thing shown for that cell. This is the one place
/// `CellDisplay::bar_percent` actually reaches the screen. When there is
/// neither a percentage nor status text (`usage_bar_has_content` is false —
/// see its doc comment), nothing is drawn at all: the cell's column position
/// is simply left blank rather than attempting to draw empty content.
fn draw_usage_bar(
    hdc: HDC,
    bar_x: i32,
    y: i32,
    segment_count: i32,
    text_width: i32,
    percent: Option<f64>,
    text: &str,
    accent: &Color,
    track: &Color,
    text_color: &Color,
    track_outline: Option<&Color>,
    is_auth_cta: bool,
    hovered_auth_cta_rect: Option<RECT>,
) -> Option<RECT> {
    if !usage_bar_has_content(percent, text) {
        return None;
    }
    let seg_w = sc(SEGMENT_W);
    let seg_h = sc(SEGMENT_H);
    let seg_gap = sc(SEGMENT_GAP);
    let corner_r = sc(CORNER_RADIUS);

    unsafe {
        if let Some(percent) = percent {
            let percent_clamped = percent.clamp(0.0, 100.0);
            let segment_percent = 100.0 / segment_count as f64;

            for i in 0..segment_count {
                let seg_x = bar_x + i * (seg_w + seg_gap);
                let seg_start = (i as f64) * segment_percent;
                let seg_end = seg_start + segment_percent;

                let seg_rect = RECT {
                    left: seg_x,
                    top: y,
                    right: seg_x + seg_w,
                    bottom: y + seg_h,
                };

                if percent_clamped >= seg_end {
                    // Fully provider-filled: never draw a track outline here
                    // (see `track_outline`'s doc) — the whole segment is
                    // provider-accent color.
                    draw_rounded_rect(hdc, &seg_rect, accent, corner_r);
                } else if percent_clamped <= seg_start {
                    draw_rounded_rect(hdc, &seg_rect, track, corner_r);
                    if let Some(outline_color) = track_outline {
                        // Fully track-colored segment: the outline is safe
                        // over its whole area, nothing here is provider fill.
                        let outline_rgn = CreateRoundRectRgn(
                            seg_rect.left,
                            seg_rect.top,
                            seg_rect.right + 1,
                            seg_rect.bottom + 1,
                            corner_r * 2,
                            corner_r * 2,
                        );
                        let outline_brush = CreateSolidBrush(COLORREF(outline_color.to_colorref()));
                        let outline_w = sc(1).max(1);
                        let _ = FrameRgn(hdc, outline_rgn, outline_brush, outline_w, outline_w);
                        let _ = DeleteObject(outline_brush);
                        let _ = DeleteObject(outline_rgn);
                    }
                } else {
                    draw_rounded_rect(hdc, &seg_rect, track, corner_r);
                    let fraction = (percent_clamped - seg_start) / segment_percent;
                    let fill_width = (seg_w as f64 * fraction) as i32;
                    if fill_width > 0 {
                        let fill_rect = RECT {
                            left: seg_x,
                            top: y,
                            right: seg_x + fill_width,
                            bottom: y + seg_h,
                        };
                        let rgn = CreateRoundRectRgn(
                            seg_rect.left,
                            seg_rect.top,
                            seg_rect.right + 1,
                            seg_rect.bottom + 1,
                            corner_r * 2,
                            corner_r * 2,
                        );
                        let _ = SelectClipRgn(hdc, rgn);
                        let brush = CreateSolidBrush(COLORREF(accent.to_colorref()));
                        FillRect(hdc, &fill_rect, brush);
                        let _ = DeleteObject(brush);
                        let _ = SelectClipRgn(hdc, HRGN::default());
                        let _ = DeleteObject(rgn);
                    }
                    if let Some(outline_color) = track_outline {
                        // Partial segment: clip to the still-track-colored
                        // remainder (right of `fill_width`) before drawing,
                        // so the outline never touches the provider-accent
                        // pixels drawn just above.
                        let track_clip = CreateRectRgn(
                            seg_x + fill_width,
                            seg_rect.top,
                            seg_rect.right,
                            seg_rect.bottom,
                        );
                        let _ = SelectClipRgn(hdc, track_clip);
                        let outline_rgn = CreateRoundRectRgn(
                            seg_rect.left,
                            seg_rect.top,
                            seg_rect.right + 1,
                            seg_rect.bottom + 1,
                            corner_r * 2,
                            corner_r * 2,
                        );
                        let outline_brush = CreateSolidBrush(COLORREF(outline_color.to_colorref()));
                        let outline_w = sc(1).max(1);
                        let _ = FrameRgn(hdc, outline_rgn, outline_brush, outline_w, outline_w);
                        let _ = DeleteObject(outline_brush);
                        let _ = DeleteObject(outline_rgn);
                        let _ = SelectClipRgn(hdc, HRGN::default());
                        let _ = DeleteObject(track_clip);
                    }
                }
            }
        }

        let text_x = bar_x + segment_count * (seg_w + seg_gap) - seg_gap + sc(BAR_RIGHT_MARGIN);
        let mut text_wide: Vec<u16> = text.encode_utf16().collect();
        let mut text_rect = RECT {
            left: text_x,
            top: y,
            right: text_x + text_width,
            bottom: y + seg_h,
        };
        let _ = SetTextColor(hdc, COLORREF(text_color.to_colorref()));
        // A zero-length `text_wide` (empty `text`) has no real backing
        // allocation — see `usage_bar_has_content`'s doc comment — so never
        // hand it to `DrawTextW`. Independent from the whole-cell skip
        // above: also covers a future `Some(percent)` with empty `text`
        // (bar segments still draw; only this text-draw step is skipped).
        if text_wide.is_empty() {
            return None;
        }
        // DT_END_ELLIPSIS (AUM-WINDOW-UI-01C-1): a value+reset string
        // that overflows `TEXT_WIDTH` (e.g. a long localized reset-in
        // phrase in a 2-3 provider Standard layout — see `TEXT_WIDTH`'s
        // own doc on its worst-case sizing not being live-measured) now
        // truncates with a visible "…" instead of a silent hard clip.
        // Matches `draw_pace_text_line`'s existing ellipsis handling for
        // the weekly pace-guidance lines below this row.
        let _ = DrawTextW(
            hdc,
            &mut text_wide,
            &mut text_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );

        if !is_auth_cta {
            return None;
        }
        // AUTH-RECOVERY-CTA-IMPLEMENT-01: `text_rect` is the cell's fixed
        // layout rect (known before `DrawTextW`, not derived from it), so it
        // can be compared directly against `hovered_auth_cta_rect` — no
        // separate "am I hovered" plumbing is needed. The underline itself
        // is measured to the actual rendered text width (`GetTextExtentPoint32W`),
        // not the full column width, so it never underlines trailing blank
        // space past a short status word.
        if hovered_auth_cta_rect == Some(text_rect) {
            let mut extent = SIZE::default();
            let _ = GetTextExtentPoint32W(hdc, &text_wide, &mut extent);
            let underline_width = extent.cx.min(text_width);
            let underline_y = text_rect.top + (seg_h + extent.cy) / 2 + 1;
            let pen = CreatePen(PS_SOLID, 1, COLORREF(text_color.to_colorref()));
            let old_pen = SelectObject(hdc, pen);
            let _ = MoveToEx(hdc, text_rect.left, underline_y, None);
            let _ = LineTo(hdc, text_rect.left + underline_width, underline_y);
            SelectObject(hdc, old_pen);
            let _ = DeleteObject(pen);
        }
        Some(text_rect)
    }
}

fn draw_rounded_rect(hdc: HDC, rect: &RECT, color: &Color, radius: i32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(color.to_colorref()));
        let rgn = CreateRoundRectRgn(
            rect.left,
            rect.top,
            rect.right + 1,
            rect.bottom + 1,
            radius * 2,
            radius * 2,
        );
        let _ = FillRgn(hdc, rgn, brush);
        let _ = DeleteObject(rgn);
        let _ = DeleteObject(brush);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{QuotaItem, QuotaItemAvailability, QuotaUnit};

    fn generic_item(metric: Option<QuotaMetric>, resets_at: Option<SystemTime>) -> QuotaItem {
        QuotaItem {
            id: "test".to_string(),
            label: "Test".to_string(),
            availability: QuotaItemAvailability::Available,
            metric,
            unit: QuotaUnit::AiCredits,
            resets_at,
        }
    }

    fn copilot_item(used: f64) -> QuotaItem {
        QuotaItem {
            id: GITHUB_COPILOT_MONTHLY_ITEM_ID.to_string(),
            label: "Monthly AI credits".to_string(),
            availability: QuotaItemAvailability::Available,
            metric: Some(QuotaMetric::Used {
                used,
                limit: Some(1_500.0),
            }),
            unit: QuotaUnit::AiCredits,
            resets_at: None,
        }
    }

    fn copilot_success(items: Vec<QuotaItem>) -> poller::ProviderPollOutcome {
        poller::ProviderPollOutcome::Success {
            source: poller::ProviderPollSource::GithubBillingApi,
            attempted_at: SystemTime::UNIX_EPOCH,
            acquired_at: SystemTime::UNIX_EPOCH,
            usage: UsageData::from_quota_items(items),
        }
    }

    #[test]
    fn app_tray_icon_data_contains_one_provider_independent_icon() {
        for language in LanguageId::ALL {
            let strings = language.strings();
            let icons = app_tray_icon_data(strings);

            assert_eq!(icons.len(), 1);
            assert_eq!(icons[0].kind, tray_icon::TrayIconKind::App);
            assert!(icons[0].percent.is_none());
            assert_eq!(icons[0].tooltip, strings.window_title);
        }
    }

    #[test]
    fn copilot_positive_usage_is_ok() {
        let outcome = copilot_success(vec![copilot_item(0.681855)]);
        assert_eq!(
            poll_quota_item_state(
                QuotaFamilyId::GithubCopilot,
                &outcome,
                GITHUB_COPILOT_MONTHLY_ITEM_ID,
            ),
            CellState::Ok
        );
    }

    #[test]
    fn copilot_zero_usage_is_ok_and_not_not_available() {
        let item = copilot_item(0.0);
        let outcome = copilot_success(vec![item.clone()]);
        let state = poll_quota_item_state(
            QuotaFamilyId::GithubCopilot,
            &outcome,
            GITHUB_COPILOT_MONTHLY_ITEM_ID,
        );
        assert_eq!(state, CellState::Ok);
        assert_ne!(
            render_generic_quota_item(
                state,
                Some(&item),
                DisplayBasis::UsedPercentage,
                LanguageId::English.strings(),
            )
            .text,
            LanguageId::English.strings().not_available
        );
    }

    #[test]
    fn copilot_missing_or_unavailable_target_item_is_not_available() {
        let other = QuotaItem {
            id: "other".to_string(),
            ..copilot_item(1.0)
        };
        assert_eq!(
            poll_quota_item_state(
                QuotaFamilyId::GithubCopilot,
                &copilot_success(vec![other]),
                GITHUB_COPILOT_MONTHLY_ITEM_ID,
            ),
            CellState::NotAvailable
        );

        let unavailable = QuotaItem {
            availability: QuotaItemAvailability::Unavailable,
            metric: None,
            ..copilot_item(1.0)
        };
        assert_eq!(
            poll_quota_item_state(
                QuotaFamilyId::GithubCopilot,
                &copilot_success(vec![unavailable]),
                GITHUB_COPILOT_MONTHLY_ITEM_ID,
            ),
            CellState::NotAvailable
        );
    }

    #[test]
    fn provider_error_display_matrix_is_specific_only_for_high_confidence_cases() {
        use poller::PollError::{AuthRequired, NoCredentials, RequestFailed, TokenExpired};

        for (provider, error, expected) in [
            (
                QuotaFamilyId::Claude,
                TokenExpired,
                CellState::AuthenticationExpired,
            ),
            (
                QuotaFamilyId::Claude,
                NoCredentials,
                CellState::CredentialsUnavailable,
            ),
            (
                QuotaFamilyId::Claude,
                AuthRequired,
                CellState::AuthenticationProblem,
            ),
            (QuotaFamilyId::Claude, RequestFailed, CellState::FetchFailed),
            (
                QuotaFamilyId::Codex,
                TokenExpired,
                CellState::AuthenticationProblem,
            ),
            (
                QuotaFamilyId::Codex,
                NoCredentials,
                CellState::CredentialsUnavailable,
            ),
            (
                QuotaFamilyId::Codex,
                AuthRequired,
                CellState::AuthenticationProblem,
            ),
            (QuotaFamilyId::Codex, RequestFailed, CellState::FetchFailed),
            (
                QuotaFamilyId::Antigravity,
                NoCredentials,
                CellState::CredentialsUnavailable,
            ),
            (
                QuotaFamilyId::Antigravity,
                AuthRequired,
                CellState::AuthenticationProblem,
            ),
            (
                QuotaFamilyId::Antigravity,
                RequestFailed,
                CellState::FetchFailed,
            ),
            // Antigravity does not currently emit TokenExpired. Keeping this
            // unrecognized provider/error combination generic verifies the
            // conservative fallback.
            (
                QuotaFamilyId::Antigravity,
                TokenExpired,
                CellState::FetchFailed,
            ),
            (
                QuotaFamilyId::GithubCopilot,
                NoCredentials,
                CellState::FetchFailed,
            ),
            (
                QuotaFamilyId::GithubCopilot,
                AuthRequired,
                CellState::FetchFailed,
            ),
            (
                QuotaFamilyId::GithubCopilot,
                RequestFailed,
                CellState::FetchFailed,
            ),
            (
                QuotaFamilyId::GithubCopilot,
                TokenExpired,
                CellState::FetchFailed,
            ),
        ] {
            assert_eq!(provider_error_cell_state(provider, error), expected);
        }
    }

    #[test]
    fn generic_request_failure_is_fetch_failed_not_retrying() {
        let strings = LanguageId::Japanese.strings();
        for provider in [
            QuotaFamilyId::Claude,
            QuotaFamilyId::Codex,
            QuotaFamilyId::Antigravity,
            QuotaFamilyId::GithubCopilot,
        ] {
            let state = provider_error_cell_state(provider, poller::PollError::RequestFailed);
            assert_eq!(state, CellState::FetchFailed);
            assert_eq!(status_text(state, strings), strings.fetch_failed);
        }
    }

    #[test]
    fn japanese_provider_error_statuses_keep_the_intended_conservative_wording() {
        let strings = LanguageId::Japanese.strings();
        assert_eq!(strings.authentication_expired, "認証切れ");
        assert_eq!(strings.credentials_unavailable, "認証情報を確認できません");
        assert_eq!(strings.authentication_problem, "認証を確認してください");
        assert_eq!(strings.fetch_failed, "取得失敗");
    }

    // ── AUTH-RECOVERY-CTA-IMPLEMENT-01: only a high-confidence auth state,
    // for a provider with a safe official CLI login entry point, is ever
    // CTA-eligible. ──────────────────────────────────────────────────────

    #[test]
    fn claude_and_codex_auth_states_are_cta_eligible() {
        for state in [
            CellState::AuthenticationExpired,
            CellState::AuthenticationProblem,
            CellState::CredentialsUnavailable,
        ] {
            assert_eq!(
                provider_auth_action(QuotaFamilyId::Claude, state),
                Some(AuthAction::ClaudeLogin)
            );
            assert_eq!(
                provider_auth_action(QuotaFamilyId::Codex, state),
                Some(AuthAction::CodexLogin)
            );
        }
    }

    #[test]
    fn non_auth_states_are_never_cta_eligible_for_claude_or_codex() {
        for state in [
            CellState::Loading,
            CellState::Ok,
            CellState::FetchFailed,
            CellState::Disabled,
            CellState::NotAvailable,
        ] {
            assert_eq!(provider_auth_action(QuotaFamilyId::Claude, state), None);
            assert_eq!(provider_auth_action(QuotaFamilyId::Codex, state), None);
        }
    }

    #[test]
    fn github_copilot_and_antigravity_are_never_cta_eligible() {
        for state in [
            CellState::Loading,
            CellState::Ok,
            CellState::AuthenticationExpired,
            CellState::AuthenticationProblem,
            CellState::CredentialsUnavailable,
            CellState::FetchFailed,
            CellState::Disabled,
            CellState::NotAvailable,
        ] {
            assert_eq!(
                provider_auth_action(QuotaFamilyId::GithubCopilot, state),
                None,
                "GitHub Copilot must never be CTA-eligible (state={state:?}): its failure \
                 signal can't distinguish not-logged-in from wrong-scope"
            );
            assert_eq!(
                provider_auth_action(QuotaFamilyId::Antigravity, state),
                None,
                "Antigravity must never be CTA-eligible (state={state:?}): its CLI has no \
                 login/auth subcommand"
            );
        }
    }

    #[test]
    fn auth_action_commands_are_fixed_official_provider_logins() {
        assert_eq!(
            AuthAction::ClaudeLogin.command(),
            ("claude", ["auth", "login"].as_slice())
        );
        assert_eq!(
            AuthAction::CodexLogin.command(),
            ("codex", ["login"].as_slice())
        );
    }

    #[test]
    fn every_language_has_non_empty_provider_error_statuses() {
        for language in LanguageId::ALL {
            let strings = language.strings();
            assert!(!strings.authentication_expired.trim().is_empty());
            assert!(!strings.authentication_problem.trim().is_empty());
            assert!(!strings.credentials_unavailable.trim().is_empty());
            assert!(!strings.fetch_failed.trim().is_empty());
        }
    }

    #[test]
    fn copilot_errors_conservatively_fall_back_to_fetch_failed() {
        for error in [
            poller::PollError::RequestFailed,
            poller::PollError::AuthRequired,
            poller::PollError::NoCredentials,
            poller::PollError::TokenExpired,
        ] {
            let outcome = poller::ProviderPollOutcome::Error {
                source: poller::ProviderPollSource::GithubBillingApi,
                attempted_at: SystemTime::UNIX_EPOCH,
                error,
            };
            assert_eq!(
                poll_quota_item_state(
                    QuotaFamilyId::GithubCopilot,
                    &outcome,
                    GITHUB_COPILOT_MONTHLY_ITEM_ID,
                ),
                CellState::FetchFailed
            );
        }
    }

    #[test]
    fn copilot_disabled_has_distinct_state() {
        assert_eq!(
            poll_quota_item_state(
                QuotaFamilyId::GithubCopilot,
                &poller::ProviderPollOutcome::Disabled,
                GITHUB_COPILOT_MONTHLY_ITEM_ID,
            ),
            CellState::Disabled
        );
    }

    #[test]
    fn legacy_session_weekly_state_classification_is_unchanged() {
        let mut usage = UsageData::default();
        usage.set_session(UsageSection {
            percentage: 25.0,
            resets_at: None,
        });
        let outcome = poller::ProviderPollOutcome::Success {
            source: poller::ProviderPollSource::AnthropicOauthUsage,
            attempted_at: SystemTime::UNIX_EPOCH,
            acquired_at: SystemTime::UNIX_EPOCH,
            usage,
        };
        assert_eq!(
            poll_cell_states(QuotaFamilyId::Claude, &outcome),
            (CellState::Ok, CellState::NotAvailable)
        );
    }

    #[test]
    fn generic_renderer_handles_percentage_limit_usage_only_and_remaining() {
        let strings = LanguageId::English.strings();
        let percentage = generic_item(Some(QuotaMetric::Percentage(25.0)), None);
        let percentage_display = render_generic_quota_item(
            CellState::Ok,
            Some(&percentage),
            DisplayBasis::UsedPercentage,
            strings,
        );
        assert_eq!(percentage_display.bar_percent, Some(25.0));
        assert_eq!(percentage_display.text, "25%");

        let limited = generic_item(
            Some(QuotaMetric::Used {
                used: 375.0,
                limit: Some(1_500.0),
            }),
            None,
        );
        let limited_display = render_generic_quota_item(
            CellState::Ok,
            Some(&limited),
            DisplayBasis::UsedPercentage,
            strings,
        );
        assert_eq!(limited_display.bar_percent, Some(25.0));
        assert_eq!(limited_display.text, "375 / 1500 ai-credits");

        let limited_remaining_display = render_generic_quota_item(
            CellState::Ok,
            Some(&limited),
            DisplayBasis::RemainingAllowance,
            strings,
        );
        assert_eq!(limited_remaining_display.bar_percent, Some(75.0));
        assert_eq!(limited_remaining_display.text, "1125");

        let usage_only = generic_item(
            Some(QuotaMetric::Used {
                used: 17.5,
                limit: None,
            }),
            None,
        );
        let usage_only_display = render_generic_quota_item(
            CellState::Ok,
            Some(&usage_only),
            DisplayBasis::UsedPercentage,
            strings,
        );
        assert_eq!(usage_only_display.bar_percent, None);
        assert_eq!(usage_only_display.text, "17.50 ai-credits");

        let remaining = generic_item(
            Some(QuotaMetric::Remaining {
                remaining: 300.0,
                limit: Some(1_500.0),
            }),
            None,
        );
        assert_eq!(
            render_generic_quota_item(
                CellState::Ok,
                Some(&remaining),
                DisplayBasis::UsedPercentage,
                strings,
            )
            .bar_percent,
            Some(80.0)
        );
    }

    #[test]
    fn generic_renderer_keeps_reset_only_unavailable_and_zero_distinct() {
        let strings = LanguageId::English.strings();
        let reset_only = generic_item(None, Some(SystemTime::now() + Duration::from_secs(3_600)));
        let reset_display = render_generic_quota_item(
            CellState::Ok,
            Some(&reset_only),
            DisplayBasis::RemainingAllowance,
            strings,
        );
        assert_eq!(reset_display.bar_percent, None);
        assert!(reset_display.text.contains(strings.reset_in));

        let used_display = render_generic_quota_item(
            CellState::Ok,
            Some(&reset_only),
            DisplayBasis::UsedPercentage,
            strings,
        );
        assert_eq!(used_display.text, strings.not_available);
        assert!(!used_display.text.contains(strings.reset_in));
        assert!(!used_display.text.contains(strings.elapsed));

        let unavailable = QuotaItem::unavailable("missing", "Missing");
        let unavailable_display = render_generic_quota_item(
            CellState::Ok,
            Some(&unavailable),
            DisplayBasis::UsedPercentage,
            strings,
        );
        assert_eq!(unavailable_display.bar_percent, None);
        assert_eq!(unavailable_display.text, strings.not_available);

        let zero = generic_item(Some(QuotaMetric::Percentage(0.0)), None);
        let zero_display = render_generic_quota_item(
            CellState::Ok,
            Some(&zero),
            DisplayBasis::UsedPercentage,
            strings,
        );
        assert_eq!(zero_display.bar_percent, Some(0.0));
        assert_eq!(zero_display.text, "0%");
    }

    #[test]
    fn copilot_remaining_text_prioritizes_exact_amount_and_reset_in_fixed_width_row() {
        let strings = LanguageId::Japanese.strings();
        let item = generic_item(
            Some(QuotaMetric::Used {
                used: 1.19,
                limit: Some(1_500.0),
            }),
            Some(SystemTime::now() + Duration::from_secs(20 * 86400)),
        );

        let display = render_generic_quota_item(
            CellState::Ok,
            Some(&item),
            DisplayBasis::RemainingAllowance,
            strings,
        );

        assert!(display.text.starts_with("1498.81 · あと"));
        assert!(!display.text.contains("1500"));
        assert!(!display.text.contains("ai-credits"));
    }

    #[test]
    fn usage_bar_has_content_is_false_for_no_percent_and_empty_text() {
        assert!(!usage_bar_has_content(None, ""));
    }

    #[test]
    fn usage_bar_has_content_is_true_for_no_percent_with_status_text() {
        assert!(usage_bar_has_content(None, "N/A"));
    }

    #[test]
    fn usage_bar_has_content_is_true_for_percent_with_empty_text() {
        assert!(usage_bar_has_content(Some(50.0), ""));
    }

    #[test]
    fn usage_bar_has_content_is_true_for_percent_with_text() {
        assert!(usage_bar_has_content(Some(50.0), "50%"));
    }

    #[test]
    fn popup_sits_above_a_normal_bottom_taskbar() {
        // work area 0..1040, popup height 46
        assert_eq!(compute_popup_y(0, 1040, 46), 994);
    }

    #[test]
    fn popup_sits_above_a_small_bottom_taskbar() {
        // work area 0..1050 (taskbar shorter than the popup itself)
        assert_eq!(compute_popup_y(0, 1050, 46), 1004);
    }

    #[test]
    fn popup_y_is_correct_on_a_negative_coordinate_monitor() {
        // e.g. a secondary monitor positioned above/left of the primary
        assert_eq!(compute_popup_y(-1040, -40, 46), -86);
    }

    #[test]
    fn popup_taller_than_work_area_clamps_to_work_area_top() {
        // Physically cannot fit above the taskbar; work_area_top wins as a
        // last resort, even though the popup then extends past
        // work_area_bottom (unavoidable given the height constraint).
        let y = compute_popup_y(1030, 1040, 46);
        assert_eq!(y, 1030);
    }

    #[test]
    fn popup_bottom_never_exceeds_work_area_bottom_in_normal_cases() {
        for (top, bottom, height) in [(0, 1040, 46), (0, 1050, 46), (-1040, -40, 46)] {
            let y = compute_popup_y(top, bottom, height);
            assert!(y + height <= bottom);
        }
    }

    #[test]
    fn popup_x_within_work_area_is_unchanged() {
        assert_eq!(clamp_popup_x(500, 0, 1920, 300), 500);
    }

    #[test]
    fn popup_x_clamps_to_left_edge_of_work_area() {
        assert_eq!(clamp_popup_x(-50, 0, 1920, 300), 0);
    }

    #[test]
    fn popup_x_clamps_to_right_edge_of_work_area() {
        assert_eq!(clamp_popup_x(1800, 0, 1920, 300), 1620);
    }

    #[test]
    fn popup_wider_than_work_area_clamps_to_left_without_panicking() {
        // popup_width (2000) > work area width (1920): must not panic on
        // clamp(min, max) with min > max, and must fall back to the left edge.
        assert_eq!(clamp_popup_x(500, 0, 1920, 2000), 0);
    }

    // ── Resizable widget geometry and persistence ───────────────────────

    fn resize_session(
        edge: HorizontalResizeEdge,
        start_cursor_screen_x: i32,
        start_window_rect: RECT,
    ) -> HorizontalResizeSession {
        HorizontalResizeSession {
            edge,
            start_cursor_screen_x,
            start_window_rect,
        }
    }

    #[test]
    fn horizontal_resize_edge_detection_finds_left_right_and_center() {
        let width = 600;
        let edge = 6;
        assert_eq!(
            horizontal_resize_edge_at(0, width, edge),
            Some(HorizontalResizeEdge::Left)
        );
        assert_eq!(
            horizontal_resize_edge_at(5, width, edge),
            Some(HorizontalResizeEdge::Left)
        );
        assert_eq!(horizontal_resize_edge_at(6, width, edge), None);
        assert_eq!(horizontal_resize_edge_at(300, width, edge), None);
        assert_eq!(
            horizontal_resize_edge_at(594, width, edge),
            Some(HorizontalResizeEdge::Right)
        );
        assert_eq!(
            horizontal_resize_edge_at(599, width, edge),
            Some(HorizontalResizeEdge::Right)
        );
        assert_eq!(horizontal_resize_edge_at(-1, width, edge), None);
        assert_eq!(horizontal_resize_edge_at(width, width, edge), None);
    }

    #[test]
    fn horizontal_resize_edge_width_scales_at_96_144_and_192_dpi() {
        assert_eq!(horizontal_resize_edge_width_for_dpi(96), 6);
        assert_eq!(horizontal_resize_edge_width_for_dpi(144), 9);
        assert_eq!(horizontal_resize_edge_width_for_dpi(192), 12);
    }

    #[test]
    fn right_resize_keeps_left_top_and_content_height_fixed() {
        let start = RECT {
            left: 100,
            top: 200,
            right: 500,
            bottom: 300,
        };
        let rect = horizontal_resize_rect(
            resize_session(HorizontalResizeEdge::Right, 500, start),
            650,
            315,
            1200,
            113,
        );
        assert_eq!((rect.left, rect.right), (100, 650));
        assert_eq!((rect.top, rect.bottom), (200, 313));
    }

    #[test]
    fn left_resize_keeps_right_top_and_content_height_fixed() {
        let start = RECT {
            left: 100,
            top: 200,
            right: 500,
            bottom: 300,
        };
        let rect = horizontal_resize_rect(
            resize_session(HorizontalResizeEdge::Left, 100, start),
            -50,
            315,
            1200,
            113,
        );
        assert_eq!((rect.left, rect.right), (-50, 500));
        assert_eq!((rect.top, rect.bottom), (200, 313));
    }

    #[test]
    fn horizontal_resize_clamps_to_minimum_and_maximum_widths() {
        let start = RECT {
            left: 100,
            top: 200,
            right: 500,
            bottom: 300,
        };
        let session = resize_session(HorizontalResizeEdge::Right, 500, start);
        let minimum = horizontal_resize_rect(session, -1000, 315, 1200, 113);
        let maximum = horizontal_resize_rect(session, 5000, 315, 1200, 113);
        assert_eq!(minimum.right - minimum.left, 315);
        assert_eq!(maximum.right - maximum.left, 1200);
        assert_eq!((minimum.left, maximum.left), (100, 100));
        assert_eq!((minimum.top, minimum.bottom), (200, 313));
        assert_eq!((maximum.top, maximum.bottom), (200, 313));
    }

    #[test]
    fn resize_edge_takes_priority_over_header_drag() {
        let help_rect = help_button_rect_for_client_width(600, 96);
        assert_eq!(
            pointer_interaction_target(2, 10, 600, 6, 30, help_rect, &[]),
            PointerInteractionTarget::HorizontalResize(HorizontalResizeEdge::Left)
        );
        assert_eq!(
            pointer_interaction_target(598, 10, 600, 6, 30, help_rect, &[]),
            PointerInteractionTarget::HorizontalResize(HorizontalResizeEdge::Right)
        );
        assert_eq!(
            pointer_interaction_target(300, 10, 600, 6, 30, help_rect, &[]),
            PointerInteractionTarget::HeaderDrag
        );
        assert_eq!(
            pointer_interaction_target(300, 40, 600, 6, 30, help_rect, &[]),
            PointerInteractionTarget::None
        );
    }

    #[test]
    fn auth_cta_hit_target_is_found_inside_its_rect_and_not_outside() {
        let help_rect = help_button_rect_for_client_width(600, 96);
        let cta_rect = RECT {
            left: 100,
            top: 200,
            right: 160,
            bottom: 220,
        };
        let hits = [AuthCtaHitTarget {
            rect: cta_rect,
            action: AuthAction::CodexLogin,
        }];
        assert_eq!(
            pointer_interaction_target(120, 210, 600, 6, 30, help_rect, &hits),
            PointerInteractionTarget::AuthCta(AuthAction::CodexLogin)
        );
        assert_eq!(
            pointer_interaction_target(300, 210, 600, 6, 30, help_rect, &hits),
            PointerInteractionTarget::None
        );
    }

    #[test]
    fn help_button_scales_with_dpi_and_stays_inside_the_right_resize_edge() {
        for dpi in [96, 144, 192] {
            let client_width = scaled_for_dpi(600, dpi);
            let rect = help_button_rect_for_client_width(client_width, dpi);
            let resize_edge = horizontal_resize_edge_width_for_dpi(dpi);
            let header_bottom = scaled_for_dpi(3 + HEADER_ROW_H + 4, dpi);

            assert_eq!(
                client_width - rect.right,
                resize_edge + scaled_for_dpi(HELP_BUTTON_RIGHT_MARGIN_LOGICAL, dpi)
            );
            assert!(rect.top >= 0);
            assert!(rect.bottom <= header_bottom);
            assert_eq!(
                rect.right - rect.left,
                scaled_for_dpi(HELP_BUTTON_SIZE_LOGICAL, dpi)
            );
        }
    }

    #[test]
    fn help_button_tracks_the_right_edge_when_the_widget_width_changes() {
        for dpi in [96, 144, 192] {
            let narrow_width = scaled_for_dpi(400, dpi);
            let wide_width = scaled_for_dpi(700, dpi);
            let narrow = help_button_rect_for_client_width(narrow_width, dpi);
            let wide = help_button_rect_for_client_width(wide_width, dpi);

            assert_eq!(wide.left - narrow.left, wide_width - narrow_width);
            assert_eq!(wide.right - narrow.right, wide_width - narrow_width);
        }
    }

    #[test]
    fn final_provider_header_is_clipped_before_the_help_button() {
        for dpi in [96, 144, 192] {
            let client_width = scaled_for_dpi(600, dpi);
            let help = help_button_rect_for_client_width(client_width, dpi);
            let column_x = scaled_for_dpi(420, dpi);
            let column_width = scaled_for_dpi(200, dpi);
            let clipped =
                header_column_width_avoiding_help(column_x, column_width, client_width, dpi, true);

            assert_eq!(
                column_x + clipped + scaled_for_dpi(HELP_BUTTON_CONTENT_GAP_LOGICAL, dpi),
                help.left
            );
            assert_eq!(
                header_column_width_avoiding_help(column_x, column_width, client_width, dpi, false,),
                column_width
            );
        }
    }

    #[test]
    fn help_button_takes_priority_over_header_drag_but_not_resize_edges() {
        let help_rect = help_button_rect_for_client_width(600, 96);
        let help_x = (help_rect.left + help_rect.right) / 2;
        let help_y = (help_rect.top + help_rect.bottom) / 2;

        assert_eq!(
            pointer_interaction_target(help_x, help_y, 600, 6, 30, help_rect, &[]),
            PointerInteractionTarget::HelpButton
        );
        assert_eq!(
            pointer_interaction_target(2, help_y, 600, 30, 30, help_rect, &[]),
            PointerInteractionTarget::HorizontalResize(HorizontalResizeEdge::Left)
        );
        assert_eq!(
            pointer_interaction_target(300, help_y, 600, 6, 30, help_rect, &[]),
            PointerInteractionTarget::HeaderDrag
        );
    }

    #[test]
    fn completed_left_resize_updates_manual_x_and_logical_width() {
        let rect = RECT {
            left: -120,
            top: 240,
            right: 480,
            bottom: 353,
        };
        let (logical_width, manual_position) =
            completed_resize_settings(rect, 144, Some((100, 240)));
        assert_eq!(logical_width, 400);
        assert_eq!(manual_position, Some((-120, 240)));

        let (_, automatic_position) = completed_resize_settings(rect, 144, None);
        assert_eq!(automatic_position, None);
    }

    #[test]
    fn cancel_or_capture_loss_clears_resize_session_once() {
        let mut session = Some(resize_session(
            HorizontalResizeEdge::Left,
            100,
            RECT {
                left: 100,
                top: 200,
                right: 500,
                bottom: 300,
            },
        ));
        assert!(clear_horizontal_resize_session(&mut session));
        assert!(session.is_none());
        assert!(!clear_horizontal_resize_session(&mut session));
    }

    #[test]
    fn legacy_settings_default_to_no_saved_width_or_position() {
        let settings: SettingsFile = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.widget_width_logical, None);
        assert_eq!(settings.manual_x, None);
        assert_eq!(settings.manual_y, None);
    }

    #[test]
    fn widget_width_and_manual_position_round_trip() {
        let settings = SettingsFile {
            widget_width_logical: Some(777),
            manual_x: Some(-1200),
            manual_y: Some(240),
            ..SettingsFile::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let decoded: SettingsFile = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.widget_width_logical, Some(777));
        assert_eq!(decoded.manual_x.zip(decoded.manual_y), Some((-1200, 240)));
    }

    #[test]
    fn invalid_saved_width_is_clamped_without_affecting_other_settings() {
        assert_eq!(normalize_saved_widget_width(Some(-50)), Some(1));
        assert_eq!(normalize_saved_widget_width(Some(5000)), Some(1200));
        assert_eq!(normalize_saved_widget_width(None), None);
        assert_eq!(
            resolved_widget_width_device(Some(1), 1, 96, 1920),
            minimum_widget_width_logical_for(1)
        );
    }

    #[test]
    fn logical_and_device_widths_convert_across_dpi() {
        assert_eq!(scaled_for_dpi(400, 96), 400);
        assert_eq!(scaled_for_dpi(400, 144), 600);
        assert_eq!(logical_from_device(600, 144), 400);
    }

    #[test]
    fn provider_count_minimum_widths_match_layout_budgets() {
        assert_eq!(minimum_widget_width_logical_for(1), 315);
        assert_eq!(minimum_widget_width_logical_for(2), 353);
        assert_eq!(minimum_widget_width_logical_for(3), 477);
        assert_eq!(minimum_widget_width_logical_for(4), 623);
    }

    #[test]
    fn client_width_is_distributed_evenly_to_provider_columns() {
        assert_eq!(provider_column_width_for_client_at_dpi(315, 1, 96), 273);
        assert_eq!(provider_column_width_for_client_at_dpi(353, 2, 96), 154);
        assert_eq!(provider_column_width_for_client_at_dpi(477, 3, 96), 143);
        assert_eq!(provider_column_width_for_client_at_dpi(623, 4, 96), 143);
        assert_eq!(provider_column_width_for_client_at_dpi(1000, 4, 96), 237);
    }

    #[test]
    fn default_position_is_work_area_bottom_right() {
        let work_area = RECT {
            left: 100,
            top: 50,
            right: 2020,
            bottom: 1090,
        };
        assert_eq!(default_popup_position(work_area, 400, 90), (1620, 1000));
    }

    #[test]
    fn saved_position_is_clamped_inside_current_work_area() {
        let work_area = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        assert_eq!(
            clamp_position_to_work_area(work_area, 500, 100, -3000, 1200),
            (-1920, 940)
        );
    }

    #[test]
    fn reset_position_clears_manual_coordinates_but_not_width() {
        let mut tray_offset = 42;
        let mut manual_position = Some((300, 400));
        let width = Some(700);
        reset_saved_position(&mut tray_offset, &mut manual_position);
        assert_eq!(tray_offset, 0);
        assert_eq!(manual_position, None);
        assert_eq!(width, Some(700));
    }

    #[test]
    fn geometry_height_changes_preserve_user_width() {
        let before = resolved_widget_size_device(Some(700), 3, 96, 1920, 70);
        let after = resolved_widget_size_device(Some(700), 3, 96, 1920, 140);
        assert_eq!(before.0, 700);
        assert_eq!(after.0, 700);
        assert_eq!((before.1, after.1), (70, 140));
    }

    // ── AUM-WINDOW-UI-01C-2-STEP1: compute_auto_popup_position ─────────────
    // `position_at_taskbar`'s x/y arithmetic, extracted verbatim into a pure
    // function — these fix the extracted function's behavior with concrete,
    // independently-computed expected coordinates (not the same formula
    // copied into the assertion) so the extraction can't silently drift from
    // what `position_at_taskbar` used to compute inline.

    #[test]
    fn compute_auto_popup_position_matches_desired_x_and_y_for_ordinary_case() {
        // 1920x1080 primary monitor, bottom taskbar, popup 300x46, tray
        // sitting 20px right of the popup's target (tray_offset = 10).
        let work_area = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let (x, y) = compute_auto_popup_position(work_area, 1900, 300, 46, 10);
        assert_eq!((x, y), (1590, 994));
    }

    #[test]
    fn compute_auto_popup_position_clamps_to_left_edge_when_desired_x_is_negative() {
        // tray sits close to the work area's left edge; the popup's own
        // desired position (tray_left - width) would land off-screen.
        let work_area = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let (x, y) = compute_auto_popup_position(work_area, 250, 300, 46, 0);
        assert_eq!((x, y), (0, 994));
    }

    #[test]
    fn compute_auto_popup_position_clamps_to_right_edge_when_popup_would_overflow() {
        // A narrow work area (e.g. a small secondary monitor) with tray_left
        // far to the right of it: the popup's desired x would overflow the
        // work area's right edge.
        let work_area = RECT {
            left: 0,
            top: 0,
            right: 500,
            bottom: 1040,
        };
        let (x, y) = compute_auto_popup_position(work_area, 2000, 300, 46, 0);
        assert_eq!((x, y), (200, 994));
    }

    #[test]
    fn compute_auto_popup_position_clamps_to_left_when_popup_wider_than_work_area() {
        // Mirrors `popup_wider_than_work_area_clamps_to_left_without_panicking`
        // at the composed function's level: popup_width (300) > work area
        // width (200).
        let work_area = RECT {
            left: 100,
            top: 0,
            right: 300,
            bottom: 1040,
        };
        let (x, y) = compute_auto_popup_position(work_area, 250, 300, 46, 0);
        assert_eq!((x, y), (100, 994));
    }

    #[test]
    fn compute_auto_popup_position_y_matches_compute_popup_y() {
        let work_area = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let (_, y) = compute_auto_popup_position(work_area, 1900, 300, 46, 10);
        assert_eq!(y, compute_popup_y(work_area.top, work_area.bottom, 46));
    }

    #[test]
    fn compute_auto_popup_position_handles_non_zero_work_area_origin() {
        // A secondary monitor to the right of and slightly below the
        // primary, so both work_area.left and work_area.top are non-zero.
        let work_area = RECT {
            left: 1920,
            top: 40,
            right: 3840,
            bottom: 1080,
        };
        let (x, y) = compute_auto_popup_position(work_area, 3800, 300, 46, 0);
        assert_eq!((x, y), (3500, 1034));
    }

    #[test]
    fn usage_geometry_sync_is_requested_only_when_window_size_differs() {
        let current = RECT {
            left: 400,
            top: 700,
            right: 700,
            bottom: 785,
        };
        assert!(!window_size_needs_sync(Some(current), 300, 85));
        assert!(window_size_needs_sync(Some(current), 320, 85));
        assert!(window_size_needs_sync(Some(current), 300, 113));
        assert!(window_size_needs_sync(None, 300, 85));

        // Position is deliberately not part of this poll-time decision. A
        // manually placed popup with the right size must not be re-anchored.
        let same_size_at_manual_position = RECT {
            left: -500,
            top: 250,
            right: -200,
            bottom: 335,
        };
        assert!(!window_size_needs_sync(
            Some(same_size_at_manual_position),
            300,
            85
        ));
    }

    #[test]
    fn startup_fixed_height_must_be_synced_to_visible_rows_before_paint() {
        // At 125% scaling, CreateWindowExW's legacy height is 98 px, but
        // the current two-row layout is 78 px. A reentrant first paint must
        // see the resized window rather than the stale creation height.
        let rows = visible_rows(PopupLayout::Standard, 0, true);
        let initial_height = scaled_for_dpi(WIDGET_HEIGHT, 120);
        let required_height = scaled_for_dpi(popup_height_logical(rows), 120);
        assert_eq!((initial_height, required_height), (98, 78));
        let initial_rect = RECT {
            left: 400,
            top: 700,
            right: 700,
            bottom: 700 + initial_height,
        };
        assert!(window_size_needs_sync(
            Some(initial_rect),
            300,
            required_height
        ));
        let resized_rect = RECT {
            bottom: 700 + required_height,
            ..initial_rect
        };
        assert!(!window_size_needs_sync(
            Some(resized_rect),
            300,
            required_height
        ));
    }

    #[test]
    fn four_provider_poll_growth_has_current_height_visible_header_and_safe_bottom() {
        let loading_rows = visible_rows_for_quota(PopupLayout::Standard, true, 0, true, true, 0);
        let polled_rows = visible_rows_for_quota(PopupLayout::Standard, true, 2, true, true, 0);
        let loading_height = widget_height_for_rows(loading_rows);
        let polled_height = widget_height_for_rows(polled_rows);

        assert_eq!(active_family_count(true, true, true, true), 4);
        assert!(polled_height > loading_height);
        let without_monthly =
            visible_rows_for_quota(PopupLayout::Standard, true, 2, true, false, 0);
        assert!(polled_height > widget_height_for_rows(without_monthly));

        let width = total_widget_width_for(4);
        let stale_loading_rect = RECT {
            left: 1500,
            top: 900,
            right: 1500 + width,
            bottom: 900 + loading_height,
        };
        assert!(window_size_needs_sync(
            Some(stale_loading_rect),
            width,
            polled_height
        ));

        let work_area = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let (_, y) = compute_auto_popup_position(work_area, 1900, width, polled_height, 0);
        assert!(y + polled_height <= work_area.bottom);

        let layout = pace_row_layout(polled_height, polled_rows);
        assert!(layout.provider_header_y >= 0);

        let synchronized_rect = RECT {
            left: 1500,
            top: y,
            right: 1500 + width,
            bottom: y + polled_height,
        };
        assert!(!window_size_needs_sync(
            Some(synchronized_rect),
            width,
            polled_height
        ));
    }

    // ── AUM-WINDOW-UI-01C-2-STEP2 (drag UX): is_drag_region_point ───────────

    #[test]
    fn drag_region_point_covers_the_full_width_header_band() {
        let width = 300;
        let header_bottom = 25;
        // Left edge, center, right edge (just inside width) all draggable.
        for x in [0, 150, width - 1] {
            assert!(
                is_drag_region_point(x, 10, width, header_bottom),
                "x={x} inside the header band should be draggable"
            );
        }
    }

    #[test]
    fn drag_region_point_excludes_rows_below_the_header_band_and_outside_popup() {
        let width = 300;
        let header_bottom = 25;
        // Right at the boundary (7d row), further down (5h row), the
        // popup's own bottom edge, and points outside the popup entirely —
        // none of these are draggable.
        for y in [header_bottom, header_bottom + 5, 78] {
            assert!(
                !is_drag_region_point(150, y, width, header_bottom),
                "y={y} at/below the header band must not be draggable"
            );
        }
        assert!(!is_drag_region_point(-1, 10, width, header_bottom));
        assert!(!is_drag_region_point(width, 10, width, header_bottom));
        assert!(!is_drag_region_point(150, -1, width, header_bottom));
    }

    #[test]
    fn settings_default_has_always_on_top_disabled() {
        assert!(!SettingsFile::default().always_on_top);
    }

    #[test]
    fn settings_without_always_on_top_field_defaults_to_false() {
        let settings: SettingsFile =
            serde_json::from_str("{}").expect("legacy settings should deserialize");
        assert!(!settings.always_on_top);
    }

    #[test]
    fn settings_with_always_on_top_true_deserializes_true() {
        let settings: SettingsFile =
            serde_json::from_str(r#"{"always_on_top":true}"#).expect("settings should deserialize");
        assert!(settings.always_on_top);
    }

    #[test]
    fn settings_serialization_includes_always_on_top() {
        let settings = SettingsFile {
            always_on_top: true,
            ..SettingsFile::default()
        };
        let value = serde_json::to_value(&settings).expect("settings should serialize");
        assert_eq!(value["always_on_top"], serde_json::json!(true));
    }

    #[test]
    fn display_basis_default_is_used_percentage() {
        assert_eq!(DisplayBasis::default(), DisplayBasis::UsedPercentage);
    }

    #[test]
    fn reset_display_mode_defaults_to_relative() {
        assert_eq!(ResetDisplayMode::default(), ResetDisplayMode::Relative);
    }

    #[test]
    fn legacy_settings_without_reset_display_mode_keep_relative_display() {
        let settings: SettingsFile = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.reset_display_mode, ResetDisplayMode::Relative);
    }

    #[test]
    fn reset_display_mode_round_trips_through_settings() {
        let settings = SettingsFile {
            reset_display_mode: ResetDisplayMode::AbsoluteDateTime,
            ..SettingsFile::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"reset_display_mode\":\"absolute_date_time\""));
        let decoded: SettingsFile = serde_json::from_str(&json).unwrap();
        assert_eq!(
            decoded.reset_display_mode,
            ResetDisplayMode::AbsoluteDateTime
        );
    }

    #[test]
    fn unknown_reset_display_mode_falls_back_without_losing_other_settings() {
        let settings: SettingsFile =
            serde_json::from_str(r#"{"reset_display_mode":"future_mode","tray_offset":17}"#)
                .unwrap();
        assert_eq!(settings.reset_display_mode, ResetDisplayMode::Relative);
        assert_eq!(settings.tray_offset, 17);
    }

    #[test]
    fn reset_display_menu_ids_map_to_modes() {
        assert_eq!(
            reset_display_mode_for_menu_id(IDM_RESET_DISPLAY_RELATIVE),
            Some(ResetDisplayMode::Relative)
        );
        assert_eq!(
            reset_display_mode_for_menu_id(IDM_RESET_DISPLAY_ABSOLUTE),
            Some(ResetDisplayMode::AbsoluteDateTime)
        );
        assert_eq!(reset_display_mode_for_menu_id(u16::MAX), None);
    }

    #[test]
    fn time_display_menu_labels_use_relative_and_datetime_wording() {
        let japanese = LanguageId::Japanese.strings();
        assert_eq!(japanese.reset_display, "時間表示");
        assert_eq!(japanese.reset_display_relative, "相対");
        assert_eq!(japanese.reset_display_datetime, "日時");

        let english = LanguageId::English.strings();
        assert_eq!(english.reset_display, "Time Display");
        assert_eq!(english.reset_display_relative, "Relative");
        assert_eq!(english.reset_display_datetime, "Date and Time");
    }

    #[test]
    fn display_value_used_percentage_passes_through() {
        assert_eq!(display_value(DisplayBasis::UsedPercentage, 64.0), 64.0);
    }

    #[test]
    fn display_value_remaining_allowance_is_the_complement() {
        assert_eq!(display_value(DisplayBasis::RemainingAllowance, 64.0), 36.0);
    }

    #[test]
    fn display_value_clamps_outside_zero_to_hundred() {
        assert_eq!(display_value(DisplayBasis::UsedPercentage, -5.0), 0.0);
        assert_eq!(display_value(DisplayBasis::UsedPercentage, 150.0), 100.0);
        assert_eq!(display_value(DisplayBasis::RemainingAllowance, -5.0), 100.0);
        assert_eq!(display_value(DisplayBasis::RemainingAllowance, 150.0), 0.0);
    }

    #[test]
    fn banked_reset_text_distinguishes_zero_one_and_unavailable() {
        let strings = LanguageId::English.strings();

        assert_eq!(
            format_banked_reset_text(BankedResetCount::Available(0), strings),
            "Full reset: 0"
        );
        assert_eq!(
            format_banked_reset_text(BankedResetCount::Available(1), strings),
            "Full reset: 1"
        );
        assert_eq!(
            format_banked_reset_text(BankedResetCount::Unavailable, strings),
            "Full reset: Not available"
        );
    }

    #[test]
    fn every_language_has_a_full_reset_label() {
        for language in LanguageId::ALL {
            assert!(!language.strings().full_reset.trim().is_empty());
        }
    }

    #[test]
    fn render_cell_normal_zero_percent_is_some_zero_not_none() {
        let strings = LanguageId::English.strings();
        let section = UsageSection {
            percentage: 0.0,
            resets_at: None,
        };
        let display = render_cell(
            CellState::Ok,
            Some(&section),
            DisplayBasis::UsedPercentage,
            strings,
        );
        // A real 0% must be distinguishable from "no value at all": Some(0.0),
        // never None, so the bar can legitimately render as empty for a
        // genuine zero without being confused with a failed/loading cell.
        assert_eq!(display.bar_percent, Some(0.0));
    }

    #[test]
    fn render_cell_non_ok_states_never_produce_a_bar_percent() {
        let strings = LanguageId::English.strings();
        for state in [
            CellState::Loading,
            CellState::AuthenticationExpired,
            CellState::AuthenticationProblem,
            CellState::CredentialsUnavailable,
            CellState::FetchFailed,
            CellState::NotAvailable,
        ] {
            let display = render_cell(state, None, DisplayBasis::UsedPercentage, strings);
            assert_eq!(display.bar_percent, None);
        }
    }

    #[test]
    fn legacy_settings_without_display_basis_default_to_used_percentage() {
        let settings: SettingsFile =
            serde_json::from_str("{}").expect("legacy settings should deserialize");
        assert_eq!(settings.display_basis, DisplayBasis::UsedPercentage);
    }

    #[test]
    fn legacy_settings_without_copilot_fields_default_to_disabled_unknown() {
        let settings: SettingsFile =
            serde_json::from_str("{}").expect("legacy settings should deserialize");
        assert!(!settings.show_github_copilot);
        assert_eq!(
            settings.github_copilot_plan,
            poller::GithubCopilotPlan::Unknown
        );
    }

    #[test]
    fn copilot_settings_round_trip_with_explicit_plan() {
        let settings = SettingsFile {
            show_github_copilot: true,
            github_copilot_plan: poller::GithubCopilotPlan::ProPlus,
            ..SettingsFile::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let decoded: SettingsFile = serde_json::from_str(&json).unwrap();
        assert!(decoded.show_github_copilot);
        assert_eq!(
            decoded.github_copilot_plan,
            poller::GithubCopilotPlan::ProPlus
        );
        assert!(json.contains("\"github_copilot_plan\":\"pro_plus\""));
    }

    #[test]
    fn saved_paid_copilot_plan_migrates_the_family_to_enabled() {
        let mut settings = SettingsFile {
            show_github_copilot: false,
            github_copilot_plan: poller::GithubCopilotPlan::Pro,
            ..SettingsFile::default()
        };
        normalize_github_copilot_settings(&mut settings);
        assert!(settings.show_github_copilot);
        assert_eq!(settings.github_copilot_plan, poller::GithubCopilotPlan::Pro);
    }

    #[test]
    fn selecting_any_copilot_plan_enables_the_family() {
        for (id, expected) in [
            (
                IDM_GITHUB_COPILOT_PLAN_UNKNOWN,
                poller::GithubCopilotPlan::Unknown,
            ),
            (IDM_GITHUB_COPILOT_PLAN_PRO, poller::GithubCopilotPlan::Pro),
            (
                IDM_GITHUB_COPILOT_PLAN_PRO_PLUS,
                poller::GithubCopilotPlan::ProPlus,
            ),
            (IDM_GITHUB_COPILOT_PLAN_MAX, poller::GithubCopilotPlan::Max),
        ] {
            let plan = github_copilot_plan_for_menu_id(id).unwrap();
            assert_eq!(plan, expected);
            let mut shown = false;
            let mut selected = poller::GithubCopilotPlan::Unknown;
            apply_github_copilot_plan_selection(&mut shown, &mut selected, plan);
            assert!(shown);
            assert_eq!(selected, expected);
        }
        assert_eq!(github_copilot_plan_for_menu_id(u16::MAX), None);
    }

    #[test]
    fn codex_display_name_is_not_chatgpt_in_any_language() {
        for language in LanguageId::ALL {
            assert_eq!(language.strings().codex_model, "Codex");
        }
    }

    #[test]
    fn settings_with_unrecognized_display_basis_falls_back_without_failing_the_whole_file() {
        let settings: SettingsFile =
            serde_json::from_str(r#"{"display_basis":"some_future_value","tray_offset":7}"#)
                .expect("an unrecognized display_basis must not fail the whole settings file");
        assert_eq!(settings.display_basis, DisplayBasis::UsedPercentage);
        assert_eq!(settings.tray_offset, 7);
    }

    #[test]
    fn settings_serialization_uses_snake_case_display_basis() {
        let settings = SettingsFile {
            display_basis: DisplayBasis::RemainingAllowance,
            ..SettingsFile::default()
        };
        let value = serde_json::to_value(&settings).expect("settings should serialize");
        assert_eq!(
            value["display_basis"],
            serde_json::json!("remaining_allowance")
        );
    }

    // ── AUM-PACE-GUIDANCE-01: display settings (type/default/persistence) ──

    #[test]
    fn display_density_default_is_standard() {
        assert_eq!(DisplayDensity::default(), DisplayDensity::Standard);
    }

    #[test]
    fn short_window_visibility_default_is_warning_only() {
        assert_eq!(
            ShortWindowVisibility::default(),
            ShortWindowVisibility::WarningOnly
        );
    }

    #[test]
    fn short_window_alert_sensitivity_default_is_standard() {
        assert_eq!(
            ShortWindowAlertSensitivity::default(),
            ShortWindowAlertSensitivity::Standard
        );
    }

    #[test]
    fn display_density_serializes_to_expected_snake_case() {
        assert_eq!(
            serde_json::to_value(DisplayDensity::Compact).unwrap(),
            serde_json::json!("compact")
        );
        assert_eq!(
            serde_json::to_value(DisplayDensity::Standard).unwrap(),
            serde_json::json!("standard")
        );
        assert_eq!(
            serde_json::to_value(DisplayDensity::Detailed).unwrap(),
            serde_json::json!("detailed")
        );
    }

    #[test]
    fn short_window_visibility_serializes_to_expected_snake_case() {
        assert_eq!(
            serde_json::to_value(ShortWindowVisibility::Always).unwrap(),
            serde_json::json!("always")
        );
        assert_eq!(
            serde_json::to_value(ShortWindowVisibility::WarningOnly).unwrap(),
            serde_json::json!("warning_only")
        );
        assert_eq!(
            serde_json::to_value(ShortWindowVisibility::Hidden).unwrap(),
            serde_json::json!("hidden")
        );
    }

    #[test]
    fn short_window_alert_sensitivity_serializes_to_expected_snake_case() {
        assert_eq!(
            serde_json::to_value(ShortWindowAlertSensitivity::Sensitive).unwrap(),
            serde_json::json!("sensitive")
        );
        assert_eq!(
            serde_json::to_value(ShortWindowAlertSensitivity::Standard).unwrap(),
            serde_json::json!("standard")
        );
        assert_eq!(
            serde_json::to_value(ShortWindowAlertSensitivity::Relaxed).unwrap(),
            serde_json::json!("relaxed")
        );
    }

    #[test]
    fn pace_settings_deserialize_from_expected_snake_case_strings() {
        let density: DisplayDensity =
            serde_json::from_value(serde_json::json!("detailed")).unwrap();
        assert_eq!(density, DisplayDensity::Detailed);

        let visibility: ShortWindowVisibility =
            serde_json::from_value(serde_json::json!("hidden")).unwrap();
        assert_eq!(visibility, ShortWindowVisibility::Hidden);

        let sensitivity: ShortWindowAlertSensitivity =
            serde_json::from_value(serde_json::json!("relaxed")).unwrap();
        assert_eq!(sensitivity, ShortWindowAlertSensitivity::Relaxed);
    }

    #[test]
    fn legacy_settings_without_pace_display_keys_deserialize_successfully() {
        let settings: SettingsFile = serde_json::from_str("{}")
            .expect("legacy settings without the new keys should still deserialize");
        assert_eq!(settings.display_density, DisplayDensity::Standard);
        assert_eq!(
            settings.short_window_visibility,
            ShortWindowVisibility::WarningOnly
        );
        assert_eq!(
            settings.short_window_alert_sensitivity,
            ShortWindowAlertSensitivity::Standard
        );
    }

    #[test]
    fn new_format_settings_load_the_saved_pace_display_values() {
        let json = r#"{
            "display_density": "detailed",
            "short_window_visibility": "always",
            "short_window_alert_sensitivity": "relaxed"
        }"#;
        let settings: SettingsFile =
            serde_json::from_str(json).expect("new-format settings should deserialize");
        assert_eq!(settings.display_density, DisplayDensity::Detailed);
        assert_eq!(
            settings.short_window_visibility,
            ShortWindowVisibility::Always
        );
        assert_eq!(
            settings.short_window_alert_sensitivity,
            ShortWindowAlertSensitivity::Relaxed
        );
    }

    #[test]
    fn settings_serialization_includes_all_pace_display_keys() {
        let value =
            serde_json::to_value(SettingsFile::default()).expect("settings should serialize");
        assert_eq!(value["display_density"], serde_json::json!("standard"));
        assert_eq!(
            value["short_window_visibility"],
            serde_json::json!("warning_only")
        );
        assert_eq!(
            value["short_window_alert_sensitivity"],
            serde_json::json!("standard")
        );
    }

    #[test]
    fn pace_display_settings_round_trip_through_serialization() {
        let settings = SettingsFile {
            display_density: DisplayDensity::Compact,
            short_window_visibility: ShortWindowVisibility::Hidden,
            short_window_alert_sensitivity: ShortWindowAlertSensitivity::Sensitive,
            ..SettingsFile::default()
        };
        let json = serde_json::to_string(&settings).expect("settings should serialize");
        let round_tripped: SettingsFile =
            serde_json::from_str(&json).expect("round trip should deserialize");
        assert_eq!(round_tripped.display_density, DisplayDensity::Compact);
        assert_eq!(
            round_tripped.short_window_visibility,
            ShortWindowVisibility::Hidden
        );
        assert_eq!(
            round_tripped.short_window_alert_sensitivity,
            ShortWindowAlertSensitivity::Sensitive
        );
    }

    #[test]
    fn existing_display_basis_survives_round_trip_alongside_new_pace_settings() {
        let settings = SettingsFile {
            display_basis: DisplayBasis::RemainingAllowance,
            display_density: DisplayDensity::Detailed,
            ..SettingsFile::default()
        };
        let json = serde_json::to_string(&settings).expect("settings should serialize");
        let round_tripped: SettingsFile =
            serde_json::from_str(&json).expect("round trip should deserialize");
        assert_eq!(
            round_tripped.display_basis,
            DisplayBasis::RemainingAllowance
        );
        assert_eq!(round_tripped.display_density, DisplayDensity::Detailed);
    }

    #[test]
    fn other_existing_settings_are_not_lost_when_pace_settings_are_present() {
        let settings = SettingsFile {
            tray_offset: 42,
            taskbar_index: 3,
            show_codex: true,
            show_claude_code: false,
            display_density: DisplayDensity::Compact,
            ..SettingsFile::default()
        };
        let json = serde_json::to_string(&settings).expect("settings should serialize");
        let round_tripped: SettingsFile =
            serde_json::from_str(&json).expect("round trip should deserialize");
        assert_eq!(round_tripped.tray_offset, 42);
        assert_eq!(round_tripped.taskbar_index, 3);
        assert!(round_tripped.show_codex);
        assert!(!round_tripped.show_claude_code);
    }

    #[test]
    fn settings_with_unrecognized_display_density_falls_back_without_failing_the_whole_file() {
        let settings: SettingsFile =
            serde_json::from_str(r#"{"display_density":"ultra_compact","tray_offset":9}"#)
                .expect("an unrecognized display_density must not fail the whole settings file");
        assert_eq!(settings.display_density, DisplayDensity::Standard);
        assert_eq!(settings.tray_offset, 9);
    }

    #[test]
    fn settings_with_unrecognized_short_window_visibility_falls_back_without_failing_the_whole_file(
    ) {
        let settings: SettingsFile =
            serde_json::from_str(r#"{"short_window_visibility":"sometimes","tray_offset":9}"#)
                .expect(
                    "an unrecognized short_window_visibility must not fail the whole settings file",
                );
        assert_eq!(
            settings.short_window_visibility,
            ShortWindowVisibility::WarningOnly
        );
        assert_eq!(settings.tray_offset, 9);
    }

    #[test]
    fn settings_with_unrecognized_short_window_alert_sensitivity_falls_back_without_failing_the_whole_file(
    ) {
        let settings: SettingsFile = serde_json::from_str(
            r#"{"short_window_alert_sensitivity":"extreme","tray_offset":9}"#,
        )
        .expect(
            "an unrecognized short_window_alert_sensitivity must not fail the whole settings file",
        );
        assert_eq!(
            settings.short_window_alert_sensitivity,
            ShortWindowAlertSensitivity::Standard
        );
        assert_eq!(settings.tray_offset, 9);
    }

    #[test]
    fn one_unrecognized_pace_display_value_does_not_affect_the_other_two() {
        let settings: SettingsFile = serde_json::from_str(
            r#"{"display_density":"bogus","short_window_visibility":"hidden","short_window_alert_sensitivity":"relaxed"}"#,
        )
        .expect("an unrecognized display_density must not fail the whole settings file");
        assert_eq!(settings.display_density, DisplayDensity::Standard);
        assert_eq!(
            settings.short_window_visibility,
            ShortWindowVisibility::Hidden
        );
        assert_eq!(
            settings.short_window_alert_sensitivity,
            ShortWindowAlertSensitivity::Relaxed
        );
    }

    // ── AUM-WINDOW-UI-01A: PopupLayout (type/default/persistence/menu) ─────

    #[test]
    fn popup_layout_default_is_compact() {
        assert_eq!(PopupLayout::default(), PopupLayout::Compact);
    }

    #[test]
    fn popup_layout_serializes_to_expected_snake_case() {
        assert_eq!(
            serde_json::to_value(PopupLayout::Compact).unwrap(),
            serde_json::json!("compact")
        );
        assert_eq!(
            serde_json::to_value(PopupLayout::Standard).unwrap(),
            serde_json::json!("standard")
        );
    }

    #[test]
    fn legacy_settings_without_popup_layout_key_deserialize_to_compact() {
        let settings: SettingsFile = serde_json::from_str("{}")
            .expect("legacy settings without popup_layout should still deserialize");
        assert_eq!(settings.popup_layout, PopupLayout::Compact);
    }

    #[test]
    fn settings_with_unrecognized_popup_layout_falls_back_to_compact_without_failing_the_whole_file(
    ) {
        let settings: SettingsFile =
            serde_json::from_str(r#"{"popup_layout":"ultra_compact","tray_offset":9}"#)
                .expect("an unrecognized popup_layout must not fail the whole settings file");
        assert_eq!(settings.popup_layout, PopupLayout::Compact);
        assert_eq!(settings.tray_offset, 9);
    }

    #[test]
    fn popup_layout_round_trips_through_serialization_alongside_other_settings() {
        let settings = SettingsFile {
            popup_layout: PopupLayout::Standard,
            tray_offset: 42,
            display_density: DisplayDensity::Detailed,
            ..SettingsFile::default()
        };
        let json = serde_json::to_string(&settings).expect("settings should serialize");
        let round_tripped: SettingsFile =
            serde_json::from_str(&json).expect("round trip should deserialize");
        assert_eq!(round_tripped.popup_layout, PopupLayout::Standard);
        assert_eq!(round_tripped.tray_offset, 42);
        assert_eq!(round_tripped.display_density, DisplayDensity::Detailed);
    }

    #[test]
    fn popup_layout_for_menu_id_maps_each_known_id_and_is_bijective() {
        assert_eq!(
            popup_layout_for_menu_id(IDM_POPUP_LAYOUT_COMPACT),
            Some(PopupLayout::Compact)
        );
        assert_eq!(
            popup_layout_for_menu_id(IDM_POPUP_LAYOUT_STANDARD),
            Some(PopupLayout::Standard)
        );
        assert_eq!(popup_layout_for_menu_id(9999), None);

        // Every menu ID maps to a distinct layout, and every layout is
        // reachable from some menu ID — the same "id <-> value" coverage
        // `display_density_for_menu_id_maps_each_known_id` checks.
        let ids = [IDM_POPUP_LAYOUT_COMPACT, IDM_POPUP_LAYOUT_STANDARD];
        let mapped: Vec<PopupLayout> = ids
            .iter()
            .map(|&id| popup_layout_for_menu_id(id).unwrap())
            .collect();
        assert_ne!(mapped[0], mapped[1]);
    }

    #[test]
    fn all_languages_have_non_empty_popup_layout_menu_strings() {
        for language in LanguageId::ALL {
            let strings = language.strings();
            assert!(!strings.popup_layout.is_empty());
            assert!(!strings.popup_layout_compact.is_empty());
            assert!(!strings.popup_layout_standard.is_empty());
        }
    }

    // ── AUM-WINDOW-UI-01B: AppTheme / PopupPalette / provider colors /
    // warning (type/default/persistence/menu/palette) ──────────────────────

    #[test]
    fn app_theme_default_is_recommended_dark() {
        assert_eq!(AppTheme::default(), AppTheme::RecommendedDark);
    }

    #[test]
    fn app_theme_serializes_to_expected_snake_case() {
        assert_eq!(
            serde_json::to_value(AppTheme::RecommendedDark).unwrap(),
            serde_json::json!("recommended_dark")
        );
        assert_eq!(
            serde_json::to_value(AppTheme::Light).unwrap(),
            serde_json::json!("light")
        );
        assert_eq!(
            serde_json::to_value(AppTheme::HighVisibility).unwrap(),
            serde_json::json!("high_visibility")
        );
    }

    #[test]
    fn legacy_settings_without_app_theme_key_deserialize_to_recommended_dark() {
        let settings: SettingsFile = serde_json::from_str("{}")
            .expect("legacy settings without app_theme should still deserialize");
        assert_eq!(settings.app_theme, AppTheme::RecommendedDark);
    }

    #[test]
    fn settings_with_unrecognized_app_theme_falls_back_to_recommended_dark_without_failing_the_whole_file(
    ) {
        let settings: SettingsFile =
            serde_json::from_str(r#"{"app_theme":"ultra_neon","tray_offset":9}"#)
                .expect("an unrecognized app_theme must not fail the whole settings file");
        assert_eq!(settings.app_theme, AppTheme::RecommendedDark);
        assert_eq!(settings.tray_offset, 9);
    }

    #[test]
    fn app_theme_round_trips_through_serialization_alongside_other_settings() {
        let settings = SettingsFile {
            app_theme: AppTheme::HighVisibility,
            tray_offset: 42,
            popup_layout: PopupLayout::Standard,
            ..SettingsFile::default()
        };
        let json = serde_json::to_string(&settings).expect("settings should serialize");
        let round_tripped: SettingsFile =
            serde_json::from_str(&json).expect("round trip should deserialize");
        assert_eq!(round_tripped.app_theme, AppTheme::HighVisibility);
        assert_eq!(round_tripped.tray_offset, 42);
        assert_eq!(round_tripped.popup_layout, PopupLayout::Standard);
    }

    #[test]
    fn app_theme_for_menu_id_maps_each_known_id_and_is_bijective() {
        assert_eq!(
            app_theme_for_menu_id(IDM_APP_THEME_RECOMMENDED_DARK),
            Some(AppTheme::RecommendedDark)
        );
        assert_eq!(
            app_theme_for_menu_id(IDM_APP_THEME_LIGHT),
            Some(AppTheme::Light)
        );
        assert_eq!(
            app_theme_for_menu_id(IDM_APP_THEME_HIGH_VISIBILITY),
            Some(AppTheme::HighVisibility)
        );
        assert_eq!(app_theme_for_menu_id(9999), None);

        let ids = [
            IDM_APP_THEME_RECOMMENDED_DARK,
            IDM_APP_THEME_LIGHT,
            IDM_APP_THEME_HIGH_VISIBILITY,
        ];
        let mapped: Vec<AppTheme> = ids
            .iter()
            .map(|&id| app_theme_for_menu_id(id).unwrap())
            .collect();
        assert_ne!(mapped[0], mapped[1]);
        assert_ne!(mapped[0], mapped[2]);
        assert_ne!(mapped[1], mapped[2]);
    }

    #[test]
    fn all_languages_have_non_empty_app_theme_menu_strings() {
        for language in LanguageId::ALL {
            let strings = language.strings();
            assert!(!strings.app_theme.is_empty());
            assert!(!strings.app_theme_recommended_dark.is_empty());
            assert!(!strings.app_theme_light.is_empty());
            assert!(!strings.app_theme_high_visibility.is_empty());
        }
    }

    #[test]
    fn virtual_desktop_scope_default_is_all() {
        assert_eq!(VirtualDesktopScope::default(), VirtualDesktopScope::All);
    }

    #[test]
    fn virtual_desktop_scope_serializes_to_expected_snake_case() {
        assert_eq!(
            serde_json::to_value(VirtualDesktopScope::All).unwrap(),
            serde_json::json!("all")
        );
        assert_eq!(
            serde_json::to_value(VirtualDesktopScope::CurrentOnly).unwrap(),
            serde_json::json!("current_only")
        );
    }

    #[test]
    fn legacy_settings_without_virtual_desktop_scope_key_deserialize_to_all() {
        let settings: SettingsFile = serde_json::from_str("{}")
            .expect("legacy settings without virtual_desktop_scope should still deserialize");
        assert_eq!(settings.virtual_desktop_scope, VirtualDesktopScope::All);
        assert_eq!(settings.virtual_desktop_target, None);
    }

    #[test]
    fn settings_with_unrecognized_virtual_desktop_scope_falls_back_to_all_without_failing_the_whole_file(
    ) {
        let settings: SettingsFile = serde_json::from_str(
            r#"{"virtual_desktop_scope":"every_other_monitor","tray_offset":9}"#,
        )
        .expect("an unrecognized virtual_desktop_scope must not fail the whole settings file");
        assert_eq!(settings.virtual_desktop_scope, VirtualDesktopScope::All);
        assert_eq!(settings.tray_offset, 9);
    }

    #[test]
    fn virtual_desktop_scope_round_trips_through_serialization_alongside_other_settings() {
        let settings = SettingsFile {
            virtual_desktop_scope: VirtualDesktopScope::CurrentOnly,
            virtual_desktop_target: Some(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
            tray_offset: 42,
            popup_layout: PopupLayout::Standard,
            ..SettingsFile::default()
        };
        let json = serde_json::to_string(&settings).expect("settings should serialize");
        let round_tripped: SettingsFile =
            serde_json::from_str(&json).expect("round trip should deserialize");
        assert_eq!(
            round_tripped.virtual_desktop_scope,
            VirtualDesktopScope::CurrentOnly
        );
        assert_eq!(
            round_tripped.virtual_desktop_target,
            Some(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef)
        );
        assert_eq!(round_tripped.tray_offset, 42);
        assert_eq!(round_tripped.popup_layout, PopupLayout::Standard);
    }

    #[test]
    fn virtual_desktop_target_is_omitted_from_json_when_scope_is_all() {
        let json = serde_json::to_string(&SettingsFile::default()).unwrap();
        assert!(
            !json.contains("virtual_desktop_target"),
            "default settings (scope=all, no target) should not serialize a target field: {json}"
        );
    }

    #[test]
    fn virtual_desktop_scope_for_menu_id_maps_each_known_id_and_is_bijective() {
        assert_eq!(
            virtual_desktop_scope_for_menu_id(IDM_VIRTUAL_DESKTOP_SCOPE_ALL),
            Some(VirtualDesktopScope::All)
        );
        assert_eq!(
            virtual_desktop_scope_for_menu_id(IDM_VIRTUAL_DESKTOP_SCOPE_CURRENT_ONLY),
            Some(VirtualDesktopScope::CurrentOnly)
        );
        assert_eq!(virtual_desktop_scope_for_menu_id(9999), None);

        assert_ne!(
            virtual_desktop_scope_for_menu_id(IDM_VIRTUAL_DESKTOP_SCOPE_ALL),
            virtual_desktop_scope_for_menu_id(IDM_VIRTUAL_DESKTOP_SCOPE_CURRENT_ONLY)
        );
    }

    #[test]
    fn all_languages_have_non_empty_virtual_desktop_scope_menu_strings() {
        for language in LanguageId::ALL {
            let strings = language.strings();
            assert!(!strings.virtual_desktop_scope.is_empty());
            assert!(!strings.virtual_desktop_scope_all.is_empty());
            assert!(!strings.virtual_desktop_scope_current_only.is_empty());
        }
    }

    fn guid(value: u128) -> GUID {
        GUID::from_u128(value)
    }

    #[test]
    fn effective_widget_visible_all_scope_follows_widget_visible_only() {
        assert!(effective_widget_visible(
            true,
            VirtualDesktopScope::All,
            None,
            None
        ));
        assert!(!effective_widget_visible(
            false,
            VirtualDesktopScope::All,
            None,
            None
        ));
        // Scope `All` ignores current/target desktop entirely, even if set.
        assert!(effective_widget_visible(
            true,
            VirtualDesktopScope::All,
            Some(guid(1)),
            Some(guid(2))
        ));
    }

    #[test]
    fn effective_widget_visible_widget_visible_false_always_hides_regardless_of_scope() {
        assert!(!effective_widget_visible(
            false,
            VirtualDesktopScope::CurrentOnly,
            Some(guid(1)),
            Some(guid(1))
        ));
    }

    #[test]
    fn effective_widget_visible_current_only_shows_on_matching_desktop() {
        assert!(effective_widget_visible(
            true,
            VirtualDesktopScope::CurrentOnly,
            Some(guid(1)),
            Some(guid(1))
        ));
    }

    #[test]
    fn effective_widget_visible_current_only_hides_on_non_matching_desktop() {
        assert!(!effective_widget_visible(
            true,
            VirtualDesktopScope::CurrentOnly,
            Some(guid(1)),
            Some(guid(2))
        ));
    }

    #[test]
    fn effective_widget_visible_current_only_fails_open_when_current_is_unknown() {
        // A transient sensor failure (current desktop not yet known) must
        // never hide the widget on its own.
        assert!(effective_widget_visible(
            true,
            VirtualDesktopScope::CurrentOnly,
            None,
            Some(guid(1))
        ));
    }

    #[test]
    fn effective_widget_visible_current_only_fails_open_when_target_is_unset() {
        assert!(effective_widget_visible(
            true,
            VirtualDesktopScope::CurrentOnly,
            Some(guid(1)),
            None
        ));
    }

    fn assert_color_hex(color: Color, hex: &str, label: &str) {
        let expected = Color::from_hex(hex);
        assert_eq!(
            (color.r, color.g, color.b),
            (expected.r, expected.g, expected.b),
            "{label} expected {hex}"
        );
    }

    #[test]
    fn popup_palette_recommended_dark_matches_spec_hex_values() {
        let palette = popup_palette(AppTheme::RecommendedDark);
        assert_color_hex(palette.background, "#11171D", "RecommendedDark background");
        assert_color_hex(palette.track, "#18222B", "RecommendedDark track");
        assert_color_hex(
            palette.primary_text,
            "#F4F8FB",
            "RecommendedDark primary_text",
        );
        assert_color_hex(
            palette.secondary_text,
            "#AAB8C3",
            "RecommendedDark secondary_text",
        );
        assert_color_hex(palette.border, "#31424F", "RecommendedDark border");
        // heading_text equals primary_text here (unchanged visible behavior)
        // — only HighVisibility gets its own heading color.
        assert_color_hex(
            palette.heading_text,
            "#F4F8FB",
            "RecommendedDark heading_text",
        );
    }

    #[test]
    fn popup_palette_light_matches_spec_hex_values() {
        let palette = popup_palette(AppTheme::Light);
        assert_color_hex(palette.background, "#F4F7FA", "Light background");
        assert_color_hex(palette.primary_text, "#17212B", "Light primary_text");
        assert_color_hex(palette.secondary_text, "#5D6C78", "Light secondary_text");
        assert_color_hex(palette.border, "#BDCCD7", "Light border");
        assert_color_hex(palette.heading_text, "#17212B", "Light heading_text");
        // The spec calls for white-or-a-visible-near-white track that
        // doesn't vanish into the light background — not pure white.
        assert_ne!(
            (palette.track.r, palette.track.g, palette.track.b),
            (0xFF, 0xFF, 0xFF),
            "Light track must not be pure white (would vanish into the background)"
        );
        assert_ne!(
            (palette.track.r, palette.track.g, palette.track.b),
            (
                palette.background.r,
                palette.background.g,
                palette.background.b
            ),
            "Light track must be visually distinct from the background"
        );
    }

    #[test]
    fn popup_palette_high_visibility_matches_spec_hex_values() {
        let palette = popup_palette(AppTheme::HighVisibility);
        assert_color_hex(palette.background, "#000000", "HighVisibility background");
        assert_color_hex(palette.track, "#484848", "HighVisibility track");
        assert_color_hex(
            palette.primary_text,
            "#FFFFFF",
            "HighVisibility primary_text",
        );
        assert_color_hex(
            palette.secondary_text,
            "#00E5FF",
            "HighVisibility secondary_text",
        );
        assert_color_hex(palette.border, "#FFFFFF", "HighVisibility border");
        assert_color_hex(palette.warning, "#FF4D4D", "HighVisibility warning");
        assert_color_hex(
            palette.heading_text,
            "#FFFF00",
            "HighVisibility heading_text",
        );
    }

    #[test]
    fn popup_palette_high_visibility_is_visually_distinct_from_recommended_dark() {
        // AUM-WINDOW-UI-01B: HighVisibility was previously too close to
        // RecommendedDark on screen — these four channels must differ so the
        // two themes are actually distinguishable.
        let high_visibility = popup_palette(AppTheme::HighVisibility);
        let recommended_dark = popup_palette(AppTheme::RecommendedDark);

        assert_ne!(
            (
                high_visibility.background.r,
                high_visibility.background.g,
                high_visibility.background.b
            ),
            (
                recommended_dark.background.r,
                recommended_dark.background.g,
                recommended_dark.background.b
            ),
            "HighVisibility background must differ from RecommendedDark"
        );
        assert_ne!(
            (
                high_visibility.track.r,
                high_visibility.track.g,
                high_visibility.track.b
            ),
            (
                recommended_dark.track.r,
                recommended_dark.track.g,
                recommended_dark.track.b
            ),
            "HighVisibility track must differ from RecommendedDark"
        );
        assert_ne!(
            (
                high_visibility.secondary_text.r,
                high_visibility.secondary_text.g,
                high_visibility.secondary_text.b
            ),
            (
                recommended_dark.secondary_text.r,
                recommended_dark.secondary_text.g,
                recommended_dark.secondary_text.b
            ),
            "HighVisibility secondary_text must differ from RecommendedDark"
        );
        assert_ne!(
            (
                high_visibility.border.r,
                high_visibility.border.g,
                high_visibility.border.b
            ),
            (
                recommended_dark.border.r,
                recommended_dark.border.g,
                recommended_dark.border.b
            ),
            "HighVisibility border must differ from RecommendedDark"
        );
        assert_ne!(
            (
                high_visibility.heading_text.r,
                high_visibility.heading_text.g,
                high_visibility.heading_text.b
            ),
            (
                recommended_dark.heading_text.r,
                recommended_dark.heading_text.g,
                recommended_dark.heading_text.b
            ),
            "HighVisibility heading_text must differ from RecommendedDark"
        );
        assert_ne!(
            (
                high_visibility.warning.r,
                high_visibility.warning.g,
                high_visibility.warning.b
            ),
            (
                recommended_dark.warning.r,
                recommended_dark.warning.g,
                recommended_dark.warning.b
            ),
            "HighVisibility warning must differ from RecommendedDark"
        );
    }

    #[test]
    fn theme_shows_column_dividers_is_true_only_for_high_visibility() {
        assert!(!theme_shows_column_dividers(AppTheme::RecommendedDark));
        assert!(!theme_shows_column_dividers(AppTheme::Light));
        assert!(theme_shows_column_dividers(AppTheme::HighVisibility));
    }

    #[test]
    fn theme_outlines_usage_track_is_true_only_for_high_visibility() {
        assert!(!theme_outlines_usage_track(AppTheme::RecommendedDark));
        assert!(!theme_outlines_usage_track(AppTheme::Light));
        assert!(theme_outlines_usage_track(AppTheme::HighVisibility));
    }

    #[test]
    fn both_draw_paths_source_colors_from_the_same_popup_palette_call() {
        // `render_layered` and `paint` each call `popup_palette(app_theme)`
        // once and thread the same `PopupPalette` value into `paint_content`
        // — see both functions' bodies. This pins the *value* half of that
        // guarantee: calling `popup_palette` twice with the same theme (as
        // the two draw paths each independently do) always yields identical
        // colors, so which path renders never matters.
        for theme in [
            AppTheme::RecommendedDark,
            AppTheme::Light,
            AppTheme::HighVisibility,
        ] {
            let a = popup_palette(theme);
            let b = popup_palette(theme);
            assert_eq!(
                (a.background.r, a.background.g, a.background.b),
                (b.background.r, b.background.g, b.background.b)
            );
            assert_eq!(
                (a.track.r, a.track.g, a.track.b),
                (b.track.r, b.track.g, b.track.b)
            );
            assert_eq!(
                (a.primary_text.r, a.primary_text.g, a.primary_text.b),
                (b.primary_text.r, b.primary_text.g, b.primary_text.b)
            );
            assert_eq!(
                (a.secondary_text.r, a.secondary_text.g, a.secondary_text.b),
                (b.secondary_text.r, b.secondary_text.g, b.secondary_text.b)
            );
            assert_eq!(
                (a.border.r, a.border.g, a.border.b),
                (b.border.r, b.border.g, b.border.b)
            );
            assert_eq!(
                (a.warning.r, a.warning.g, a.warning.b),
                (b.warning.r, b.warning.g, b.warning.b)
            );
            assert_eq!(
                (a.heading_text.r, a.heading_text.g, a.heading_text.b),
                (b.heading_text.r, b.heading_text.g, b.heading_text.b)
            );
        }
    }

    #[test]
    fn claude_accent_color_matches_spec_hex() {
        assert_color_hex(claude_accent_color(), "#D97757", "Claude accent");
    }

    #[test]
    fn codex_accent_color_matches_spec_hex() {
        assert_color_hex(codex_accent_color(), "#7477E8", "Codex accent");
    }

    #[test]
    fn antigravity_accent_color_matches_spec_hex() {
        assert_color_hex(antigravity_accent_color(), "#4285F4", "Antigravity accent");
    }

    #[test]
    fn overpacing_session_cell_still_carries_a_real_percent_so_the_bar_keeps_rendering_in_provider_color(
    ) {
        // `draw_row` passes each provider's fixed accent color to
        // `draw_usage_bar` unconditionally — `claude_is_warning`/
        // `codex_is_warning`/`antigravity_is_warning` only pick the *value
        // text* color (`PopupPalette::warning` vs. the normal text color),
        // never the bar's `accent` argument. So as long as `percent` stays
        // `Some`, the bar segments keep rendering in the provider's own
        // color even while `is_warning` is true.
        let lines = PaceGuidanceLines {
            primary: "80% overpacing".to_string(),
            secondary: None,
            detail: None,
            is_warning: true,
        };
        let (shows, percent, _text, is_warning) = session_cell_decision(
            CellState::Ok,
            Some(80.0),
            "80%",
            Some(&lines),
            ShortWindowVisibility::Always,
        );
        assert!(shows);
        assert!(is_warning);
        assert_eq!(percent, Some(80.0));
    }

    #[test]
    fn app_theme_variants_do_not_affect_visible_rows_or_popup_height() {
        // `visible_rows`/`popup_height_logical` take `PopupLayout` and
        // weekly-lines/session-row inputs, but no `AppTheme` at all — so a
        // theme switch cannot change row count or height by construction.
        // Computing every theme's palette in between exercises that real
        // decoupling rather than just asserting it in a comment.
        let rows_before = visible_rows(PopupLayout::Standard, 2, true);
        let height_before = popup_height_logical(rows_before);

        let _ = popup_palette(AppTheme::RecommendedDark);
        let _ = popup_palette(AppTheme::Light);
        let _ = popup_palette(AppTheme::HighVisibility);

        let rows_after = visible_rows(PopupLayout::Standard, 2, true);
        let height_after = popup_height_logical(rows_after);
        assert_eq!(height_before, height_after);
        assert_eq!(rows_before.session_row, rows_after.session_row);
        assert_eq!(
            rows_before.weekly_extra_lines,
            rows_after.weekly_extra_lines
        );
    }

    #[test]
    fn visible_rows_for_compact_forces_every_optional_row_off() {
        let rows = visible_rows(PopupLayout::Compact, 2, true);
        assert_eq!(rows.weekly_extra_lines, 0);
        assert!(!rows.session_row);
    }

    #[test]
    fn visible_rows_for_compact_forces_off_even_with_nothing_to_show() {
        let rows = visible_rows(PopupLayout::Compact, 0, false);
        assert_eq!(rows.weekly_extra_lines, 0);
        assert!(!rows.session_row);
    }

    #[test]
    fn visible_rows_for_standard_passes_content_through_unchanged() {
        let rows = visible_rows(PopupLayout::Standard, 2, true);
        assert_eq!(rows.weekly_extra_lines, 2);
        assert!(rows.session_row);

        let rows = visible_rows(PopupLayout::Standard, 0, false);
        assert_eq!(rows.weekly_extra_lines, 0);
        assert!(!rows.session_row);
    }

    #[test]
    fn three_visible_quota_rows_at_125_percent_use_component_scaled_height() {
        let rows = visible_rows_for_quota(PopupLayout::Standard, true, 0, true, true, 0);

        assert_eq!(popup_height_logical(rows), 85);
        assert_eq!(scaled_for_dpi(popup_height_logical(rows), 120), 106);
        assert_eq!(popup_height_device_for_dpi(rows, 120), 107);
    }

    #[test]
    fn two_weekly_pace_lines_at_125_percent_use_component_scaled_height() {
        let rows = visible_rows_for_quota(PopupLayout::Standard, true, 2, true, true, 0);

        assert_eq!(
            scaled_for_dpi(SEGMENT_H, 120) + 2 * scaled_for_dpi(PACE_LINE_H, 120),
            52
        );
        assert_eq!(scaled_for_dpi(SEGMENT_H + 2 * PACE_LINE_H, 120), 51);
        assert_eq!(popup_height_device_for_dpi(rows, 120), 143);
    }

    #[test]
    fn antigravity_extra_rows_add_one_segment_height_each() {
        let none = visible_rows_for_quota(PopupLayout::Standard, true, 0, false, false, 0);
        let two = visible_rows_for_quota(PopupLayout::Standard, true, 0, false, false, 2);
        assert_eq!(
            popup_height_logical(two) - popup_height_logical(none),
            2 * SEGMENT_H + 2 * ROW_GAP_H,
            "each extra row adds its own segment height plus a gap before it"
        );
    }

    #[test]
    fn antigravity_extra_rows_are_shown_in_compact_layout_too() {
        // Unlike `session_row`, extra Antigravity items are current quota
        // data, not an optional detail — Compact must not zero them out.
        let rows = visible_rows_for_quota(PopupLayout::Compact, true, 0, true, false, 3);
        assert_eq!(rows.antigravity_extra_rows, 3);
        assert!(!rows.session_row);
    }

    #[test]
    fn pace_row_layout_places_antigravity_extra_rows_after_monthly_row_without_panicking() {
        let rows = visible_rows_for_quota(PopupLayout::Standard, true, 0, false, true, 2);
        let height = sc(popup_height_logical(rows));
        let layout = pace_row_layout(height, rows); // must not hit the debug_assert
        assert_eq!(layout.antigravity_extra_rows_y.len(), 2);
        let monthly_y = layout.monthly_row_y.expect("monthly row is visible");
        assert!(layout.antigravity_extra_rows_y[0] > monthly_y);
        assert!(layout.antigravity_extra_rows_y[1] > layout.antigravity_extra_rows_y[0]);
    }

    #[test]
    fn pace_row_layout_antigravity_extra_rows_empty_when_none() {
        let rows = visible_rows_for_quota(PopupLayout::Standard, true, 0, false, true, 0);
        let height = sc(popup_height_logical(rows));
        let layout = pace_row_layout(height, rows);
        assert!(layout.antigravity_extra_rows_y.is_empty());
    }

    #[test]
    fn antigravity_provider_state_is_ok_on_success_regardless_of_session_weekly_flags() {
        // `UsageData::from_quota_items` never sets `session_available`/
        // `weekly_available` — the whole point of this function is to not
        // depend on those flags for Antigravity.
        let usage = UsageData::from_quota_items(vec![QuotaItem::percentage(
            "gemini-weekly",
            "Gemini Weekly",
            10.0,
            None,
        )]);
        let outcome = poller::ProviderPollOutcome::Success {
            source: poller::ProviderPollSource::AntigravityQuotaUsage,
            attempted_at: SystemTime::UNIX_EPOCH,
            acquired_at: SystemTime::UNIX_EPOCH,
            usage,
        };
        assert_eq!(antigravity_provider_cell_state(&outcome), CellState::Ok);
    }

    #[test]
    fn antigravity_provider_state_is_disabled_when_provider_disabled() {
        assert_eq!(
            antigravity_provider_cell_state(&poller::ProviderPollOutcome::Disabled),
            CellState::Disabled
        );
    }

    #[test]
    fn antigravity_weekly_pace_section_is_none_for_missing_item() {
        assert_eq!(antigravity_weekly_pace_section(None), None);
    }

    #[test]
    fn antigravity_weekly_pace_section_is_none_for_stale_item() {
        let item = crate::models::QuotaItem {
            id: "gemini-weekly".to_string(),
            label: "Gemini Weekly".to_string(),
            availability: crate::models::QuotaItemAvailability::Stale,
            metric: None,
            unit: crate::models::QuotaUnit::Percent,
            resets_at: None,
        };
        assert_eq!(
            antigravity_weekly_pace_section(Some(&item)),
            None,
            "a stale gemini-weekly item must never feed pace guidance a pre-reset value"
        );
    }

    #[test]
    fn antigravity_weekly_pace_section_is_some_for_available_item() {
        let resets_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let item = QuotaItem::percentage("gemini-weekly", "Gemini Weekly", 37.0, Some(resets_at));
        let section = antigravity_weekly_pace_section(Some(&item)).expect("available item");
        assert_eq!(section.percentage, 37.0);
        assert_eq!(section.resets_at, Some(resets_at));
    }

    #[test]
    fn antigravity_extra_items_excludes_gemini_weekly_and_keeps_others() {
        let items = vec![
            QuotaItem::percentage("gemini-weekly", "Gemini Weekly", 10.0, None),
            QuotaItem::percentage("3p-weekly", "3p-weekly", 5.0, None),
        ];
        let extras = antigravity_extra_items_from(
            &items,
            CellState::Ok,
            DisplayBasis::UsedPercentage,
            ResetDisplayMode::Relative,
            LanguageId::English.strings(),
        );
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0].label, "3p-weekly");
        assert_eq!(extras[0].percent, Some(5.0));
    }

    #[test]
    fn antigravity_extra_items_handles_stale_and_unknown_keys_without_panicking() {
        let items = vec![
            crate::models::QuotaItem {
                id: "3p-weekly".to_string(),
                label: "3p-weekly".to_string(),
                availability: crate::models::QuotaItemAvailability::Stale,
                metric: None,
                unit: crate::models::QuotaUnit::Percent,
                resets_at: None,
            },
            QuotaItem::percentage("some-future-key", "some-future-key", 42.0, None),
        ];
        let extras = antigravity_extra_items_from(
            &items,
            CellState::Ok,
            DisplayBasis::UsedPercentage,
            ResetDisplayMode::Relative,
            LanguageId::English.strings(),
        );
        assert_eq!(extras.len(), 2);
        assert_eq!(extras[0].percent, None, "stale item must not show a value");
        assert_eq!(extras[1].label, "some-future-key");
        assert_eq!(extras[1].percent, Some(42.0));
    }

    #[test]
    fn antigravity_extra_items_is_empty_list_when_provider_state_is_not_ok() {
        let items = vec![QuotaItem::percentage("3p-weekly", "3p-weekly", 5.0, None)];
        let extras = antigravity_extra_items_from(
            &items,
            CellState::FetchFailed,
            DisplayBasis::UsedPercentage,
            ResetDisplayMode::Relative,
            LanguageId::English.strings(),
        );
        // The item still exists in the slice, but a non-Ok provider state
        // must short-circuit every row to its status text, never a value.
        assert_eq!(extras[0].percent, None);
    }

    #[test]
    fn quota_row_with_every_provider_hidden_has_no_label_or_height() {
        assert!(!quota_row_visible(&[
            (true, false),
            (false, true),
            (false, false),
        ]));
        let rows = visible_rows_for_quota(PopupLayout::Standard, false, 2, false, false, 0);
        assert!(!rows.weekly_row);
        assert_eq!(rows.weekly_extra_lines, 0);
        assert_eq!(rows.session_row, false);
        assert_eq!(rows.monthly_row, false);
        assert_eq!(popup_height_logical(rows), 3 + HEADER_ROW_H + 4 + 5);
        let layout = pace_row_layout(sc(popup_height_logical(rows)), rows);
        assert_eq!(layout.weekly_row_y, None);
        assert_eq!(layout.session_row_y, None);
        assert_eq!(layout.monthly_row_y, None);
    }

    #[test]
    fn quota_row_remains_when_any_shown_provider_has_content() {
        assert!(quota_row_visible(&[
            (true, false),
            (true, true),
            (false, true),
        ]));
    }

    #[test]
    fn hidden_by_setting_and_not_available_are_distinct() {
        let strings = LanguageId::Japanese.strings();
        let hidden = session_cell_decision(
            CellState::NotAvailable,
            None,
            strings.not_available,
            None,
            ShortWindowVisibility::Hidden,
        );
        let shown_unavailable = session_cell_decision(
            CellState::NotAvailable,
            None,
            strings.not_available,
            None,
            ShortWindowVisibility::Always,
        );
        assert!(!hidden.0);
        assert_eq!(hidden.2, "");
        assert!(shown_unavailable.0);
        assert_eq!(shown_unavailable.2, "対象なし");
    }

    #[test]
    fn five_hour_weekly_and_monthly_rows_share_visibility_and_height_rules() {
        for visible_row in [
            visible_rows_for_quota(PopupLayout::Standard, true, 0, false, false, 0),
            visible_rows_for_quota(PopupLayout::Standard, false, 0, true, false, 0),
            visible_rows_for_quota(PopupLayout::Standard, false, 0, false, true, 0),
        ] {
            assert_eq!(
                popup_height_logical(visible_row),
                3 + HEADER_ROW_H + 4 + SEGMENT_H + 5
            );
            let layout = pace_row_layout(sc(popup_height_logical(visible_row)), visible_row);
            assert_eq!(
                i32::from(layout.weekly_row_y.is_some())
                    + i32::from(layout.session_row_y.is_some())
                    + i32::from(layout.monthly_row_y.is_some()),
                1
            );
        }
    }

    #[test]
    fn row_visibility_and_height_add_exactly_one_row_and_gap() {
        let weekly_only = visible_rows_for_quota(PopupLayout::Standard, true, 0, false, false, 0);
        let weekly_and_monthly =
            visible_rows_for_quota(PopupLayout::Standard, true, 0, false, true, 0);
        assert_eq!(
            popup_height_logical(weekly_and_monthly) - popup_height_logical(weekly_only),
            ROW_GAP_H + SEGMENT_H
        );
    }

    #[test]
    fn popup_height_logical_for_compact_is_header_plus_weekly_row_only() {
        // No basis-label row and no session row, regardless of what the
        // underlying pace content would otherwise show.
        let rows = visible_rows(PopupLayout::Compact, 2, true);
        assert_eq!(
            popup_height_logical(rows),
            WIDGET_HEIGHT - BASIS_LABEL_ROW_H - ROW_GAP_H - SEGMENT_H
        );
    }

    #[test]
    fn popup_height_logical_for_standard_matches_pre_popup_layout_behavior() {
        // AUM-WINDOW-UI-01C-1: the basis-label row was removed from the
        // popup body for every `PopupLayout`, so `Standard`'s height is now
        // `WIDGET_HEIGHT - BASIS_LABEL_ROW_H` at baseline (previously just
        // `WIDGET_HEIGHT`, back when the basis-label row was Standard-only).
        let with_session = visible_rows(PopupLayout::Standard, 0, true);
        assert_eq!(
            popup_height_logical(with_session),
            WIDGET_HEIGHT - BASIS_LABEL_ROW_H
        );

        let without_session = visible_rows(PopupLayout::Standard, 2, false);
        assert_eq!(
            popup_height_logical(without_session),
            WIDGET_HEIGHT - BASIS_LABEL_ROW_H - ROW_GAP_H - SEGMENT_H + 2 * PACE_LINE_H
        );
    }

    #[test]
    fn compact_layout_is_never_taller_than_standard_for_the_same_content() {
        let weekly_extra_lines = 2;
        let needs_session_row = true;
        let compact = popup_height_logical(visible_rows(
            PopupLayout::Compact,
            weekly_extra_lines,
            needs_session_row,
        ));
        let standard = popup_height_logical(visible_rows(
            PopupLayout::Standard,
            weekly_extra_lines,
            needs_session_row,
        ));
        assert!(compact < standard);
    }

    #[test]
    fn pace_row_layout_for_compact_omits_session_row() {
        let rows = visible_rows(PopupLayout::Compact, 2, true);
        let height = sc(popup_height_logical(rows));
        let layout = pace_row_layout(height, rows);
        assert_eq!(layout.session_row_y, None);
        assert_eq!(layout.weekly_secondary_y, None);
    }

    #[test]
    fn pace_row_layout_for_standard_matches_pre_popup_layout_positions() {
        let rows = visible_rows(PopupLayout::Standard, 1, true);
        let height = sc(popup_height_logical(rows));
        let layout = pace_row_layout(height, rows);
        assert!(layout.session_row_y.is_some());
        assert!(layout.weekly_secondary_y.is_some());
    }

    // ── AUM-PACE-GUIDANCE-01: settings menu (IDs, mapping, localization) ───

    #[test]
    fn display_density_for_menu_id_maps_each_known_id() {
        assert_eq!(
            display_density_for_menu_id(IDM_DISPLAY_DENSITY_COMPACT),
            Some(DisplayDensity::Compact)
        );
        assert_eq!(
            display_density_for_menu_id(IDM_DISPLAY_DENSITY_STANDARD),
            Some(DisplayDensity::Standard)
        );
        assert_eq!(
            display_density_for_menu_id(IDM_DISPLAY_DENSITY_DETAILED),
            Some(DisplayDensity::Detailed)
        );
    }

    #[test]
    fn short_window_visibility_for_menu_id_maps_each_known_id() {
        assert_eq!(
            short_window_visibility_for_menu_id(IDM_SHORT_WINDOW_VISIBILITY_ALWAYS),
            Some(ShortWindowVisibility::Always)
        );
        assert_eq!(
            short_window_visibility_for_menu_id(IDM_SHORT_WINDOW_VISIBILITY_WARNING_ONLY),
            Some(ShortWindowVisibility::WarningOnly)
        );
        assert_eq!(
            short_window_visibility_for_menu_id(IDM_SHORT_WINDOW_VISIBILITY_HIDDEN),
            Some(ShortWindowVisibility::Hidden)
        );
    }

    #[test]
    fn short_window_alert_sensitivity_for_menu_id_maps_each_known_id() {
        assert_eq!(
            short_window_alert_sensitivity_for_menu_id(
                IDM_SHORT_WINDOW_ALERT_SENSITIVITY_SENSITIVE
            ),
            Some(ShortWindowAlertSensitivity::Sensitive)
        );
        assert_eq!(
            short_window_alert_sensitivity_for_menu_id(IDM_SHORT_WINDOW_ALERT_SENSITIVITY_STANDARD),
            Some(ShortWindowAlertSensitivity::Standard)
        );
        assert_eq!(
            short_window_alert_sensitivity_for_menu_id(IDM_SHORT_WINDOW_ALERT_SENSITIVITY_RELAXED),
            Some(ShortWindowAlertSensitivity::Relaxed)
        );
    }

    #[test]
    fn unknown_menu_id_maps_to_none_for_each_pace_display_group() {
        assert_eq!(display_density_for_menu_id(9999), None);
        assert_eq!(short_window_visibility_for_menu_id(9999), None);
        assert_eq!(short_window_alert_sensitivity_for_menu_id(9999), None);
    }

    #[test]
    fn new_pace_display_menu_ids_are_pairwise_distinct() {
        let ids = [
            IDM_DISPLAY_DENSITY_COMPACT,
            IDM_DISPLAY_DENSITY_STANDARD,
            IDM_DISPLAY_DENSITY_DETAILED,
            IDM_SHORT_WINDOW_VISIBILITY_ALWAYS,
            IDM_SHORT_WINDOW_VISIBILITY_WARNING_ONLY,
            IDM_SHORT_WINDOW_VISIBILITY_HIDDEN,
            IDM_SHORT_WINDOW_ALERT_SENSITIVITY_SENSITIVE,
            IDM_SHORT_WINDOW_ALERT_SENSITIVITY_STANDARD,
            IDM_SHORT_WINDOW_ALERT_SENSITIVITY_RELAXED,
        ];
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(
                    ids[i], ids[j],
                    "duplicate pace-display menu ID at indices {i} and {j}"
                );
            }
        }
    }

    #[test]
    fn wm_command_menu_ids_are_globally_unique() {
        // Every value dispatched through the main window's `WM_COMMAND`
        // handler, across both `window.rs` and `tray_icon.rs` (the tray
        // icon's own action reaches the same handler via
        // `tray_icon::IDM_TOGGLE_WIDGET`). `1`/`2` are the literal, unnamed
        // IDs `show_context_menu` uses directly for "Refresh"/"Exit" — kept
        // here as plain values since they aren't named constants.
        let mut ids: Vec<u16> = vec![
            1,
            2,
            IDM_FREQ_1MIN,
            IDM_FREQ_5MIN,
            IDM_FREQ_15MIN,
            IDM_FREQ_1HOUR,
            IDM_START_WITH_WINDOWS,
            IDM_ALWAYS_ON_TOP,
            IDM_RESET_POSITION,
            IDM_LANG_SYSTEM,
            IDM_LANG_ENGLISH,
            IDM_LANG_DUTCH,
            IDM_LANG_SPANISH,
            IDM_LANG_FRENCH,
            IDM_LANG_GERMAN,
            IDM_LANG_JAPANESE,
            IDM_LANG_KOREAN,
            IDM_LANG_TRADITIONAL_CHINESE,
            IDM_LANG_RUSSIAN,
            IDM_LANG_PORTUGUESE_BRAZIL,
            IDM_LANG_SIMPLIFIED_CHINESE,
            IDM_MODEL_CLAUDE_CODE,
            IDM_MODEL_CODEX,
            IDM_MODEL_GITHUB_COPILOT,
            IDM_DISPLAY_BASIS_USED,
            IDM_DISPLAY_BASIS_REMAINING,
            IDM_DISPLAY_DENSITY_COMPACT,
            IDM_DISPLAY_DENSITY_STANDARD,
            IDM_DISPLAY_DENSITY_DETAILED,
            IDM_SHORT_WINDOW_VISIBILITY_ALWAYS,
            IDM_SHORT_WINDOW_VISIBILITY_WARNING_ONLY,
            IDM_SHORT_WINDOW_VISIBILITY_HIDDEN,
            IDM_SHORT_WINDOW_ALERT_SENSITIVITY_SENSITIVE,
            IDM_SHORT_WINDOW_ALERT_SENSITIVITY_STANDARD,
            IDM_SHORT_WINDOW_ALERT_SENSITIVITY_RELAXED,
            IDM_POPUP_LAYOUT_COMPACT,
            IDM_POPUP_LAYOUT_STANDARD,
            IDM_APP_THEME_RECOMMENDED_DARK,
            IDM_APP_THEME_LIGHT,
            IDM_APP_THEME_HIGH_VISIBILITY,
            IDM_GITHUB_COPILOT_PLAN_UNKNOWN,
            IDM_GITHUB_COPILOT_PLAN_PRO,
            IDM_GITHUB_COPILOT_PLAN_PRO_PLUS,
            IDM_GITHUB_COPILOT_PLAN_MAX,
            IDM_RESET_DISPLAY_RELATIVE,
            IDM_RESET_DISPLAY_ABSOLUTE,
            IDM_HELP_DISPLAY_GUIDE,
            IDM_HELP_AI_QUOTAS,
            IDM_HELP_README,
            IDM_HELP_VERSION_INFORMATION,
            IDM_VIRTUAL_DESKTOP_SCOPE_ALL,
            IDM_VIRTUAL_DESKTOP_SCOPE_CURRENT_ONLY,
            tray_icon::IDM_TOGGLE_WIDGET,
        ];
        #[cfg(feature = "self-update")]
        ids.push(IDM_VERSION_ACTION);
        #[cfg(feature = "antigravity")]
        ids.push(IDM_MODEL_ANTIGRAVITY);
        #[cfg(feature = "antigravity")]
        ids.push(IDM_HELP_ANTIGRAVITY_SETUP);

        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(
                    ids[i], ids[j],
                    "duplicate WM_COMMAND menu ID {} at indices {i} and {j}",
                    ids[i]
                );
            }
        }
    }

    #[cfg(feature = "antigravity")]
    #[test]
    fn antigravity_model_menu_entry_matches_the_registered_quota_family() {
        assert_eq!(IDM_MODEL_ANTIGRAVITY, 62);
        assert_eq!(QuotaFamilyId::Antigravity.stable_id(), "antigravity");
        assert_eq!(QuotaFamilyId::Antigravity.display_name(), "Antigravity");
    }

    #[test]
    fn all_languages_have_non_empty_organized_menu_strings() {
        for language in LanguageId::ALL {
            let strings = language.strings();
            assert!(!strings.displayed_ai.is_empty());
            assert!(!strings.display_settings.is_empty());
            assert!(!strings.github_copilot.is_empty());
            assert!(!strings.github_copilot_plan_unknown.is_empty());
            assert!(!strings.github_copilot_plan_pro.is_empty());
            assert!(!strings.github_copilot_plan_pro_plus.is_empty());
            assert!(!strings.github_copilot_plan_max.is_empty());
            assert!(!strings.help.is_empty());
            assert!(!strings.help_display_guide.is_empty());
            assert!(!strings.help_ai_quotas.is_empty());
            assert!(!strings.help_readme.is_empty());
            assert!(!strings.help_version_information.is_empty());
            assert!(!strings.help_display_guide_body.is_empty());
            assert!(!strings.help_ai_quotas_intro.is_empty());
            assert_eq!(strings.help_provider_notes.len(), 4);
            assert!(strings
                .help_provider_notes
                .iter()
                .all(|provider| !provider.name.is_empty() && !provider.description.is_empty()));
            assert!(!strings.display_density.is_empty());
            assert!(!strings.display_density_compact.is_empty());
            assert!(!strings.standard_level.is_empty());
            assert!(!strings.display_density_detailed.is_empty());
            assert!(!strings.short_window_visibility.is_empty());
            assert!(!strings.short_window_visibility_always.is_empty());
            assert!(!strings.short_window_visibility_warning_only.is_empty());
            assert!(!strings.short_window_visibility_hidden.is_empty());
            assert!(!strings.short_window_alert_sensitivity.is_empty());
            assert!(!strings.short_window_alert_sensitivity_sensitive.is_empty());
            assert!(!strings.short_window_alert_sensitivity_relaxed.is_empty());
            assert!(!strings.reset_display.is_empty());
            assert!(!strings.reset_display_relative.is_empty());
            assert!(!strings.reset_display_datetime.is_empty());
            assert!(!strings.monthly_window.is_empty());
            assert!(strings.weekday_short.iter().all(|day| !day.is_empty()));
        }
    }

    #[test]
    fn provider_help_is_appendable_and_keeps_the_required_provider_order() {
        for language in LanguageId::ALL {
            let strings = language.strings();
            let provider_names: Vec<_> = strings
                .help_provider_notes
                .iter()
                .map(|provider| provider.name)
                .collect();
            assert_eq!(
                provider_names,
                ["Claude", "Codex", "Antigravity", "GitHub Copilot"]
            );

            let help = build_provider_help_text(strings);
            assert!(help.starts_with(strings.help_ai_quotas_intro));
            for provider in strings.help_provider_notes {
                assert!(help.contains(provider.name));
                assert!(help.contains(provider.description));
            }
        }
    }

    #[test]
    fn japanese_help_covers_display_semantics_and_provider_distinctions() {
        let strings = LanguageId::Japanese.strings();
        for term in [
            "残り利用枠",
            "使用済み率",
            "5h",
            "7d",
            "Mo",
            "経過時間",
            "残り時間",
            "日時",
            "目安",
            "予定との差",
            "使いすぎ",
            "使用量0",
            "取得失敗",
            "対象なし",
            "非表示",
        ] {
            assert!(
                strings.help_display_guide_body.contains(term),
                "Japanese display help is missing {term}"
            );
        }

        let provider_help = build_provider_help_text(strings);
        for distinction in ["ChatGPT本体", "Gemini本体", "Microsoft Copilot", "不明"] {
            assert!(
                provider_help.contains(distinction),
                "Japanese provider help is missing {distinction}"
            );
        }
        assert!(!provider_help.contains("フルリセット"));
        assert!(provider_help.contains("ChatGPT本体の利用枠とは別です"));
        assert!(provider_help.contains("Gemini本体の利用枠とは別です"));
        assert!(provider_help.contains("プランが不明な場合は総使用量のみ表示します"));
    }

    #[test]
    fn japanese_display_help_has_no_manually_forced_orphan_lines() {
        let body = LanguageId::Japanese.strings().help_display_guide_body;
        for line in body.lines().filter(|line| !line.is_empty()) {
            assert!(
                line.chars().count() > 2,
                "Japanese display help has an orphan line: {line}"
            );
        }
        assert!(!body.lines().any(|line| line == "す。"));
    }

    #[test]
    fn help_readme_and_version_use_this_apps_public_metadata() {
        assert_eq!(
            README_URL,
            "https://github.com/ysawase/ai-usage-monitor#readme"
        );

        let strings = LanguageId::Japanese.strings();
        let version = version_information_text(strings);
        assert!(version.contains(strings.window_title));
        assert!(version.contains(&format!("v{}", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn help_copy_does_not_expose_endpoints_or_credentials() {
        for language in LanguageId::ALL {
            let strings = language.strings();
            let help = format!(
                "{}\n{}",
                strings.help_display_guide_body,
                build_provider_help_text(strings)
            )
            .to_ascii_lowercase();
            for forbidden in ["https://", "/v1/", "access token", "credential"] {
                assert!(!help.contains(forbidden), "Help copy exposes {forbidden}");
            }
        }
    }

    // ── AUM-PACE-GUIDANCE-01: popup display model (text generation only) ───

    #[test]
    fn weekly_pace_status_text_maps_each_status() {
        let strings = LanguageId::English.strings();
        assert_eq!(
            weekly_pace_status_text(WeeklyPaceStatus::Judging, strings),
            strings.weekly_pace_judging
        );
        assert_eq!(
            weekly_pace_status_text(WeeklyPaceStatus::UnderPace, strings),
            strings.weekly_pace_under_pace
        );
        assert_eq!(
            weekly_pace_status_text(WeeklyPaceStatus::OnTrack, strings),
            strings.weekly_pace_on_track
        );
        assert_eq!(
            weekly_pace_status_text(WeeklyPaceStatus::SlightlyOverpacing, strings),
            strings.weekly_pace_slightly_overpacing
        );
        assert_eq!(
            weekly_pace_status_text(WeeklyPaceStatus::Overpacing, strings),
            strings.weekly_pace_overpacing
        );
    }

    #[test]
    fn all_languages_have_non_empty_pace_guidance_strings() {
        for language in LanguageId::ALL {
            let strings = language.strings();
            assert!(!strings.weekly_pace_judging.is_empty());
            assert!(!strings.weekly_pace_under_pace.is_empty());
            assert!(!strings.weekly_pace_on_track.is_empty());
            assert!(!strings.weekly_pace_slightly_overpacing.is_empty());
            assert!(!strings.weekly_pace_overpacing.is_empty());
            assert!(!strings.future_pace_label.is_empty());
            assert!(!strings.pace_diff_label.is_empty());
            assert!(!strings.exhaustion_label.is_empty());
            assert!(!strings.exhaustion_before_reset.is_empty());
            assert!(!strings.session_window_label.is_empty());
            assert!(!strings.weekly_window_label.is_empty());
            assert!(!strings.per_day_suffix.is_empty());
            assert!(!strings.per_hour_suffix.is_empty());
            assert!(!strings.pace_used_prefix.is_empty());
            assert!(!strings.pace_remaining_prefix.is_empty());
            assert!(!strings.elapsed.is_empty());
        }
    }

    #[test]
    fn weekly_pace_guidance_compact_used_basis_shows_only_value_and_elapsed() {
        let now = SystemTime::now();
        let elapsed = WEEKLY_WINDOW_SECS / 2;
        let remaining = WEEKLY_WINDOW_SECS - elapsed;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();

        let lines = weekly_pace_guidance_lines(
            Some(69.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Compact,
            strings,
        )
        .expect("known value with valid reset data should produce lines");

        assert!(lines.primary.contains("69%"));
        assert!(lines.primary.contains(strings.elapsed));
        assert!(!lines.primary.contains(strings.reset_in));
        assert_eq!(lines.secondary, None);
        assert_eq!(lines.detail, None);
        assert!(!lines.is_warning);
    }

    #[test]
    fn weekly_pace_guidance_standard_shows_status_and_future_pace() {
        let now = SystemTime::now();
        let elapsed = WEEKLY_WINDOW_SECS / 2;
        let remaining = WEEKLY_WINDOW_SECS - elapsed;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();

        let lines = weekly_pace_guidance_lines(
            Some(69.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Standard,
            strings,
        )
        .expect("known value with valid reset data should produce lines");

        assert!(lines
            .primary
            .contains(strings.weekly_pace_slightly_overpacing));
        let secondary = lines
            .secondary
            .expect("standard density should produce a secondary line");
        assert!(secondary.contains(strings.future_pace_label));
        assert!(secondary.contains("%/"));
        assert_eq!(lines.detail, None);
    }

    #[test]
    fn weekly_pace_guidance_detailed_shows_pace_diff() {
        let now = SystemTime::now();
        let elapsed = WEEKLY_WINDOW_SECS / 2;
        let remaining = WEEKLY_WINDOW_SECS - elapsed;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();

        let lines = weekly_pace_guidance_lines(
            Some(69.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Detailed,
            strings,
        )
        .expect("known value with valid reset data should produce lines");

        let detail = lines
            .detail
            .expect("detailed density should produce a detail line");
        assert!(detail.contains(strings.pace_diff_label));
        assert!(detail.contains("+19pt"));
    }

    #[test]
    fn weekly_pace_guidance_detailed_omits_exhaustion_when_projected_after_reset() {
        let now = SystemTime::now();
        let elapsed = WEEKLY_WINDOW_SECS / 2;
        let remaining = WEEKLY_WINDOW_SECS - elapsed;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();

        // used=20% at 50% elapsed projects exhaustion far beyond the
        // remaining time in this window.
        let lines = weekly_pace_guidance_lines(
            Some(20.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Detailed,
            strings,
        )
        .expect("known value with valid reset data should produce lines");

        let detail = lines.detail.expect("pace diff should still be present");
        assert!(!detail.contains(strings.exhaustion_label));
    }

    #[test]
    fn weekly_pace_guidance_detailed_hides_exhaustion_while_on_track_or_under_pace() {
        let now = SystemTime::now();
        let elapsed = WEEKLY_WINDOW_SECS / 5;
        let remaining = WEEKLY_WINDOW_SECS - elapsed;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();

        // At 20% elapsed, 25% used is +5pt: OnTrack. The unchanged linear
        // projection still reaches 100% before reset, but must not be shown.
        assert_eq!(
            weekly_pace_status(elapsed, WEEKLY_WINDOW_SECS, 25.0),
            WeeklyPaceStatus::OnTrack
        );
        assert!(weekly_exhaustion_lead_secs(elapsed, remaining, 25.0).is_some());
        let on_track = weekly_pace_guidance_lines(
            Some(25.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Detailed,
            strings,
        )
        .unwrap();
        assert!(on_track.primary.contains(strings.weekly_pace_on_track));
        let detail = on_track.detail.expect("pace diff should remain visible");
        assert!(detail.contains("+5pt"));
        assert!(!detail.contains(strings.exhaustion_label));

        // Under Pace also never pairs its status word with exhaustion text.
        let under_pace = weekly_pace_guidance_lines(
            Some(5.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Detailed,
            strings,
        )
        .unwrap();
        assert!(under_pace.primary.contains(strings.weekly_pace_under_pace));
        assert!(!under_pace
            .detail
            .unwrap()
            .contains(strings.exhaustion_label));
    }

    #[test]
    fn weekly_pace_guidance_detailed_shows_exhaustion_lead_time_before_reset() {
        let now = SystemTime::now();
        let elapsed = WEEKLY_WINDOW_SECS / 2;
        let remaining = WEEKLY_WINDOW_SECS - elapsed;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();

        // Both warning statuses may still show a valid pre-reset projection.
        for (used_percent, status_text) in [
            (69.0, strings.weekly_pace_slightly_overpacing),
            (80.0, strings.weekly_pace_overpacing),
        ] {
            let lines = weekly_pace_guidance_lines(
                Some(used_percent),
                resets_at,
                now,
                DisplayBasis::UsedPercentage,
                DisplayDensity::Detailed,
                strings,
            )
            .expect("known value with valid reset data should produce lines");

            assert!(lines.primary.contains(status_text));
            let detail = lines.detail.expect("exhaustion text should be present");
            assert!(detail.contains(strings.exhaustion_label));
        }
    }

    #[test]
    fn weekly_pace_guidance_time_axis_follows_basis_and_keeps_future_pace() {
        let now = SystemTime::now();
        let elapsed = 14 * 3600 + 26 * 60;
        let remaining = WEEKLY_WINDOW_SECS - elapsed;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();

        let used_lines = weekly_pace_guidance_lines(
            Some(69.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Standard,
            strings,
        )
        .unwrap();
        let remaining_lines = weekly_pace_guidance_lines(
            Some(69.0),
            resets_at,
            now,
            DisplayBasis::RemainingAllowance,
            DisplayDensity::Standard,
            strings,
        )
        .unwrap();

        assert!(used_lines.primary.contains("69%"));
        assert!(!used_lines.primary.contains("31%"));
        assert!(remaining_lines.primary.contains("31%"));
        assert!(!remaining_lines.primary.contains("69%"));

        let used_secondary = used_lines.secondary.unwrap();
        assert!(used_secondary.contains(strings.elapsed));
        assert!(used_secondary.contains("14h"));
        assert!(!used_secondary.contains("26m"));
        assert!(!used_secondary.contains(strings.reset_in));
        assert!(used_secondary.contains(strings.future_pace_label));

        let remaining_secondary = remaining_lines.secondary.unwrap();
        assert!(remaining_secondary.contains(strings.reset_in));
        assert!(remaining_secondary.contains("6d9h"));
        assert!(!remaining_secondary.contains("34m"));
        assert!(!remaining_secondary.contains(strings.elapsed));
        assert!(remaining_secondary.contains(strings.future_pace_label));
    }

    #[test]
    fn weekly_pace_guidance_with_unknown_reset_shows_current_value_only() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let lines = weekly_pace_guidance_lines(
            Some(69.0),
            None,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Detailed,
            strings,
        )
        .unwrap();
        assert!(lines.primary.contains("69%"));
        assert_eq!(lines.secondary, None);
        assert_eq!(lines.detail, None);
    }

    #[test]
    fn weekly_pace_guidance_with_past_reset_does_not_panic() {
        let now = SystemTime::now();
        let resets_at = Some(now - Duration::from_secs(5));
        let strings = LanguageId::English.strings();
        let lines = weekly_pace_guidance_lines(
            Some(69.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Standard,
            strings,
        )
        .unwrap();
        assert!(lines.primary.contains("69%"));
        assert_eq!(lines.secondary, None);
    }

    #[test]
    fn weekly_pace_guidance_standard_omits_judging_word_before_min_elapsed() {
        let now = SystemTime::now();
        // Elapsed well under `PACE_JUDGING_MIN_ELAPSED_SECS` (6h): pace is
        // too noisy to judge yet, regardless of `used_percent`.
        let elapsed = PACE_JUDGING_MIN_ELAPSED_SECS - 3600;
        let remaining = WEEKLY_WINDOW_SECS - elapsed;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();

        let lines = weekly_pace_guidance_lines(
            Some(5.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Standard,
            strings,
        )
        .expect("known value with valid reset data should produce lines");

        assert!(lines.primary.contains("5%"));
        assert!(!lines.primary.contains(strings.weekly_pace_judging));
        assert!(lines.secondary.is_some());
    }

    #[test]
    fn japanese_reset_time_is_compact_for_long_and_short_windows() {
        let strings = LanguageId::Japanese.strings();
        assert_eq!(
            format_window_time(
                DisplayBasis::RemainingAllowance,
                Some(6 * 86400 + 8 * 3600),
                None,
                DurationGranularity::LongWindow,
                strings,
            )
            .as_deref(),
            Some("あと6日8時間")
        );
        assert_eq!(
            format_window_time(
                DisplayBasis::RemainingAllowance,
                Some(3 * 3600 + 16 * 60),
                None,
                DurationGranularity::ShortWindow,
                strings,
            )
            .as_deref(),
            Some("あと3時間16分")
        );
    }

    #[test]
    fn relative_remaining_mode_preserves_existing_output() {
        let now = SystemTime::now();
        let remaining = 2 * 86400 + 5 * 3600;
        let strings = LanguageId::Japanese.strings();
        let existing = format_window_time(
            DisplayBasis::RemainingAllowance,
            Some(remaining),
            None,
            DurationGranularity::LongWindow,
            strings,
        );
        let selected = format_window_time_with_reset_mode(
            DisplayBasis::RemainingAllowance,
            Some(now + Duration::from_secs(remaining)),
            now,
            Some(remaining),
            None,
            DurationGranularity::LongWindow,
            ResetDisplayMode::Relative,
            strings,
        );
        assert_eq!(selected, existing);
        assert_eq!(selected.as_deref(), Some("あと2日5時間"));
    }

    #[test]
    fn relative_used_rate_shows_elapsed_only() {
        let now = SystemTime::now();
        let reset_at = now + Duration::from_secs(3 * 3600);
        let strings = LanguageId::Japanese.strings();
        let text = format_window_time_with_reset_mode(
            DisplayBasis::UsedPercentage,
            Some(reset_at),
            now,
            Some(3 * 3600),
            Some(2 * 3600),
            DurationGranularity::ShortWindow,
            ResetDisplayMode::Relative,
            strings,
        )
        .unwrap();
        let reset_text = format_absolute_reset_time(Some(reset_at), now, strings).unwrap();
        assert_eq!(text, "経過 2時間0分");
        assert!(!text.contains(&reset_text));
    }

    #[test]
    fn japanese_absolute_reset_uses_compact_day_weekday_and_colon_time() {
        let strings = LanguageId::Japanese.strings();
        let now = LocalDateTimeParts {
            year: 2026,
            month: 8,
            day: 16,
            weekday: 0,
            hour: 10,
            minute: 0,
        };
        let reset = LocalDateTimeParts {
            year: 2026,
            month: 8,
            day: 19,
            weekday: 3,
            hour: 6,
            minute: 59,
        };
        assert_eq!(
            format_absolute_reset_parts(reset, now, strings),
            "19(水) 6:59リセット"
        );
    }

    #[test]
    fn japanese_absolute_reset_omits_month_and_year_across_boundaries() {
        let strings = LanguageId::Japanese.strings();
        let now = LocalDateTimeParts {
            year: 2026,
            month: 8,
            day: 31,
            weekday: 1,
            hour: 23,
            minute: 0,
        };
        let next_month = LocalDateTimeParts {
            year: 2026,
            month: 9,
            day: 1,
            weekday: 2,
            hour: 20,
            minute: 0,
        };
        let next_year = LocalDateTimeParts {
            year: 2027,
            month: 1,
            day: 1,
            weekday: 5,
            hour: 9,
            minute: 30,
        };
        assert_eq!(
            format_absolute_reset_parts(next_month, now, strings),
            "1(火) 20:00リセット"
        );
        assert_eq!(
            format_absolute_reset_parts(next_year, now, strings),
            "1(金) 9:30リセット"
        );
    }

    #[test]
    fn absolute_reset_uses_the_windows_local_time_conversion() {
        let now = SystemTime::now();
        let reset_at = now + Duration::from_secs(26 * 3600);
        let strings = LanguageId::English.strings();
        let expected = format_absolute_reset_parts(
            system_time_to_local_parts(reset_at).expect("reset local time"),
            system_time_to_local_parts(now).expect("current local time"),
            strings,
        );
        assert_eq!(
            format_absolute_reset_time(Some(reset_at), now, strings),
            Some(expected)
        );
    }

    #[test]
    fn absolute_used_rate_replaces_elapsed_with_reset_datetime() {
        let now = SystemTime::now();
        let reset_at = now + Duration::from_secs(3 * 3600);
        let strings = LanguageId::English.strings();
        let relative = format_window_time_with_reset_mode(
            DisplayBasis::UsedPercentage,
            Some(reset_at),
            now,
            Some(3 * 3600),
            Some(2 * 3600),
            DurationGranularity::ShortWindow,
            ResetDisplayMode::Relative,
            strings,
        );
        let absolute = format_window_time_with_reset_mode(
            DisplayBasis::UsedPercentage,
            Some(reset_at),
            now,
            Some(3 * 3600),
            Some(2 * 3600),
            DurationGranularity::ShortWindow,
            ResetDisplayMode::AbsoluteDateTime,
            strings,
        );
        let reset_text = format_absolute_reset_time(Some(reset_at), now, strings).unwrap();
        let relative = relative.unwrap();
        let absolute = absolute.unwrap();
        assert!(relative.contains(strings.elapsed));
        assert!(!relative.contains(&reset_text));
        assert!(!absolute.contains(strings.elapsed));
        assert_eq!(absolute, reset_text);
    }

    #[test]
    fn absolute_remaining_replaces_countdown_with_reset_datetime() {
        let now = SystemTime::now();
        let reset_at = now + Duration::from_secs(2 * 86400 + 5 * 3600);
        let strings = LanguageId::Japanese.strings();
        let text = format_window_time_with_reset_mode(
            DisplayBasis::RemainingAllowance,
            Some(reset_at),
            now,
            Some(2 * 86400 + 5 * 3600),
            None,
            DurationGranularity::LongWindow,
            ResetDisplayMode::AbsoluteDateTime,
            strings,
        )
        .unwrap();
        assert_eq!(
            text,
            format_absolute_reset_time(Some(reset_at), now, strings).unwrap()
        );
        assert!(!text.contains(strings.reset_in));
    }

    #[test]
    fn absolute_mode_without_reset_timestamp_does_not_guess_datetime() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        for basis in [
            DisplayBasis::UsedPercentage,
            DisplayBasis::RemainingAllowance,
        ] {
            assert_eq!(
                format_window_time_with_reset_mode(
                    basis,
                    None,
                    now,
                    Some(3 * 3600),
                    Some(2 * 3600),
                    DurationGranularity::ShortWindow,
                    ResetDisplayMode::AbsoluteDateTime,
                    strings,
                ),
                None
            );
        }
    }

    #[test]
    fn absolute_mode_flows_through_weekly_and_generic_remaining_displays() {
        let now = SystemTime::now();
        let reset_at = now + Duration::from_secs(2 * 86400 + 5 * 3600);
        let strings = LanguageId::Japanese.strings();
        let weekly = weekly_pace_guidance_lines_with_reset_mode(
            Some(40.0),
            Some(reset_at),
            now,
            DisplayBasis::RemainingAllowance,
            DisplayDensity::Standard,
            ResetDisplayMode::AbsoluteDateTime,
            strings,
        )
        .unwrap();
        let weekly_time = weekly.secondary.unwrap();
        assert!(!weekly_time.contains(strings.reset_in));
        assert!(weekly_time.contains("リセット"));

        let used_weekly = weekly_pace_guidance_lines_with_reset_mode(
            Some(40.0),
            Some(reset_at),
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Standard,
            ResetDisplayMode::AbsoluteDateTime,
            strings,
        )
        .unwrap();
        let used_weekly_time = used_weekly.secondary.unwrap();
        assert!(!used_weekly_time.contains(strings.elapsed));
        assert!(used_weekly_time.contains("リセット"));
        assert!(used_weekly_time.contains(strings.future_pace_label));

        let item = generic_item(
            Some(QuotaMetric::Used {
                used: 100.0,
                limit: Some(1_500.0),
            }),
            Some(reset_at),
        );
        let generic = render_generic_quota_item_with_reset_mode(
            CellState::Ok,
            Some(&item),
            DisplayBasis::RemainingAllowance,
            ResetDisplayMode::AbsoluteDateTime,
            strings,
        );
        assert!(!generic.text.contains(strings.reset_in));
        assert!(generic.text.contains("リセット"));

        let generic_used = render_generic_quota_item_with_reset_mode(
            CellState::Ok,
            Some(&item),
            DisplayBasis::UsedPercentage,
            ResetDisplayMode::AbsoluteDateTime,
            strings,
        );
        assert!(generic_used.text.contains("リセット"));
    }

    #[test]
    fn every_language_formats_absolute_reset_without_unresolved_placeholders() {
        let now = LocalDateTimeParts {
            year: 2026,
            month: 12,
            day: 31,
            weekday: 4,
            hour: 23,
            minute: 0,
        };
        let reset = LocalDateTimeParts {
            year: 2027,
            month: 11,
            day: 2,
            weekday: 5,
            hour: 9,
            minute: 5,
        };
        for language in LanguageId::ALL {
            let strings = language.strings();
            let text = format_absolute_reset_parts(reset, now, strings);
            assert!(!text.trim().is_empty(), "language={language:?}");
            assert!(!text.contains('{'), "language={language:?}: {text}");
            assert!(text.contains(strings.weekday_short[5]));
            assert!(!text.contains("2027"), "language={language:?}: {text}");
            assert!(!text.contains("11"), "language={language:?}: {text}");
        }
    }

    #[test]
    fn format_future_pace_guidance_per_day() {
        let strings = LanguageId::English.strings();
        let guidance = FuturePaceGuidance {
            value: 14.0,
            unit: FuturePaceUnit::PerDay,
        };
        let text = format_future_pace_guidance(&guidance, strings).unwrap();
        assert_eq!(text, format!("14%/{}", strings.per_day_suffix));
    }

    #[test]
    fn format_future_pace_guidance_per_hour() {
        let strings = LanguageId::English.strings();
        let guidance = FuturePaceGuidance {
            value: 3.0,
            unit: FuturePaceUnit::PerHour,
        };
        let text = format_future_pace_guidance(&guidance, strings).unwrap();
        // 3.0 falls in `format_pace_rate_value`'s [1, 10) bucket, which keeps
        // one decimal place — not "3%/hr".
        assert_eq!(text, format!("3.0%/{}", strings.per_hour_suffix));
    }

    #[test]
    fn format_pace_diff_pt_normalizes_negative_zero() {
        assert_eq!(format_pace_diff_pt(-0.0), Some("0pt".to_string()));
        assert_eq!(format_pace_diff_pt(0.0), Some("0pt".to_string()));
    }

    #[test]
    fn format_pace_rate_value_rounds_by_magnitude_bucket() {
        // (0, 1): two decimal places.
        assert_eq!(format_pace_rate_value(0.75), "0.75");
        // [1, 10): one decimal place, lower boundary inclusive.
        assert_eq!(format_pace_rate_value(1.0), "1.0");
        assert_eq!(format_pace_rate_value(9.5), "9.5");
        // [10, ..): integer, lower boundary inclusive.
        assert_eq!(format_pace_rate_value(10.0), "10");
    }

    #[test]
    fn short_window_pace_guidance_time_axis_follows_display_basis() {
        let now = SystemTime::now();
        let elapsed = 1 * 3600 + 16 * 60 + 42;
        let remaining = SESSION_WINDOW_SECS - elapsed;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();
        let used_lines = short_window_pace_guidance_lines(
            Some(24.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::Always,
            ShortWindowAlertSensitivity::Standard,
            strings,
        )
        .unwrap();
        assert!(used_lines.primary.contains(strings.elapsed));
        assert!(used_lines.primary.contains("1h16m"));
        assert!(!used_lines.primary.contains("42s"));
        assert!(!used_lines.primary.contains(strings.reset_in));

        let remaining_lines = short_window_pace_guidance_lines(
            Some(24.0),
            resets_at,
            now,
            DisplayBasis::RemainingAllowance,
            ShortWindowVisibility::Always,
            ShortWindowAlertSensitivity::Standard,
            strings,
        )
        .unwrap();
        assert!(remaining_lines.primary.contains(strings.reset_in));
        assert!(remaining_lines.primary.contains("3h43m"));
        assert!(!remaining_lines.primary.contains("18s"));
        assert!(!remaining_lines.primary.contains(strings.elapsed));
    }

    #[test]
    fn short_window_pace_guidance_warning_only_hides_normal_window() {
        let now = SystemTime::now();
        let resets_at = Some(now + Duration::from_secs(SESSION_WINDOW_SECS / 2));
        let strings = LanguageId::English.strings();
        let lines = short_window_pace_guidance_lines(
            Some(24.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::WarningOnly,
            ShortWindowAlertSensitivity::Standard,
            strings,
        );
        assert_eq!(lines, None);
    }

    #[test]
    fn short_window_pace_guidance_warning_only_shows_overpacing_window() {
        let now = SystemTime::now();
        // elapsed = 3600s (past the 1800s Standard grace period); used=60%
        // projects exhaustion well before this window's reset.
        let remaining = SESSION_WINDOW_SECS - 3600;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();
        let lines = short_window_pace_guidance_lines(
            Some(60.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::WarningOnly,
            ShortWindowAlertSensitivity::Standard,
            strings,
        )
        .expect("overpacing window should be shown even under WarningOnly");
        assert!(lines.is_warning);
        // Also covers "5時間枠の警告表示に「使いすぎ」が含まれる".
        assert!(lines.primary.contains(strings.weekly_pace_overpacing));
    }

    /// "使いすぎモード＋95%相当：表示" — a used% comfortably below 100 still
    /// projects exhaustion well before reset, so it must show under
    /// `WarningOnly` exactly like the 60%-used projection case above.
    #[test]
    fn short_window_pace_guidance_warning_only_shows_ninety_five_percent_overpacing() {
        let now = SystemTime::now();
        let sensitivity = ShortWindowAlertSensitivity::Standard;
        let t = sensitivity.thresholds();
        let elapsed = t.grace_secs + 1;
        let remaining = SESSION_WINDOW_SECS - elapsed;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();
        let lines = short_window_pace_guidance_lines(
            Some(95.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::WarningOnly,
            sensitivity,
            strings,
        )
        .expect("95%-used projecting exhaustion well before reset must show under WarningOnly");
        assert!(lines.is_warning);
    }

    /// AUM-WINDOW-UI-01C-1-HF1 regression: "使いすぎモード＋100%：表示",
    /// including "リセット直前でも100%が表示される" — reproduces the exact
    /// live bug report (Claude Code at 100% used, ~35 minutes to reset,
    /// hidden under "使いすぎのときだけ" before this fix). 100%-used must
    /// always be a warning under `WarningOnly`, regardless of how little of
    /// the window remains — see `short_window_is_overpacing`'s `used >=
    /// 100.0` branch.
    #[test]
    fn short_window_pace_guidance_warning_only_shows_hundred_percent_even_near_reset() {
        let now = SystemTime::now();
        let remaining = Duration::from_secs(35 * 60).as_secs(); // ~35 minutes, as reported live
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();
        let lines = short_window_pace_guidance_lines(
            Some(100.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::WarningOnly,
            ShortWindowAlertSensitivity::Standard,
            strings,
        )
        .expect("100%-used must show under WarningOnly even with little time left before reset");
        assert!(lines.is_warning);

        // Reset itself imminent (0 seconds remaining): still must show.
        let lines_at_reset = short_window_pace_guidance_lines(
            Some(100.0),
            Some(now + Duration::from_secs(SESSION_WINDOW_SECS)),
            now + Duration::from_secs(SESSION_WINDOW_SECS),
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::WarningOnly,
            ShortWindowAlertSensitivity::Standard,
            strings,
        );
        assert!(lines_at_reset.is_some());
    }

    #[test]
    fn short_window_pace_guidance_hidden_never_shows_even_when_overpacing() {
        let now = SystemTime::now();
        let remaining = SESSION_WINDOW_SECS - 3600;
        let resets_at = Some(now + Duration::from_secs(remaining));
        let strings = LanguageId::English.strings();
        let lines = short_window_pace_guidance_lines(
            Some(60.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::Hidden,
            ShortWindowAlertSensitivity::Standard,
            strings,
        );
        assert_eq!(lines, None);
    }

    /// "表示しない＋100%：非表示" — `Hidden` suppresses everything
    /// unconditionally, including a 100%-used cell that `WarningOnly` would
    /// now always show (see the HF1 regression test above).
    #[test]
    fn short_window_pace_guidance_hidden_never_shows_hundred_percent() {
        let now = SystemTime::now();
        let resets_at = Some(now + Duration::from_secs(35 * 60));
        let strings = LanguageId::English.strings();
        let lines = short_window_pace_guidance_lines(
            Some(100.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::Hidden,
            ShortWindowAlertSensitivity::Standard,
            strings,
        );
        assert_eq!(lines, None);
    }

    #[test]
    fn short_window_pace_guidance_normal_display_has_no_pace_status_wording() {
        let now = SystemTime::now();
        let resets_at = Some(now + Duration::from_secs(SESSION_WINDOW_SECS / 2));
        let strings = LanguageId::English.strings();
        let lines = short_window_pace_guidance_lines(
            Some(24.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::Always,
            ShortWindowAlertSensitivity::Standard,
            strings,
        )
        .unwrap();
        assert!(!lines.primary.contains(strings.weekly_pace_under_pace));
        assert!(!lines.primary.contains(strings.weekly_pace_on_track));
        assert!(!lines
            .primary
            .contains(strings.weekly_pace_slightly_overpacing));
        assert!(!lines.primary.contains(strings.weekly_pace_overpacing));
        assert!(!lines.is_warning);
    }

    #[test]
    fn pace_guidance_handles_nan_infinite_and_extreme_inputs_without_panicking() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();

        assert_eq!(
            weekly_pace_guidance_lines(
                Some(f64::NAN),
                None,
                now,
                DisplayBasis::UsedPercentage,
                DisplayDensity::Detailed,
                strings
            ),
            None
        );
        assert_eq!(
            weekly_pace_guidance_lines(
                Some(f64::INFINITY),
                None,
                now,
                DisplayBasis::UsedPercentage,
                DisplayDensity::Detailed,
                strings
            ),
            None
        );
        assert_eq!(
            short_window_pace_guidance_lines(
                Some(f64::NAN),
                None,
                now,
                DisplayBasis::UsedPercentage,
                ShortWindowVisibility::Always,
                ShortWindowAlertSensitivity::Standard,
                strings
            ),
            None
        );

        // Absurdly-far-future reset (clock skew / bad server data): falls
        // back to current-value-only rather than panicking or misreporting
        // elapsed.
        let far_future = Some(now + Duration::from_secs(WEEKLY_WINDOW_SECS * 1000));
        let lines = weekly_pace_guidance_lines(
            Some(50.0),
            far_future,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Detailed,
            strings,
        )
        .expect("a known value should still produce a current-value-only line");
        assert_eq!(lines.secondary, None);

        // Pure-formatter guards, directly.
        assert_eq!(format_pace_diff_pt(f64::NAN), None);
        assert_eq!(format_pace_diff_pt(f64::INFINITY), None);
        let bad_guidance = FuturePaceGuidance {
            value: f64::NAN,
            unit: FuturePaceUnit::PerDay,
        };
        assert_eq!(format_future_pace_guidance(&bad_guidance, strings), None);
    }

    // ── AUM-PACE-GUIDANCE-01: popup connection (CellState gating, extra
    // line composition) ─────────────────────────────────────────────────

    #[test]
    fn weekly_pace_for_cell_is_none_for_every_non_ok_state() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let section = UsageSection {
            percentage: 42.0,
            resets_at: Some(now + Duration::from_secs(WEEKLY_WINDOW_SECS / 2)),
        };
        for state in [
            CellState::Loading,
            CellState::AuthenticationExpired,
            CellState::AuthenticationProblem,
            CellState::CredentialsUnavailable,
            CellState::FetchFailed,
            CellState::NotAvailable,
        ] {
            assert_eq!(
                weekly_pace_for_cell(
                    state,
                    Some(&section),
                    now,
                    DisplayBasis::UsedPercentage,
                    DisplayDensity::Detailed,
                    strings,
                ),
                None,
                "state {state:?} must not produce pace guidance even with a cached section"
            );
        }
    }

    #[test]
    fn weekly_pace_for_cell_is_none_when_ok_but_section_missing() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        assert_eq!(
            weekly_pace_for_cell(
                CellState::Ok,
                None,
                now,
                DisplayBasis::UsedPercentage,
                DisplayDensity::Standard,
                strings,
            ),
            None
        );
    }

    #[test]
    fn weekly_pace_for_cell_is_some_when_ok_with_section() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let section = UsageSection {
            percentage: 42.0,
            resets_at: Some(now + Duration::from_secs(WEEKLY_WINDOW_SECS / 2)),
        };
        assert!(weekly_pace_for_cell(
            CellState::Ok,
            Some(&section),
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Standard,
            strings,
        )
        .is_some());
    }

    // ── AUM-WINDOW-UI-01C-1: Compact provider-header weekly-remaining text ─
    // Every test below fixes `now` to a synthetic epoch-based instant rather
    // than calling `SystemTime::now()`, so the exact formatted duration
    // string can be asserted without any risk of a real-clock second
    // boundary making the test flaky.

    #[test]
    fn compact_weekly_remaining_text_formats_days_and_hours() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let strings = LanguageId::Japanese.strings();
        let remaining = Duration::from_secs(5 * 86400 + 23 * 3600);
        let text = compact_weekly_remaining_text(Some(now + remaining), now, strings);
        assert_eq!(text.as_deref(), Some("残り 5日23時間"));
    }

    #[test]
    fn compact_weekly_remaining_text_formats_hours_only_under_one_day() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let strings = LanguageId::Japanese.strings();
        let remaining = Duration::from_secs(10 * 3600 + 30 * 60);
        let text = compact_weekly_remaining_text(Some(now + remaining), now, strings);
        assert_eq!(text.as_deref(), Some("残り 10時間"));
    }

    #[test]
    fn compact_weekly_remaining_text_formats_zero_hours_under_one_hour() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let strings = LanguageId::Japanese.strings();
        let remaining = Duration::from_secs(5 * 60 + 30);
        let text = compact_weekly_remaining_text(Some(now + remaining), now, strings);
        assert_eq!(text.as_deref(), Some("残り 0時間"));
    }

    #[test]
    fn compact_weekly_remaining_text_is_none_without_resets_at() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let strings = LanguageId::English.strings();
        assert_eq!(compact_weekly_remaining_text(None, now, strings), None);
    }

    #[test]
    fn compact_weekly_remaining_text_is_none_when_reset_already_passed() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let strings = LanguageId::English.strings();
        let resets_at = now - Duration::from_secs(5);
        assert_eq!(
            compact_weekly_remaining_text(Some(resets_at), now, strings),
            None
        );
    }

    #[test]
    fn compact_weekly_remaining_for_cell_is_none_for_every_non_ok_state() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let strings = LanguageId::English.strings();
        let section = UsageSection {
            percentage: 42.0,
            resets_at: Some(now + Duration::from_secs(WEEKLY_WINDOW_SECS / 2)),
        };
        for state in [
            CellState::Loading,
            CellState::AuthenticationExpired,
            CellState::AuthenticationProblem,
            CellState::CredentialsUnavailable,
            CellState::FetchFailed,
            CellState::NotAvailable,
        ] {
            assert_eq!(
                compact_weekly_remaining_for_cell(state, Some(&section), now, strings),
                None,
                "state {state:?} must not produce remaining text even with a cached section"
            );
        }
    }

    #[test]
    fn compact_weekly_remaining_for_cell_is_none_when_ok_but_section_missing() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let strings = LanguageId::English.strings();
        assert_eq!(
            compact_weekly_remaining_for_cell(CellState::Ok, None, now, strings),
            None
        );
    }

    #[test]
    fn compact_weekly_remaining_for_cell_is_some_when_ok_with_section() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let strings = LanguageId::English.strings();
        let section = UsageSection {
            percentage: 42.0,
            resets_at: Some(now + Duration::from_secs(WEEKLY_WINDOW_SECS / 2)),
        };
        assert!(
            compact_weekly_remaining_for_cell(CellState::Ok, Some(&section), now, strings)
                .is_some()
        );
    }

    /// Each provider's Compact weekly-remaining text is derived solely from
    /// that provider's own `resets_at` — `compact_weekly_remaining_for_cell`
    /// takes no shared/global state, so two providers with different reset
    /// times can never end up showing each other's remaining time.
    #[test]
    fn compact_weekly_remaining_for_cell_does_not_mix_values_across_providers() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let strings = LanguageId::English.strings();
        let claude_section = UsageSection {
            percentage: 10.0,
            resets_at: Some(now + Duration::from_secs(5 * 86400 + 23 * 3600)),
        };
        let codex_section = UsageSection {
            percentage: 90.0,
            resets_at: Some(now + Duration::from_secs(2 * 86400 + 3 * 3600)),
        };
        let claude_text =
            compact_weekly_remaining_for_cell(CellState::Ok, Some(&claude_section), now, strings)
                .unwrap();
        let codex_text =
            compact_weekly_remaining_for_cell(CellState::Ok, Some(&codex_section), now, strings)
                .unwrap();
        assert_ne!(claude_text, codex_text);
        assert!(claude_text.contains("5d"));
        assert!(codex_text.contains("2d"));
    }

    #[test]
    fn session_pace_for_cell_is_none_for_every_non_ok_state() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let section = UsageSection {
            percentage: 80.0,
            resets_at: Some(now + Duration::from_secs(SESSION_WINDOW_SECS / 2)),
        };
        for state in [
            CellState::Loading,
            CellState::AuthenticationExpired,
            CellState::AuthenticationProblem,
            CellState::CredentialsUnavailable,
            CellState::FetchFailed,
            CellState::NotAvailable,
        ] {
            assert_eq!(
                session_pace_for_cell(
                    state,
                    Some(&section),
                    now,
                    DisplayBasis::UsedPercentage,
                    ShortWindowVisibility::Always,
                    ShortWindowAlertSensitivity::Standard,
                    strings,
                ),
                None,
                "state {state:?} must not produce pace guidance even with a cached section"
            );
        }
    }

    #[test]
    fn session_pace_for_cell_is_some_when_ok_with_section() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let section = UsageSection {
            percentage: 24.0,
            resets_at: Some(now + Duration::from_secs(SESSION_WINDOW_SECS / 2)),
        };
        assert!(session_pace_for_cell(
            CellState::Ok,
            Some(&section),
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::Always,
            ShortWindowAlertSensitivity::Standard,
            strings,
        )
        .is_some());
    }

    #[test]
    fn weekly_pace_extra_lines_for_is_zero_without_secondary_or_detail() {
        let lines = PaceGuidanceLines {
            primary: "x".to_string(),
            secondary: None,
            detail: None,
            is_warning: false,
        };
        assert_eq!(weekly_pace_extra_lines_for(Some(&lines)), 0);
        assert_eq!(weekly_pace_extra_lines_for(None), 0);
    }

    #[test]
    fn weekly_pace_extra_lines_for_is_one_with_secondary_only() {
        let lines = PaceGuidanceLines {
            primary: "x".to_string(),
            secondary: Some("y".to_string()),
            detail: None,
            is_warning: false,
        };
        assert_eq!(weekly_pace_extra_lines_for(Some(&lines)), 1);
    }

    #[test]
    fn weekly_pace_extra_lines_for_is_two_with_secondary_and_detail() {
        let lines = PaceGuidanceLines {
            primary: "x".to_string(),
            secondary: Some("y".to_string()),
            detail: Some("z".to_string()),
            is_warning: false,
        };
        assert_eq!(weekly_pace_extra_lines_for(Some(&lines)), 2);
    }

    // ── AUM-PACE-GUIDANCE-01: existing-5h-row popup connection (row reuse,
    // reordering, height/layout pure helpers) ──────────────────────────────

    #[test]
    fn weekly_pace_extra_lines_shown_ignores_hidden_providers_pace() {
        let lines = PaceGuidanceLines {
            primary: "x".to_string(),
            secondary: Some("y".to_string()),
            detail: Some("z".to_string()),
            is_warning: false,
        };
        // codex has 2 lines worth of data but isn't shown; must not count.
        assert_eq!(
            weekly_pace_extra_lines_shown(true, None, false, Some(&lines), false, None),
            0
        );
    }

    #[test]
    fn weekly_pace_extra_lines_shown_uses_max_across_shown_providers() {
        let one_line = PaceGuidanceLines {
            primary: "x".to_string(),
            secondary: Some("y".to_string()),
            detail: None,
            is_warning: false,
        };
        let two_lines = PaceGuidanceLines {
            primary: "x".to_string(),
            secondary: Some("y".to_string()),
            detail: Some("z".to_string()),
            is_warning: false,
        };
        assert_eq!(
            weekly_pace_extra_lines_shown(
                true,
                Some(&one_line),
                true,
                Some(&two_lines),
                false,
                None
            ),
            2
        );
    }

    #[test]
    fn weekly_pace_extra_lines_for_compact_density_is_zero() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let resets_at = Some(now + Duration::from_secs(WEEKLY_WINDOW_SECS / 2));
        let lines = weekly_pace_guidance_lines(
            Some(69.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Compact,
            strings,
        )
        .unwrap();
        assert_eq!(weekly_pace_extra_lines_for(Some(&lines)), 0);
    }

    #[test]
    fn weekly_pace_extra_lines_for_standard_density_is_one() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let resets_at = Some(now + Duration::from_secs(WEEKLY_WINDOW_SECS / 2));
        let lines = weekly_pace_guidance_lines(
            Some(69.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Standard,
            strings,
        )
        .unwrap();
        assert_eq!(weekly_pace_extra_lines_for(Some(&lines)), 1);
    }

    #[test]
    fn weekly_pace_extra_lines_for_detailed_density_is_two() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        // Same 69%-at-50%-elapsed inputs as
        // `weekly_pace_guidance_detailed_shows_pace_diff`, which produces
        // both a pace-diff and an exhaustion detail segment.
        let resets_at = Some(now + Duration::from_secs(WEEKLY_WINDOW_SECS / 2));
        let lines = weekly_pace_guidance_lines(
            Some(69.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Detailed,
            strings,
        )
        .unwrap();
        assert_eq!(weekly_pace_extra_lines_for(Some(&lines)), 2);
    }

    // ── session_cell_decision: existing status text vs. pace guidance,
    // per `ShortWindowVisibility` (regression fix — the existing 5h bar
    // row must keep showing Loading/provider errors/
    // NotAvailable, not just pace guidance) ─────────────────────────────

    #[test]
    fn session_cell_decision_always_ok_with_pace_uses_percent_and_primary() {
        let lines = PaceGuidanceLines {
            primary: "68% overpacing".to_string(),
            secondary: None,
            detail: None,
            is_warning: true,
        };
        let (shows, percent, text, is_warning) = session_cell_decision(
            CellState::Ok,
            Some(42.0),
            "42%",
            Some(&lines),
            ShortWindowVisibility::Always,
        );
        assert!(shows);
        assert_eq!(percent, Some(42.0));
        assert_eq!(text, "68% overpacing");
        // The bar itself never depends on this flag (see `draw_row` — only
        // the value text does), but the flag must still reach the caller
        // faithfully.
        assert!(is_warning);
    }

    #[test]
    fn session_cell_decision_always_loading_keeps_existing_text() {
        let strings = LanguageId::English.strings();
        let (shows, percent, text, _is_warning) = session_cell_decision(
            CellState::Loading,
            None,
            strings.loading,
            None,
            ShortWindowVisibility::Always,
        );
        assert!(shows);
        assert_eq!(percent, None);
        assert_eq!(text, strings.loading);
    }

    #[test]
    fn session_cell_decision_always_fetch_failed_keeps_existing_text() {
        let strings = LanguageId::English.strings();
        let (shows, percent, text, _is_warning) = session_cell_decision(
            CellState::FetchFailed,
            None,
            strings.fetch_failed,
            None,
            ShortWindowVisibility::Always,
        );
        assert!(shows);
        assert_eq!(percent, None);
        assert_eq!(text, strings.fetch_failed);
    }

    #[test]
    fn session_cell_decision_always_authentication_problem_keeps_existing_text() {
        let strings = LanguageId::English.strings();
        let (shows, percent, text, _is_warning) = session_cell_decision(
            CellState::AuthenticationProblem,
            None,
            strings.authentication_problem,
            None,
            ShortWindowVisibility::Always,
        );
        assert!(shows);
        assert_eq!(percent, None);
        assert_eq!(text, strings.authentication_problem);
    }

    #[test]
    fn session_cell_decision_always_credentials_unavailable_keeps_existing_text() {
        let strings = LanguageId::English.strings();
        let (shows, percent, text, _is_warning) = session_cell_decision(
            CellState::CredentialsUnavailable,
            None,
            strings.credentials_unavailable,
            None,
            ShortWindowVisibility::Always,
        );
        assert!(shows);
        assert_eq!(percent, None);
        assert_eq!(text, strings.credentials_unavailable);
    }

    #[test]
    fn session_cell_decision_always_not_available_keeps_existing_text() {
        let strings = LanguageId::English.strings();
        let (shows, percent, text, _is_warning) = session_cell_decision(
            CellState::NotAvailable,
            None,
            strings.not_available,
            None,
            ShortWindowVisibility::Always,
        );
        assert!(shows);
        assert_eq!(percent, None);
        assert_eq!(text, strings.not_available);
    }

    #[test]
    fn session_cell_decision_warning_only_ok_normal_is_blank() {
        let (shows, percent, text, _is_warning) = session_cell_decision(
            CellState::Ok,
            Some(24.0),
            "24%",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        assert!(!shows);
        assert_eq!(percent, None);
        assert_eq!(text, "");
    }

    #[test]
    fn session_cell_decision_warning_only_overpacing_shows_pace() {
        let lines = PaceGuidanceLines {
            primary: "68% overpacing".to_string(),
            secondary: None,
            detail: None,
            is_warning: true,
        };
        let (shows, percent, text, is_warning) = session_cell_decision(
            CellState::Ok,
            Some(68.0),
            "68%",
            Some(&lines),
            ShortWindowVisibility::WarningOnly,
        );
        assert!(shows);
        assert_eq!(percent, Some(68.0));
        assert_eq!(text, "68% overpacing");
        assert!(is_warning);
    }

    /// AUM-WINDOW-UI-01C-1-HF1 end-to-end proof: chains `session_pace_for_cell`
    /// (the same helper `refresh_usage_texts` uses to populate
    /// `AppState.session_pace`) into `session_cell_decision` (the same
    /// predicate `needs_session_row`/`session_row_visible`/`paint_content` use
    /// to decide whether the 5h row is drawn at all), for the exact
    /// live-reported shape: 100%-used, `WarningOnly`, ~35 minutes to reset.
    /// The other HF1 tests only prove `short_window_pace_guidance_lines`
    /// returns `Some` — this proves the row itself actually shows.
    #[test]
    fn session_cell_decision_shows_hundred_percent_row_under_warning_only_near_reset() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let section = UsageSection {
            percentage: 100.0,
            resets_at: Some(now + Duration::from_secs(35 * 60)),
        };
        let pace = session_pace_for_cell(
            CellState::Ok,
            Some(&section),
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::WarningOnly,
            ShortWindowAlertSensitivity::Standard,
            strings,
        );
        let (shows, percent, _text, is_warning) = session_cell_decision(
            CellState::Ok,
            Some(100.0),
            "100%",
            pace.as_ref(),
            ShortWindowVisibility::WarningOnly,
        );
        assert!(
            shows,
            "the 5h row itself must be shown, not just its pace text computed"
        );
        assert_eq!(percent, Some(100.0));
        assert!(is_warning);
    }

    #[test]
    fn session_cell_decision_warning_only_non_ok_shows_existing_text() {
        let strings = LanguageId::English.strings();
        for state in [
            CellState::Loading,
            CellState::AuthenticationExpired,
            CellState::AuthenticationProblem,
            CellState::CredentialsUnavailable,
            CellState::FetchFailed,
            CellState::NotAvailable,
        ] {
            let text = status_text(state, strings);
            let (shows, percent, out_text, _is_warning) =
                session_cell_decision(state, None, text, None, ShortWindowVisibility::WarningOnly);
            assert!(
                shows,
                "state {state:?} should keep its status text under WarningOnly"
            );
            assert_eq!(percent, None);
            assert_eq!(out_text, text);
        }
    }

    #[test]
    fn session_cell_decision_ok_with_missing_data_keeps_not_available_text_even_under_warning_only()
    {
        // The `render_cell`/`status_text` caller-bug fail-safe:
        // `CellState::Ok` paired with no section, so `render_cell` produced
        // `not_available` text and a `None` percent (see `status_text`'s own
        // doc comment). `session_pace_for_cell` also can't produce a `pace`
        // here (its match requires `Some(section)` too). Even under
        // `WarningOnly`, this must still show the fail-safe text rather
        // than going blank.
        let strings = LanguageId::English.strings();
        let (shows, percent, text, _is_warning) = session_cell_decision(
            CellState::Ok,
            None,
            strings.not_available,
            None,
            ShortWindowVisibility::WarningOnly,
        );
        assert!(shows);
        assert_eq!(percent, None);
        assert_eq!(text, strings.not_available);
    }

    #[test]
    fn session_cell_decision_hidden_blanks_every_state_including_errors() {
        let strings = LanguageId::English.strings();
        let lines = PaceGuidanceLines {
            primary: "68% overpacing".to_string(),
            secondary: None,
            detail: None,
            is_warning: true,
        };
        // Ok + warning pace data would show under Always/WarningOnly, but
        // not Hidden.
        let (shows, percent, text, _is_warning) = session_cell_decision(
            CellState::Ok,
            Some(68.0),
            "68%",
            Some(&lines),
            ShortWindowVisibility::Hidden,
        );
        assert!(!shows);
        assert_eq!(percent, None);
        assert_eq!(text, "");
        // Error states must also stay blank under Hidden.
        for state in [
            CellState::Loading,
            CellState::AuthenticationExpired,
            CellState::AuthenticationProblem,
            CellState::CredentialsUnavailable,
            CellState::FetchFailed,
            CellState::NotAvailable,
        ] {
            let existing = status_text(state, strings);
            let (shows, percent, text, _is_warning) =
                session_cell_decision(state, None, existing, None, ShortWindowVisibility::Hidden);
            assert!(!shows, "state {state:?} must stay hidden under Hidden");
            assert_eq!(percent, None);
            assert_eq!(text, "");
        }
    }

    #[test]
    fn session_pace_for_cell_and_render_cell_agree_after_ok_to_fetch_failed_transition() {
        // Simulates the exact poll-to-poll data `refresh_usage_texts`
        // produces: first poll is `Ok` with warning-worthy pace, second
        // poll transitions to `FetchFailed`. Both `render_cell` (text) and
        // `session_pace_for_cell` (pace) independently reflect the
        // *current* state, so `session_cell_decision` can never be handed a
        // stale `Some(pace)` alongside a fresh non-`Ok` state.
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let remaining = SESSION_WINDOW_SECS - 3600;
        let section = UsageSection {
            percentage: 60.0,
            resets_at: Some(now + Duration::from_secs(remaining)),
        };

        let ok_pace = session_pace_for_cell(
            CellState::Ok,
            Some(&section),
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::Always,
            ShortWindowAlertSensitivity::Standard,
            strings,
        )
        .expect("first poll should have warning-worthy pace");

        // Provider errors out on the next poll; the cached `section` is no
        // longer paired with `CellState::Ok`.
        let failed_text = render_cell(
            CellState::FetchFailed,
            None,
            DisplayBasis::UsedPercentage,
            strings,
        )
        .text;
        let failed_pace = session_pace_for_cell(
            CellState::FetchFailed,
            None,
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::Always,
            ShortWindowAlertSensitivity::Standard,
            strings,
        );
        assert_eq!(failed_pace, None);

        let (shows, percent, text, _is_warning) = session_cell_decision(
            CellState::FetchFailed,
            None,
            &failed_text,
            failed_pace.as_ref(),
            ShortWindowVisibility::Always,
        );
        assert!(shows);
        assert_eq!(percent, None);
        assert_eq!(text, strings.fetch_failed);
        assert_ne!(text, ok_pace.primary);
    }

    #[test]
    fn session_row_visible_is_true_only_when_a_shown_providers_decision_says_show() {
        assert!(!session_row_visible(true, false, true, false, true, false));
        assert!(session_row_visible(true, true, false, false, false, false));
        assert!(session_row_visible(false, false, true, true, false, false));
        assert!(session_row_visible(false, false, false, false, true, true));
    }

    #[test]
    fn session_row_visible_ignores_hidden_providers_error_state() {
        // codex has an error-state decision (`shows == true`) but isn't
        // currently shown; must not keep the row alive.
        assert!(!session_row_visible(true, false, false, true, false, false));
    }

    #[test]
    fn session_row_visible_false_for_warning_only_with_no_warnings_or_errors_anywhere() {
        let (claude_shows, _, _, _) = session_cell_decision(
            CellState::Ok,
            Some(24.0),
            "24%",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        let (codex_shows, _, _, _) = session_cell_decision(
            CellState::Ok,
            Some(10.0),
            "10%",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        assert!(!session_row_visible(
            true,
            claude_shows,
            true,
            codex_shows,
            false,
            false
        ));
    }

    #[test]
    fn session_row_visible_false_for_warning_only_when_one_provider_is_in_error() {
        let claude_shows = session_row_cell_shows(
            CellState::Ok,
            Some(24.0),
            "24%",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        let strings = LanguageId::English.strings();
        let codex_shows = session_row_cell_shows(
            CellState::FetchFailed,
            None,
            strings.fetch_failed,
            None,
            ShortWindowVisibility::WarningOnly,
        );
        assert!(!session_row_visible(
            true,
            claude_shows,
            true,
            codex_shows,
            false,
            false
        ));
    }

    #[test]
    fn session_row_visible_false_for_hidden_regardless_of_state() {
        let strings = LanguageId::English.strings();
        let lines = PaceGuidanceLines {
            primary: "x".to_string(),
            secondary: None,
            detail: None,
            is_warning: true,
        };
        let (claude_shows, _, _, _) = session_cell_decision(
            CellState::Ok,
            Some(90.0),
            "90%",
            Some(&lines),
            ShortWindowVisibility::Hidden,
        );
        let (codex_shows, _, _, _) = session_cell_decision(
            CellState::FetchFailed,
            None,
            strings.fetch_failed,
            None,
            ShortWindowVisibility::Hidden,
        );
        assert!(!session_row_visible(
            true,
            claude_shows,
            true,
            codex_shows,
            false,
            false
        ));
    }

    // ── AUM-WINDOW-UI-SHORT-WINDOW-WARNING-ROW-01 ──────────────────────────
    // `session_cell_justifies_warning_only_row` / `session_row_cell_shows`:
    // under WarningOnly, only a real pace-driven warning (including HF1's
    // 100%-used case) may keep the 5h row alive. Status-only cells collapse.

    #[test]
    fn justifies_warning_only_row_is_false_for_not_available_alone() {
        assert!(!session_cell_justifies_warning_only_row(
            CellState::NotAvailable,
            None
        ));
    }

    #[test]
    fn justifies_warning_only_row_is_false_for_ok_without_pace() {
        // A normal, non-warning value: pace is None (WarningOnly suppresses
        // non-overpacing pace text), and Ok is excluded on its own.
        assert!(!session_cell_justifies_warning_only_row(
            CellState::Ok,
            None
        ));
    }

    #[test]
    fn justifies_warning_only_row_is_true_for_any_pace_present() {
        let lines = PaceGuidanceLines {
            primary: "x".to_string(),
            secondary: None,
            detail: None,
            is_warning: true,
        };
        assert!(session_cell_justifies_warning_only_row(
            CellState::Ok,
            Some(&lines)
        ));
    }

    #[test]
    fn justifies_warning_only_row_is_false_for_every_status_without_pace() {
        for state in [
            CellState::Loading,
            CellState::AuthenticationExpired,
            CellState::AuthenticationProblem,
            CellState::CredentialsUnavailable,
            CellState::FetchFailed,
        ] {
            assert!(
                !session_cell_justifies_warning_only_row(state, None),
                "status-only state {state:?} must not justify the row"
            );
        }
    }

    #[test]
    fn session_row_cell_shows_matches_session_cell_decision_for_always_and_hidden() {
        let lines = PaceGuidanceLines {
            primary: "x".to_string(),
            secondary: None,
            detail: None,
            is_warning: false,
        };
        for visibility in [ShortWindowVisibility::Always, ShortWindowVisibility::Hidden] {
            for (state, percent, pace) in [
                (CellState::Ok, Some(26.0), Some(&lines)),
                (CellState::NotAvailable, None, None),
                (CellState::FetchFailed, None, None),
            ] {
                let expected = session_cell_decision(state, percent, "text", pace, visibility).0;
                assert_eq!(
                    session_row_cell_shows(state, percent, "text", pace, visibility),
                    expected,
                    "state={state:?} visibility={visibility:?} must match session_cell_decision outside WarningOnly"
                );
            }
        }
    }

    /// Case 1: Always + Claude 26% (normal) + Codex NotAvailable → row shown
    /// (unchanged from today).
    #[test]
    fn session_row_always_shows_for_normal_value_and_not_available() {
        let lines = PaceGuidanceLines {
            primary: "26%".to_string(),
            secondary: None,
            detail: None,
            is_warning: false,
        };
        let claude_shows = session_row_cell_shows(
            CellState::Ok,
            Some(26.0),
            "26%",
            Some(&lines),
            ShortWindowVisibility::Always,
        );
        let codex_shows = session_row_cell_shows(
            CellState::NotAvailable,
            None,
            "N/A",
            None,
            ShortWindowVisibility::Always,
        );
        assert!(session_row_visible(
            true,
            claude_shows,
            true,
            codex_shows,
            false,
            false
        ));
    }

    /// Case 2 (the reported bug): WarningOnly + Claude 26% (non-warning) +
    /// Codex NotAvailable → row must now be hidden entirely.
    #[test]
    fn session_row_warning_only_hides_for_normal_value_and_not_available() {
        let claude_shows = session_row_cell_shows(
            CellState::Ok,
            Some(26.0),
            "26%",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        let codex_shows = session_row_cell_shows(
            CellState::NotAvailable,
            None,
            "N/A",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        assert!(!session_row_visible(
            true,
            claude_shows,
            true,
            codex_shows,
            false,
            false
        ));
    }

    /// Case 3 (HF1 regression guard): WarningOnly + Claude 100% + Codex
    /// NotAvailable → row must still be shown, via the real
    /// `session_pace_for_cell` pipeline (not a hand-built
    /// `PaceGuidanceLines`), so a future change to `short_window_is_overpacing`
    /// or `short_window_pace_guidance_lines` would also be caught here.
    #[test]
    fn session_row_warning_only_shows_for_hundred_percent_via_real_pace_pipeline() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let section = UsageSection {
            percentage: 100.0,
            resets_at: Some(now + Duration::from_secs(35 * 60)),
        };
        let pace = session_pace_for_cell(
            CellState::Ok,
            Some(&section),
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::WarningOnly,
            ShortWindowAlertSensitivity::Standard,
            strings,
        );
        let claude_shows = session_row_cell_shows(
            CellState::Ok,
            Some(100.0),
            "100%",
            pace.as_ref(),
            ShortWindowVisibility::WarningOnly,
        );
        let codex_shows = session_row_cell_shows(
            CellState::NotAvailable,
            None,
            "N/A",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        assert!(session_row_visible(
            true,
            claude_shows,
            true,
            codex_shows,
            false,
            false
        ));
    }

    /// Case 4: WarningOnly + Claude genuinely overpacing (not the 100%-used
    /// edge case) + Codex NotAvailable → row shown.
    #[test]
    fn session_row_warning_only_shows_for_overpacing_and_not_available() {
        let lines = PaceGuidanceLines {
            primary: "68% overpacing".to_string(),
            secondary: None,
            detail: None,
            is_warning: true,
        };
        let claude_shows = session_row_cell_shows(
            CellState::Ok,
            Some(68.0),
            "68%",
            Some(&lines),
            ShortWindowVisibility::WarningOnly,
        );
        let codex_shows = session_row_cell_shows(
            CellState::NotAvailable,
            None,
            "N/A",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        assert!(session_row_visible(
            true,
            claude_shows,
            true,
            codex_shows,
            false,
            false
        ));
    }

    /// Case 5: WarningOnly + Claude non-warning + Codex Ok/non-warning (a
    /// real 5h value that just isn't overpacing) → row hidden.
    #[test]
    fn session_row_warning_only_hides_when_no_provider_is_warning_or_non_ok() {
        let claude_shows = session_row_cell_shows(
            CellState::Ok,
            Some(10.0),
            "10%",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        let codex_shows = session_row_cell_shows(
            CellState::Ok,
            Some(15.0),
            "15%",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        assert!(!session_row_visible(
            true,
            claude_shows,
            true,
            codex_shows,
            false,
            false
        ));
    }

    /// Case 6: WarningOnly + every active provider NotAvailable → row
    /// hidden (generalizes the reported bug to all-N/A).
    #[test]
    fn session_row_warning_only_hides_when_all_active_providers_are_not_available() {
        let claude_shows = session_row_cell_shows(
            CellState::NotAvailable,
            None,
            "N/A",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        let codex_shows = session_row_cell_shows(
            CellState::NotAvailable,
            None,
            "N/A",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        assert!(!session_row_visible(
            true,
            claude_shows,
            true,
            codex_shows,
            false,
            false
        ));
    }

    /// Case 7: WarningOnly + FetchFailed only → status-only row collapses.
    #[test]
    fn session_row_warning_only_hides_for_fetch_failed_alone() {
        let strings = LanguageId::English.strings();
        let claude_shows = session_row_cell_shows(
            CellState::FetchFailed,
            None,
            strings.fetch_failed,
            None,
            ShortWindowVisibility::WarningOnly,
        );
        assert!(!session_row_visible(
            true,
            claude_shows,
            false,
            false,
            false,
            false
        ));
    }

    /// Case 8: WarningOnly + Loading / provider errors alone also collapse.
    #[test]
    fn session_row_warning_only_hides_for_loading_and_provider_errors() {
        let strings = LanguageId::English.strings();
        for (state, text) in [
            (CellState::Loading, strings.loading),
            (
                CellState::AuthenticationExpired,
                strings.authentication_expired,
            ),
            (
                CellState::AuthenticationProblem,
                strings.authentication_problem,
            ),
            (
                CellState::CredentialsUnavailable,
                strings.credentials_unavailable,
            ),
            (CellState::FetchFailed, strings.fetch_failed),
        ] {
            let claude_shows =
                session_row_cell_shows(state, None, text, None, ShortWindowVisibility::WarningOnly);
            assert!(
                !session_row_visible(true, claude_shows, false, false, false, false),
                "status-only state {state:?} must not keep the row visible under WarningOnly"
            );
        }
    }

    /// Case 9: Hidden + any state → row never shown, regardless of the new
    /// WarningOnly-only logic (Hidden short-circuits before it).
    #[test]
    fn session_row_hidden_never_shows_regardless_of_state() {
        let lines = PaceGuidanceLines {
            primary: "x".to_string(),
            secondary: None,
            detail: None,
            is_warning: true,
        };
        let claude_shows = session_row_cell_shows(
            CellState::Ok,
            Some(100.0),
            "100%",
            Some(&lines),
            ShortWindowVisibility::Hidden,
        );
        let codex_shows = session_row_cell_shows(
            CellState::FetchFailed,
            None,
            "fetch failed",
            None,
            ShortWindowVisibility::Hidden,
        );
        assert!(!session_row_visible(
            true,
            claude_shows,
            true,
            codex_shows,
            false,
            false
        ));
    }

    /// Case 10: an OFF provider never contributes to row existence, even
    /// when its own decision would justify the row (e.g. 100%-used) — the
    /// `show_*` gate in `session_row_visible` is the authority.
    #[test]
    fn session_row_ignores_an_off_providers_justification() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let section = UsageSection {
            percentage: 100.0,
            resets_at: Some(now + Duration::from_secs(35 * 60)),
        };
        let pace = session_pace_for_cell(
            CellState::Ok,
            Some(&section),
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::WarningOnly,
            ShortWindowAlertSensitivity::Standard,
            strings,
        );
        let claude_shows = session_row_cell_shows(
            CellState::Ok,
            Some(100.0),
            "100%",
            pace.as_ref(),
            ShortWindowVisibility::WarningOnly,
        );
        let codex_shows = session_row_cell_shows(
            CellState::NotAvailable,
            None,
            "N/A",
            None,
            ShortWindowVisibility::WarningOnly,
        );
        // Claude Code is OFF despite its decision justifying the row.
        assert!(!session_row_visible(
            false,
            claude_shows,
            true,
            codex_shows,
            false,
            false
        ));
    }

    #[test]
    fn popup_height_logical_with_session_row_is_widget_height_plus_weekly_extra() {
        // AUM-WINDOW-UI-01C-1: baseline is `WIDGET_HEIGHT - BASIS_LABEL_ROW_H`
        // now that the basis-label row is never drawn (see
        // `popup_height_logical_for_standard_matches_pre_popup_layout_behavior`).
        assert_eq!(
            popup_height_logical(visible_rows(PopupLayout::Standard, 0, true)),
            WIDGET_HEIGHT - BASIS_LABEL_ROW_H
        );
        assert_eq!(
            popup_height_logical(visible_rows(PopupLayout::Standard, 2, true)),
            WIDGET_HEIGHT - BASIS_LABEL_ROW_H + 2 * PACE_LINE_H
        );
    }

    #[test]
    fn popup_height_logical_without_session_row_shrinks_by_one_row_and_gap() {
        assert_eq!(
            popup_height_logical(visible_rows(PopupLayout::Standard, 0, false)),
            WIDGET_HEIGHT - BASIS_LABEL_ROW_H - ROW_GAP_H - SEGMENT_H
        );
        assert_eq!(
            popup_height_logical(visible_rows(PopupLayout::Standard, 1, false)),
            WIDGET_HEIGHT - BASIS_LABEL_ROW_H - ROW_GAP_H - SEGMENT_H + PACE_LINE_H
        );
    }

    /// AUM-WINDOW-UI-01C-2-STEP2 (drag UX): `header_band_bottom` (the
    /// header/7d-row boundary `is_drag_region_point` uses) is
    /// `pace_row_layout`'s own `weekly_row_y` — identical regardless of
    /// `PopupLayout`/content state, since `popup_height_logical` always
    /// grows or shrinks the popup's total height by exactly the extra
    /// content's own size, so the top-anchored header row never moves. A
    /// single static drag-region boundary is therefore correct for both
    /// Compact and every Standard content variation, without needing to
    /// special-case either.
    #[test]
    fn header_band_bottom_is_the_same_boundary_for_compact_and_standard() {
        let compact_rows = visible_rows(PopupLayout::Compact, 2, true);
        let compact_bottom =
            pace_row_layout(sc(popup_height_logical(compact_rows)), compact_rows).weekly_row_y;

        let standard_with_session_rows = visible_rows(PopupLayout::Standard, 2, true);
        let standard_with_session_bottom = pace_row_layout(
            sc(popup_height_logical(standard_with_session_rows)),
            standard_with_session_rows,
        )
        .weekly_row_y;

        let standard_without_session_rows = visible_rows(PopupLayout::Standard, 0, false);
        let standard_without_session_bottom = pace_row_layout(
            sc(popup_height_logical(standard_without_session_rows)),
            standard_without_session_rows,
        )
        .weekly_row_y;

        assert_eq!(compact_bottom, standard_with_session_bottom);
        assert_eq!(compact_bottom, standard_without_session_bottom);
    }

    // ── AUM-WINDOW-UI-01C-2-STEP2: drag_follow_position ─────────────────────

    #[test]
    fn drag_follow_position_at_drag_start_equals_start_window() {
        // Cursor hasn't moved yet: the popup stays exactly where it started.
        assert_eq!(
            drag_follow_position((500, 800), (1000, 1000), (1000, 1000)),
            (500, 800)
        );
    }

    #[test]
    fn drag_follow_position_tracks_positive_cursor_movement() {
        assert_eq!(
            drag_follow_position((500, 800), (1000, 1000), (1150, 1100)),
            (650, 900)
        );
    }

    #[test]
    fn drag_follow_position_tracks_negative_cursor_movement() {
        assert_eq!(
            drag_follow_position((500, 800), (1000, 1000), (850, 700)),
            (350, 500)
        );
    }

    #[test]
    fn drag_follow_position_axes_move_independently() {
        // X moves right, Y stays put.
        assert_eq!(
            drag_follow_position((500, 800), (1000, 1000), (1200, 1000)),
            (700, 800)
        );
        // Y moves up, X stays put.
        assert_eq!(
            drag_follow_position((500, 800), (1000, 1000), (1000, 700)),
            (500, 500)
        );
    }

    #[test]
    fn weekly_pace_guidance_primary_never_contains_weekly_window_label() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let resets_at = Some(now + Duration::from_secs(WEEKLY_WINDOW_SECS / 2));
        for density in [
            DisplayDensity::Compact,
            DisplayDensity::Standard,
            DisplayDensity::Detailed,
        ] {
            let lines = weekly_pace_guidance_lines(
                Some(69.0),
                resets_at,
                now,
                DisplayBasis::UsedPercentage,
                density,
                strings,
            )
            .unwrap();
            assert!(!lines.primary.contains(strings.weekly_window_label));
        }
        // Unknown-reset branch too — a separate early return in the function.
        let lines = weekly_pace_guidance_lines(
            Some(69.0),
            None,
            now,
            DisplayBasis::UsedPercentage,
            DisplayDensity::Standard,
            strings,
        )
        .unwrap();
        assert!(!lines.primary.contains(strings.weekly_window_label));
    }

    #[test]
    fn short_window_pace_guidance_primary_never_contains_session_window_label() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let resets_at = Some(now + Duration::from_secs(SESSION_WINDOW_SECS / 2));
        let lines = short_window_pace_guidance_lines(
            Some(24.0),
            resets_at,
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::Always,
            ShortWindowAlertSensitivity::Standard,
            strings,
        )
        .unwrap();
        assert!(!lines.primary.contains(strings.session_window_label));
    }

    #[test]
    fn short_window_pace_guidance_with_unknown_reset_shows_current_value_only() {
        let now = SystemTime::now();
        let strings = LanguageId::English.strings();
        let lines = short_window_pace_guidance_lines(
            Some(24.0),
            None,
            now,
            DisplayBasis::UsedPercentage,
            ShortWindowVisibility::Always,
            ShortWindowAlertSensitivity::Standard,
            strings,
        )
        .unwrap();
        assert!(lines.primary.contains("24%"));
        assert!(!lines.primary.contains(strings.reset_in));
        assert!(!lines.primary.contains(strings.session_window_label));
    }

    #[test]
    fn countdown_text_is_none_when_resets_at_is_none() {
        let strings = LanguageId::English.strings();
        assert_eq!(countdown_text(None, strings), None);
    }

    #[test]
    fn countdown_text_uses_now_word_after_reset_has_passed() {
        let strings = LanguageId::English.strings();
        let past = SystemTime::now() - Duration::from_secs(5);
        assert_eq!(
            countdown_text(Some(past), strings),
            Some(strings.now.to_string())
        );
    }

    #[test]
    fn countdown_text_just_under_one_hour_shows_minutes() {
        let strings = LanguageId::English.strings();
        let reset = SystemTime::now() + Duration::from_secs(3599);
        let text = countdown_text(Some(reset), strings).unwrap();
        assert!(text.ends_with(strings.minute_suffix));
    }

    #[test]
    fn countdown_text_just_over_one_hour_shows_hours() {
        let strings = LanguageId::English.strings();
        let reset = SystemTime::now() + Duration::from_secs(3605);
        let text = countdown_text(Some(reset), strings).unwrap();
        assert!(text.ends_with(strings.hour_suffix));
    }

    fn usage_data_with_session_percent(percentage: f64) -> UsageData {
        let mut usage = UsageData::default();
        usage.set_session(UsageSection {
            percentage,
            resets_at: None,
        });
        usage
    }

    #[test]
    fn merge_successful_providers_updates_only_this_polls_successes() {
        // Previous poll cached Claude at 10% and Codex at 20%.
        let mut cached: Option<AppUsageData> = Some({
            let mut data = AppUsageData::default();
            data.upsert(
                usage_data_with_session_percent(10.0).into_quota_family(QuotaFamilyId::Claude),
            );
            data.upsert(
                usage_data_with_session_percent(20.0).into_quota_family(QuotaFamilyId::Codex),
            );
            data
        });

        // This poll: Claude succeeds fresh at 70%, Codex errors (transient).
        let report = poller::PollReport {
            claude_code: poller::ProviderPollOutcome::Success {
                source: poller::ProviderPollSource::AnthropicOauthUsage,
                attempted_at: SystemTime::now(),
                acquired_at: SystemTime::now(),
                usage: usage_data_with_session_percent(70.0),
            },
            codex: poller::ProviderPollOutcome::Error {
                source: poller::ProviderPollSource::ChatgptWhamUsage,
                attempted_at: SystemTime::now(),
                error: poller::PollError::RequestFailed,
            },
            antigravity: poller::ProviderPollOutcome::Disabled,
            github_copilot: poller::ProviderPollOutcome::Disabled,
        };

        merge_successful_providers(&mut cached, &report);
        let merged = cached.expect("merge must not drop the cache");
        let claude = merged
            .family(QuotaFamilyId::Claude)
            .expect("claude succeeded this poll");
        assert_eq!(
            claude.item("session").unwrap().used_percentage(),
            Some(70.0)
        );

        // Codex didn't succeed this poll, so its cache is left as-is (still
        // the previous 20%) — merge only overwrites providers that actually
        // succeeded this round.
        let codex = merged
            .family(QuotaFamilyId::Codex)
            .expect("previous Codex cache should remain untouched by merge");
        assert_eq!(codex.item("session").unwrap().used_percentage(), Some(20.0));

        // Rendering with the states this same report would produce (as
        // do_poll does) must show Claude's fresh value, and must never show
        // a bar for Codex — even though a real (stale) Codex section exists
        // in the cache and is explicitly passed in here, `CellState::FetchFailed`
        // (derived from the same report's `Error` outcome) must make
        // `render_cell` ignore it rather than display the old 20% as current.
        let strings = LanguageId::English.strings();
        let (claude_session_state, _) =
            poll_cell_states(QuotaFamilyId::Claude, &report.claude_code);
        let claude_display = render_cell(
            claude_session_state,
            quota_item_section(Some(&merged), QuotaFamilyId::Claude, "session").as_ref(),
            DisplayBasis::UsedPercentage,
            strings,
        );
        assert_eq!(claude_display.bar_percent, Some(70.0));

        let (codex_session_state, _) = poll_cell_states(QuotaFamilyId::Codex, &report.codex);
        assert_eq!(codex_session_state, CellState::FetchFailed);
        let codex_display = render_cell(
            codex_session_state,
            quota_item_section(Some(&merged), QuotaFamilyId::Codex, "session").as_ref(),
            DisplayBasis::UsedPercentage,
            strings,
        );
        assert_eq!(codex_display.bar_percent, None);
        assert_eq!(codex_display.text, strings.fetch_failed);
    }

    // ── remaining_secs_at ──────────────────────────────────────────────

    #[test]
    fn partial_provider_merge_leaves_other_provider_cache_untouched() {
        let mut cached = Some({
            let mut data = AppUsageData::default();
            data.upsert(
                usage_data_with_session_percent(10.0).into_quota_family(QuotaFamilyId::Claude),
            );
            data.upsert(
                usage_data_with_session_percent(20.0).into_quota_family(QuotaFamilyId::Codex),
            );
            data
        });
        let claude_update = poller::ProviderPollOutcome::Success {
            source: poller::ProviderPollSource::AnthropicOauthUsage,
            attempted_at: SystemTime::now(),
            acquired_at: SystemTime::now(),
            usage: usage_data_with_session_percent(70.0),
        };

        merge_successful_provider(&mut cached, QuotaFamilyId::Claude, &claude_update);

        let cached = cached.expect("partial success should keep the cache");
        assert_eq!(
            cached
                .family(QuotaFamilyId::Claude)
                .unwrap()
                .item("session")
                .unwrap()
                .used_percentage(),
            Some(70.0)
        );
        assert_eq!(
            cached
                .family(QuotaFamilyId::Codex)
                .unwrap()
                .item("session")
                .unwrap()
                .used_percentage(),
            Some(20.0)
        );
    }

    #[test]
    fn remaining_secs_at_is_none_without_resets_at() {
        assert_eq!(remaining_secs_at(None, SystemTime::now()), None);
    }

    #[test]
    fn remaining_secs_at_is_none_when_reset_is_in_the_past() {
        let now = SystemTime::now();
        let past = now - Duration::from_secs(5);
        assert_eq!(remaining_secs_at(Some(past), now), None);
    }

    #[test]
    fn remaining_secs_at_is_zero_when_reset_is_exactly_now() {
        let now = SystemTime::now();
        assert_eq!(remaining_secs_at(Some(now), now), Some(0));
    }

    // ── elapsed_secs_in_window ─────────────────────────────────────────

    #[test]
    fn elapsed_secs_in_window_at_exact_window_length_is_zero() {
        assert_eq!(
            elapsed_secs_in_window(WEEKLY_WINDOW_SECS, WEEKLY_WINDOW_SECS),
            Some(0)
        );
    }

    #[test]
    fn elapsed_secs_in_window_within_tolerance_past_window_length_is_zero() {
        assert_eq!(
            elapsed_secs_in_window(
                WEEKLY_WINDOW_SECS + WINDOW_TIME_TOLERANCE_SECS,
                WEEKLY_WINDOW_SECS
            ),
            Some(0)
        );
    }

    #[test]
    fn elapsed_secs_in_window_beyond_tolerance_is_none() {
        assert_eq!(
            elapsed_secs_in_window(
                WEEKLY_WINDOW_SECS + WINDOW_TIME_TOLERANCE_SECS + 1,
                WEEKLY_WINDOW_SECS
            ),
            None
        );
    }

    // ── weekly_pace_status ─────────────────────────────────────────────

    #[test]
    fn weekly_pace_status_before_judging_window_is_judging() {
        assert_eq!(
            weekly_pace_status(PACE_JUDGING_MIN_ELAPSED_SECS - 1, WEEKLY_WINDOW_SECS, 50.0),
            WeeklyPaceStatus::Judging
        );
    }

    #[test]
    fn weekly_pace_status_at_judging_boundary_exits_judging() {
        assert_ne!(
            weekly_pace_status(PACE_JUDGING_MIN_ELAPSED_SECS, WEEKLY_WINDOW_SECS, 0.0),
            WeeklyPaceStatus::Judging
        );
    }

    #[test]
    fn weekly_pace_status_under_pace_boundary() {
        let elapsed = WEEKLY_WINDOW_SECS / 2; // elapsed% = 50
        let used_at_boundary = 50.0 + PACE_UNDER_PACE_MAX_PT; // pace_diff == -10.0
        assert_eq!(
            weekly_pace_status(elapsed, WEEKLY_WINDOW_SECS, used_at_boundary),
            WeeklyPaceStatus::UnderPace
        );
        assert_eq!(
            weekly_pace_status(elapsed, WEEKLY_WINDOW_SECS, used_at_boundary + 0.1),
            WeeklyPaceStatus::OnTrack
        );
    }

    #[test]
    fn weekly_pace_status_on_track_upper_boundary() {
        let elapsed = WEEKLY_WINDOW_SECS / 2;
        let used_at_boundary = 50.0 + PACE_ON_TRACK_MAX_PT; // pace_diff == 10.0
        assert_eq!(
            weekly_pace_status(elapsed, WEEKLY_WINDOW_SECS, used_at_boundary),
            WeeklyPaceStatus::OnTrack
        );
        assert_eq!(
            weekly_pace_status(elapsed, WEEKLY_WINDOW_SECS, used_at_boundary + 0.1),
            WeeklyPaceStatus::SlightlyOverpacing
        );
    }

    #[test]
    fn weekly_pace_status_slightly_overpacing_upper_boundary() {
        let elapsed = WEEKLY_WINDOW_SECS / 2;
        let used_at_boundary = 50.0 + PACE_SLIGHTLY_OVER_MAX_PT; // pace_diff == 25.0
        assert_eq!(
            weekly_pace_status(elapsed, WEEKLY_WINDOW_SECS, used_at_boundary),
            WeeklyPaceStatus::SlightlyOverpacing
        );
        assert_eq!(
            weekly_pace_status(elapsed, WEEKLY_WINDOW_SECS, used_at_boundary + 0.1),
            WeeklyPaceStatus::Overpacing
        );
    }

    #[test]
    fn weekly_pace_status_used_zero_percent() {
        let elapsed = WEEKLY_WINDOW_SECS / 2;
        assert_eq!(
            weekly_pace_status(elapsed, WEEKLY_WINDOW_SECS, 0.0),
            WeeklyPaceStatus::UnderPace
        );
    }

    #[test]
    fn weekly_pace_status_used_hundred_percent() {
        let elapsed = WEEKLY_WINDOW_SECS / 2;
        assert_eq!(
            weekly_pace_status(elapsed, WEEKLY_WINDOW_SECS, 100.0),
            WeeklyPaceStatus::Overpacing
        );
    }

    // ── future_pace_guidance ───────────────────────────────────────────

    #[test]
    fn future_pace_guidance_uses_per_day_at_or_above_threshold() {
        let guidance = future_pace_guidance(60.0, FUTURE_PACE_HOURLY_THRESHOLD_SECS)
            .expect("remaining_secs > 0 must produce guidance");
        assert_eq!(guidance.unit, FuturePaceUnit::PerDay);
        assert!((guidance.value - 40.0).abs() < 1e-9);
    }

    #[test]
    fn future_pace_guidance_uses_per_hour_below_threshold() {
        let guidance = future_pace_guidance(60.0, FUTURE_PACE_HOURLY_THRESHOLD_SECS - 1)
            .expect("remaining_secs > 0 must produce guidance");
        assert_eq!(guidance.unit, FuturePaceUnit::PerHour);
    }

    #[test]
    fn future_pace_guidance_is_none_when_remaining_is_zero() {
        assert_eq!(future_pace_guidance(50.0, 0), None);
    }

    // ── ShortWindowAlertSensitivity::thresholds ─────────────────────────

    #[test]
    fn short_window_sensitivity_thresholds_match_spec() {
        let sensitive = ShortWindowAlertSensitivity::Sensitive.thresholds();
        assert_eq!(sensitive.grace_secs, 20 * 60);
        assert_eq!(sensitive.min_used_percent, 30.0);
        assert_eq!(sensitive.exhaustion_lead_secs, 30 * 60);

        let standard = ShortWindowAlertSensitivity::Standard.thresholds();
        assert_eq!(standard.grace_secs, 30 * 60);
        assert_eq!(standard.min_used_percent, 40.0);
        assert_eq!(standard.exhaustion_lead_secs, 45 * 60);

        let relaxed = ShortWindowAlertSensitivity::Relaxed.thresholds();
        assert_eq!(relaxed.grace_secs, 45 * 60);
        assert_eq!(relaxed.min_used_percent, 50.0);
        assert_eq!(relaxed.exhaustion_lead_secs, 75 * 60);
    }

    // ── short_window_is_overpacing ───────────────────────────────────────

    #[test]
    fn short_window_just_before_grace_is_never_overpacing() {
        let sensitivity = ShortWindowAlertSensitivity::Standard;
        let t = sensitivity.thresholds();
        assert!(!short_window_is_overpacing(
            t.grace_secs - 1,
            100_000,
            99.0,
            sensitivity
        ));
    }

    #[test]
    fn short_window_grace_boundary_allows_evaluation() {
        let sensitivity = ShortWindowAlertSensitivity::Standard;
        let t = sensitivity.thresholds();
        let elapsed = t.grace_secs; // exactly at the grace boundary
        let used = 50.0;
        let secs_to_exhaustion = (100.0 - used) * elapsed as f64 / used;
        let remaining_at_boundary = secs_to_exhaustion as u64 + t.exhaustion_lead_secs;

        assert!(short_window_is_overpacing(
            elapsed,
            remaining_at_boundary,
            used,
            sensitivity
        ));
        assert!(!short_window_is_overpacing(
            elapsed,
            remaining_at_boundary - 1,
            used,
            sensitivity
        ));
    }

    #[test]
    fn short_window_min_used_percent_boundary() {
        let sensitivity = ShortWindowAlertSensitivity::Standard;
        let t = sensitivity.thresholds();
        let elapsed = t.grace_secs + 100; // comfortably past grace

        assert!(!short_window_is_overpacing(
            elapsed,
            100_000,
            t.min_used_percent - 0.1,
            sensitivity
        ));

        let used = t.min_used_percent;
        let secs_to_exhaustion = (100.0 - used) * elapsed as f64 / used;
        let remaining = secs_to_exhaustion as u64 + t.exhaustion_lead_secs;
        assert!(short_window_is_overpacing(
            elapsed,
            remaining,
            used,
            sensitivity
        ));
    }

    #[test]
    fn short_window_exhaustion_lead_boundary() {
        let sensitivity = ShortWindowAlertSensitivity::Standard;
        let t = sensitivity.thresholds();
        let elapsed = t.grace_secs + 600;
        let used = 60.0;
        let secs_to_exhaustion = (100.0 - used) * elapsed as f64 / used;
        let remaining_at_lead = secs_to_exhaustion as u64 + t.exhaustion_lead_secs;

        assert!(short_window_is_overpacing(
            elapsed,
            remaining_at_lead,
            used,
            sensitivity
        ));
        assert!(!short_window_is_overpacing(
            elapsed,
            remaining_at_lead - 1,
            used,
            sensitivity
        ));
    }

    /// AUM-WINDOW-UI-01C-1-HF1: 100%-used is always overpacing once past the
    /// grace period, regardless of how much of the window remains —
    /// `exhaustion_lead_secs` only gates the *projection* branch below
    /// (warning this long *before* an upcoming 100%), and has nothing left
    /// to gate once 100% has already been reached. Previously this branch
    /// returned `remaining_secs >= t.exhaustion_lead_secs`, which silently
    /// hid a real 100%-used cell under `WarningOnly` whenever the window's
    /// reset drew within `exhaustion_lead_secs` (e.g. Standard sensitivity:
    /// 45 minutes) — reproduced live with Claude Code at 100% used, ~35
    /// minutes to reset.
    #[test]
    fn short_window_hundred_percent_used_is_always_overpacing_regardless_of_remaining() {
        let sensitivity = ShortWindowAlertSensitivity::Standard;
        let t = sensitivity.thresholds();
        let elapsed = t.grace_secs + 1;

        // Comfortably more than exhaustion_lead_secs remaining: already
        // covered by the old boundary, still holds.
        assert!(short_window_is_overpacing(
            elapsed,
            t.exhaustion_lead_secs,
            100.0,
            sensitivity
        ));
        // Less than exhaustion_lead_secs remaining (the old, backwards
        // cutoff — the exact shape of the reported bug): must still be
        // overpacing.
        assert!(short_window_is_overpacing(
            elapsed,
            t.exhaustion_lead_secs - 1,
            100.0,
            sensitivity
        ));
        // Live-reported case: ~35 minutes remaining (well under Standard's
        // 45-minute exhaustion_lead_secs).
        assert!(short_window_is_overpacing(
            elapsed,
            35 * 60,
            100.0,
            sensitivity
        ));
        // Right at the reset instant: still overpacing — 100% used doesn't
        // stop being true just because the window is about to roll over.
        assert!(short_window_is_overpacing(elapsed, 0, 100.0, sensitivity));
    }

    #[test]
    fn short_window_each_sensitivity_agrees_with_its_own_thresholds() {
        for sensitivity in [
            ShortWindowAlertSensitivity::Sensitive,
            ShortWindowAlertSensitivity::Standard,
            ShortWindowAlertSensitivity::Relaxed,
        ] {
            let t = sensitivity.thresholds();
            let elapsed = t.grace_secs + 60;
            let used = t.min_used_percent + 10.0;
            let secs_to_exhaustion = (100.0 - used) * elapsed as f64 / used;
            let remaining_at_boundary = secs_to_exhaustion as u64 + t.exhaustion_lead_secs;

            assert!(
                short_window_is_overpacing(elapsed, remaining_at_boundary, used, sensitivity),
                "{sensitivity:?} should be overpacing exactly at its own lead boundary"
            );
            assert!(
                !short_window_is_overpacing(elapsed, remaining_at_boundary - 1, used, sensitivity),
                "{sensitivity:?} should not be overpacing just inside its own lead boundary"
            );
        }
    }
}
