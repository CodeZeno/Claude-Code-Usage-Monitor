use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Globalization::GetLocaleInfoW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_GETTASKBARPOS, APPBARDATA};
use windows::Win32::UI::WindowsAndMessaging::*;

const LOCALE_USER_DEFAULT: u32 = 0x0400;
// Short date format pattern (e.g. "M/d/yyyy")
const LOCALE_SSHORTDATE: u32 = 0x001F;

// Window style constants
pub const WS_POPUP_STYLE: u32 = 0x80000000;
pub const WS_CHILD_STYLE: u32 = 0x40000000;
pub const WS_CLIPSIBLINGS_STYLE: u32 = 0x04000000;

// Win event constants
pub const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
pub const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;

// Timer IDs
pub const TIMER_POLL: usize = 1;
pub const TIMER_COUNTDOWN: usize = 2;
pub const TIMER_RESET_POLL: usize = 3;
pub const TIMER_UPDATE_CHECK: usize = 4;
pub const TIMER_DRAG: usize = 5;
pub const TIMER_WIDGET_KEEPALIVE: usize = 6;

// Custom messages
pub const WM_APP: u32 = 0x8000;
pub const WM_APP_USAGE_UPDATED: u32 = WM_APP + 1;
pub const WM_APP_TRAY: u32 = WM_APP + 3;

#[derive(Clone, Copy, Debug)]
pub struct TaskbarWindow {
    pub hwnd: HWND,
    pub rect: RECT,
}

pub fn find_taskbars() -> Vec<TaskbarWindow> {
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let taskbars = &mut *(lparam.0 as *mut Vec<TaskbarWindow>);
        let mut class_name = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut class_name) };
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_name[..len as usize]);
            if class_name == "Shell_TrayWnd" || class_name == "Shell_SecondaryTrayWnd" {
                if let Some(rect) = get_taskbar_rect(hwnd).or_else(|| get_window_rect_safe(hwnd)) {
                    taskbars.push(TaskbarWindow { hwnd, rect });
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
                if rect.right > rect.left {
                    scan.visible_left = if scan.found {
                        scan.visible_left.min(rect.left)
                    } else {
                        rect.left
                    };
                    scan.found = true;
                }
            }
        }
    }
    unsafe {
        let _ = EnumChildWindows(hwnd, Some(scan_visible_left_proc), lparam);
    }
    BOOL(1)
}

fn taskbar_visible_left(taskbar_hwnd: HWND, taskbar_rect: RECT) -> i32 {
    let mut scan = VisibleLeftScan {
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

/// Get taskbar position via SHAppBarMessage
pub fn get_taskbar_rect(taskbar_hwnd: HWND) -> Option<RECT> {
    unsafe {
        let mut class_name = [0u16; 64];
        let len = GetClassNameW(taskbar_hwnd, &mut class_name);
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_name[..len as usize]);
            if class_name == "Shell_SecondaryTrayWnd" {
                return get_window_rect_safe(taskbar_hwnd);
            }
        }

        let mut abd = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: taskbar_hwnd,
            ..Default::default()
        };
        let result = SHAppBarMessage(ABM_GETTASKBARPOS, &mut abd);
        if result == 0 {
            return None;
        }
        Some(abd.rc)
    }
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

/// Left edge of visible taskbar chrome (relative to taskbar rect).
pub fn taskbar_content_left(taskbar_hwnd: HWND, taskbar_rect: RECT) -> i32 {
    taskbar_visible_left_screen(taskbar_hwnd, taskbar_rect).saturating_sub(taskbar_rect.left)
}

/// Left edge of visible taskbar chrome in screen coordinates.
pub fn taskbar_visible_left_screen(taskbar_hwnd: HWND, taskbar_rect: RECT) -> i32 {
    taskbar_visible_left(taskbar_hwnd, taskbar_rect)
}

/// Right edge of the pinned-app band in screen coordinates.
pub fn pin_band_right(taskbar_hwnd: HWND, taskbar_rect: RECT) -> i32 {
    scan_taskbar_band(taskbar_hwnd, taskbar_rect).1
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

/// Place the popup widget above Shell_TrayWnd in the topmost z-order band.
pub fn raise_above_taskbar(hwnd: HWND, _taskbar_hwnd: Option<HWND>) {
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

/// Place the layered popup immediately above the taskbar in Z-order.
pub fn position_above_taskbar(hwnd: HWND, _taskbar_hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND_NOTOPMOST,
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE,
        );
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
