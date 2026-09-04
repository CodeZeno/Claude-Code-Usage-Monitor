use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, ReleaseCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app_settings::{
    self, load_settings, save_settings, LegacyPlacement, SettingsFile, POLL_15_MIN,
    POLL_15_MIN_SECONDS, POLL_1_HOUR, POLL_1_HOUR_SECONDS, POLL_1_MIN, POLL_1_MIN_SECONDS,
    POLL_5_MIN, POLL_5_MIN_SECONDS,
};
use crate::context_menu::{self, ContextMenuAction, ContextMenuItem, ContextMenuItemKind};
use crate::diagnose;
use crate::localization::{self, LanguageId, Strings};
use crate::models::AppUsageData;
use crate::native_interop::{
    self, TIMER_CLOCK, TIMER_COUNTDOWN, TIMER_MOUSE_CLICK, TIMER_POLL, TIMER_RESET_POLL,
    TIMER_TRAY_HOVER, TIMER_UPDATE_CHECK, TIMER_WINDOW_STATE, WM_APP_OPEN_DASHBOARD, WM_APP_QUIT,
    WM_APP_REFRESH_NOW, WM_APP_SETTINGS_UPDATED, WM_APP_TRAY, WM_APP_USAGE_UPDATED,
};
use crate::poller;
use crate::providers::{ProviderId, ProviderSet};
use crate::theme;
use crate::theme_engine::{
    self, Canvas, DataContext, HorizontalAnchor, MouseActionEffect, MouseActionOverrideKey,
    MouseEventKind, ReferenceRegion, SurfaceNest, ThemeDocument, ThemeRuntime, VerticalAnchor,
};
use crate::tray_icon;
use crate::updater::{self, InstallChannel, ReleaseDescriptor, UpdateCheckResult};

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
    drag_start_client_x: i32,
    drag_start_offset: i32,

    custom_theme_enabled: bool,
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

#[derive(Clone, Debug)]
enum UpdateStatus {
    Idle,
    Checking,
    Applying,
    UpToDate,
    Available(ReleaseDescriptor),
}

const RETRY_BASE_MS: u32 = 30_000; // 30 seconds

// Menu item IDs for update frequency
const IDM_FREQ_1MIN: u16 = 10;
const IDM_FREQ_5MIN: u16 = 11;
const IDM_FREQ_15MIN: u16 = 12;
const IDM_FREQ_1HOUR: u16 = 13;
const IDM_START_WITH_WINDOWS: u16 = 20;
const IDM_VERSION_ACTION: u16 = 31;
const IDM_LANG_SYSTEM: u16 = 100;
const IDM_LANG_FIRST: u16 = 101;
const IDM_DASHBOARD: u16 = 71;

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
    if !context_menu::supported_url(url) {
        return;
    }
    unsafe {
        let operation = native_interop::wide_str("open");
        let url = native_interop::wide_str(url.trim());
        let result = ShellExecuteW(
            Some(hwnd),
            PCWSTR::from_raw(operation.as_ptr()),
            PCWSTR::from_raw(url.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if result.0 as isize <= 32 {
            diagnose::log(failure_message);
        }
    }
}

/// How often the watchdog thread polls for an explorer.exe restart (which
/// recreates the taskbar and wipes our tray-icon registration).
const TASKBAR_WATCH_INTERVAL_SECS: u64 = 2;

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
    if unsafe { GetDpiForMonitor(display.handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
        .is_ok()
        && dpi_x > 0
    {
        (dpi_x as f64 / 96.0).clamp(0.25, 8.0)
    } else {
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
        .map(|taskbar| unsafe { MonitorFromWindow(taskbar.hwnd, MONITOR_DEFAULTTOPRIMARY) })
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

pub(crate) fn theme_runtime_for_surface(
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
        native_interop::find_taskbars()
            .into_iter()
            .find(|taskbar| unsafe {
                MonitorFromWindow(taskbar.hwnd, MONITOR_DEFAULTTOPRIMARY) == display.handle
            })
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
/// When the shell restarts it destroys our embedded child window outright (the
/// window is gone, not merely orphaned - `IsWindow` returns false) and leaves
/// the UI thread parked in `GetMessage` with no window to recreate in place.
/// Spawning a clean new process - which re-embeds into the freshly created
/// taskbar - and exiting this one is the robust recovery. The child is flagged
/// via `ENV_RELAUNCH` so it waits for this instance's single-instance mutex to
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
/// Explorer owns both taskbar and desktop surface hosts. When it restarts, any
/// child widget windows are destroyed; if the primary window was hosted there,
/// the UI message loop is lost as well. A dedicated thread checks all native
/// surface handles and relaunches after the shell has returned.
fn spawn_taskbar_watchdog() {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(TASKBAR_WATCH_INTERVAL_SECS));
        let (shell_hosted, windows) = {
            let state = lock_state();
            let Some(state) = state.as_ref() else {
                continue;
            };
            let shell_hosted = state.active_theme.as_ref().is_some_and(|theme| {
                theme.surfaces.iter().any(|surface| {
                    matches!(
                        surface
                            .placement
                            .nest
                            .resolve(surface.placement.reference.region),
                        SurfaceNest::Taskbar | SurfaceNest::Desktop
                    )
                })
            });
            (
                shell_hosted,
                std::iter::once(state.hwnd)
                    .chain(state.mirror_hwnds.iter().copied())
                    .chain(state.desktop_hwnds.iter().flatten().copied())
                    .collect::<Vec<_>>(),
            )
        };
        if !shell_hosted {
            continue;
        }
        let invalid = windows
            .iter()
            .any(|window| unsafe { !IsWindow(Some(window.to_hwnd())).as_bool() });
        if invalid && !native_interop::find_taskbars().is_empty() {
            diagnose::log("watchdog: shell-hosted surface was destroyed -> relaunching");
            relaunch_self();
        }
    });
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
    ThemeRuntime::from_providers(state.providers)
        .with_poll_state(poll_ok, has_error)
        .with_language(state.language)
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
    let has_usable_stale_data = !auth_error_paused_polling
        && data.is_some_and(|data| data.iter().any(|(_, usage)| usage.stale));
    (
        last_poll_ok || has_usable_stale_data,
        retry_count > 0 && !has_usable_stale_data,
    )
}

fn effective_theme_from_state(state: &AppState) -> Option<ThemeDocument> {
    state.active_theme.as_ref().map(|theme| {
        theme_engine::apply_mouse_action_overrides(theme, &state.mouse_action_overrides)
    })
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

fn sync_window_state_timer(hwnd: HWND) {
    let required = {
        let state = lock_state();
        state.as_ref().is_some_and(|state| {
            state.custom_theme_enabled
                && state
                    .active_theme
                    .as_ref()
                    .is_some_and(theme_has_floating_surface)
        })
    };
    unsafe {
        if required {
            SetTimer(
                Some(hwnd),
                TIMER_WINDOW_STATE,
                WINDOW_STATE_INTERVAL_MS,
                None,
            );
        } else {
            let _ = KillTimer(Some(hwnd), TIMER_WINDOW_STATE);
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
        persisted.custom_theme_enabled = s.custom_theme_enabled;
        persisted.active_theme_path = s
            .active_theme_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string());
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

fn tray_usage_summary_lines(
    data: &AppUsageData,
    providers: ProviderSet,
    language: LanguageId,
) -> Vec<String> {
    let strings = language.strings();
    providers
        .iter()
        .filter_map(|provider| {
            let usage = data.get(provider)?;
            let descriptor = provider.descriptor();
            let weekly_label = usage
                .weekly_label
                .as_deref()
                .unwrap_or(strings.weekly_window);
            Some(format!(
                "{} {}: {:.0}% | {}: {:.0}%",
                language.text(descriptor.display_name),
                strings.session_window,
                usage.session.percentage,
                weekly_label,
                usage.weekly.percentage,
            ))
        })
        .collect()
}

fn tray_usage_summary_from_state() -> Option<String> {
    let state = lock_state();
    let state = state.as_ref()?;
    if !state.last_poll_ok {
        return None;
    }
    let lines = tray_usage_summary_lines(state.data.as_ref()?, state.providers, state.language);
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn tray_icon_tooltip_from_state() -> String {
    tray_usage_summary_from_state().unwrap_or_else(|| {
        lock_state()
            .as_ref()
            .map(|state| state.language.strings().window_title.to_string())
            .unwrap_or_else(|| "Claude Code Usage Monitor".to_string())
    })
}

fn sync_tray_icon(hwnd: HWND) {
    let usage_tooltip = tray_usage_summary_from_state();
    let themed = {
        let state = lock_state();
        state.as_ref().and_then(|state| {
            effective_theme_from_state(state)
                .map(|theme| (theme, state.data.clone(), theme_runtime_from_state(state)))
        })
    };
    if let Some((theme, data, runtime)) = themed {
        let has_tray_surfaces = theme.surfaces.iter().any(|surface| {
            surface
                .placement
                .nest
                .resolve(surface.placement.reference.region)
                == SurfaceNest::TrayIcon
        });
        if has_tray_surfaces {
            let icons = theme
                .surfaces
                .iter()
                .enumerate()
                .filter(|(surface_index, surface)| {
                    let surface_runtime =
                        theme_runtime_for_surface(&theme, *surface_index, runtime);
                    surface
                        .placement
                        .nest
                        .resolve(surface.placement.reference.region)
                        == SurfaceNest::TrayIcon
                        && theme_engine::surface_should_render(
                            &theme,
                            *surface_index,
                            data.as_ref(),
                            surface_runtime,
                        )
                })
                .filter_map(|(surface_index, surface)| {
                    let surface_runtime = theme_runtime_for_surface(&theme, surface_index, runtime);
                    let (logical_width, logical_height) = theme_engine::resolve_surface_size(
                        &theme,
                        surface_index,
                        data.as_ref(),
                        surface_runtime,
                    );
                    let max_dimension = logical_width.max(logical_height) as f64;
                    let scale =
                        theme_surface_scale(&theme, surface_index).min(if max_dimension > 0.0 {
                            512.0 / max_dimension
                        } else {
                            1.0
                        });
                    if scale < 0.25 {
                        diagnose::log(format!(
                            "tray-icon theme surface '{}' exceeds the 512px source limit",
                            surface.name
                        ));
                        return None;
                    }
                    let rendered = theme_engine::render_theme_surface_with_runtime_at_scale(
                        &theme,
                        surface_index,
                        data.as_ref(),
                        surface_runtime,
                        scale,
                    );
                    Some(tray_icon::ThemedTrayIcon {
                        surface_index,
                        tooltip: usage_tooltip
                            .clone()
                            .unwrap_or_else(|| surface.name.clone()),
                        width: rendered.width,
                        height: rendered.height,
                        pixels: rendered.pixels,
                    })
                })
                .collect::<Vec<_>>();
            tray_icon::sync_themed(hwnd, &icons);
            return;
        }
    }
    tray_icon::sync(hwnd, &tray_icon_tooltip_from_state());
}

fn theme_tray_uses_current_time(theme: &ThemeDocument) -> bool {
    theme
        .surfaces
        .iter()
        .enumerate()
        .filter(|(_, surface)| {
            surface
                .placement
                .nest
                .resolve(surface.placement.reference.region)
                == SurfaceNest::TrayIcon
        })
        .any(|(surface_index, _)| {
            theme
                .surface_current_time_refresh_interval(surface_index)
                .is_some()
        })
}

fn taskbar_created_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    *MESSAGE.get_or_init(|| unsafe {
        let name = native_interop::wide_str("TaskbarCreated");
        RegisterWindowMessageW(PCWSTR::from_raw(name.as_ptr()))
    })
}

fn attach_to_taskbar(hwnd: HWND, requested_index: usize) -> bool {
    let taskbars = native_interop::find_taskbars();
    if taskbars.is_empty() {
        diagnose::log("taskbar not found; using fallback popup window");
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
        native_interop::unhook_win_event(hook.to_hook());
    }

    native_interop::embed_in_taskbar(hwnd, taskbar.hwnd);

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
        s.taskbar_hwnd = Some(SendHwnd::from_hwnd(taskbar.hwnd));
        s.tray_notify_hwnd = tray_notify.map(SendHwnd::from_hwnd);
        s.win_event_hook = hook.map(SendWinEventHook::from_hook);
        s.taskbar_index = index;
        s.embedded = true;
    }
    true
}

fn taskbar_at_point(pt: POINT) -> Option<(usize, native_interop::TaskbarWindow)> {
    native_interop::find_taskbars()
        .into_iter()
        .enumerate()
        .find(|(_, taskbar)| {
            pt.x >= taskbar.rect.left
                && pt.x < taskbar.rect.right
                && pt.y >= taskbar.rect.top
                && pt.y < taskbar.rect.bottom
        })
}

fn tray_left_for_taskbar(taskbar_hwnd: HWND, taskbar_rect: RECT) -> i32 {
    let mut tray_left = taskbar_rect.right;
    if let Some(tray_hwnd) = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd") {
        if let Some(tray_rect) = native_interop::get_window_rect_safe(tray_hwnd) {
            tray_left = tray_rect.left;
        }
    }
    tray_left
}

fn clamp_offset_for_taskbar(taskbar_hwnd: HWND, taskbar_rect: RECT, offset: i32) -> i32 {
    let tray_left = tray_left_for_taskbar(taskbar_hwnd, taskbar_rect);
    let max_offset = (tray_left - taskbar_rect.left - total_widget_width()).max(0);
    offset.clamp(0, max_offset)
}

fn offset_for_drop_point(
    taskbar_hwnd: HWND,
    taskbar_rect: RECT,
    pt: POINT,
    drag_start_client_x: i32,
) -> i32 {
    let tray_left = tray_left_for_taskbar(taskbar_hwnd, taskbar_rect);
    let desired_left = pt.x - taskbar_rect.left - drag_start_client_x;
    let offset = tray_left - taskbar_rect.left - total_widget_width() - desired_left;
    clamp_offset_for_taskbar(taskbar_hwnd, taskbar_rect, offset)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn update_check_interval() -> Duration {
    Duration::from_secs(24 * 60 * 60)
}

fn auto_update_check_due(last_update_check_unix: Option<u64>) -> bool {
    let Some(last_update_check_unix) = last_update_check_unix else {
        return true;
    };

    now_unix_secs().saturating_sub(last_update_check_unix) >= update_check_interval().as_secs()
}

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
        let _ = KillTimer(Some(hwnd), TIMER_UPDATE_CHECK);
        if let Some(delay_ms) = delay_ms {
            SetTimer(Some(hwnd), TIMER_UPDATE_CHECK, delay_ms.max(1), None);
        }
    }
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
            Some(hwnd),
            PCWSTR::from_raw(message_wide.as_ptr()),
            PCWSTR::from_raw(title_wide.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn show_error_message(hwnd: HWND, title: &str, message: &str) {
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

fn show_update_prompt(hwnd: HWND, strings: Strings, release: &ReleaseDescriptor) -> bool {
    let message = strings
        .update_prompt_now
        .replace("{version}", &release.latest_version);

    unsafe {
        let title_wide = native_interop::wide_str(strings.update_available);
        let message_wide = native_interop::wide_str(&message);
        MessageBoxW(
            Some(hwnd),
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
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_APP_UPDATE_CHECK_COMPLETE,
                        WPARAM(0),
                        LPARAM(0),
                    );
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
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_APP_UPDATE_CHECK_COMPLETE,
                        WPARAM(0),
                        LPARAM(0),
                    );
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
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_APP_UPDATE_CHECK_COMPLETE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        }
    });
}

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
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
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
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_APP_UPDATE_CHECK_COMPLETE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            }
        }
    });
}

fn begin_winget_update(hwnd: HWND) {
    let strings = {
        let state = lock_state();
        state.as_ref().map(|s| s.language.strings())
    }
    .unwrap_or(LanguageId::English.strings());

    match updater::begin_winget_update() {
        Ok(()) => unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        Err(error) => {
            let message = format!("{}.\n\n{}", strings.update_failed, error);
            show_error_message(hwnd, strings.updates, &message);
        }
    }
}

const STARTUP_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_REGISTRY_KEY: &str = "ClaudeCodeUsageMonitor";

/// Returns true only if the startup registry value points to this executable.
pub(crate) fn is_startup_enabled() -> bool {
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);
        let key_name = native_interop::wide_str(STARTUP_REGISTRY_KEY);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            None,
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

pub(crate) fn set_startup_enabled(enable: bool) {
    unsafe {
        let path = native_interop::wide_str(STARTUP_REGISTRY_PATH);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            None,
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
                    None,
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

fn total_widget_width_for_state(state: &AppState) -> i32 {
    effective_theme_from_state(state)
        .as_ref()
        .map_or(1, |theme| {
            let runtime = theme_runtime_for_surface(theme, 0, theme_runtime_from_state(state));
            theme_engine::resolve_surface_size(theme, 0, state.data.as_ref(), runtime).0 as i32
        })
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
        state.active_theme = Some(loaded);
        state.theme_clock_interval = theme_clock_interval;
        state.tray_theme_uses_current_time = tray_theme_uses_current_time;
        state.mouse_action_overrides.clear();
        state.hovered_mouse_layer = None;
        state.pending_mouse_click = None;
        state.suppress_next_left_up = false;
        if path.is_some() {
            state.active_theme_path = path;
        }
        state.embedded = false;
        state.win_event_hook.take()
    };
    if let Some(hook) = old_hook {
        native_interop::unhook_win_event(hook.to_hook());
    }
    unsafe {
        native_interop::make_popup(hwnd, false);
        reset_layered_window(hwnd);
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
    sync_custom_mirrors();
    sync_window_state_timer(hwnd);
    schedule_countdown_timer();
    schedule_clock_timer();
    Ok(())
}

fn sync_custom_mirrors() {
    let (desired_total, desktop_surfaces) = {
        let state = lock_state();
        state
            .as_ref()
            .map(|state| {
                let surfaces = state
                    .active_theme
                    .as_ref()
                    .map(|theme| {
                        theme
                            .surfaces
                            .iter()
                            .map(|surface| {
                                surface
                                    .placement
                                    .nest
                                    .resolve(surface.placement.reference.region)
                                    == SurfaceNest::Desktop
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                (surfaces.len().max(1), surfaces)
            })
            .unwrap_or_else(|| (1, Vec::new()))
    };
    let desired_mirrors = desired_total.saturating_sub(1);
    loop {
        let remove = {
            let mut state = lock_state();
            state.as_mut().and_then(|state| {
                if state.mirror_hwnds.len() > desired_mirrors {
                    state.mirror_hwnds.pop()
                } else {
                    None
                }
            })
        };
        match remove {
            Some(hwnd) => unsafe {
                let _ = DestroyWindow(hwnd.to_hwnd());
            },
            None => break,
        }
    }
    while lock_state()
        .as_ref()
        .map(|state| state.mirror_hwnds.len())
        .unwrap_or(0)
        < desired_mirrors
    {
        let mirror = unsafe { create_mirror_window() };
        if mirror.is_invalid() {
            break;
        }
        if let Some(state) = lock_state().as_mut() {
            state.mirror_hwnds.push(SendHwnd::from_hwnd(mirror));
        }
    }

    let stale_desktop_windows = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        state.desktop_hwnds.resize_with(desired_total, || None);
        let removed = state.desktop_hwnds.split_off(desired_total);
        let mut stale = removed.into_iter().flatten().collect::<Vec<_>>();
        for (surface_index, window) in state.desktop_hwnds.iter_mut().enumerate() {
            let wanted = desktop_surfaces.get(surface_index) == Some(&true);
            let valid =
                window.is_some_and(|window| unsafe { IsWindow(Some(window.to_hwnd())).as_bool() });
            if !wanted || !valid {
                if let Some(window) = window.take() {
                    stale.push(window);
                }
            }
        }
        stale
    };
    for window in stale_desktop_windows {
        unsafe {
            let _ = DestroyWindow(window.to_hwnd());
        }
    }
    for (surface_index, wanted) in desktop_surfaces.into_iter().enumerate() {
        if !wanted {
            continue;
        }
        let missing = lock_state()
            .as_ref()
            .and_then(|state| state.desktop_hwnds.get(surface_index))
            .is_none_or(Option::is_none);
        if !missing {
            continue;
        }
        let window = unsafe { create_desktop_surface_window() };
        if window.is_invalid() {
            continue;
        }
        unsafe {
            let _ = ShowWindow(window, SW_HIDE);
        }
        if let Some(slot) = lock_state()
            .as_mut()
            .and_then(|state| state.desktop_hwnds.get_mut(surface_index))
        {
            *slot = Some(SendHwnd::from_hwnd(window));
        } else {
            unsafe {
                let _ = DestroyWindow(window);
            }
        }
    }
}

unsafe fn create_desktop_surface_window() -> HWND {
    let Some(desktop) = native_interop::find_desktop_host() else {
        return HWND::default();
    };
    let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
    let class = native_interop::wide_str("CCUMDesktopSurface");
    let title = native_interop::wide_str("");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(mirror_wnd_proc),
        hInstance: HINSTANCE(instance.0),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszClassName: PCWSTR::from_raw(class.as_ptr()),
        ..Default::default()
    };
    RegisterClassExW(&wc);
    let previous_hosting = SetThreadDpiHostingBehavior(DPI_HOSTING_BEHAVIOR_MIXED);
    let previous_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE);
    let window = CreateWindowExW(
        WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE,
        PCWSTR::from_raw(class.as_ptr()),
        PCWSTR::from_raw(title.as_ptr()),
        WINDOW_STYLE(
            native_interop::WS_CHILD_STYLE | native_interop::WS_CLIPSIBLINGS_STYLE | WS_VISIBLE.0,
        ),
        0,
        0,
        198,
        144,
        Some(desktop.parent),
        None,
        Some(HINSTANCE(instance.0)),
        None,
    )
    .unwrap_or_default();
    let _ = SetThreadDpiAwarenessContext(previous_dpi);
    let _ = SetThreadDpiHostingBehavior(previous_hosting);
    if window.is_invalid() {
        diagnose::log("unable to create raised-desktop surface window");
    }
    window
}

unsafe fn create_mirror_window() -> HWND {
    let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
    let class = native_interop::wide_str("CCUMThemeMirror");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(mirror_wnd_proc),
        hInstance: HINSTANCE(instance.0),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszClassName: PCWSTR::from_raw(class.as_ptr()),
        ..Default::default()
    };
    RegisterClassExW(&wc);
    let title = native_interop::wide_str("Usage theme mirror");
    CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
        PCWSTR::from_raw(class.as_ptr()),
        PCWSTR::from_raw(title.as_ptr()),
        WS_POPUP,
        0,
        0,
        1,
        1,
        None,
        None,
        Some(HINSTANCE(instance.0)),
        None,
    )
    .unwrap_or_default()
}

unsafe extern "system" fn mirror_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_SETCURSOR if set_surface_cursor(hwnd) => LRESULT(1),
        WM_MOUSEMOVE => {
            update_mouse_hover(hwnd, lparam);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            clear_mouse_hover(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let suppressed = {
                let mut state = lock_state();
                state.as_mut().is_some_and(|state| {
                    let suppressed = state.suppress_next_left_up;
                    state.suppress_next_left_up = false;
                    suppressed
                })
            };
            if !suppressed {
                if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                    schedule_or_dispatch_click(hwnd, surface, object);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                dispatch_double_click(hwnd, surface, object);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                let _ = dispatch_mouse_event(surface, &object, MouseEventKind::RightClick);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut paint);
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DESTROY => {
            crate::desktop_compositor::remove(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn total_widget_height_for_state(state: &AppState) -> i32 {
    effective_theme_from_state(state)
        .as_ref()
        .map_or(1, |theme| {
            let runtime = theme_runtime_for_surface(theme, 0, theme_runtime_from_state(state));
            theme_engine::resolve_surface_size(theme, 0, state.data.as_ref(), runtime).1 as i32
        })
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
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CURRENT_DPI.store(GetDpiForSystem(), Ordering::Relaxed);
    }
    diagnose::log("window::run started");

    // Single-instance guard: silently exit if another instance is running.
    // Exception: when relaunched after an explorer restart (ENV_RELAUNCH set),
    // wait for the previous instance to release the mutex, then take over.
    let is_relaunch = std::env::var(ENV_RELAUNCH).is_ok();
    let mutex_name = native_interop::wide_str(&if allow_multiple {
        format!("Global\\ClaudeCodeUsageMonitor-{}", std::process::id())
    } else {
        "Global\\ClaudeCodeUsageMonitor".to_string()
    });
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

    let class_name = native_interop::wide_str("ClaudeCodeUsageMonitor");

    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap();
        let (large_icon, small_icon) = tray_icon::load_app_icons();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wnd_proc),
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

        // Create as layered popup (will be reparented into taskbar)
        let title = native_interop::wide_str(language.strings().window_title);
        let initial_runtime = ThemeRuntime::from_providers(settings.enabled_providers())
            .with_poll_state(false, false)
            .with_language(language);
        let (initial_width, initial_height) = active_theme
            .as_ref()
            .map(|theme| {
                let initial_runtime = theme_runtime_for_surface(theme, 0, initial_runtime);
                let (width, height) =
                    theme_engine::resolve_surface_size(theme, 0, None, initial_runtime);
                let scale = theme_surface_scale(theme, 0);
                (
                    scaled_theme_dimension(width, scale),
                    scaled_theme_dimension(height, scale),
                )
            })
            .unwrap_or((1, 1));
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            initial_width,
            initial_height,
            None,
            None,
            Some(HINSTANCE(hinstance.0)),
            None,
        )
        .unwrap();

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
                update_status: UpdateStatus::Idle,
                last_update_check_unix: settings.last_update_check_unix,
                taskbar_index: settings.taskbar_index,
                tray_offset: settings.tray_offset,
                dragging: false,
                drag_start_mouse_x: 0,
                drag_start_client_x: 0,
                drag_start_offset: 0,
                custom_theme_enabled,
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

        if let Err(error) = crate::dashboard::start_request_listener(hwnd) {
            diagnose::log_error("dashboard request listener failed", error);
        }

        sync_custom_mirrors();
        native_interop::make_popup(hwnd, false);

        // Register the persistent application tray icon.
        if !no_poll {
            sync_tray_icon(hwnd);
        }

        // Theme surfaces decide whether their windows render.
        position_at_taskbar();
        diagnose::log("window shown");

        // Initial render using the presenter selected by the surface nest.
        render_layered();
        schedule_countdown_timer();
        schedule_clock_timer();

        if open_dashboard_on_start {
            crate::dashboard::show(hwnd);
        }

        // Poll timer: 15 minutes
        let initial_poll_ms = {
            let state = lock_state();
            state
                .as_ref()
                .map(|s| s.poll_interval_ms)
                .unwrap_or(POLL_15_MIN)
        };
        SetTimer(Some(hwnd), TIMER_POLL, initial_poll_ms, None);
        sync_window_state_timer(hwnd);

        // Watch for explorer.exe restarts so we can re-embed and re-add the tray
        // icon (the shell discards tray registrations when it restarts). This
        // runs on a dedicated thread, NOT a window timer: once explorer destroys
        // the taskbar, our embedded child window stops receiving all messages
        // (WM_TIMER included), so a timer would never fire again.
        spawn_taskbar_watchdog();

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
    sync_custom_mirrors();
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
            unsafe {
                let _ = ShowWindow(regular_hwnd, SW_HIDE);
            }
            desktop_hwnds
                .get(surface_index)
                .and_then(|window| *window)
                .map(SendHwnd::to_hwnd)
                .unwrap_or(regular_hwnd)
        } else {
            regular_hwnd
        };
        if nest == SurfaceNest::TrayIcon {
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
        position_custom_theme(target_hwnd, &positioned, scale);
        if desktop_nested {
            unsafe {
                let _ = ShowWindow(target_hwnd, SW_SHOWNOACTIVATE);
            }
        }
        render_custom_window(target_hwnd, &rendered, desktop_nested);
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

fn request_poll(hwnd: HWND) {
    request_poll_inner(hwnd, true);
}

/// Request a timer-driven poll without extending an already-running poll cycle.
fn request_scheduled_poll(hwnd: HWND) {
    request_poll_inner(hwnd, false);
}

fn request_poll_inner(hwnd: HWND, queue_if_busy: bool) {
    if POLL_IN_FLIGHT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        if queue_if_busy {
            POLL_PENDING.store(true, Ordering::Release);
        }
        return;
    }
    let send_hwnd = SendHwnd::from_hwnd(hwnd);
    std::thread::spawn(move || poll_worker(send_hwnd));
}

fn poll_worker(send_hwnd: SendHwnd) {
    loop {
        do_poll_once(send_hwnd.to_hwnd());
        if POLL_PENDING.swap(false, Ordering::AcqRel) {
            continue;
        }

        POLL_IN_FLIGHT.store(false, Ordering::Release);
        if !POLL_PENDING.swap(false, Ordering::AcqRel) {
            break;
        }

        // A request can arrive between the pending check and releasing the
        // in-flight flag. Reacquire ownership unless that request already
        // started a replacement worker.
        if POLL_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            break;
        }
    }
}

fn do_poll_once(hwnd: HWND) {
    let enabled_providers = {
        let state = lock_state();
        state
            .as_ref()
            .map(|state| state.providers)
            .unwrap_or_default()
    };

    match poller::poll(enabled_providers) {
        Ok(data) => {
            let mut state = lock_state();
            let data = match state.as_ref().and_then(|s| s.data.as_ref()) {
                Some(previous) => poller::carry_forward_failures(data, previous, enabled_providers),
                None => data,
            };
            let cache_data = data.clone();
            if let Some(s) = state.as_mut() {
                // Stop fast-poll if reset data is now fresh
                if !poller::app_is_past_reset(&data) {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_RESET_POLL);
                    }
                }

                s.data = Some(data);
                s.last_poll_ok = true;

                // Recovered from errors — restore normal poll interval
                if s.retry_count > 0 {
                    s.retry_count = 0;
                    let interval = s.poll_interval_ms;
                    unsafe {
                        SetTimer(Some(hwnd), TIMER_POLL, interval, None);
                    }
                }
                s.force_notify_auth_error = false;
                s.auth_error_paused_polling = false;
                s.auth_watch_mode = poller::CredentialWatchMode::ActiveSource(
                    s.providers.first().unwrap_or_default(),
                );
                s.auth_watch_snapshot.clear();
            }
            drop(state);
            let _ = app_settings::save_usage_cache(&cache_data, true);

            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
        Err(failure) => {
            let auth_watch = match failure.error {
                poller::PollError::AuthRequired | poller::PollError::TokenExpired => {
                    let mode = poller::CredentialWatchMode::ActiveSource(failure.provider);
                    Some((mode, poller::credential_watch_snapshot(mode)))
                }
                poller::PollError::NoCredentials => {
                    let mode = poller::CredentialWatchMode::AllSources(failure.provider);
                    Some((mode, poller::credential_watch_snapshot(mode)))
                }
                poller::PollError::RequestFailed => None,
            };
            // Distinguish auth-required errors from transient errors.
            let (notify_auth_error, cache_data, cache_poll_ok) = {
                let mut state = lock_state();
                let mut should_notify = false;
                if let Some(s) = state.as_mut() {
                    if matches!(failure.error, poller::PollError::RequestFailed) {
                        if let Some(previous) = s.data.as_ref() {
                            let carried = poller::carry_forward_failures(
                                AppUsageData::default(),
                                previous,
                                enabled_providers,
                            );
                            s.data = Some(carried);
                        }
                    }
                    s.last_poll_ok = false;
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
                                let _ = KillTimer(Some(hwnd), TIMER_POLL);
                                let _ = KillTimer(Some(hwnd), TIMER_RESET_POLL);
                                let _ = KillTimer(Some(hwnd), TIMER_COUNTDOWN);
                                SetTimer(Some(hwnd), TIMER_POLL, s.poll_interval_ms, None);
                            }
                        }
                        _ => {
                            // Transient network / credential-missing errors: exponential backoff.
                            s.force_notify_auth_error = false;
                            s.auth_error_paused_polling = false;
                            s.auth_watch_mode = poller::CredentialWatchMode::ActiveSource(
                                s.providers.first().unwrap_or_default(),
                            );
                            s.auth_watch_snapshot.clear();
                            s.retry_count = s.retry_count.saturating_add(1);
                            let backoff = RETRY_BASE_MS.saturating_mul(
                                1u32.checked_shl(s.retry_count - 1).unwrap_or(u32::MAX),
                            );
                            let retry_ms = backoff.min(s.poll_interval_ms);
                            unsafe {
                                let _ = KillTimer(Some(hwnd), TIMER_RESET_POLL);
                                SetTimer(Some(hwnd), TIMER_POLL, retry_ms, None);
                            }
                        }
                    }
                }
                let cache_data = state
                    .as_ref()
                    .and_then(|state| state.data.clone())
                    .unwrap_or_default();
                let cache_poll_ok = state.as_ref().is_some_and(|state| {
                    poll_display_state(
                        state.last_poll_ok,
                        state.retry_count,
                        state.auth_error_paused_polling,
                        state.data.as_ref(),
                    )
                    .0
                });
                (should_notify, cache_data, cache_poll_ok)
            };
            // Theme Studio is a separate process and follows this cache. A
            // transient failure with usable stale data remains displayable;
            // hard failures and failures without a reading stay errors.
            let _ = app_settings::save_usage_cache(&cache_data, cache_poll_ok);

            if notify_auth_error {
                let balloon = {
                    let state = lock_state();
                    state
                        .as_ref()
                        .map(|state| state.language.provider_auth_error(failure.provider))
                };
                if let Some((title, body)) = balloon {
                    tray_icon::notify_balloon(hwnd, title, body);
                }
            }

            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
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
            let _ = KillTimer(Some(hwnd), TIMER_COUNTDOWN);
            let _ = KillTimer(Some(hwnd), TIMER_RESET_POLL);
        }
        return;
    }

    // If a reset time has passed, poll every 5s to pick up fresh data
    if s.data.as_ref().is_some_and(poller::app_is_past_reset) {
        unsafe {
            SetTimer(Some(hwnd), TIMER_RESET_POLL, 5_000, None);
        }
    }

    let min_delay = s.data.as_ref().and_then(|data| {
        data.iter()
            .flat_map(|(_, usage)| {
                [
                    Some(&usage.session),
                    Some(&usage.weekly),
                    usage.fable.as_ref(),
                ]
            })
            .flatten()
            .filter_map(|section| poller::time_until_display_change(section.resets_at))
            .min()
    });

    let ms = min_delay
        .unwrap_or(Duration::from_secs(60))
        .as_millis()
        .max(1000) as u32;

    unsafe {
        SetTimer(Some(hwnd), TIMER_COUNTDOWN, ms, None);
    }
}

fn schedule_clock_timer() {
    let state = lock_state();
    let Some(s) = state.as_ref() else {
        return;
    };
    let hwnd = s.hwnd.to_hwnd();
    let Some(interval) = s.theme_clock_interval else {
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_CLOCK);
        }
        return;
    };
    let ms = time_until_next_clock_refresh(interval).as_millis().max(1) as u32;
    unsafe {
        SetTimer(Some(hwnd), TIMER_CLOCK, ms, None);
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

fn reload_external_settings(hwnd: HWND) {
    let settings = load_settings();
    let language_override = settings.language.as_deref().and_then(LanguageId::from_code);
    let theme_path = settings.active_theme_path.as_ref().map(PathBuf::from);
    let providers_changed;
    {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        providers_changed = state.providers != settings.enabled_providers();
        state.poll_interval_ms = settings.poll_interval_ms;
        state.providers = settings.enabled_providers();
        state.taskbar_index = settings.taskbar_index;
        apply_language_to_state(state, language_override);
    }
    unsafe {
        SetTimer(Some(hwnd), TIMER_POLL, settings.poll_interval_ms, None);
    }
    let _ = apply_custom_theme(hwnd, settings.custom_theme_enabled, theme_path, None);
    if providers_changed {
        request_poll(hwnd);
    }
    sync_tray_icon(hwnd);
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

mod message_loop;
use message_loop::wnd_proc;
mod positioning;
use positioning::*;
mod mouse;
use mouse::*;
mod window_context_menu;
use window_context_menu::*;

#[cfg(test)]
mod placement_tests;

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
mod tray_usage_summary_tests {
    use super::*;
    use crate::models::{UsageData, UsageSection};

    fn usage(session: f64, weekly: f64, weekly_label: Option<&str>) -> UsageData {
        UsageData {
            session: UsageSection {
                percentage: session,
                resets_at: None,
            },
            weekly: UsageSection {
                percentage: weekly,
                resets_at: None,
            },
            weekly_label: weekly_label.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn tray_summary_formats_enabled_provider_usage() {
        let data = [(ProviderId::Claude, usage(4.6, 42.4, None))]
            .into_iter()
            .collect();

        assert_eq!(
            tray_usage_summary_lines(
                &data,
                ProviderSet::from_enabled([ProviderId::Claude]),
                LanguageId::English,
            ),
            ["Claude Code 5h: 5% | 7d: 42%"]
        );
    }

    #[test]
    fn tray_summary_uses_provider_window_labels_and_selection() {
        let data = [
            (ProviderId::Claude, usage(10.0, 20.0, None)),
            (ProviderId::OpenCode, usage(30.0, 40.0, Some("30d"))),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            tray_usage_summary_lines(
                &data,
                ProviderSet::from_enabled([ProviderId::OpenCode]),
                LanguageId::English,
            ),
            ["OpenCode 5h: 30% | 30d: 40%"]
        );
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
