use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Accessibility::HWINEVENTHOOK;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use crate::models::UsageData;
use crate::native_interop::{self, Color, TIMER_COUNTDOWN, TIMER_POLL, TIMER_RESET_POLL, WM_APP_USAGE_UPDATED};
use crate::poller;
use crate::theme;

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

/// Shared application state
struct AppState {
    hwnd: SendHwnd,
    taskbar_hwnd: Option<HWND>,
    tray_notify_hwnd: Option<HWND>,
    win_event_hook: Option<HWINEVENTHOOK>,
    is_dark: bool,
    embedded: bool,

    session_percent: f64,
    session_text: String,
    weekly_percent: f64,
    weekly_text: String,

    data: Option<UsageData>,

    poll_interval_ms: u32,
    retry_count: u32,
    last_poll_ok: bool,

    tray_offset: i32,
    dragging: bool,
    drag_start_mouse_x: i32,
    drag_start_offset: i32,
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
const IDM_RESET_POSITION: u16 = 30;

const DIVIDER_HIT_ZONE: i32 = 13; // LEFT_DIVIDER_W + DIVIDER_RIGHT_MARGIN

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

fn parse_json_i32(content: &str, key: &str) -> Option<i32> {
    let needle = format!("\"{}\"", key);
    let pos = content.find(&needle)?;
    let rest = &content[pos + needle.len()..];
    let colon = rest.find(':')?;
    let num_str: String = rest[colon + 1..]
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    num_str.parse().ok()
}

fn load_settings() -> (i32, u32) {
    let content = match std::fs::read_to_string(settings_path()) {
        Ok(c) => c,
        Err(_) => return (0, POLL_15_MIN),
    };
    let tray_offset = parse_json_i32(&content, "tray_offset").unwrap_or(0);
    let poll_interval = parse_json_i32(&content, "poll_interval_ms")
        .map(|v| v as u32)
        .unwrap_or(POLL_15_MIN);
    (tray_offset, poll_interval)
}

fn save_settings(tray_offset: i32, poll_interval_ms: u32) {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = format!(
        "{{\n  \"tray_offset\": {},\n  \"poll_interval_ms\": {}\n}}",
        tray_offset, poll_interval_ms
    );
    let _ = std::fs::write(path, json);
}

fn save_state_settings() {
    let state = lock_state();
    if let Some(s) = state.as_ref() {
        save_settings(s.tray_offset, s.poll_interval_ms);
    }
}

const STARTUP_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_REGISTRY_KEY: &str = "ClaudeCodeUsageMonitor";

/// Returns true only if the startup registry value points to this executable.
fn is_startup_enabled() -> bool {
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
        let wide_slice = std::slice::from_raw_parts(
            buf.as_ptr() as *const u16,
            data_size as usize / 2,
        );
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
const TEXT_WIDTH: i32 = 62;
const RIGHT_MARGIN: i32 = 1;
const WIDGET_HEIGHT: i32 = 46;

fn total_widget_width() -> i32 {
    LEFT_DIVIDER_W
        + DIVIDER_RIGHT_MARGIN
        + LABEL_WIDTH
        + LABEL_RIGHT_MARGIN
        + (SEGMENT_W + SEGMENT_GAP) * SEGMENT_COUNT
        - SEGMENT_GAP
        + BAR_RIGHT_MARGIN
        + TEXT_WIDTH
        + RIGHT_MARGIN
}

pub fn run() {
    // Single-instance guard: silently exit if another instance is running
    let mutex_name = native_interop::wide_str("Global\\ClaudeCodeUsageMonitor");
    let _mutex = unsafe {
        let handle = CreateMutexW(None, false, PCWSTR::from_raw(mutex_name.as_ptr()));
        match handle {
            Ok(h) => {
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    return;
                }
                h
            }
            Err(_) => return,
        }
    };

    let class_name = native_interop::wide_str("ClaudeCodeUsageMonitor");

    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: HINSTANCE(hinstance.0),
            hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };

        RegisterClassExW(&wc);

        // Create as layered popup (will be reparented into taskbar)
        let title = native_interop::wide_str("Claude Code Usage Monitor");
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP,
            0,
            0,
            total_widget_width(),
            WIDGET_HEIGHT,
            HWND::default(),
            HMENU::default(),
            hinstance,
            None,
        )
        .unwrap();

        let is_dark = theme::is_dark_mode();
        let mut embedded = false;
        let (saved_offset, saved_poll_interval) = load_settings();

        {
            let mut state = lock_state();
            *state = Some(AppState {
                hwnd: SendHwnd::from_hwnd(hwnd),
                taskbar_hwnd: None,
                tray_notify_hwnd: None,
                win_event_hook: None,
                is_dark,
                embedded: false,
                session_percent: 0.0,
                session_text: "--".to_string(),
                weekly_percent: 0.0,
                weekly_text: "--".to_string(),
                data: None,
                poll_interval_ms: saved_poll_interval,
                retry_count: 0,
                last_poll_ok: false,
                tray_offset: saved_offset,
                dragging: false,
                drag_start_mouse_x: 0,
                drag_start_offset: 0,
            });
        }

        // Try to embed in taskbar
        if let Some(taskbar_hwnd) = native_interop::find_taskbar() {
            native_interop::embed_in_taskbar(hwnd, taskbar_hwnd);
            embedded = true;

            let mut state = lock_state();
            let s = state.as_mut().unwrap();
            s.taskbar_hwnd = Some(taskbar_hwnd);
            s.embedded = true;

            let tray_notify = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd");
            s.tray_notify_hwnd = tray_notify;

            if let Some(tray_hwnd) = tray_notify {
                let thread_id = native_interop::get_window_thread_id(tray_hwnd);
                let hook =
                    native_interop::set_tray_event_hook(thread_id, on_tray_location_changed);
                s.win_event_hook = hook;
            }
        }

        // If not embedded, fall back to topmost popup with SetLayeredWindowAttributes
        if !embedded {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
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

        // Position and show
        position_at_taskbar();
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        // Initial render via UpdateLayeredWindow (for embedded) or InvalidateRect (fallback)
        render_layered();

        // Poll timer: 15 minutes
        let initial_poll_ms = {
            let state = lock_state();
            state.as_ref().map(|s| s.poll_interval_ms).unwrap_or(POLL_15_MIN)
        };
        SetTimer(hwnd, TIMER_POLL, initial_poll_ms, None);

        // Initial poll
        let send_hwnd = SendHwnd::from_hwnd(hwnd);
        std::thread::spawn(move || {
            do_poll(send_hwnd);
        });

        // Initial theme check
        check_theme_change();

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Render widget content and push to the layered window via UpdateLayeredWindow.
/// Renders fully opaque with the actual taskbar background colour so that
/// ClearType sub-pixel font rendering can be used for crisp, OS-native text.
fn render_layered() {
    let (hwnd_val, is_dark, embedded, session_pct, session_text, weekly_pct, weekly_text) = {
        let state = lock_state();
        match state.as_ref() {
            Some(s) => (
                s.hwnd,
                s.is_dark,
                s.embedded,
                s.session_percent,
                s.session_text.clone(),
                s.weekly_percent,
                s.weekly_text.clone(),
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

    let width = total_widget_width();
    let height = WIDGET_HEIGHT;

    let accent = Color::from_hex("#D97757");
    let track = if is_dark {
        Color::from_hex("#444444")
    } else {
        Color::from_hex("#AAAAAA")
    };
    let text_color = if is_dark {
        Color::from_hex("#888888")
    } else {
        Color::from_hex("#404040")
    };
    let bg_color = if is_dark {
        Color::from_hex("#1C1C1C")
    } else {
        Color::from_hex("#F3F3F3")
    };

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
        let dib = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .unwrap_or_default();

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
        paint_content(mem_dc, width, height, is_dark, &bg_color,
                      &text_color, &accent, &track,
                      session_pct, &session_text, weekly_pct, &weekly_text);

        // Background pixels → alpha 1 (nearly invisible but still hittable for right-click).
        // Content pixels → fully opaque (preserves ClearType sub-pixel rendering).
        let bg_bgr = bg_color.to_colorref();
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
}

/// Paint all widget content onto a DC with a given background color.
fn paint_content(
    hdc: HDC,
    width: i32,
    height: i32,
    is_dark: bool,
    bg: &Color,
    text_color: &Color,
    accent: &Color,
    track: &Color,
    session_pct: f64,
    session_text: &str,
    weekly_pct: f64,
    weekly_text: &str,
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

        // Left divider
        let divider_top = (height - 25) / 2;
        let divider_bottom = divider_top + 25;

        let (div_left, div_right) = if is_dark {
            ((80, 80, 80), (40, 40, 40))
        } else {
            ((160, 160, 160), (230, 230, 230))
        };

        let left_brush = CreateSolidBrush(COLORREF(native_interop::colorref(div_left.0, div_left.1, div_left.2)));
        let left_rect = RECT {
            left: 0,
            top: divider_top,
            right: 2,
            bottom: divider_bottom,
        };
        FillRect(hdc, &left_rect, left_brush);
        let _ = DeleteObject(left_brush);

        let right_brush = CreateSolidBrush(COLORREF(native_interop::colorref(div_right.0, div_right.1, div_right.2)));
        let right_rect = RECT {
            left: 2,
            top: divider_top,
            right: 3,
            bottom: divider_bottom,
        };
        FillRect(hdc, &right_rect, right_brush);
        let _ = DeleteObject(right_brush);

        let content_x = LEFT_DIVIDER_W + DIVIDER_RIGHT_MARGIN;
        let row1_y = 5;
        let row2_y = 5 + SEGMENT_H + 10;

        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, COLORREF(text_color.to_colorref()));

        let font_name = native_interop::wide_str("Segoe UI");
        let font = CreateFontW(
            -12,
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

        draw_row(hdc, content_x, row1_y, "5h", session_pct, session_text, accent, track);
        draw_row(hdc, content_x, row2_y, "7d", weekly_pct, weekly_text, accent, track);

        SelectObject(hdc, old_font);
        let _ = DeleteObject(font);
    }
}

fn do_poll(send_hwnd: SendHwnd) {
    let hwnd = send_hwnd.to_hwnd();
    match poller::poll() {
        Ok(data) => {
            let session_text = poller::format_line(&data.session);
            let weekly_text = poller::format_line(&data.weekly);

            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                s.session_percent = data.session.percentage;
                s.weekly_percent = data.weekly.percentage;
                s.session_text = session_text;
                s.weekly_text = weekly_text;
                // Stop fast-poll if reset data is now fresh
                if !poller::is_past_reset(&data) {
                    unsafe {
                        let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                    }
                }

                s.data = Some(data);
                s.last_poll_ok = true;

                // Recovered from errors — restore normal poll interval
                if s.retry_count > 0 {
                    s.retry_count = 0;
                    let interval = s.poll_interval_ms;
                    unsafe {
                        SetTimer(hwnd, TIMER_POLL, interval, None);
                    }
                }
            }

            unsafe {
                let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
            }
        }
        Err(_e) => {
            // Show refresh indicator — retry will recover silently
            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                s.session_text = "...".to_string();
                s.weekly_text = "...".to_string();
                s.last_poll_ok = false;

                // Exponential backoff retry: 30s, 60s, 120s, ... up to poll_interval
                s.retry_count = s.retry_count.saturating_add(1);
                let backoff = RETRY_BASE_MS.saturating_mul(
                    1u32.checked_shl(s.retry_count - 1).unwrap_or(u32::MAX),
                );
                let retry_ms = backoff.min(s.poll_interval_ms);

                unsafe {
                    // Kill the 5-second reset poll so it doesn't bypass backoff
                    let _ = KillTimer(hwnd, TIMER_RESET_POLL);
                    SetTimer(hwnd, TIMER_POLL, retry_ms, None);
                }
            }

            unsafe {
                let _ = PostMessageW(hwnd, WM_APP_USAGE_UPDATED, WPARAM(0), LPARAM(0));
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

    let data = match &s.data {
        Some(d) => d,
        None => return,
    };

    let hwnd = s.hwnd.to_hwnd();

    // If a reset time has passed, poll every 5s to pick up fresh data
    if poller::is_past_reset(data) {
        unsafe {
            SetTimer(hwnd, TIMER_RESET_POLL, 5_000, None);
        }
    }

    let session_delay = poller::time_until_display_change(data.session.resets_at);
    let weekly_delay = poller::time_until_display_change(data.weekly.resets_at);

    let min_delay = match (session_delay, weekly_delay) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

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

    if let Some(ref data) = s.data {
        s.session_text = poller::format_line(&data.session);
        s.weekly_text = poller::format_line(&data.weekly);
    }
}

fn position_at_taskbar() {
    let state = lock_state();
    let s = match state.as_ref() {
        Some(s) => s,
        None => return,
    };

    // Don't fight the user's drag
    if s.dragging {
        return;
    }

    let hwnd = s.hwnd.to_hwnd();
    let embedded = s.embedded;
    let tray_offset = s.tray_offset;

    let taskbar_hwnd = match s.taskbar_hwnd {
        Some(h) => h,
        None => return,
    };

    let taskbar_rect = match native_interop::get_taskbar_rect(taskbar_hwnd) {
        Some(r) => r,
        None => return,
    };

    let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
    let mut tray_left = taskbar_rect.right;

    if let Some(tray_hwnd) = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd") {
        if let Some(tray_rect) = native_interop::get_window_rect_safe(tray_hwnd) {
            tray_left = tray_rect.left;
        }
    }

    let widget_width = total_widget_width();

    if embedded {
        // Child window: coordinates relative to parent (taskbar)
        let x = tray_left - taskbar_rect.left - widget_width - tray_offset;
        let y = (taskbar_height - WIDGET_HEIGHT) / 2;
        native_interop::move_window(hwnd, x, y, widget_width, WIDGET_HEIGHT);
    } else {
        // Topmost popup: screen coordinates
        let x = tray_left - widget_width - tray_offset;
        let y = taskbar_rect.top + (taskbar_height - WIDGET_HEIGHT) / 2;
        native_interop::move_window(hwnd, x, y, widget_width, WIDGET_HEIGHT);
    }
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
        WM_DISPLAYCHANGE => {
            position_at_taskbar();
            LRESULT(0)
        }
        WM_TIMER => {
            let timer_id = wparam.0;
            match timer_id {
                TIMER_POLL => {
                    let sh = SendHwnd::from_hwnd(hwnd);
                    std::thread::spawn(move || {
                        do_poll(sh);
                    });
                }
                TIMER_COUNTDOWN => {
                    update_display();
                    render_layered();
                    schedule_countdown_timer();
                }
                TIMER_RESET_POLL => {
                    let sh = SendHwnd::from_hwnd(hwnd);
                    std::thread::spawn(move || {
                        do_poll(sh);
                    });
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_USAGE_UPDATED => {
            check_theme_change();
            render_layered();
            schedule_countdown_timer();
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let is_dragging = {
                let state = lock_state();
                state.as_ref().map(|s| s.dragging).unwrap_or(false)
            };
            // Always show resize cursor while dragging or when hovering divider zone
            let hit_test = (lparam.0 & 0xFFFF) as u16;
            if is_dragging {
                let cursor = LoadCursorW(HINSTANCE::default(), IDC_SIZEWE)
                    .unwrap_or_default();
                SetCursor(cursor);
                return LRESULT(1);
            }
            if hit_test == 1 { // HTCLIENT
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                if pt.x < DIVIDER_HIT_ZONE {
                    let cursor = LoadCursorW(HINSTANCE::default(), IDC_SIZEWE)
                        .unwrap_or_default();
                    SetCursor(cursor);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_LBUTTONDOWN => {
            let client_x = (lparam.0 & 0xFFFF) as i16 as i32;
            if client_x < DIVIDER_HIT_ZONE {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    s.dragging = true;
                    s.drag_start_mouse_x = pt.x;
                    s.drag_start_offset = s.tray_offset;
                }
                SetCapture(hwnd);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let is_dragging = {
                let state = lock_state();
                state.as_ref().map(|s| s.dragging).unwrap_or(false)
            };
            if is_dragging {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);

                let mut state = lock_state();
                let s = match state.as_mut() {
                    Some(s) => s,
                    None => return LRESULT(0),
                };

                // Moving mouse left = positive delta = larger offset (further left)
                let delta = s.drag_start_mouse_x - pt.x;
                let mut new_offset = s.drag_start_offset + delta;

                // Clamp: offset >= 0 (can't go right of default)
                if new_offset < 0 {
                    new_offset = 0;
                }

                // Clamp: don't go past left edge of taskbar
                if let Some(taskbar_hwnd) = s.taskbar_hwnd {
                    if let Some(taskbar_rect) = native_interop::get_taskbar_rect(taskbar_hwnd) {
                        let mut tray_left = taskbar_rect.right;
                        if let Some(tray_hwnd) = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd") {
                            if let Some(tray_rect) = native_interop::get_window_rect_safe(tray_hwnd) {
                                tray_left = tray_rect.left;
                            }
                        }
                        let widget_width = total_widget_width();
                        let max_offset = if s.embedded {
                            tray_left - taskbar_rect.left - widget_width
                        } else {
                            tray_left - taskbar_rect.left - widget_width
                        };
                        if new_offset > max_offset {
                            new_offset = max_offset;
                        }
                    }
                }

                s.tray_offset = new_offset;

                // Move window directly
                let hwnd_val = s.hwnd.to_hwnd();
                if let Some(taskbar_hwnd) = s.taskbar_hwnd {
                    if let Some(taskbar_rect) = native_interop::get_taskbar_rect(taskbar_hwnd) {
                        let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
                        let mut tray_left = taskbar_rect.right;
                        if let Some(tray_hwnd) = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd") {
                            if let Some(tray_rect) = native_interop::get_window_rect_safe(tray_hwnd) {
                                tray_left = tray_rect.left;
                            }
                        }
                        let widget_width = total_widget_width();
                        if s.embedded {
                            let x = tray_left - taskbar_rect.left - widget_width - new_offset;
                            let y = (taskbar_height - WIDGET_HEIGHT) / 2;
                            native_interop::move_window(hwnd_val, x, y, widget_width, WIDGET_HEIGHT);
                        } else {
                            let x = tray_left - widget_width - new_offset;
                            let y = taskbar_rect.top + (taskbar_height - WIDGET_HEIGHT) / 2;
                            native_interop::move_window(hwnd_val, x, y, widget_width, WIDGET_HEIGHT);
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let was_dragging = {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    if s.dragging {
                        s.dragging = false;
                        let offset = s.tray_offset;
                        Some(offset)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if was_dragging.is_some() {
                let _ = ReleaseCapture();
                save_state_settings();
            }
            LRESULT(0)
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
                            s.session_text = "...".to_string();
                            s.weekly_text = "...".to_string();
                        }
                    }
                    render_layered();
                    let sh = SendHwnd::from_hwnd(hwnd);
                    std::thread::spawn(move || {
                        do_poll(sh);
                    });
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
                            s.tray_offset = 0;
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                }
                IDM_START_WITH_WINDOWS => {
                    set_startup_enabled(!is_startup_enabled());
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
                _ => {}
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
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn show_context_menu(hwnd: HWND) {
    unsafe {
        let current_interval = {
            let state = lock_state();
            state.as_ref().map(|s| s.poll_interval_ms).unwrap_or(POLL_15_MIN)
        };

        let menu = CreatePopupMenu().unwrap();

        let refresh_str = native_interop::wide_str("Refresh");
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            1,
            PCWSTR::from_raw(refresh_str.as_ptr()),
        );

        // Update Frequency submenu
        let freq_menu = CreatePopupMenu().unwrap();
        let freq_items: &[(u16, u32, &str)] = &[
            (IDM_FREQ_1MIN, POLL_1_MIN, "1 Minute"),
            (IDM_FREQ_5MIN, POLL_5_MIN, "5 Minutes"),
            (IDM_FREQ_15MIN, POLL_15_MIN, "15 Minutes"),
            (IDM_FREQ_1HOUR, POLL_1_HOUR, "1 Hour"),
        ];
        for &(id, interval, label) in freq_items {
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

        let freq_label = native_interop::wide_str("Update Frequency");
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            freq_menu.0 as usize,
            PCWSTR::from_raw(freq_label.as_ptr()),
        );

        // Settings submenu
        let settings_menu = CreatePopupMenu().unwrap();

        let startup_str = native_interop::wide_str("Start with Windows");
        let startup_flags = if is_startup_enabled() {
            MF_CHECKED
        } else {
            MENU_ITEM_FLAGS(0)
        };
        let _ = AppendMenuW(
            settings_menu,
            startup_flags,
            IDM_START_WITH_WINDOWS as usize,
            PCWSTR::from_raw(startup_str.as_ptr()),
        );

        let reset_pos_str = native_interop::wide_str("Reset Position");
        let _ = AppendMenuW(
            settings_menu,
            MENU_ITEM_FLAGS(0),
            IDM_RESET_POSITION as usize,
            PCWSTR::from_raw(reset_pos_str.as_ptr()),
        );

        let _ = AppendMenuW(settings_menu, MF_SEPARATOR, 0, PCWSTR::null());

        let version_str = native_interop::wide_str(&format!("v{}", env!("APP_VERSION")));
        let _ = AppendMenuW(
            settings_menu,
            MF_GRAYED,
            0,
            PCWSTR::from_raw(version_str.as_ptr()),
        );

        let settings_label = native_interop::wide_str("Settings");
        let _ = AppendMenuW(
            menu,
            MF_POPUP,
            settings_menu.0 as usize,
            PCWSTR::from_raw(settings_label.as_ptr()),
        );

        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        let exit_str = native_interop::wide_str("Exit");
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            2,
            PCWSTR::from_raw(exit_str.as_ptr()),
        );

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, None);
        let _ = DestroyMenu(menu);
    }
}

/// Paint for non-embedded fallback (normal WM_PAINT path)
fn paint(hdc: HDC, hwnd: HWND) {
    let (is_dark, session_pct, session_text, weekly_pct, weekly_text) = {
        let state = lock_state();
        match state.as_ref() {
            Some(s) => (
                s.is_dark,
                s.session_percent,
                s.session_text.clone(),
                s.weekly_percent,
                s.weekly_text.clone(),
            ),
            None => return,
        }
    };

    let accent = Color::from_hex("#D97757");
    let track = if is_dark {
        Color::from_hex("#444444")
    } else {
        Color::from_hex("#AAAAAA")
    };
    let text_color = if is_dark {
        Color::from_hex("#888888")
    } else {
        Color::from_hex("#404040")
    };
    let bg_color = if is_dark {
        Color::from_hex("#1C1C1C")
    } else {
        Color::from_hex("#F3F3F3")
    };

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

        paint_content(mem_dc, width, height, is_dark, &bg_color,
                      &text_color, &accent, &track,
                      session_pct, &session_text, weekly_pct, &weekly_text);

        let _ = BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);

        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(mem_bmp);
        let _ = DeleteDC(mem_dc);
    }
}

fn draw_row(
    hdc: HDC,
    x: i32,
    y: i32,
    label: &str,
    percent: f64,
    text: &str,
    accent: &Color,
    track: &Color,
) {
    unsafe {
        let mut label_wide: Vec<u16> = label.encode_utf16().collect();
        let mut label_rect = RECT {
            left: x,
            top: y,
            right: x + LABEL_WIDTH,
            bottom: y + SEGMENT_H,
        };
        let _ = DrawTextW(
            hdc,
            &mut label_wide,
            &mut label_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );

        let bar_x = x + LABEL_WIDTH + LABEL_RIGHT_MARGIN;
        let percent_clamped = percent.clamp(0.0, 100.0);

        for i in 0..SEGMENT_COUNT {
            let seg_x = bar_x + i * (SEGMENT_W + SEGMENT_GAP);
            let seg_start = (i as f64) * 10.0;
            let seg_end = seg_start + 10.0;

            let seg_rect = RECT {
                left: seg_x,
                top: y,
                right: seg_x + SEGMENT_W,
                bottom: y + SEGMENT_H,
            };

            if percent_clamped >= seg_end {
                draw_rounded_rect(hdc, &seg_rect, accent, CORNER_RADIUS);
            } else if percent_clamped <= seg_start {
                draw_rounded_rect(hdc, &seg_rect, track, CORNER_RADIUS);
            } else {
                draw_rounded_rect(hdc, &seg_rect, track, CORNER_RADIUS);
                let fraction = (percent_clamped - seg_start) / 10.0;
                let fill_width = (SEGMENT_W as f64 * fraction) as i32;
                if fill_width > 0 {
                    let fill_rect = RECT {
                        left: seg_x,
                        top: y,
                        right: seg_x + fill_width,
                        bottom: y + SEGMENT_H,
                    };
                    let rgn = CreateRoundRectRgn(
                        seg_rect.left,
                        seg_rect.top,
                        seg_rect.right + 1,
                        seg_rect.bottom + 1,
                        CORNER_RADIUS * 2,
                        CORNER_RADIUS * 2,
                    );
                    let _ = SelectClipRgn(hdc, rgn);
                    let brush = CreateSolidBrush(COLORREF(accent.to_colorref()));
                    FillRect(hdc, &fill_rect, brush);
                    let _ = DeleteObject(brush);
                    let _ = SelectClipRgn(hdc, HRGN::default());
                    let _ = DeleteObject(rgn);
                }
            }
        }

        let text_x =
            bar_x + SEGMENT_COUNT * (SEGMENT_W + SEGMENT_GAP) - SEGMENT_GAP + BAR_RIGHT_MARGIN;
        let mut text_wide: Vec<u16> = text.encode_utf16().collect();
        let mut text_rect = RECT {
            left: text_x,
            top: y,
            right: text_x + TEXT_WIDTH,
            bottom: y + SEGMENT_H,
        };
        let _ = DrawTextW(
            hdc,
            &mut text_wide,
            &mut text_rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE,
        );
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
