use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows::Win32::Globalization::GetLocaleInfoW;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_EXCLUDED_FROM_PEEK};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_GETTASKBARPOS, APPBARDATA};
use windows::Win32::UI::WindowsAndMessaging::*;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static PEEK_LATCH_UNTIL_MS: AtomicU64 = AtomicU64::new(0);

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Extend peek latch only while taskbar thumbnail/preview UI is present.
pub fn refresh_taskbar_peek_latch(_self_hwnd: HWND, _taskbar_hwnd: Option<HWND>) {
    if taskbar_interactive_preview_active() || shell_preview_ui_active() {
        let until = now_unix_ms().saturating_add(5000);
        let _ = PEEK_LATCH_UNTIL_MS.fetch_max(until, Ordering::Relaxed);
    }
}

pub fn taskbar_peek_latch_active() -> bool {
    now_unix_ms() < PEEK_LATCH_UNTIL_MS.load(Ordering::Relaxed)
}


const LOCALE_USER_DEFAULT: u32 = 0x0400;
// Short date format pattern (e.g. "M/d/yyyy")
const LOCALE_SSHORTDATE: u32 = 0x001F;

// Window style constants
pub const WS_POPUP_STYLE: u32 = 0x80000000;
pub const WS_CHILD_STYLE: u32 = 0x40000000;
pub const WS_CLIPSIBLINGS_STYLE: u32 = 0x04000000;

// Win event constants
pub const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
pub const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
pub const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;

// Timer IDs
pub const TIMER_POLL: usize = 1;
pub const TIMER_COUNTDOWN: usize = 2;
pub const TIMER_RESET_POLL: usize = 3;
pub const TIMER_UPDATE_CHECK: usize = 4;
pub const TIMER_DRAG: usize = 5;
pub const TIMER_WIDGET_KEEPALIVE: usize = 6;
pub const TIMER_FULLSCREEN_CHECK: usize = 7;

// Custom messages
pub const WM_APP: u32 = 0x8000;
pub const WM_APP_USAGE_UPDATED: u32 = WM_APP + 1;
pub const WM_APP_TRAY: u32 = WM_APP + 3;
pub const WM_APP_REQUEST_PROOF: u32 = WM_APP + 7;
pub const WM_APP_FOREGROUND_CHANGED: u32 = WM_APP + 8;

#[derive(Clone, Copy, Debug)]
pub struct TaskbarWindow {
    pub hwnd: HWND,
    pub rect: RECT,
    pub is_primary: bool,
}

fn taskbar_has_notification_area(taskbar_hwnd: HWND) -> bool {
    find_descendant_window(taskbar_hwnd, "TrayNotifyWnd")
        .or_else(|| find_child_window(taskbar_hwnd, "TrayNotifyWnd"))
        .is_some()
}

pub fn find_taskbar_by_class(class_name: &str) -> Option<HWND> {
    struct Search {
        target: String,
        found: Option<HWND>,
    }
    let mut search = Search {
        target: class_name.to_string(),
        found: None,
    };
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        let mut class_buf = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut class_buf) };
        if len > 0 {
            let class = String::from_utf16_lossy(&class_buf[..len as usize]);
            if class == search.target {
                search.found = Some(hwnd);
                return BOOL(0);
            }
        }
        BOOL(1)
    }
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut search as *mut _ as isize));
    }
    search.found
}

pub fn taskbar_hwnd_for_settings_index(taskbar_index: usize) -> Option<HWND> {
    if taskbar_index > 0 {
        if let Some(hwnd) = find_taskbar_by_class("Shell_SecondaryTrayWnd") {
            return Some(hwnd);
        }
    }
    let direct = find_taskbar_by_class("Shell_TrayWnd");
    if direct.is_none() {
        let taskbars = find_taskbars();
        crate::diagnose::log(format!(
            "taskbar_hwnd_for_settings_index: direct Shell_TrayWnd lookup FAILED, falling back. find_taskbars() -> {:?}",
            taskbars
                .iter()
                .map(|t| format!(
                    "hwnd={:?} is_primary={} rect=({},{},{},{})",
                    t.hwnd, t.is_primary, t.rect.left, t.rect.top, t.rect.right, t.rect.bottom
                ))
                .collect::<Vec<_>>()
        ));
        return taskbars
            .into_iter()
            .find(|taskbar| taskbar_index == 0 || !taskbar.is_primary)
            .map(|taskbar| taskbar.hwnd);
    }
    direct
}

pub fn find_taskbars() -> Vec<TaskbarWindow> {
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let taskbars = &mut *(lparam.0 as *mut Vec<TaskbarWindow>);
        let mut class_name = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut class_name) };
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_name[..len as usize]);
            let is_primary = class_name == "Shell_TrayWnd";
            if is_primary || class_name == "Shell_SecondaryTrayWnd" {
                let has_tray = taskbar_has_notification_area(hwnd);
                if is_primary && !has_tray {
                    if let (Some(rect), Some(mon)) =
                        (get_window_rect_safe(hwnd), primary_monitor_rect())
                    {
                        if !rects_overlap(rect, mon) {
                            crate::diagnose::log(format!(
                                "find_taskbars: excluding primary hwnd={hwnd:?} rect=({},{},{},{}) - no tray, doesn't overlap primary monitor ({},{},{},{})",
                                rect.left, rect.top, rect.right, rect.bottom,
                                mon.left, mon.top, mon.right, mon.bottom
                            ));
                            return BOOL(1);
                        }
                    }
                }
                if let Some(rect) = get_taskbar_rect(hwnd).or_else(|| get_window_rect_safe(hwnd)) {
                    taskbars.push(TaskbarWindow {
                        hwnd,
                        rect,
                        is_primary,
                    });
                }
            }
        }
        BOOL(1)
    }

    let mut taskbars: Vec<TaskbarWindow> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut taskbars as *mut _ as isize));
    }
    taskbars.sort_by_key(|taskbar| {
        (
            !taskbar.is_primary,
            taskbar.rect.top,
            taskbar.rect.left,
            taskbar.rect.bottom,
            taskbar.rect.right,
        )
    });
    taskbars
}

/// Find a child window by class name (direct children only).
pub fn find_child_window(parent: HWND, class_name: &str) -> Option<HWND> {
    find_next_child_window(parent, HWND::default(), class_name)
}

/// Find a descendant window by class name anywhere under `parent`.
pub fn find_descendant_window(parent: HWND, class_name: &str) -> Option<HWND> {
    struct Search {
        target: String,
        found: Option<HWND>,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);
        let mut class_buf = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class_buf);
        if len > 0 {
            let class = String::from_utf16_lossy(&class_buf[..len as usize]);
            if class == search.target {
                search.found = Some(hwnd);
                return BOOL(0);
            }
        }
        BOOL(1)
    }

    let mut search = Search {
        target: class_name.to_string(),
        found: None,
    };
    unsafe {
        let _ = EnumChildWindows(parent, Some(enum_proc), LPARAM(&mut search as *mut _ as isize));
    }
    search.found
}

struct TaskbarBandScan {
    taskbar_rect: RECT,
    content_left: i32,
    pin_right: i32,
}

unsafe extern "system" fn scan_taskbar_band_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let scan = &mut *(lparam.0 as *mut TaskbarBandScan);
    let mut class_buf = [0u16; 64];
    let len = GetClassNameW(hwnd, &mut class_buf);
    if len <= 0 {
        return BOOL(1);
    }
    let class = String::from_utf16_lossy(&class_buf[..len as usize]);
    if class != "MSTaskListWClass" && class != "MSTaskSwWClass" {
        return BOOL(1);
    }
    if let Some(rect) = get_window_rect_safe(hwnd) {
        if !rects_overlap(rect, scan.taskbar_rect) {
            return BOOL(1);
        }
        scan.pin_right = scan.pin_right.max(rect.right);
        let relative_right = rect.right.saturating_sub(scan.taskbar_rect.left);
        let relative_left = rect.left.saturating_sub(scan.taskbar_rect.left);
        let taskbar_width = scan.taskbar_rect.right - scan.taskbar_rect.left;
        if relative_right > relative_left && relative_right < taskbar_width {
            scan.content_left = scan.content_left.max(relative_right);
        }
    }
    BOOL(1)
}


struct VisibleLeftScan {
    band: RECT,
    visible_left: i32,
    found: bool,
}

unsafe extern "system" fn scan_visible_left_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let scan = &mut *(lparam.0 as *mut VisibleLeftScan);
    let mut class_buf = [0u16; 64];
    let len = GetClassNameW(hwnd, &mut class_buf);
    if len > 0 {
        let class = String::from_utf16_lossy(&class_buf[..len as usize]);
        let is_chrome = class.contains("Start")
            || class == "MSTaskListWClass"
            || class == "MSTaskSwWClass"
            || class == "ReBarWindow32"
            || class == "ToolbarWindow32";
        if is_chrome {
            if let Some(rect) = get_window_rect_safe(hwnd) {
                if rect.right > rect.left && rects_overlap(rect, scan.band) {
                    scan.visible_left = if scan.found {
                        scan.visible_left.min(rect.left)
                    } else {
                        rect.left
                    };
                    scan.found = true;
                }
            }
        }
        // Task buttons live under ReBarWindow32; avoid full-tree recursion here
        // (can deadlock with WinEvent-driven SetWindowPos during EnumChildWindows).
        if class == "ReBarWindow32" {
            let _ = EnumChildWindows(hwnd, Some(scan_visible_left_proc), lparam);
        }
    }
    BOOL(1)
}

fn taskbar_visible_left(taskbar_hwnd: HWND, taskbar_rect: RECT) -> i32 {
    let mut scan = VisibleLeftScan {
        band: taskbar_rect,
        visible_left: taskbar_rect.left,
        found: false,
    };
    unsafe {
        let _ = EnumChildWindows(
            taskbar_hwnd,
            Some(scan_visible_left_proc),
            LPARAM(&mut scan as *mut _ as isize),
        );
    }
    if scan.found {
        scan.visible_left
    } else {
        taskbar_rect.left
    }
}

fn scan_taskbar_band(taskbar_hwnd: HWND, taskbar_rect: RECT) -> (i32, i32) {
    let mut scan = TaskbarBandScan {
        taskbar_rect,
        content_left: 0,
        pin_right: 0,
    };
    unsafe {
        let _ = EnumChildWindows(
            taskbar_hwnd,
            Some(scan_taskbar_band_proc),
            LPARAM(&mut scan as *mut _ as isize),
        );
    }
    (scan.content_left, scan.pin_right)
}

/// Find the next sibling child window matching `class_name`.
pub fn find_next_child_window(parent: HWND, after: HWND, class_name: &str) -> Option<HWND> {
    unsafe {
        let class = wide_str(class_name);
        match FindWindowExW(
            parent,
            after,
            PCWSTR::from_raw(class.as_ptr()),
            PCWSTR::null(),
        ) {
            Ok(h) if h != HWND::default() => Some(h),
            _ => None,
        }
    }
}


fn rect_fits_monitor(rect: RECT, mon: RECT) -> bool {
    rect.left >= mon.left
        && rect.top >= mon.top
        && rect.right <= mon.right
        && rect.bottom <= mon.bottom
}

fn taskbar_band_height(rect: RECT) -> i32 {
    (rect.bottom - rect.top).max(1)
}

fn is_plausible_taskbar_height(height: i32) -> bool {
    (16..=160).contains(&height)
}

fn primary_monitor_rect() -> Option<RECT> {
    unsafe {
        let mut found: Option<RECT> = None;
        unsafe extern "system" fn monitor_proc(
            monitor: HMONITOR,
            _dc: HDC,
            _rect: *mut RECT,
            lparam: LPARAM,
        ) -> BOOL {
            let found = &mut *(lparam.0 as *mut Option<RECT>);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(monitor, &mut info).as_bool() && info.dwFlags & 1 != 0 {
                *found = Some(info.rcMonitor);
                return BOOL(0);
            }
            BOOL(1)
        }
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(monitor_proc),
            LPARAM(&mut found as *mut _ as isize),
        );
        found
    }
}

fn taskbar_window_class(taskbar_hwnd: HWND) -> Option<String> {
    unsafe {
        let mut class_name = [0u16; 64];
        let len = GetClassNameW(taskbar_hwnd, &mut class_name);
        if len > 0 {
            Some(String::from_utf16_lossy(&class_name[..len as usize]))
        } else {
            None
        }
    }
}

fn monitors_all() -> Vec<RECT> {
    unsafe {
        let mut monitors: Vec<RECT> = Vec::new();
        unsafe extern "system" fn monitor_proc(
            monitor: HMONITOR,
            _dc: HDC,
            _rect: *mut RECT,
            lparam: LPARAM,
        ) -> BOOL {
            let monitors = &mut *(lparam.0 as *mut Vec<RECT>);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                monitors.push(info.rcMonitor);
            }
            BOOL(1)
        }
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(monitor_proc),
            LPARAM(&mut monitors as *mut _ as isize),
        );
        monitors
    }
}

fn monitors_primary_first() -> Vec<RECT> {
    let mut monitors = monitors_all();
    monitors.sort_by_key(|mon| {
        (
            mon.top >= 0,
            mon.top,
            mon.left,
            mon.bottom,
            mon.right,
        )
    });
    monitors
}

fn monitor_rect_for_taskbar(taskbar_hwnd: HWND) -> Option<RECT> {
    let monitors = monitors_all();
    if monitors.is_empty() {
        return primary_monitor_rect();
    }
    let class = taskbar_window_class(taskbar_hwnd).unwrap_or_default();
    if class.contains("Secondary") {
        return monitors
            .iter()
            .copied()
            .max_by_key(|mon| mon.right - mon.left);
    }
    monitors
        .iter()
        .copied()
        .min_by_key(|mon| mon.right - mon.left)
        .or_else(primary_monitor_rect)
}

fn clip_taskbar_band_to_monitor(mut band: RECT, mon: RECT) -> RECT {
    band.left = band.left.max(mon.left);
    band.right = band.right.min(mon.right);
    if band.right <= band.left {
        band.left = mon.left;
        band.right = mon.right;
    }
    band
}

fn taskbar_band_for_monitor_edge(mon: RECT, edge: u32, height: i32) -> RECT {
    let height = height.max(16);
    match edge {
        1 => RECT {
            left: mon.left,
            top: mon.top,
            right: mon.right,
            bottom: mon.top.saturating_add(height),
        },
        2 => RECT {
            left: mon.right.saturating_sub(height),
            top: mon.top,
            right: mon.right,
            bottom: mon.bottom,
        },
        0 => RECT {
            left: mon.left,
            top: mon.top,
            right: mon.left.saturating_add(height),
            bottom: mon.bottom,
        },
        _ => RECT {
            left: mon.left,
            top: mon.bottom.saturating_sub(height),
            right: mon.right,
            bottom: mon.bottom,
        },
    }
}

fn taskbar_appbar_edge(taskbar_hwnd: HWND) -> Option<u32> {
    unsafe {
        let mut abd = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: taskbar_hwnd,
            ..Default::default()
        };
        if SHAppBarMessage(ABM_GETTASKBARPOS, &mut abd) == 0 {
            return None;
        }
        Some(abd.uEdge)
    }
}

fn normalize_rect_to_virtual_screen(hwnd: HWND, rect: RECT) -> RECT {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return rect;
        }
        let mon = info.rcMonitor;
        if rect_fits_monitor(rect, mon) {
            return rect;
        }
        // SHAppBarMessage returns monitor-relative coordinates on some setups.
        let translated = RECT {
            left: mon.left.saturating_add(rect.left),
            top: mon.top.saturating_add(rect.top),
            right: mon.left.saturating_add(rect.right),
            bottom: mon.top.saturating_add(rect.bottom),
        };
        if rect_fits_monitor(translated, mon) {
            return translated;
        }
        // Avoid double-translating coords that are already virtual but span monitors.
        if rects_overlap(rect, mon) && is_plausible_taskbar_height(taskbar_band_height(rect)) {
            return clip_taskbar_band_to_monitor(rect, mon);
        }
        if rects_overlap(translated, mon) && is_plausible_taskbar_height(taskbar_band_height(translated))
        {
            return clip_taskbar_band_to_monitor(translated, mon);
        }
        rect
    }
}


/// Taskbar band in virtual-screen coords for the monitor that owns `taskbar_hwnd`.
pub fn screen_taskbar_rect(taskbar_hwnd: HWND, fallback: RECT) -> RECT {
    resolved_taskbar_band(taskbar_hwnd, fallback).unwrap_or_else(|| {
        normalize_rect_to_virtual_screen(taskbar_hwnd, fallback)
    })
}

/// True when `widget` sits on the taskbar band for `taskbar_hwnd`.
pub fn widget_on_taskbar_band(taskbar_hwnd: HWND, widget: RECT) -> bool {
    let fallback = get_window_rect_safe(taskbar_hwnd).unwrap_or_default();
    let band = match resolved_taskbar_band(taskbar_hwnd, fallback) {
        Some(band) => band,
        None => return false,
    };
    if !is_plausible_taskbar_height(taskbar_band_height(band)) {
        return false;
    }
    if !rects_overlap(widget, band) {
        return false;
    }
    let widget_h = (widget.bottom - widget.top).max(1);
    let vertical_gap = (widget.bottom - band.bottom)
        .abs()
        .min((widget.top - band.top).abs());
    vertical_gap <= (widget_h / 2).max(8)
}

fn infer_taskbar_edge(raw: RECT, mon: RECT) -> u32 {
    let cy = (raw.top + raw.bottom) / 2;
    let dist_top = (cy - mon.top).abs();
    let dist_bottom = (mon.bottom - cy).abs();
    let dist_left = (raw.left - mon.left).abs();
    let dist_right = (mon.right - raw.right).abs();
    let min_vert = dist_top.min(dist_bottom);
    let min_horiz = dist_left.min(dist_right);
    if min_horiz < min_vert {
        if dist_left < dist_right {
            0
        } else {
            2
        }
    } else if dist_top < dist_bottom {
        1
    } else {
        3
    }
}

fn resolved_taskbar_band(taskbar_hwnd: HWND, fallback: RECT) -> Option<RECT> {
    let mon = monitor_rect_for_taskbar(taskbar_hwnd)?;
    let mut height = taskbar_band_height(fallback);
    if !is_plausible_taskbar_height(height) {
        height = 48;
    }
    height = height.clamp(16, 120);
    let class = taskbar_window_class(taskbar_hwnd).unwrap_or_default();
    if class.contains("Secondary") || class == "Shell_TrayWnd" {
        return Some(RECT {
            left: mon.left,
            top: mon.bottom.saturating_sub(height),
            right: mon.right,
            bottom: mon.bottom,
        });
    }
    let edge = get_window_rect_safe(taskbar_hwnd)
        .map(|raw| infer_taskbar_edge(raw, mon))
        .unwrap_or_else(|| taskbar_appbar_edge(taskbar_hwnd).unwrap_or(3));
    Some(taskbar_band_for_monitor_edge(mon, edge, height))
}

fn monitor_rect_for_hwnd(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            Some(info.rcMonitor)
        } else {
            None
        }
    }
}

fn monitor_rect_at_point(pt: POINT) -> Option<RECT> {
    unsafe {
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            Some(info.rcMonitor)
        } else {
            None
        }
    }
}

fn monitor_rect_for_tray(tray_rect: RECT) -> Option<RECT> {
    let cy = (tray_rect.top + tray_rect.bottom) / 2;
    monitor_rect_at_point(POINT {
        x: (tray_rect.left + tray_rect.right) / 2,
        y: cy,
    })
}

fn point_in_rect(pt: POINT, rect: RECT) -> bool {
    pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom
}

fn rects_overlap(a: RECT, b: RECT) -> bool {
    a.right > b.left && a.left < b.right && a.bottom > b.top && a.top < b.bottom
}

/// TrayNotifyWnd on the monitor that owns `band` (virtual-screen taskbar rect).
pub fn tray_rect_for_screen_band(taskbar_hwnd: HWND, band: RECT) -> Option<RECT> {
    let band_center = POINT {
        x: band.left + (band.right - band.left) / 2,
        y: band.top + (band.bottom - band.top) / 2,
    };
    let try_tray = |tray_hwnd: HWND| -> Option<RECT> {
        let rect = get_window_rect_safe(tray_hwnd)?;
        if rects_overlap(rect, band) {
            return Some(rect);
        }
        // Same monitor but shell reported tray coords on a spanning primary bar.
        let band_mon = monitor_rect_at_point(band_center)?;
        let tray_mon = monitor_rect_for_tray(rect)?;
        if band_mon.left == tray_mon.left && rect.left >= band.left {
            Some(rect)
        } else {
            None
        }
    };

    let tray_hwnd = find_child_window(taskbar_hwnd, "TrayNotifyWnd")?;
    try_tray(tray_hwnd)
}

/// Left edge of the notification area for a screen-positioned taskbar band.
pub fn tray_left_for_screen_band(taskbar_hwnd: HWND, band: RECT) -> i32 {
    if let Some(rect) = tray_rect_for_screen_band(taskbar_hwnd, band) {
        return rect.left;
    }
    // Typical notification-area width when the shell reports a mismatched tray HWND.
    band.right.saturating_sub(176)
}

/// Taskbar rectangle in virtual-screen coordinates for SetWindowPos.
pub fn get_taskbar_rect(taskbar_hwnd: HWND) -> Option<RECT> {
    let raw = get_window_rect_safe(taskbar_hwnd).or_else(|| {
        unsafe {
            let mut abd = APPBARDATA {
                cbSize: std::mem::size_of::<APPBARDATA>() as u32,
                hWnd: taskbar_hwnd,
                ..Default::default()
            };
            if SHAppBarMessage(ABM_GETTASKBARPOS, &mut abd) == 0 {
                return None;
            }
            Some(abd.rc)
        }
    })?;
    resolved_taskbar_band(taskbar_hwnd, raw)
        .or_else(|| Some(normalize_rect_to_virtual_screen(taskbar_hwnd, raw)))
}

/// Get the bounding rectangle of a window
pub fn get_window_rect_safe(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            Some(rect)
        } else {
            None
        }
    }
}

/// Hide only for true exclusive fullscreen: borderless foreground covers the
/// monitor and the shell has retracted taskbar chrome. Thumbnail peek must not trigger this.
pub fn should_hide_widget_for_fullscreen(self_hwnd: HWND, taskbar_hwnd: Option<HWND>) -> bool {
    if is_taskbar_peek_context(self_hwnd, taskbar_hwnd) {
        return false;
    }
    foreground_covers_monitor_borderless(self_hwnd, taskbar_hwnd)
}

/// Thumbnail peek / taskbar hover — only when preview UI is actually active.
pub fn is_taskbar_peek_context(self_hwnd: HWND, _taskbar_hwnd: Option<HWND>) -> bool {
    taskbar_peek_latch_active()
        || taskbar_interactive_preview_active()
        || shell_preview_ui_active()
        || taskbar_thumbnail_preview_present()
}

fn any_system_taskbar_visible() -> bool {
    for class in ["Shell_TrayWnd", "Shell_SecondaryTrayWnd"] {
        let hwnd = find_top_level_window(class);
        if !hwnd.0.is_null() && taskbar_hwnd_is_on_screen(hwnd) {
            return true;
        }
    }
    false
}

/// Cursor in the bottom band of the monitor under the pointer (taskbar + thumbnail zone).
pub fn cursor_in_taskbar_zone(_anchor_hwnd: HWND) -> bool {
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return false;
        }
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let mon = info.rcMonitor;
        const ZONE_PX: i32 = 320;
        pt.y >= mon.bottom - ZONE_PX && pt.x >= mon.left && pt.x <= mon.right
    }
}

fn window_covers_monitor(win_rect: RECT, mon: RECT) -> bool {
    const SLACK: i32 = 12;
    let cover_w = (win_rect.right.min(mon.right) - win_rect.left.max(mon.left)).max(0);
    let cover_h = (win_rect.bottom.min(mon.bottom) - win_rect.top.max(mon.top)).max(0);
    let mon_w = mon.right - mon.left;
    let mon_h = mon.bottom - mon.top;
    cover_w >= mon_w - SLACK && cover_h >= mon_h - SLACK
}

fn foreground_covers_monitor_borderless(self_hwnd: HWND, taskbar_hwnd: Option<HWND>) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() || fg == self_hwnd {
            return false;
        }

        if let Some(class_name) = window_class_name(fg) {
            if is_shell_foreground_class(&class_name) {
                return false;
            }
        }

        let Some(win_rect) = get_window_rect_safe(fg) else {
            return false;
        };

        let monitor = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);

        // Only suppress if the fullscreen-covering window is on the same
        // monitor as the widget's own taskbar. A fullscreen app on a second
        // monitor must not hide a widget whose taskbar is untouched.
        if let Some(taskbar_hwnd) = taskbar_hwnd {
            let widget_monitor = MonitorFromWindow(taskbar_hwnd, MONITOR_DEFAULTTONEAREST);
            if widget_monitor != monitor {
                return false;
            }
        }

        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() == false {
            return false;
        }

        if !window_covers_monitor(win_rect, info.rcMonitor) {
            return false;
        }

        let work_w = info.rcWork.right - info.rcWork.left;
        let work_h = info.rcWork.bottom - info.rcWork.top;
        let mon_w = info.rcMonitor.right - info.rcMonitor.left;
        let mon_h = info.rcMonitor.bottom - info.rcMonitor.top;
        let taskbar_chrome = monitor_shows_taskbar_chrome(work_w, work_h, mon_w, mon_h);

        // Exclusive fullscreen: shell retracted the work area.
        if !taskbar_chrome {
            return true;
        }

        // Browser/player fullscreen can still leave rcWork inset while the HWND
        // rect covers the monitor (including over the taskbar band).
        const BAND_SLACK: i32 = 4;
        win_rect.bottom >= info.rcMonitor.bottom - BAND_SLACK
    }
}

fn window_class_name(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut class_name = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class_name);
        if len > 0 {
            Some(String::from_utf16_lossy(&class_name[..len as usize]))
        } else {
            None
        }
    }
}

fn is_shell_foreground_class(class_name: &str) -> bool {
    matches!(
        class_name,
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "Windows.UI.Core.CoreWindow"
            | "TaskListThumbnailWnd"
            | "CThumbnailWnd"
            | "MultitaskingViewFrame"
            | "XamlExplorerHostIslandWindow"
            | "TopLevelWindowForOverflowXamlIsland"
            | "Windows.UI.Composition.DesktopWindowContentBridge"
    ) || class_name.starts_with("WindowsInternal")
}

/// Taskbar hover / thumbnail peek — including when the peeked app has focus.
fn taskbar_interactive_preview_active() -> bool {
    taskbar_thumbnail_preview_visible()
}

/// Visible thumbnail flyout above a taskbar icon.
fn taskbar_thumbnail_preview_visible() -> bool {
    thumbnail_preview_visible_for_class("TaskListThumbnailWnd")
        || thumbnail_preview_visible_for_class("CThumbnailWnd")
}

/// Thumbnail flyout visible (ignore helper HWNDs that exist permanently).
fn taskbar_thumbnail_preview_present() -> bool {
    taskbar_thumbnail_preview_visible()
}

fn thumbnail_preview_visible_for_class(class: &str) -> bool {
    unsafe {
        let hwnd = find_top_level_window(class);
        if hwnd.0.is_null() {
            return false;
        }
        IsWindowVisible(hwnd).as_bool()
    }
}

fn thumbnail_preview_exists_for_class(class: &str) -> bool {
    unsafe {
        let hwnd = find_top_level_window(class);
        !hwnd.0.is_null()
    }
}

fn find_top_level_window(class: &str) -> HWND {
    unsafe {
        let wide: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
        FindWindowW(PCWSTR(wide.as_ptr()), PCWSTR::null()).unwrap_or_default()
    }
}

fn is_preview_ui_class(class_name: &str) -> bool {
    matches!(
        class_name,
        "TaskListThumbnailWnd" | "CThumbnailWnd" | "TaskListWnd" | "MSTaskSwWClass"
    )
}

/// Any visible shell preview flyout (taskbar hover / Win11 variants).
pub fn shell_preview_ui_active() -> bool {
    struct Scan {
        found: bool,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let scan = &mut *(lparam.0 as *mut Scan);
        if scan.found {
            return BOOL(1);
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        if let Some(class_name) = window_class_name(hwnd) {
            if is_preview_ui_class(&class_name) {
                scan.found = true;
            }
        }
        BOOL(1)
    }

    unsafe {
        let mut scan = Scan { found: false };
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut scan as *mut _ as isize));
        scan.found
    }
}

/// Taskbar peek minimizes other windows; foreground is the peeked app.
pub fn desktop_peek_active(self_hwnd: HWND) -> bool {
    struct Scan {
        fg: HWND,
        self_hwnd: HWND,
        iconic: u32,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let scan = &mut *(lparam.0 as *mut Scan);
        if hwnd == scan.fg || hwnd == scan.self_hwnd || !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
            return BOOL(1);
        }
        if let Some(class_name) = window_class_name(hwnd) {
            if is_shell_foreground_class(&class_name) {
                return BOOL(1);
            }
        }
        if IsIconic(hwnd).as_bool() {
            scan.iconic += 1;
        }
        BOOL(1)
    }

    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() || fg == self_hwnd || IsIconic(fg).as_bool() {
            return false;
        }
        let mut scan = Scan {
            fg,
            self_hwnd,
            iconic: 0,
        };
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut scan as *mut _ as isize));
        scan.iconic >= 1
    }
}

/// Physical taskbar window is mostly visible on its monitor (not auto-hidden off-screen).
pub fn taskbar_hwnd_is_on_screen(taskbar_hwnd: HWND) -> bool {
    unsafe {
        if taskbar_hwnd.0.is_null() || !IsWindow(taskbar_hwnd).as_bool() {
            return false;
        }
        if !IsWindowVisible(taskbar_hwnd).as_bool() {
            return false;
        }
        let Some(rect) = get_window_rect_safe(taskbar_hwnd) else {
            return false;
        };
        let monitor = MonitorFromWindow(taskbar_hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let mon = info.rcMonitor;
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return false;
        }
        let intersect_w = (rect.right.min(mon.right) - rect.left.max(mon.left)).max(0);
        let intersect_h = (rect.bottom.min(mon.bottom) - rect.top.max(mon.top)).max(0);
        let intersect_area = intersect_w * intersect_h;
        let window_area = width * height;
        intersect_area * 2 >= window_area
    }
}

/// True when the shell still reserves taskbar space on the monitor (work area < monitor).
pub fn taskbar_still_visible_on_monitor(hwnd: HWND) -> bool {
    unsafe {
        if hwnd.0.is_null() {
            return true;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return true;
        }
        let work_w = info.rcWork.right - info.rcWork.left;
        let work_h = info.rcWork.bottom - info.rcWork.top;
        let mon_w = info.rcMonitor.right - info.rcMonitor.left;
        let mon_h = info.rcMonitor.bottom - info.rcMonitor.top;
        monitor_shows_taskbar_chrome(work_w, work_h, mon_w, mon_h)
    }
}

fn monitor_shows_taskbar_chrome(work_w: i32, work_h: i32, mon_w: i32, mon_h: i32) -> bool {
    const SLACK: i32 = 8;
    (mon_w - work_w) > SLACK || (mon_h - work_h) > SLACK
}

#[cfg(test)]
mod fullscreen_tests {
    use super::*;

    #[test]
    fn shell_classes_include_taskbar_preview_and_switcher() {
        assert!(is_shell_foreground_class("TaskListThumbnailWnd"));
        assert!(is_shell_foreground_class("MultitaskingViewFrame"));
        assert!(is_shell_foreground_class("WindowsInternal.ComposableShellWindow"));
        assert!(!is_shell_foreground_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn monitor_shows_taskbar_when_work_area_is_inset() {
        assert!(monitor_shows_taskbar_chrome(1920, 1032, 1920, 1080));
        assert!(!monitor_shows_taskbar_chrome(1920, 1080, 1920, 1080));
    }

    #[test]
    fn peek_context_includes_work_area_inset() {
        // monitor_shows_taskbar_chrome is the rcWork vs rcMonitor proxy used at runtime
        assert!(monitor_shows_taskbar_chrome(1920, 1040, 1920, 1080));
    }

    #[test]
    fn preview_ui_class_names_are_recognized() {
        assert!(is_preview_ui_class("TaskListThumbnailWnd"));
        assert!(is_preview_ui_class("CThumbnailWnd"));
        assert!(!is_preview_ui_class("SomeAppPreviewHost"));
        assert!(!is_preview_ui_class("Chrome_WidgetWin_1"));
    }
}

/// Left edge of the draggable widget band, relative to `taskbar_rect.left`.
/// Secondary taskbars span a full monitor edge; do not treat the pinned-app list
/// right edge as the left bound (that pushed TRAY_OFFSET_LEFTMOST to x≈1174).
/// HARD REQUIREMENT (do not "improve" this): the widget's default/leftmost
/// position is screen x=0 — the taskbar's own physical left edge — always,
/// unconditionally, on every taskbar (primary or secondary). Not "the left
/// edge of the icon cluster," not "clear of Start/Search/Task View chrome."
/// x=0. This has been re-litigated by more than one agent session already;
/// each time, "leftmost" got reinterpreted as "leftmost without overlapping
/// existing taskbar content," which is a *different, smaller* requirement
/// the owner never asked for and has explicitly rejected. If some other
/// caller needs a collision-aware inset for a different purpose, give it a
/// new, differently-named function — do not reintroduce that behavior here.
pub fn taskbar_placement_band_left(_taskbar_hwnd: HWND, _taskbar_rect: RECT) -> i32 {
    0
}

/// Left edge of visible taskbar chrome (relative to taskbar rect).
pub fn taskbar_content_left(taskbar_hwnd: HWND, taskbar_rect: RECT) -> i32 {
    taskbar_visible_left_screen(taskbar_hwnd, taskbar_rect)
        .saturating_sub(taskbar_rect.left)
        .max(0)
}

/// Left edge of visible taskbar chrome in screen coordinates.
pub fn taskbar_visible_left_screen(taskbar_hwnd: HWND, taskbar_rect: RECT) -> i32 {
    taskbar_visible_left(taskbar_hwnd, taskbar_rect)
}

/// Right edge of the pinned-app band in screen coordinates.
pub fn pin_band_right(taskbar_hwnd: HWND, taskbar_rect: RECT) -> i32 {
    scan_taskbar_band(taskbar_hwnd, taskbar_rect).1
}

/// Tell DWM to never ghost/hide this window during Aero Peek — both the
/// taskbar-thumbnail peek (hovering a taskbar icon's preview) and Show
/// Desktop peek. This is the actual fix for "widget disappears on preview":
/// DWM peek is a compositor-level effect that makes other top-level windows
/// nearly transparent while the peeked window (or the desktop) is shown, and
/// it does this regardless of our own window's visibility/z-order state — no
/// amount of polling GetForegroundWindow, watching for TaskListThumbnailWnd,
/// or reordering z-order after the fact can prevent DWM from ghosting a
/// window it doesn't know should be excluded. `DWMWA_EXCLUDED_FROM_PEEK` is
/// the attribute that opts a window out of that ghosting entirely. Call this
/// once, right after the window is created.
pub fn exclude_from_peek(hwnd: HWND) {
    unsafe {
        let exclude: BOOL = BOOL(1);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_EXCLUDED_FROM_PEEK,
            &exclude as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<BOOL>() as u32,
        );
    }
}

/// Ensure WS_EX_LAYERED is set so UpdateLayeredWindow can push pixels.
pub fn ensure_layered_style(hwnd: HWND) {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        if ex_style & (WS_EX_LAYERED.0 as i32) == 0 {
            let _ = SetWindowLongW(
                hwnd,
                GWL_EXSTYLE,
                ex_style | WS_EX_LAYERED.0 as i32 | WS_EX_TOOLWINDOW.0 as i32 | WS_EX_NOACTIVATE.0 as i32,
            );
        }
    }
}

/// Remove WS_EX_LAYERED so the child paints via normal WM_PAINT inside Shell_TrayWnd.
pub fn strip_layered_style(hwnd: HWND) {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let cleared = ex_style & !(WS_EX_LAYERED.0 as i32);
        let _ = SetWindowLongW(
            hwnd,
            GWL_EXSTYLE,
            cleared | WS_EX_TOOLWINDOW.0 as i32 | WS_EX_NOACTIVATE.0 as i32,
        );
    }
}

/// Embed our window as a child of the taskbar
pub fn embed_in_taskbar(hwnd: HWND, taskbar_hwnd: HWND) {
    unsafe {
        // Change from popup to child
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let new_style = (style & !WS_POPUP_STYLE) | WS_CHILD_STYLE | WS_CLIPSIBLINGS_STYLE;
        let _ = SetWindowLongW(hwnd, GWL_STYLE, new_style as i32);

        let _ = SetParent(hwnd, taskbar_hwnd);
    }
}

/// Detach our window from the taskbar, restoring popup style and topmost z-order
pub fn detach_from_taskbar(hwnd: HWND) {
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let new_style = (style & !(WS_CHILD_STYLE | WS_CLIPSIBLINGS_STYLE)) | WS_POPUP_STYLE;
        let _ = SetWindowLongW(hwnd, GWL_STYLE, new_style as i32);
        let _ = SetParent(hwnd, None);
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Reassert popup z-order (TOPMOST normally; taskbar band during peek).
pub fn raise_above_taskbar(hwnd: HWND, taskbar_hwnd: Option<HWND>) {
    position_popup_zorder(hwnd, taskbar_hwnd, false, 0, 0, 0, 0);
}

pub fn raise_on_taskbar_band(hwnd: HWND, taskbar_hwnd: Option<HWND>) {
    position_popup_zorder(hwnd, taskbar_hwnd, true, 0, 0, 0, 0);
}

/// Place the widget directly above the foreground window, then restore topmost.
pub fn raise_above_foreground(hwnd: HWND) {
    unsafe {
        let fg = GetForegroundWindow();
        if !fg.0.is_null() && fg != hwnd {
            let _ = SetWindowPos(
                hwnd,
                fg,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Drop below exclusive-fullscreen apps (real taskbar behaviour) without SW_HIDE.
pub fn lower_below_fullscreen(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND_NOTOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}


/// Place the layered popup above the taskbar (TOPMOST — peek band handled in sync).
pub fn position_above_taskbar(hwnd: HWND, taskbar_hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    position_popup_zorder(hwnd, Some(taskbar_hwnd), false, x, y, w, h);
}

/// Place/move the popup without HWND_TOPMOST (exclusive fullscreen suppression).
pub fn position_notopmost_popup(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let _ = SetWindowPos(hwnd, HWND_NOTOPMOST, x, y, w, h, SWP_NOACTIVATE);
    }
}

fn position_popup_zorder(
    hwnd: HWND,
    taskbar_hwnd: Option<HWND>,
    use_taskbar_band: bool,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    unsafe {
        if use_taskbar_band {
            if let Some(tb) = taskbar_hwnd {
                if !tb.0.is_null() {
                    if w > 0 && h > 0 {
                        let _ = SetWindowPos(hwnd, tb, x, y, w, h, SWP_NOACTIVATE);
                    } else {
                        let _ = SetWindowPos(
                            hwnd,
                            tb,
                            0,
                            0,
                            0,
                            0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                    }
                    return;
                }
            }
        }
        if w > 0 && h > 0 {
            let _ = SetWindowPos(hwnd, HWND_NOTOPMOST, x, y, w, h, SWP_NOACTIVATE);
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
        } else {
            let _ = SetWindowPos(
                hwnd,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

/// Fallback when no taskbar handle is available yet.
pub fn position_topmost_popup(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE,
        );
    }
}

/// Place a popup layered widget in the taskbar band (screen coords), just above the taskbar z-order.
pub fn position_on_taskbar_band(
    hwnd: HWND,
    taskbar_hwnd: HWND,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            taskbar_hwnd,
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE,
        );
    }
}

/// Move the window
pub fn move_window(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let _ = MoveWindow(hwnd, x, y, w, h, true);
    }
}

/// Move the window asynchronously — posts a move request to the owning thread's queue
/// instead of blocking cross-process. Required for WS_CHILD windows embedded in Explorer.
pub fn move_window_async(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND::default(),
            x,
            y,
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        );
    }
}

/// Set up a WinEvent hook for tray location changes
pub fn set_tray_event_hook(
    thread_id: u32,
    callback: unsafe extern "system" fn(HWINEVENTHOOK, u32, HWND, i32, i32, u32, u32),
) -> Option<HWINEVENTHOOK> {
    unsafe {
        let hook = SetWinEventHook(
            EVENT_OBJECT_LOCATIONCHANGE,
            EVENT_OBJECT_LOCATIONCHANGE,
            None,
            Some(callback),
            0,
            thread_id,
            WINEVENT_OUTOFCONTEXT,
        );
        if hook.is_invalid() {
            None
        } else {
            Some(hook)
        }
    }
}

/// Set up a system-wide WinEvent hook for foreground window changes. Used to
/// force an immediate repaint the instant focus moves to/from a shell/XAML
/// surface (Start menu, Search, task switches), instead of waiting up to
/// TIMER_FULLSCREEN_CHECK's 500ms poll interval to notice DWM dropped this
/// window's composited content during the transition.
pub fn set_foreground_event_hook(
    callback: unsafe extern "system" fn(HWINEVENTHOOK, u32, HWND, i32, i32, u32, u32),
) -> Option<HWINEVENTHOOK> {
    unsafe {
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(callback),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if hook.is_invalid() {
            None
        } else {
            Some(hook)
        }
    }
}

/// Get the thread ID that owns a window
pub fn get_window_thread_id(hwnd: HWND) -> u32 {
    unsafe { GetWindowThreadProcessId(hwnd, None) }
}

/// Unhook a WinEvent hook
pub fn unhook_win_event(hook: HWINEVENTHOOK) {
    unsafe {
        let _ = UnhookWinEvent(hook);
    }
}

/// Convert a Rust string to a null-terminated wide string
pub fn wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Format a month/day pair respecting the Windows system locale
/// (separator, and whether day or month comes first).
/// Returns e.g. "9/15" (en-US), "15/9" (en-GB), "15.9" (de-DE).
pub fn format_month_day_locale(month: u8, day: u8) -> String {
    if let Some(pattern) = locale_short_date_pattern() {
        let lower = pattern.to_lowercase();
        // Find the separator: first non-alphabetic, non-quote character
        let sep = lower
            .chars()
            .find(|c| !c.is_alphabetic() && *c != '\'')
            .unwrap_or('/');
        // day-first when 'd' appears before 'm' in the pattern (e.g. "dd/MM/yyyy")
        let d_pos = lower.find('d');
        let m_pos = lower.find('m');
        return match (d_pos, m_pos) {
            (Some(d), Some(m)) if d < m => format!("{}{}{}", day, sep, month),
            (Some(_), Some(_)) => format!("{}{}{}", month, sep, day),
            _ => format!("{}/{}", month, day), // malformed pattern — safe fallback
        };
    }
    format!("{}/{}", month, day)
}

fn locale_short_date_pattern() -> Option<String> {
    unsafe {
        let mut buf = [0u16; 256];
        let len = GetLocaleInfoW(LOCALE_USER_DEFAULT, LOCALE_SSHORTDATE, Some(&mut buf));
        if len > 1 && (len as usize) <= buf.len() {
            Some(String::from_utf16_lossy(&buf[..len as usize - 1]).to_string())
        } else {
            None
        }
    }
}

/// COLORREF wrapper (RGB packed into u32)
pub fn colorref(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

/// Color helper
#[derive(Clone, Copy, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    #[allow(dead_code)]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn from_hex(hex: &str) -> Self {
        let hex = hex.trim_start_matches('#');
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        Self { r, g, b }
    }

    pub fn to_colorref(self) -> u32 {
        colorref(self.r, self.g, self.b)
    }
}
