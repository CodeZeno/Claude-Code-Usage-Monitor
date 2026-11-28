#![allow(unsafe_code, reason = "calls Win32 APIs through the windows crate")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Diagnostics::Debug::IsDebuggerPresent;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
use windows::Win32::UI::Controls::{HOVER_DEFAULT, WM_MOUSEHOVER, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, ReleaseCapture, SetCapture, TrackMouseEvent, TME_HOVER, TME_LEAVE,
    TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app_settings::{
    self, load_settings, save_settings, LegacyPlacement, PlacementOverride, SettingsFile,
    POLL_15_MIN, POLL_15_MIN_SECONDS, POLL_1_HOUR, POLL_1_HOUR_SECONDS, POLL_1_MIN,
    POLL_1_MIN_SECONDS, POLL_5_MIN, POLL_5_MIN_SECONDS,
};
use crate::context_menu::{self, ContextMenuAction, ContextMenuItem, ContextMenuItemKind};
use crate::diagnose;
use crate::localization::{self, LanguageId, Strings};
use crate::models::AppUsageData;
use crate::native_interop::{
    self, TIMER_CLOCK, TIMER_COUNTDOWN, TIMER_CROSSFADE_SCHEDULE, TIMER_MOUSE_CLICK, TIMER_POLL,
    TIMER_RESET_POLL, TIMER_TRAY_HOVER, TIMER_TRAY_REPOSITION, TIMER_UPDATE_CHECK,
    TIMER_WINDOW_STATE, WM_APP_DISABLE_DIAGNOSTICS, WM_APP_ENABLE_DIAGNOSTICS,
    WM_APP_OPEN_DASHBOARD, WM_APP_QUIT, WM_APP_REFRESH_NOW, WM_APP_SETTINGS_UPDATED,
    WM_APP_TASKBAR_COLLISION, WM_APP_TRAY, WM_APP_USAGE_UPDATED,
};
use crate::poller;
use crate::providers::{ProviderId, ProviderSet};
use crate::theme;
use crate::theme_engine::{
    self, Canvas, DataContext, HorizontalAnchor, MouseActionEffect, MouseActionOverrideKey,
    MouseEventKind, ReferenceRegion, SurfaceNest, ThemeDocument, ThemeRuntime, VerticalAnchor,
};
use crate::tray_icon;
use crate::updater::{self, AvailableUpdate, InstallChannel, ReleaseDescriptor, UpdateCheckResult};

/// Copyable HWND value used by the watchdog after the UI thread publishes it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct SendHwnd(isize);

// SAFETY: this wrapper never transfers ownership of a window. Cross-thread users
// only pass the value back to Win32 APIs that explicitly accept handles created
// by another thread (for example IsWindow and PostMessageW).
unsafe impl Send for SendHwnd {}

impl SendHwnd {
    fn from_hwnd(hwnd: HWND) -> Self {
        Self(hwnd.0 as isize)
    }
    fn to_hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}

/// Copyable event-hook value whose lifetime remains owned by the UI controller.
#[derive(Clone, Copy)]
struct SendWinEventHook(isize);

// SAFETY: the hook is only stored or passed to UnhookWinEvent. Callback work is
// marshalled through Win32; Rust data is never dereferenced through this value.
unsafe impl Send for SendWinEventHook {}

impl SendWinEventHook {
    fn from_hook(hook: HWINEVENTHOOK) -> Self {
        Self(hook.0 as isize)
    }

    fn to_hook(self) -> HWINEVENTHOOK {
        HWINEVENTHOOK(self.0 as *mut _)
    }
}

/// Shared application state
struct AppState {
    hwnd: SendHwnd,
    taskbar_hwnd: Option<SendHwnd>,
    tray_notify_hwnd: Option<SendHwnd>,
    win_event_hook: Option<SendWinEventHook>,
    is_dark: bool,
    embedded: bool,
    language_override: Option<LanguageId>,
    language: LanguageId,
    install_channel: InstallChannel,

    providers: ProviderSet,
    pub(super) show_antigravity_gemini: bool,
    pub(super) show_antigravity_external: bool,
    crossfade_opacity: f64,
    accounts: crate::accounts::AccountSettings,

    data: Option<AppUsageData>,

    poll_interval_ms: u32,
    retry_count: u32,
    force_notify_auth_error: bool,
    auth_error_paused_polling: bool,
    auth_watch_mode: poller::CredentialWatchMode,
    auth_watch_snapshot: poller::CredentialWatchSnapshot,
    last_poll_ok: bool,
    last_poll_failure: Option<poller::PollFailure>,
    update_status: UpdateStatus,
    last_update_check_unix: Option<u64>,

    taskbar_index: usize,
    tray_offset: i32,
    dragging: bool,
    pending_drag: bool,
    drag_start_cursor: POINT,
    drag_start_origin: POINT,
    drag_start_client_x: i32,
    auto_ejected: bool,
    auto_ejected_for_capacity: bool,
    auto_ejected_origin: Option<POINT>,
    auto_ejected_host: Option<app_settings::FloatingHost>,
    /// When true, never auto-eject to floating on taskbar collisions.
    /// The widget stays docked (but can still be dragged along the bar).
    lock_taskbar: bool,
    is_switching_window_style: bool,
    is_snapped: bool,
    placement_override: Option<PlacementOverride>,
    floating_card_opacity: Option<u8>,
    window_state_timer_active: bool,

    custom_theme_enabled: bool,
    usage_countdown: bool,
    use_windows_default_font: bool,
    active_theme_path: Option<PathBuf>,
    active_theme: Option<ThemeDocument>,
    theme_clock_interval: Option<Duration>,
    tray_theme_uses_current_time: bool,
    mirror_hwnds: Vec<SendHwnd>,
    desktop_hwnds: Vec<Option<SendHwnd>>,
    mouse_action_overrides: HashMap<MouseActionOverrideKey, theme_engine::Expression>,
    hovered_mouse_layer: Option<(usize, String)>,
    pending_mouse_click: Option<PendingMouseClick>,
    suppress_next_left_up: bool,
}

#[derive(Clone, Debug)]
struct PendingMouseClick {
    surface_index: usize,
    object_id: String,
}

const RETRY_BASE_MS: u32 = 30_000; // 30 seconds

// Menu item IDs for update frequency
const IDM_FREQ_1MIN: u16 = 10;
const IDM_FREQ_5MIN: u16 = 11;
const IDM_FREQ_15MIN: u16 = 12;
const IDM_FREQ_1HOUR: u16 = 13;
const IDM_START_WITH_WINDOWS: u16 = 20;
const IDM_LOCK_TASKBAR: u16 = 21;
const IDM_VERSION_ACTION: u16 = 31;
const IDM_LANG_SYSTEM: u16 = 100;
const IDM_LANG_FIRST: u16 = 101;
const IDM_DASHBOARD: u16 = 71;
// 62 stays the whole-provider Antigravity toggle used by saved custom menus.
pub(super) const IDM_PROVIDER_ANTIGRAVITY_GEMINI: u16 = 68;
pub(super) const IDM_PROVIDER_ANTIGRAVITY_EXTERNAL: u16 = 67;

const WM_DPICHANGED_MSG: u32 = 0x02E0;
const WM_APP_UPDATE_CHECK_COMPLETE: u32 = WM_APP + 2;
const TRAY_ICON_UPDATE_REPOSITION_SUPPRESS_MS: u64 = 750;
const WINDOW_STATE_INTERVAL_MS: u32 = 250;

fn language_menu_command_id(language: LanguageId) -> u16 {
    IDM_LANG_FIRST
        .checked_add(u16::try_from(language.index()).expect("language index exceeds u16"))
        .expect("language menu command id exceeds u16")
}

fn language_from_menu_command_id(command: u16) -> Option<LanguageId> {
    command
        .checked_sub(IDM_LANG_FIRST)
        .and_then(|index| LanguageId::from_index(index.into()))
}

fn open_web_url(hwnd: HWND, url: &str, failure_message: &'static str) {
    if !native_interop::open_web_url(Some(hwnd), url) {
        diagnose::log(failure_message);
    }
}

static SUPPRESS_TRAY_REPOSITION_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// Current system DPI (96 = 100% scaling, 144 = 150%, 192 = 200%, etc.)
static CURRENT_DPI: AtomicU32 = AtomicU32::new(96);
static POLL_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static POLL_PENDING: AtomicBool = AtomicBool::new(false);

/// Re-query the monitor DPI for our window and update the cached value.
/// Uses GetDpiForWindow which returns the live DPI (unlike GetDpiForSystem
/// which is cached at process startup and never changes).
fn refresh_dpi() {
    let hwnd = {
        let state = lock_state();
        state.as_ref().map(|s| s.hwnd.to_hwnd())
    };
    if let Some(hwnd) = hwnd {
        // SAFETY: GetDpiForWindow takes the surface HWND by value and no pointers;
        // a stale handle makes it return 0, which is ignored below.
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if dpi > 0 {
            CURRENT_DPI.store(dpi, Ordering::Relaxed);
        }
    }
}

fn display_scale(display_index: usize) -> f64 {
    let displays = native_interop::find_monitors();
    let Some(display) = displays
        .get(display_index)
        .copied()
        .or_else(|| displays.first().copied())
    else {
        return 1.0;
    };
    monitor_scale(display)
}

fn monitor_scale(display: native_interop::DisplayMonitor) -> f64 {
    let mut dpi_x = 96;
    let mut dpi_y = 96;
    // SAFETY: the monitor handle comes from EnumDisplayMonitors and is only read by
    // value (an invalid one returns an error); both out-pointers are live locals.
    if unsafe { GetDpiForMonitor(display.handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
        .is_ok()
        && dpi_x > 0
    {
        (dpi_x as f64 / 96.0).clamp(0.25, 8.0)
    } else {
        // SAFETY: GetDpiForSystem takes no arguments and has no preconditions.
        let system_dpi = unsafe { GetDpiForSystem() };
        if system_dpi > 0 {
            (system_dpi as f64 / 96.0).clamp(0.25, 8.0)
        } else {
            1.0
        }
    }
}

fn migrated_theme_placement(legacy: LegacyPlacement) -> (usize, i32) {
    let displays = native_interop::find_monitors();
    let taskbars = native_interop::find_taskbars();
    let display_index = taskbars
        .get(legacy.taskbar_index)
        .or_else(|| taskbars.first())
        .and_then(|taskbar| native_interop::monitor_for_taskbar(&taskbars, &displays, taskbar.hwnd))
        .and_then(|monitor| {
            displays
                .iter()
                .position(|display| display.handle == monitor)
        })
        .unwrap_or_else(|| legacy.taskbar_index.min(displays.len().saturating_sub(1)));
    let offset_x = legacy_offset_to_theme_offset(legacy.tray_offset, display_scale(display_index));
    (display_index, offset_x)
}

fn legacy_offset_to_theme_offset(tray_offset: i32, scale: f64) -> i32 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    -((tray_offset.max(0) as f64 / scale).round() as i32)
}

fn theme_surface_scale(theme: &ThemeDocument, surface_index: usize) -> f64 {
    let display_index = theme
        .surfaces
        .get(surface_index)
        .map(|surface| surface.placement.reference.display)
        .unwrap_or(theme.placement.reference.display);
    display_scale(display_index)
}

fn logical_host_dimension(physical: i32, scale: f64) -> u32 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (physical.max(1) as f64 / scale)
        .round()
        .clamp(1.0, u32::MAX as f64) as u32
}

// The studio runs in a separate process without the monitor's layout cache.
pub(crate) fn query_theme_runtime_for_surface(
    theme: &ThemeDocument,
    surface_index: usize,
    runtime: ThemeRuntime,
) -> ThemeRuntime {
    let Some(surface) = theme.surfaces.get(surface_index) else {
        return runtime;
    };
    let displays = native_interop::find_monitors();
    let Some(display) = displays
        .get(surface.placement.reference.display)
        .copied()
        .or_else(|| displays.first().copied())
    else {
        return runtime;
    };
    let nest = surface
        .placement
        .nest
        .resolve(surface.placement.reference.region);
    let host_rect = if matches!(nest, SurfaceNest::Taskbar | SurfaceNest::TrayIcon) {
        native_interop::taskbar_for_monitor(
            &native_interop::find_taskbars(),
            &displays,
            display.handle,
        )
        .map(|taskbar| taskbar.rect)
        .unwrap_or(display.rect)
    } else {
        display.rect
    };
    let scale = monitor_scale(display);
    runtime.with_host_dimensions(
        logical_host_dimension(host_rect.right - host_rect.left, scale),
        logical_host_dimension(host_rect.bottom - host_rect.top, scale),
    )
}

fn scaled_theme_dimension(logical: u32, scale: f64) -> i32 {
    (logical as f64 * scale).round().clamp(1.0, 8192.0) as i32
}

static STATE: Mutex<Option<AppState>> = Mutex::new(None);

/// Lock STATE safely, recovering from poisoned mutex
fn lock_state() -> MutexGuard<'static, Option<AppState>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn theme_runtime_from_state(state: &AppState) -> ThemeRuntime {
    let (poll_ok, has_error) = poll_display_state(
        state.last_poll_ok,
        state.retry_count,
        state.auth_error_paused_polling,
        state.data.as_ref(),
    );
    let nest = if state.auto_ejected {
        SurfaceNest::Floating
    } else if let Some(ref ov) = state.placement_override {
        if ov.nest == "floating" {
            SurfaceNest::Floating
        } else {
            SurfaceNest::Taskbar
        }
    } else {
        SurfaceNest::Taskbar
    };
    let opacity = state.floating_card_opacity.unwrap_or(85);
    ThemeRuntime::from_providers(state.providers)
        .with_poll_state(poll_ok, has_error)
        .with_language(state.language)
        .with_countdown(state.usage_countdown)
        .with_windows_default_font(state.use_windows_default_font)
        .with_nest(nest)
        .with_floating_card_opacity(opacity)
        .with_antigravity_pools(
            state.show_antigravity_gemini,
            state.show_antigravity_external,
            state.crossfade_opacity,
        )
}

/// A transient outage can keep presenting the last real reading while its
/// retry runs. Authentication failures and failures without cached data still
/// need the explicit error state.
fn poll_display_state(
    last_poll_ok: bool,
    retry_count: u32,
    auth_error_paused_polling: bool,
    data: Option<&AppUsageData>,
) -> (bool, bool) {
    if let Some(data) = data.filter(|data| !data.accounts.is_empty()) {
        let has_error = data.accounts.iter().any(|account| account.error.is_some());
        return (
            !data.is_empty(),
            data.is_empty() && (has_error || retry_count > 0),
        );
    }
    let has_usable_stale_data = !auth_error_paused_polling
        && data.is_some_and(|data| data.iter().any(|(_, usage)| usage.stale));
    (
        last_poll_ok || has_usable_stale_data,
        // A partial-failure retry after an overall success is not an error.
        !last_poll_ok && retry_count > 0 && !has_usable_stale_data,
    )
}

fn effective_theme_from_state(state: &AppState) -> Option<ThemeDocument> {
    theme_with_placement(state, state.auto_ejected)
}

fn theme_with_placement(state: &AppState, auto_ejected: bool) -> Option<ThemeDocument> {
    let mut theme = state.active_theme.as_ref().map(|theme| {
        theme_engine::apply_mouse_action_overrides(theme, &state.mouse_action_overrides)
    })?;
    let floating = if auto_ejected {
        state.auto_ejected_origin.map(|point| (None, point))
    } else {
        state
            .placement_override
            .as_ref()
            .filter(|p| p.nest == "floating")
            .map(|p| {
                (
                    Some((p.monitor_index, p.monitor_device.clone())),
                    POINT {
                        x: p.screen_x,
                        y: p.screen_y,
                    },
                )
            })
    };
    if let Some((saved_monitor, point)) = floating {
        let displays = native_interop::find_monitors();
        let selected = saved_monitor
            .and_then(|(index, device)| {
                positioning::resolve_saved_monitor(&displays, index, device.as_deref())
            })
            .and_then(|index| displays.get(index).copied().map(|d| (index, d)))
            .unwrap_or_else(|| positioning::monitor_for_point(&displays, point));
        apply_floating_position(&mut theme, state, selected.0, selected.1, point);
    } else if let Some(p) = state
        .placement_override
        .as_ref()
        .filter(|p| p.nest == "taskbar")
    {
        let displays = native_interop::find_monitors();
        let index = positioning::resolve_saved_monitor(
            &displays,
            p.monitor_index,
            p.monitor_device.as_deref(),
        )
        .unwrap_or(0);
        let horizontal = taskbar_is_horizontal(index);
        let placement = if p.screen_x > 0 || p.screen_y > 0 {
            let offset = if horizontal { p.screen_x } else { p.screen_y };
            positioning::taskbar_dock_placement(index, offset, display_scale(index), horizontal)
        } else {
            positioning::dock_placement(index, p.tray_offset, display_scale(index), horizontal)
        };
        positioning::override_primary_placement(&mut theme, placement);
    }
    Some(theme)
}

fn apply_floating_position(
    theme: &mut ThemeDocument,
    state: &AppState,
    index: usize,
    display: native_interop::DisplayMonitor,
    point: POINT,
) {
    let mut placement = positioning::floating_placement(index);
    let saved_host = if state.auto_ejected {
        state.auto_ejected_host.as_ref()
    } else {
        state
            .placement_override
            .as_ref()
            .and_then(|p| p.floating_host.as_ref())
    };
    placement.host_dimensions =
        floating_host_for_theme(theme, saved_host).map(|host| (host.width, host.height));
    positioning::override_primary_placement(theme, placement.clone());
    let scale = monitor_scale(display);
    let runtime = theme_runtime_for_surface(theme, 0, theme_runtime_from_state(state));
    let frame = positioning::widget_frame(theme, state.data.as_ref(), runtime, scale);
    let offset = positioning::clamped_floating_offset(point, display.rect, &frame, scale);
    placement.offset_x = offset.x;
    placement.offset_y = offset.y;
    positioning::override_primary_placement(theme, placement);
}

/// Resolve the actual rectangle that will be restored, including authored
/// placement or a saved drag offset. The watchdog must test this same target.
fn restored_dock_rect(state: &AppState, taskbar: HWND, taskbar_rect: RECT) -> Option<RECT> {
    let theme = theme_with_placement(state, false)?;
    let surface = theme.surfaces.first()?;
    if surface
        .placement
        .nest
        .resolve(surface.placement.reference.region)
        != SurfaceNest::Taskbar
    {
        return None;
    }
    let displays = native_interop::find_monitors();
    let display = displays
        .get(surface.placement.reference.display)
        .or_else(|| displays.first())?;
    if native_interop::taskbar_for_monitor(
        &native_interop::find_taskbars(),
        &displays,
        display.handle,
    )
    .map(|candidate| candidate.hwnd)
        != Some(taskbar)
    {
        return None;
    }
    let runtime = theme_runtime_for_surface(&theme, 0, theme_runtime_from_state(state));
    let scale = monitor_scale(*display);
    let frame = positioning::widget_frame(&theme, state.data.as_ref(), runtime, scale);
    let offsets = theme_engine::resolve_surface_placement(&theme, 0, state.data.as_ref(), runtime);
    let mut placement = surface.placement.clone();
    placement.offset_x = offsets.offset_x;
    placement.offset_y = offsets.offset_y;
    let tray = native_interop::find_child_window(taskbar, "TrayNotifyWnd")
        .and_then(native_interop::get_window_rect_safe);
    Some(positioning::surface_screen_rect(
        &placement,
        frame.width,
        frame.height,
        scale,
        display.rect,
        Some(taskbar_rect),
        tray,
    ))
}

fn theme_has_floating_surface(theme: &ThemeDocument) -> bool {
    theme.surfaces.iter().any(|surface| {
        surface
            .placement
            .nest
            .resolve(surface.placement.reference.region)
            == SurfaceNest::Floating
    })
}

fn window_state_timer_required(state: &AppState) -> bool {
    state.custom_theme_enabled
        && effective_theme_from_state(state)
            .as_ref()
            .is_some_and(theme_has_floating_surface)
}

fn sync_window_state_timer(hwnd: HWND) {
    let required = lock_state()
        .as_ref()
        .is_some_and(window_state_timer_required);
    set_window_state_timer(hwnd, required);
}

fn set_window_state_timer(hwnd: HWND, required: bool) {
    {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        if required == state.window_state_timer_active {
            return;
        }
        state.window_state_timer_active = required;
    }
    // SAFETY: set_controller_timer/kill_controller_timer forward to SetTimer/KillTimer
    // with no TIMERPROC, so no callback pointer must stay valid; the HWND is
    // passed by value and Windows rejects a stale one.
    unsafe {
        if required {
            set_controller_timer(
                Some(hwnd),
                TIMER_WINDOW_STATE,
                WINDOW_STATE_INTERVAL_MS,
                None,
            );
        } else {
            let _ = kill_controller_timer(Some(hwnd), TIMER_WINDOW_STATE);
        }
    }
}

fn save_state_settings() {
    let state = lock_state();
    if let Some(s) = state.as_ref() {
        let mut persisted = load_settings();
        persisted.tray_offset = s.tray_offset;
        persisted.taskbar_index = s.taskbar_index;
        persisted.legacy_placement_pending = false;
        persisted.widget_visible = true;
        persisted.legacy_visibility_pending = false;
        persisted.poll_interval_ms = s.poll_interval_ms;
        persisted.language = s
            .language_override
            .map(|language| language.code().to_string());
        persisted.last_update_check_unix = s.last_update_check_unix;
        persisted.set_enabled_providers(s.providers);
        // Pool flags are only meaningful while Antigravity is enabled; persisting
        // them as on for a disabled provider would re-enable it on the next load.
        let antigravity_on = s.providers.contains(ProviderId::Antigravity);
        persisted.show_antigravity_gemini = Some(antigravity_on && s.show_antigravity_gemini);
        persisted.show_antigravity_external = Some(antigravity_on && s.show_antigravity_external);
        persisted.custom_theme_enabled = s.custom_theme_enabled;
        persisted.active_theme_path = s
            .active_theme_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());
        persisted.placement_override = s.placement_override.clone();
        persisted.floating_card_opacity = s.floating_card_opacity;
        persisted.lock_taskbar = s.lock_taskbar;
        // The dashboard process owns its dimensions, so leave the freshly
        // loaded values unchanged when monitor actions persist settings.
        if let Err(error) = save_settings(&persisted) {
            diagnose::log(format!("unable to save settings: {error}"));
        }
    }
}

fn save_settings_or_log(settings: &SettingsFile, context: &str) {
    if let Err(error) = save_settings(settings) {
        diagnose::log(format!("{context}: {error}"));
    }
}

pub(super) fn toggle_taskbar_lock(hwnd: HWND) {
    let enabled = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        state.lock_taskbar = !state.lock_taskbar;
        state.lock_taskbar
    };
    save_state_settings();
    if enabled {
        redock_locked_widget(hwnd);
    }
    diagnose::log(format!("taskbar lock toggled: {enabled}"));
}

fn taskbar_created_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer that stays alive for the
    // RegisterWindowMessageW call, which only reads it.
    *MESSAGE.get_or_init(|| unsafe {
        let name = native_interop::wide_str("TaskbarCreated");
        RegisterWindowMessageW(PCWSTR::from_raw(name.as_ptr()))
    })
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn set_window_title(hwnd: HWND, strings: Strings) {
    // SAFETY: `title` is a NUL-terminated UTF-16 buffer that outlives SetWindowTextW;
    // the HWND is passed by value and Windows validates it.
    unsafe {
        let title = native_interop::wide_str(strings.window_title);
        let _ = SetWindowTextW(hwnd, PCWSTR::from_raw(title.as_ptr()));
    }
}

fn show_info_message(hwnd: HWND, title: &str, message: &str) {
    // SAFETY: both strings are NUL-terminated UTF-16 buffers kept alive by locals until
    // MessageBoxW returns; the owner HWND is only used by value.
    unsafe {
        let title_wide = native_interop::wide_str(title);
        let message_wide = native_interop::wide_str(message);
        let _ = MessageBoxW(
            Some(hwnd),
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn show_error_message(hwnd: HWND, title: &str, message: &str) {
    // SAFETY: both strings are NUL-terminated UTF-16 buffers kept alive by locals until
    // MessageBoxW returns; the owner HWND is only used by value.
    unsafe {
        let title_wide = native_interop::wide_str(title);
        let message_wide = native_interop::wide_str(message);
        let _ = MessageBoxW(
            Some(hwnd),
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn apply_language_to_state(state: &mut AppState, language_override: Option<LanguageId>) {
    state.language_override = language_override;
    state.language = localization::resolve_language(language_override);
    set_window_title(state.hwnd.to_hwnd(), state.language.strings());
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

fn total_widget_width_for_state(state: &AppState) -> i32 {
    widget_frame_for_state(state, None).width
}

fn widget_frame_for_state(state: &AppState, nest: Option<SurfaceNest>) -> positioning::WidgetFrame {
    effective_theme_from_state(state).as_ref().map_or(
        positioning::WidgetFrame {
            width: 1,
            height: 1,
            content_width: 1,
            inset: 0,
        },
        |theme| {
            let runtime = theme_runtime_for_surface(theme, 0, theme_runtime_from_state(state));
            let mut runtime = nest.map_or(runtime, |nest| runtime.with_nest(nest));
            if nest == Some(SurfaceNest::Floating) {
                if let Some(host) = floating_host_for_theme(theme, None) {
                    runtime = runtime.with_host_dimensions(host.width, host.height);
                }
            }
            let scale = theme_surface_scale(theme, 0);
            positioning::widget_frame(theme, state.data.as_ref(), runtime, scale)
        },
    )
}

fn floating_frame_for_state(
    state: &AppState,
    display_index: usize,
    scale: f64,
) -> Option<positioning::WidgetFrame> {
    let mut theme = effective_theme_from_state(state)?;
    let mut placement = positioning::floating_placement(display_index);
    placement.host_dimensions =
        floating_host_for_theme(&theme, None).map(|host| (host.width, host.height));
    positioning::override_primary_placement(&mut theme, placement);
    let runtime = theme_runtime_for_surface(&theme, 0, theme_runtime_from_state(state));
    Some(positioning::widget_frame(
        &theme,
        state.data.as_ref(),
        runtime,
        scale,
    ))
}

fn clear_auto_ejection(state: &mut AppState) {
    state.auto_ejected = false;
    state.auto_ejected_for_capacity = false;
    state.auto_ejected_origin = None;
    state.auto_ejected_host = None;
}

fn replace_active_theme(state: &mut AppState, loaded: ThemeDocument, path: Option<PathBuf>) {
    // A fallback belongs to the configured root that was ejected. Preserve it
    // during ordinary refreshes, but never carry it into a new theme or host.
    let changed = state.active_theme.as_ref().is_none_or(|current| {
        current.id != loaded.id
            || current.surfaces.first().map(|s| (&s.id, &s.placement))
                != loaded.surfaces.first().map(|s| (&s.id, &s.placement))
    }) || path
        .as_ref()
        .is_some_and(|p| state.active_theme_path.as_ref() != Some(p));
    if changed {
        clear_auto_ejection(state);
    }
    state.active_theme = Some(loaded);
    if path.is_some() {
        state.active_theme_path = path;
    }
}

fn update_configured_placement(
    state: &mut AppState,
    placement: Option<PlacementOverride>,
    taskbar_index: usize,
    tray_offset: i32,
) {
    if state.placement_override != placement
        || state.taskbar_index != taskbar_index
        || state.tray_offset != tray_offset
    {
        clear_auto_ejection(state);
    }
    state.placement_override = placement;
    state.taskbar_index = taskbar_index;
    state.tray_offset = tray_offset;
}

fn apply_custom_theme(
    hwnd: HWND,
    _enabled: bool,
    path: Option<PathBuf>,
    document: Option<ThemeDocument>,
) -> Result<(), String> {
    let loaded = match (document, path.as_deref()) {
        (Some(document), _) => Some(document),
        (None, Some(path)) => Some(theme_engine::load_theme(path)?),
        (None, None) => lock_state()
            .as_ref()
            .and_then(|state| state.active_theme.clone()),
    };
    let loaded = loaded.unwrap_or_else(ThemeDocument::starter);
    let theme_clock_interval = loaded.current_time_refresh_interval();
    let tray_theme_uses_current_time = theme_tray_uses_current_time(&loaded);
    let old_hook = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return Err("Application is not ready".into());
        };
        state.custom_theme_enabled = true;
        replace_active_theme(state, loaded, path);
        state.theme_clock_interval = theme_clock_interval;
        state.tray_theme_uses_current_time = tray_theme_uses_current_time;
        state.mouse_action_overrides.clear();
        state.hovered_mouse_layer = None;
        state.pending_mouse_click = None;
        state.suppress_next_left_up = false;
        state.embedded = false;
        state.win_event_hook.take()
    };
    if let Some(hook) = old_hook {
        native_interop::unhook_win_event(hook.to_hook());
    }
    // SAFETY: SetWindowPos takes the primary surface HWND and the HWND_NOTOPMOST
    // sentinel by value, with no pointers; Windows validates the handle.
    unsafe {
        native_interop::make_popup(hwnd, false);
        ensure_layered_window(hwnd);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
    sync_surface_windows();
    sync_window_state_timer(hwnd);
    schedule_countdown_timer();
    schedule_clock_timer();
    Ok(())
}

fn total_widget_height_for_state(state: &AppState) -> i32 {
    widget_frame_for_state(state, None).height
}

fn total_widget_height() -> i32 {
    lock_state()
        .as_ref()
        .map(total_widget_height_for_state)
        .unwrap_or(1)
}

fn total_widget_width() -> i32 {
    lock_state()
        .as_ref()
        .map(total_widget_width_for_state)
        .unwrap_or(1)
}

pub fn run() {
    let run_args: Vec<String> = std::env::args().collect();
    let open_dashboard_on_start = run_args.iter().any(|argument| argument == "--dashboard");
    let allow_multiple = run_args
        .iter()
        .any(|argument| argument == "--allow-multiple");
    let no_poll = run_args.iter().any(|argument| argument == "--no-poll");
    // SAFETY: both calls take only values: a predefined DPI awareness context for this
    // thread, and no arguments for GetDpiForSystem.
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CURRENT_DPI.store(GetDpiForSystem(), Ordering::Relaxed);
    }
    diagnose::log("window::run started");

    // Single-instance guard: silently exit if another instance is running in this session.
    // Use the local namespace so other users' desktop/RDP sessions remain independent.
    // Exception: when relaunched after a UI hang (ENV_RELAUNCH set),
    // wait for the previous instance to release the mutex, then take over.
    let is_relaunch = std::env::var(ENV_RELAUNCH).is_ok();
    let mutex_name = native_interop::wide_str(&if allow_multiple {
        format!("Local\\ClaudeCodeUsageMonitor-{}", std::process::id())
    } else {
        "Local\\ClaudeCodeUsageMonitor".to_string()
    });
    // SAFETY: `mutex_name` is a NUL-terminated buffer that outlives CreateMutexW;
    // GetLastError is read immediately after it, and WaitForSingleObject is
    // given the mutex handle just returned, which stays open for the process.
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
                        if open_dashboard_on_start {
                            if let Err(error) = crate::dashboard::request_from_existing_monitor() {
                                crate::dashboard::report_launch_failure(HWND::default(), &error);
                            }
                        }
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

    tray_icon::cleanup_legacy_entries();
    upgrade_legacy_startup_command();
    let class_name = native_interop::wide_str("ClaudeCodeUsageMonitor");

    // SAFETY: this is the UI thread for the rest of the process. Class and title
    // buffers are NUL-terminated locals that outlive the calls, `wc.cbSize` is
    // set, and both registered procedures have the WNDPROC signature. Windows
    // created here are destroyed only on this thread; WM_SETICON passes icon
    // handles by value; timers install no TIMERPROC; and `msg` is a live local
    // for GetMessageW, TranslateMessage and DispatchMessageW.
    unsafe {
        let hinstance =
            GetModuleHandleW(PCWSTR::null()).expect("the running executable has a module handle");
        let (large_icon, small_icon) = tray_icon::load_app_icons();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(lifecycle::controller_wnd_proc),
            hInstance: HINSTANCE(hinstance.0),
            hIcon: large_icon,
            hIconSm: small_icon,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };

        let atom = RegisterClassExW(&wc);
        if atom == 0 {
            diagnose::log("RegisterClassExW returned 0");
        }

        let mut settings = load_settings();
        let classic_theme_path = theme_engine::ensure_starter_theme().ok();
        let mut configured_theme_path = settings.active_theme_path.as_deref().map(PathBuf::from);
        let mut configured_theme = configured_theme_path
            .as_deref()
            .and_then(|path| theme_engine::load_theme(path).ok())
            .filter(|theme| !theme.is_obsolete_studio_starter());
        let legacy_placement = settings.legacy_placement();
        let legacy_visibility = settings.legacy_widget_visibility();
        if legacy_placement.is_some() || legacy_visibility.is_some() {
            if configured_theme
                .as_ref()
                .is_some_and(|theme| !theme.is_builtin_classic())
            {
                // A user-selected writable theme already owns its presentation.
                // Consume the obsolete settings without replacing that theme.
                settings.consume_legacy_placement();
                settings.consume_legacy_widget_visibility();
                save_settings_or_log(&settings, "unable to consume legacy settings");
            } else if legacy_placement.is_some() || legacy_visibility == Some(false) {
                let placement = legacy_placement.map(migrated_theme_placement);
                let migrated = ThemeDocument::migrated_from_legacy(
                    placement,
                    legacy_visibility.unwrap_or(true),
                );
                match theme_engine::save_theme(&migrated) {
                    Ok(path) => {
                        configured_theme_path = Some(path.clone());
                        configured_theme = Some(migrated);
                        settings.active_theme_path = Some(path.to_string_lossy().into_owned());
                        settings.custom_theme_enabled = true;
                        settings.consume_legacy_placement();
                        settings.consume_legacy_widget_visibility();
                        if let Err(error) = save_settings(&settings) {
                            diagnose::log(format!(
                                "migrated theme created but settings cleanup failed: {error}"
                            ));
                        } else {
                            diagnose::log(
                                "legacy placement and visibility migrated to Migrated Theme",
                            );
                        }
                    }
                    Err(error) => diagnose::log(format!(
                        "legacy theme migration deferred because the copied theme could not be saved: {error}"
                    )),
                }
            } else {
                // An explicitly visible v1.4.9 widget already matches the
                // built-in theme's Render value, so no copy is necessary.
                settings.consume_legacy_widget_visibility();
                save_settings_or_log(&settings, "unable to consume legacy visibility");
            }
        }
        let (active_theme_path, active_theme) = configured_theme
            .map(|theme| (configured_theme_path, Some(theme)))
            .unwrap_or_else(|| {
                let path = classic_theme_path;
                let theme = path
                    .as_deref()
                    .and_then(|path| theme_engine::load_theme(path).ok())
                    .or_else(|| Some(ThemeDocument::starter()));
                (path, theme)
            });
        let custom_theme_enabled = true;
        let theme_clock_interval = active_theme
            .as_ref()
            .and_then(ThemeDocument::current_time_refresh_interval);
        let tray_theme_uses_current_time = active_theme
            .as_ref()
            .is_some_and(theme_tray_uses_current_time);
        if let Some(path) = &active_theme_path {
            let path = path.to_string_lossy().into_owned();
            if settings.active_theme_path.as_deref() != Some(path.as_str())
                || !settings.custom_theme_enabled
            {
                settings.active_theme_path = Some(path);
                settings.custom_theme_enabled = true;
                save_settings_or_log(&settings, "unable to persist active theme");
            }
        }
        let language_override = settings.language.as_deref().and_then(LanguageId::from_code);
        let language = localization::resolve_language(language_override);
        let install_channel = updater::current_install_channel();

        refresh_theme_host_geometry();

        // Hidden top-level lifetime owner: receives shell broadcasts and is
        // never parented into Explorer. The public class stays stable for IPC.
        let title = native_interop::wide_str(language.strings().window_title);
        let controller = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(HINSTANCE(hinstance.0)),
            None,
        )
        .expect("create application controller");
        CONTROLLER_HWND.store(SendHwnd::from_hwnd(controller).0, Ordering::Relaxed);
        let hwnd = lifecycle::create_primary_surface();
        if hwnd.is_invalid() {
            let _ = DestroyWindow(controller);
            return;
        }

        if !large_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(large_icon.0 as isize)),
            );
        }
        if !small_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(small_icon.0 as isize)),
            );
        }

        diagnose::log(format!("main window created hwnd={:?}", hwnd));
        diagnose::log(format!("controller window created hwnd={controller:?}"));

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
                providers: settings.enabled_providers(),
                show_antigravity_gemini: settings.antigravity_gemini_enabled(),
                show_antigravity_external: settings.antigravity_external_enabled(),
                crossfade_opacity: 1.0,
                accounts: settings.accounts.clone(),
                data: None,
                poll_interval_ms: settings.poll_interval_ms,
                retry_count: 0,
                force_notify_auth_error: false,
                auth_error_paused_polling: false,
                auth_watch_mode: poller::CredentialWatchMode::ActiveSource(
                    settings.enabled_providers().first().unwrap_or_default(),
                ),
                auth_watch_snapshot: Vec::new(),
                last_poll_ok: false,
                last_poll_failure: None,
                update_status: UpdateStatus::Idle,
                last_update_check_unix: settings.last_update_check_unix,
                taskbar_index: settings.taskbar_index,
                tray_offset: settings.tray_offset,
                dragging: false,
                pending_drag: false,
                drag_start_cursor: POINT::default(),
                drag_start_origin: POINT::default(),
                drag_start_client_x: 0,
                auto_ejected: false,
                auto_ejected_for_capacity: false,
                auto_ejected_origin: None,
                auto_ejected_host: None,
                lock_taskbar: settings.lock_taskbar,
                is_switching_window_style: false,
                is_snapped: false,
                placement_override: settings.placement_override.clone(),
                floating_card_opacity: settings.floating_card_opacity,
                window_state_timer_active: false,
                custom_theme_enabled,
                usage_countdown: settings.usage_countdown,
                use_windows_default_font: settings.use_windows_default_font,
                active_theme_path,
                active_theme,
                theme_clock_interval,
                tray_theme_uses_current_time,
                mirror_hwnds: Vec::new(),
                desktop_hwnds: Vec::new(),
                mouse_action_overrides: HashMap::new(),
                hovered_mouse_layer: None,
                pending_mouse_click: None,
                suppress_next_left_up: false,
            });
        }

        if let Some(state) = lock_state().as_ref() {
            publish_update_status(state);
        }
        if let Err(error) = crate::dashboard::start_request_listener(controller) {
            diagnose::log_error("dashboard request listener failed", error);
        }

        sync_surface_windows();
        native_interop::make_popup(hwnd, false);

        // Register the persistent application tray icon.
        if !no_poll {
            sync_tray_icon(hwnd);
        }

        // Theme surfaces decide whether their windows render.
        position_at_taskbar();
        diagnose::log("window shown");

        // Initial render using the presenter selected by the surface nest.
        schedule_countdown_timer();
        schedule_clock_timer();
        let startup_dual_mode = {
            let state = lock_state();
            state.as_ref().is_some_and(|s| {
                s.show_antigravity_gemini
                    && s.show_antigravity_external
                    && s.providers.contains(ProviderId::Antigravity)
            })
        };
        if startup_dual_mode {
            arm_crossfade_schedule(hwnd);
        }
        render_layered();

        if open_dashboard_on_start {
            crate::dashboard::show(controller_for(hwnd));
        }

        // Poll timer: 15 minutes
        let initial_poll_ms = {
            let state = lock_state();
            state
                .as_ref()
                .map(|s| s.poll_interval_ms)
                .unwrap_or(POLL_15_MIN)
        };
        set_controller_timer(Some(hwnd), TIMER_POLL, initial_poll_ms, None);
        sync_window_state_timer(hwnd);

        // Sample collision geometry off-thread and ask the stable controller to
        // reconcile shell-hosted windows every two seconds.
        taskbar_collision::spawn_reader();
        spawn_taskbar_watchdog();
        spawn_ui_hang_watchdog();

        // Initial poll
        if !no_poll {
            diagnose::log("initial poll requested");
            request_poll(hwnd);
        }

        if !no_poll {
            schedule_auto_update_check(hwnd);
        }
        let should_check_updates = {
            let state = lock_state();
            state
                .as_ref()
                .map(|s| auto_update_check_due(s.last_update_check_unix))
                .unwrap_or(false)
        };
        if should_check_updates && !no_poll {
            begin_update_check(hwnd, false);
        }

        // Initial theme check
        check_theme_change();

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Render every theme surface, then dispatch it to the presenter selected by
/// its nest: DirectComposition for desktop and layered windows elsewhere.
fn render_layered() {
    refresh_dpi();
    sync_surface_windows();
    let (hwnd_val, active_theme, usage_data, runtime, mirror_hwnds, desktop_hwnds) = {
        let state = lock_state();
        let Some(state) = state.as_ref() else {
            return;
        };
        (
            state.hwnd,
            effective_theme_from_state(state),
            state.data.clone(),
            theme_runtime_from_state(state),
            state.mirror_hwnds.clone(),
            state.desktop_hwnds.clone(),
        )
    };

    // Theme rendering is the widget renderer. Startup and theme changes always
    // install Classic in memory when a selected theme cannot be loaded.
    let theme = active_theme.unwrap_or_else(ThemeDocument::starter);
    let hwnd = hwnd_val.to_hwnd();
    set_window_state_timer(hwnd, theme_has_floating_surface(&theme));
    let target_count = theme.surfaces.len();
    for surface_index in 0..target_count {
        let regular_hwnd = if surface_index == 0 {
            hwnd
        } else if let Some(mirror) = mirror_hwnds.get(surface_index - 1) {
            mirror.to_hwnd()
        } else {
            continue;
        };
        let surface = &theme.surfaces[surface_index];
        let surface_runtime = theme_runtime_for_surface(&theme, surface_index, runtime);
        let nest = surface
            .placement
            .nest
            .resolve(surface.placement.reference.region);
        let desktop_nested = nest == SurfaceNest::Desktop;
        let target_hwnd = if desktop_nested {
            // SAFETY: ShowWindow takes the surface HWND by value with no pointers; Windows
            // validates the handle.
            unsafe {
                let _ = ShowWindow(regular_hwnd, SW_HIDE);
            }
            desktop_hwnds
                .get(surface_index)
                .and_then(|window| *window)
                .map(SendHwnd::to_hwnd)
                .unwrap_or_default()
        } else {
            regular_hwnd
        };
        if target_hwnd.is_invalid() {
            continue;
        }
        if nest == SurfaceNest::TrayIcon {
            // SAFETY: ShowWindow takes the surface HWND by value with no pointers; Windows
            // validates the handle.
            unsafe {
                let _ = ShowWindow(target_hwnd, SW_HIDE);
            }
            continue;
        }
        if !theme_engine::surface_should_render(
            &theme,
            surface_index,
            usage_data.as_ref(),
            surface_runtime,
        ) {
            // SAFETY: ShowWindow takes the surface HWND by value with no pointers; Windows
            // validates the handle.
            unsafe {
                let _ = ShowWindow(target_hwnd, SW_HIDE);
            }
            continue;
        }

        let scale = theme_surface_scale(&theme, surface_index);
        let rendered = theme_engine::render_theme_surface_with_runtime_at_scale(
            &theme,
            surface_index,
            usage_data.as_ref(),
            surface_runtime,
            scale,
        );
        let mut positioned = theme_for_surface(&theme, surface_index);
        let (logical_width, logical_height) = theme_engine::resolve_surface_size(
            &theme,
            surface_index,
            usage_data.as_ref(),
            surface_runtime,
        );
        positioned.canvas.width = logical_width;
        positioned.canvas.height = logical_height;
        let placement = theme_engine::resolve_surface_placement(
            &theme,
            surface_index,
            usage_data.as_ref(),
            surface_runtime,
        );
        positioned.placement.offset_x = placement.offset_x;
        positioned.placement.offset_y = placement.offset_y;
        let hosted = position_custom_theme(target_hwnd, &positioned, scale);
        if !hosted {
            continue;
        }
        if desktop_nested {
            // SAFETY: ShowWindow takes the surface HWND by value with no pointers; Windows
            // validates the handle.
            unsafe {
                let _ = ShowWindow(target_hwnd, SW_SHOWNOACTIVATE);
            }
        }
        render_custom_window(target_hwnd, &rendered, desktop_nested);
        // SAFETY: ShowWindow takes the surface HWND by value with no pointers; Windows
        // validates the handle.
        unsafe {
            let show = nest != SurfaceNest::Floating
                || !foreground_is_fullscreen_on_display(positioned.placement.reference.display);
            let _ = ShowWindow(target_hwnd, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    for target in std::iter::once(hwnd)
        .chain(mirror_hwnds.iter().map(|mirror| mirror.to_hwnd()))
        .skip(target_count)
    {
        // SAFETY: ShowWindow takes the surface HWND by value with no pointers; Windows
        // validates the handle.
        unsafe {
            let _ = ShowWindow(target, SW_HIDE);
        }
    }
}
fn theme_for_surface(theme: &ThemeDocument, surface_index: usize) -> ThemeDocument {
    let mut result = theme.clone();
    if let Some(surface) = theme.surfaces.get(surface_index) {
        result.canvas.width_expression = Some(surface.width.clone());
        result.canvas.height_expression = Some(surface.height.clone());
        result.canvas.background = match &surface.background {
            crate::theme_engine::LayerBackground::Colour { colour } => colour.clone(),
            crate::theme_engine::LayerBackground::None
            | crate::theme_engine::LayerBackground::Gradient { .. }
            | crate::theme_engine::LayerBackground::Image { .. } => Default::default(),
        };
        result.placement = surface.placement.clone();
        result.children = surface.children.clone();
    }
    result
}

fn schedule_countdown_timer() {
    let state = lock_state();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return,
    };

    let hwnd = s.hwnd.to_hwnd();
    if !s.last_poll_ok {
        // SAFETY: kill_controller_timer forwards to KillTimer, which takes the HWND by
        // value and no pointers; Windows validates the handle.
        unsafe {
            let _ = kill_controller_timer(Some(hwnd), TIMER_COUNTDOWN);
            let _ = kill_controller_timer(Some(hwnd), TIMER_RESET_POLL);
        }
        return;
    }

    // If a reset time has passed, poll every 5s to pick up fresh data
    if s.retry_count == 0 && s.data.as_ref().is_some_and(poller::app_is_past_reset) {
        // SAFETY: set_controller_timer forwards to SetTimer with no TIMERPROC, so no
        // callback pointer must stay valid; the HWND is passed by value.
        unsafe {
            set_controller_timer(Some(hwnd), TIMER_RESET_POLL, 5_000, None);
        }
    }

    let min_delay = s.data.as_ref().and_then(|data| {
        data.all_usage()
            .flat_map(|usage| usage.sections())
            .filter_map(|section| poller::time_until_display_change(section.resets_at))
            .min()
    });

    let ms = min_delay
        .unwrap_or(Duration::from_secs(60))
        .as_millis()
        .max(1000) as u32;

    // SAFETY: set_controller_timer forwards to SetTimer with no TIMERPROC, so no
    // callback pointer must stay valid; the HWND is passed by value.
    unsafe {
        set_controller_timer(Some(hwnd), TIMER_COUNTDOWN, ms, None);
    }
}

fn schedule_clock_timer() {
    let state = lock_state();
    let Some(s) = state.as_ref() else {
        return;
    };
    let hwnd = s.hwnd.to_hwnd();
    let Some(interval) = s.theme_clock_interval else {
        // SAFETY: kill_controller_timer forwards to KillTimer, which takes the HWND by
        // value and no pointers; Windows validates the handle.
        unsafe {
            let _ = kill_controller_timer(Some(hwnd), TIMER_CLOCK);
        }
        return;
    };
    let ms = time_until_next_clock_refresh(interval).as_millis().max(1) as u32;
    // SAFETY: set_controller_timer forwards to SetTimer with no TIMERPROC, so no
    // callback pointer must stay valid; the HWND is passed by value.
    unsafe {
        set_controller_timer(Some(hwnd), TIMER_CLOCK, ms, None);
    }
}

fn time_until_next_clock_refresh(interval: Duration) -> Duration {
    let interval_ms = interval.as_millis().max(1);
    let elapsed_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let remaining_ms = interval_ms - elapsed_ms % interval_ms;
    Duration::from_millis(remaining_ms as u64)
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

/// All widget surfaces use the same wall-clock phase, including after a
/// settings change or a delayed timer following sleep.
fn antigravity_crossfade_frame(now_ms: u64) -> (f64, u32) {
    let phase = now_ms % 20_000;
    let smooth = |elapsed: u64| {
        let t = elapsed as f64 / 2_000.0;
        t * t * (3.0 - 2.0 * t)
    };
    match phase {
        0..8_000 => (1.0, (8_000 - phase) as u32),
        8_000..10_000 => (1.0 - smooth(phase - 8_000), 30),
        10_000..18_000 => (0.0, (18_000 - phase) as u32),
        _ => (smooth(phase - 18_000), 30),
    }
}

pub(super) fn arm_crossfade_schedule(hwnd: HWND) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let (opacity, delay_ms) = antigravity_crossfade_frame(now_ms);
    let mut state = lock_state();
    let dual_mode = state.as_ref().is_some_and(|state| {
        state.show_antigravity_gemini
            && state.show_antigravity_external
            && state.providers.contains(ProviderId::Antigravity)
    });
    if let Some(state) = state.as_mut() {
        state.crossfade_opacity = if dual_mode { opacity } else { 1.0 };
    }
    // SAFETY: the timer helpers forward to SetTimer/KillTimer with no TIMERPROC, so no
    // callback pointer must stay valid; the HWND is passed by value.
    unsafe {
        if dual_mode {
            let _ = set_controller_timer(Some(hwnd), TIMER_CROSSFADE_SCHEDULE, delay_ms, None);
        } else {
            let _ = kill_controller_timer(Some(hwnd), TIMER_CROSSFADE_SCHEDULE);
        }
    }
}

fn reload_external_settings(hwnd: HWND) {
    let settings = load_settings();
    let language_override = settings.language.as_deref().and_then(LanguageId::from_code);
    let theme_path = settings.active_theme_path.as_ref().map(PathBuf::from);
    let providers_changed;
    let lock_enabled;
    let antigravity_changed;
    let dual_mode;
    {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        providers_changed =
            state.providers != settings.enabled_providers() || state.accounts != settings.accounts;
        let gemini_enabled = settings.antigravity_gemini_enabled();
        let external_enabled = settings.antigravity_external_enabled();
        antigravity_changed = state.show_antigravity_gemini != gemini_enabled
            || state.show_antigravity_external != external_enabled;
        state.show_antigravity_gemini = gemini_enabled;
        state.show_antigravity_external = external_enabled;
        state.accounts = settings.accounts.clone();
        if let Some(data) = state.data.as_mut() {
            data.select_accounts(&settings.accounts);
        }
        state.poll_interval_ms = settings.poll_interval_ms;
        state.providers = settings.enabled_providers();
        state.usage_countdown = settings.usage_countdown;
        state.use_windows_default_font = settings.use_windows_default_font;
        dual_mode = state.show_antigravity_gemini
            && state.show_antigravity_external
            && state.providers.contains(ProviderId::Antigravity);
        if !dual_mode {
            // SAFETY: kill_controller_timer forwards to KillTimer, which takes the HWND by
            // value and no pointers; Windows validates the handle.
            unsafe {
                let _ = kill_controller_timer(Some(hwnd), TIMER_CROSSFADE_SCHEDULE);
            }
            state.crossfade_opacity = 1.0;
        }
        update_configured_placement(
            state,
            settings.placement_override,
            settings.taskbar_index,
            settings.tray_offset,
        );
        state.floating_card_opacity = settings.floating_card_opacity;
        state.lock_taskbar = settings.lock_taskbar;
        lock_enabled = state.lock_taskbar;
        apply_language_to_state(state, language_override);
    }
    if dual_mode && (antigravity_changed || providers_changed) {
        arm_crossfade_schedule(hwnd);
    }
    // SAFETY: set_controller_timer forwards to SetTimer with no TIMERPROC, so no
    // callback pointer must stay valid; the HWND is passed by value.
    unsafe {
        set_controller_timer(Some(hwnd), TIMER_POLL, settings.poll_interval_ms, None);
    }
    let _ = apply_custom_theme(hwnd, settings.custom_theme_enabled, theme_path, None);
    if lock_enabled {
        // apply_custom_theme leaves the window as a popup; re-embed a
        // previously ejected widget so the lock takes effect at once.
        redock_locked_widget(hwnd);
    }
    if providers_changed {
        request_poll(hwnd);
    }
    sync_tray_icon(hwnd);
    position_at_taskbar();
    render_layered();
}

/// Use the configured host, not the transient popup created by a drag or
/// auto-ejection. An intentional floating placement takes precedence over the
/// theme's authored host.
fn taskbar_lock_applies(state: &AppState) -> bool {
    if !state.lock_taskbar {
        return false;
    }
    match state.placement_override.as_ref().map(|p| p.nest.as_str()) {
        Some("taskbar") => true,
        Some("floating") => false,
        _ => state.active_theme.as_ref().is_some_and(|theme| {
            theme.surfaces.first().is_some_and(|surface| {
                surface
                    .placement
                    .nest
                    .resolve(surface.placement.reference.region)
                    == SurfaceNest::Taskbar
            })
        }),
    }
}

/// A locked taskbar widget dragged off the taskbar turns the lock off, so the
/// drop lands as an ordinary floating widget. A widget whose placement the
/// lock does not cover (already floating) keeps the user's preference.
/// Returns whether the lock was released.
fn release_taskbar_lock_for_desktop_drop(state: &mut AppState) -> bool {
    if !taskbar_lock_applies(state) {
        return false;
    }
    state.lock_taskbar = false;
    true
}

fn clear_locked_auto_ejection(state: &mut AppState) -> bool {
    if !taskbar_lock_applies(state) || !state.auto_ejected {
        return false;
    }
    clear_auto_ejection(state);
    true
}

/// Restore a locked taskbar widget after auto-ejection. Intentional floating,
/// desktop and tray hosts retain their placement.
fn redock_locked_widget(hwnd: HWND) {
    let taskbar_hwnd = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        // Locking suppresses collisions with app buttons, but must not restore
        // a mostly hidden layout into the parent. Use the same rule as ejection.
        if let Some(taskbar) = state.taskbar_hwnd.map(SendHwnd::to_hwnd) {
            let Some(bounds) = native_interop::get_taskbar_rect(taskbar) else {
                return;
            };
            let Some(target) = restored_dock_rect(state, taskbar, bounds) else {
                return;
            };
            if !taskbar_collision::taskbar_shown(bounds)
                || positioning::taskbar_ejection_required(bounds, target, false, true)
            {
                return;
            }
        }
        if !clear_locked_auto_ejection(state) {
            return;
        }
        state.is_switching_window_style = true;
        state.taskbar_hwnd.map(|h| h.to_hwnd())
    };
    // Re-embed first so position_at_taskbar sees a docked window.
    if let Some(taskbar) = taskbar_hwnd {
        native_interop::embed_as_child(hwnd, taskbar);
        diagnose::log("taskbar lock enabled: re-docked floating widget");
    }
    if let Some(state) = lock_state().as_mut() {
        state.is_switching_window_style = false;
    }
    position_at_taskbar();
    render_layered();
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

mod lifecycle;
use lifecycle::{controller_for, kill_controller_timer, set_controller_timer, WM_APP_RECONCILE};
mod host_geometry;
mod message_loop;
use host_geometry::*;
use message_loop::wnd_proc;
mod positioning;
mod taskbar_collision;
use positioning::*;
mod mouse;
mod widget_tooltip;
use mouse::*;
mod window_context_menu;
use window_context_menu::*;
mod polling;
mod startup;
mod surfaces;
mod tray_summary;
mod updates;
mod watchdog;
use polling::*;
use startup::*;
pub(crate) use startup::{is_startup_enabled, set_startup_enabled};
use surfaces::*;
use tray_summary::*;
use updates::*;
use watchdog::*;

#[cfg(test)]
mod placement_tests;

#[cfg(test)]
mod layered_window_tests;

#[cfg(test)]
mod language_menu_tests {
    use super::*;

    #[test]
    fn generated_language_menu_commands_round_trip() {
        assert_eq!(language_from_menu_command_id(IDM_LANG_SYSTEM), None);
        for language in LanguageId::ALL {
            assert_eq!(
                language_from_menu_command_id(language_menu_command_id(language)),
                Some(language)
            );
        }
    }
}

#[cfg(test)]
mod poll_display_state_tests {
    use super::*;
    use crate::models::UsageData;

    fn cached_usage(stale: bool) -> AppUsageData {
        let usage = UsageData {
            stale,
            ..Default::default()
        };
        [(ProviderId::Claude, usage)].into_iter().collect()
    }

    #[test]
    fn transient_failure_keeps_a_stale_reading_displayable() {
        let data = cached_usage(true);
        assert_eq!(
            poll_display_state(false, 1, false, Some(&data)),
            (true, false)
        );
    }

    #[test]
    fn failures_without_stale_data_remain_errors() {
        let fresh = cached_usage(false);
        let stale = cached_usage(true);

        assert_eq!(poll_display_state(false, 1, false, None), (false, true));
        assert_eq!(poll_display_state(true, 1, false, None), (true, false));
        assert_eq!(
            poll_display_state(false, 1, false, Some(&fresh)),
            (false, true)
        );
        assert_eq!(
            poll_display_state(false, 1, true, Some(&stale)),
            (false, true)
        );
    }
}

#[cfg(test)]
mod placement_regression_tests;

#[cfg(test)]
mod antigravity_crossfade_tests {
    use super::antigravity_crossfade_frame;

    #[test]
    fn pool_crossfade_follows_the_same_phase_after_startup_sleep_and_rearming() {
        for cycle in [0, 20_000, 180_000] {
            for (phase, opacity, delay) in [
                (0, 1.0, 8_000),
                (7_000, 1.0, 1_000),
                (8_000, 1.0, 30),
                (9_000, 0.5, 30),
                (10_000, 0.0, 8_000),
                (17_000, 0.0, 1_000),
                (18_000, 0.0, 30),
                (19_000, 0.5, 30),
            ] {
                assert_eq!(antigravity_crossfade_frame(cycle + phase), (opacity, delay));
            }
        }
        let before_wrap = antigravity_crossfade_frame(19_999).0;
        assert!((1.0 - before_wrap).abs() < 0.000_001);
    }
}
