use std::sync::Mutex;

use windows::core::BOOL;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_GETTASKBARPOS, APPBARDATA};
use windows::Win32::UI::WindowsAndMessaging::*;

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
pub const TIMER_WINDOW_STATE: usize = 5;
pub const TIMER_MOUSE_CLICK: usize = 6;
pub const TIMER_TRAY_HOVER: usize = 7;
pub const TIMER_CLOCK: usize = 8;

// Custom messages
pub const WM_APP: u32 = 0x8000;
pub const WM_APP_USAGE_UPDATED: u32 = WM_APP + 1;
pub const WM_APP_TRAY: u32 = WM_APP + 3;
pub const WM_APP_SETTINGS_UPDATED: u32 = WM_APP + 5;
pub const WM_APP_REFRESH_NOW: u32 = WM_APP + 6;
pub const WM_APP_QUIT: u32 = WM_APP + 7;
pub const WM_APP_OPEN_DASHBOARD: u32 = WM_APP + 8;

static DESKTOP_HOST: Mutex<Option<(isize, isize)>> = Mutex::new(None);

#[derive(Clone, Copy, Debug)]
pub struct TaskbarWindow {
    pub hwnd: HWND,
    pub rect: RECT,
}

#[derive(Clone, Copy, Debug)]
pub struct DisplayMonitor {
    pub handle: HMONITOR,
    pub rect: RECT,
    pub primary: bool,
}

/// Parenting and sibling placement for a desktop-nested theme surface.
/// `insert_after` places the surface in Explorer's desktop z-order band.
#[derive(Clone, Copy, Debug)]
pub struct DesktopHost {
    pub parent: HWND,
    pub insert_after: HWND,
}

pub fn find_monitors() -> Vec<DisplayMonitor> {
    unsafe extern "system" fn enum_proc(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let result = &mut *(data.0 as *mut Vec<DisplayMonitor>);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info).as_bool() } {
            result.push(DisplayMonitor {
                handle: monitor,
                rect: info.rcMonitor,
                primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        BOOL(1)
    }
    let mut result: Vec<DisplayMonitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(enum_proc),
            LPARAM(&mut result as *mut _ as isize),
        );
    }
    result.sort_by_key(|display| (!display.primary, display.rect.left, display.rect.top));
    result
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

/// Find a child window by class name
pub fn find_child_window(parent: HWND, class_name: &str) -> Option<HWND> {
    unsafe {
        let class = wide_str(class_name);
        match FindWindowExW(
            Some(parent),
            None,
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

pub fn window_class_name(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut class_name = [0u16; 128];
        let len = GetClassNameW(hwnd, &mut class_name);
        (len > 0).then(|| String::from_utf16_lossy(&class_name[..len as usize]))
    }
}

/// Embed our window as a child of the taskbar
pub fn embed_in_taskbar(hwnd: HWND, taskbar_hwnd: HWND) {
    embed_as_child(hwnd, taskbar_hwnd);
}

/// Host a layered surface inside a shell-owned window. Parenting makes the
/// surface share the host's visibility and z-order instead of competing with
/// it as an independent topmost popup.
pub fn embed_as_child(hwnd: HWND, parent: HWND) {
    unsafe {
        let current_parent = GetParent(hwnd).ok();
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongW(
            hwnd,
            GWL_EXSTYLE,
            (ex_style | WS_EX_TOOLWINDOW.0 as i32 | WS_EX_NOACTIVATE.0 as i32)
                & !(WS_EX_TOPMOST.0 as i32),
        );

        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let new_style = (style & !WS_POPUP_STYLE) | WS_CHILD_STYLE | WS_CLIPSIBLINGS_STYLE;
        let _ = SetWindowLongW(hwnd, GWL_STYLE, new_style as i32);

        if current_parent != Some(parent) {
            let _ = SetParent(hwnd, Some(parent));
        }
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Restore a shell-hosted surface to a regular top-level popup.
pub fn make_popup(hwnd: HWND, topmost: bool) {
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let new_style = (style & !WS_CHILD_STYLE & !WS_CLIPSIBLINGS_STYLE) | WS_POPUP_STYLE;
        let _ = SetWindowLongW(hwnd, GWL_STYLE, new_style as i32);
        let _ = SetParent(hwnd, None);

        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let ex_style = if topmost {
            ex_style | WS_EX_TOPMOST.0 as i32
        } else {
            ex_style & !(WS_EX_TOPMOST.0 as i32)
        };
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style);
        let _ = SetWindowPos(
            hwnd,
            Some(if topmost {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            }),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Resolve Explorer's desktop rendering band. Current Windows 11 builds need
/// our layered child to be parented to Progman, below SHELLDLL_DefView but
/// above the wallpaper WorkerW. Older shells accept a child of the separate
/// top-level wallpaper WorkerW.
pub fn find_desktop_host() -> Option<DesktopHost> {
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if find_child_window(hwnd, "SHELLDLL_DefView").is_some() {
            let class = wide_str("WorkerW");
            if let Ok(worker) = unsafe {
                FindWindowExW(
                    None,
                    Some(hwnd),
                    PCWSTR::from_raw(class.as_ptr()),
                    PCWSTR::null(),
                )
            } {
                if !worker.is_invalid() {
                    let result = unsafe { &mut *(lparam.0 as *mut HWND) };
                    *result = worker;
                    return BOOL(0);
                }
            }
        }
        BOOL(1)
    }

    unsafe {
        let cached = *DESKTOP_HOST
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some((parent_value, insert_after_value)) = cached {
            let parent = HWND(parent_value as *mut _);
            let insert_after = HWND(insert_after_value as *mut _);
            let sibling_is_valid =
                insert_after == HWND_BOTTOM || IsWindow(Some(insert_after)).as_bool();
            if IsWindow(Some(parent)).as_bool()
                && IsWindowVisible(parent).as_bool()
                && sibling_is_valid
            {
                return Some(DesktopHost {
                    parent,
                    insert_after,
                });
            }
        }

        let progman_class = wide_str("Progman");
        let progman = FindWindowW(PCWSTR::from_raw(progman_class.as_ptr()), PCWSTR::null())
            .ok()
            .filter(|hwnd| !hwnd.is_invalid())?;

        // Ask Explorer to create the wallpaper WorkerW on shell versions that
        // do not keep it alive until a desktop-hosted surface is requested.
        let _ = SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0xD),
            LPARAM(0),
            SMTO_NORMAL,
            1_000,
            None,
        );
        let _ = SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0xD),
            LPARAM(1),
            SMTO_NORMAL,
            1_000,
            None,
        );

        // In the raised-desktop architecture Progman has both DefView and the
        // wallpaper WorkerW as direct children. A layered child of WorkerW is
        // suppressed by its DirectComposition wallpaper surface. Instead, our
        // child must be a Progman sibling immediately below DefView; DefView is
        // mostly transparent and paints the icons above us.
        let direct_def_view = find_child_window(progman, "SHELLDLL_DefView");
        let host = if let Some(def_view) = direct_def_view {
            DesktopHost {
                parent: progman,
                insert_after: def_view,
            }
        } else {
            // Older shells move DefView under another top-level window and
            // expose a separate wallpaper WorkerW behind it.
            let mut top_level_worker = HWND::default();
            let _ = EnumWindows(
                Some(enum_proc),
                LPARAM(&mut top_level_worker as *mut _ as isize),
            );
            DesktopHost {
                parent: if top_level_worker.is_invalid() {
                    progman
                } else {
                    top_level_worker
                },
                insert_after: if top_level_worker.is_invalid() {
                    HWND_TOP
                } else {
                    HWND_BOTTOM
                },
            }
        };
        *DESKTOP_HOST
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some((host.parent.0 as isize, host.insert_after.0 as isize));
        Some(host)
    }
}

/// Move the window
pub fn move_window(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let _ = MoveWindow(hwnd, x, y, w, h, true);
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
