use std::sync::Mutex;

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::UI::Shell::{
    ExtractIconExW, Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE,
    NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::native_interop::WM_APP_TRAY;

const APP_TRAY_ICON_ID: u32 = 1;
const THEME_TRAY_ICON_ID_BASE: u32 = 1_000;

static REGISTERED_THEME_ICON_IDS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Rasterized Theme Studio root to expose as a genuine notification-area icon.
pub struct ThemedTrayIcon {
    pub surface_index: usize,
    pub tooltip: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

/// Actions the tray message handler can request from the main window.
pub enum TrayAction {
    None,
    OpenDashboard,
    ShowContextMenu,
}

/// Load the application icons embedded by build.rs from src/icons/icon.ico.
/// Native windows and the system tray share this source so Windows can choose
/// the exact large or small icon instead of scaling a single bitmap.
pub fn load_app_icons() -> (HICON, HICON) {
    unsafe {
        let mut exe_buf = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
        if len == 0 {
            return (HICON::default(), HICON::default());
        }

        let mut small_icon = HICON::default();
        let mut large_icon = HICON::default();
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

fn load_app_icon() -> HICON {
    let (large_icon, small_icon) = load_app_icons();
    if !small_icon.is_invalid() {
        if !large_icon.is_invalid() {
            unsafe {
                let _ = DestroyIcon(large_icon);
            }
        }
        small_icon
    } else {
        large_icon
    }
}

fn themed_icon_id(surface_index: usize) -> u32 {
    THEME_TRAY_ICON_ID_BASE.saturating_add(surface_index.min(u32::MAX as usize) as u32)
}

pub fn themed_surface_index(id: u32) -> Option<usize> {
    (id >= THEME_TRAY_ICON_ID_BASE).then(|| (id - THEME_TRAY_ICON_ID_BASE) as usize)
}

pub fn cursor_over_themed_icon(hwnd: HWND, surface_index: usize) -> bool {
    unsafe {
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: hwnd,
            uID: themed_icon_id(surface_index),
            ..Default::default()
        };
        let Ok(rect) = Shell_NotifyIconGetRect(&identifier) else {
            return false;
        };
        let mut point = POINT::default();
        GetCursorPos(&mut point).is_ok()
            && point.x >= rect.left
            && point.x < rect.right
            && point.y >= rect.top
            && point.y < rect.bottom
    }
}

fn create_themed_icon(icon: &ThemedTrayIcon) -> HICON {
    if icon.width == 0
        || icon.height == 0
        // Explorer ultimately displays one square notification-area slot. A
        // bounded source prevents an accidental Studio expression from asking
        // GDI and the shell to retain an enormous icon bitmap.
        || icon.width > 512
        || icon.height > 512
        || icon.pixels.len() != icon.width as usize * icon.height as usize
    {
        return HICON::default();
    }

    unsafe {
        let screen_dc = GetDC(None);
        let memory_dc = CreateCompatibleDC(Some(screen_dc));
        let bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: icon.width as i32,
                // Theme pixels are top-down, so use a top-down DIB as well.
                biHeight: -(icon.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color_bitmap = CreateDIBSection(
            Some(memory_dc),
            &bitmap_info,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
        .unwrap_or_default();
        if color_bitmap.is_invalid() || bits.is_null() {
            let _ = DeleteDC(memory_dc);
            ReleaseDC(None, screen_dc);
            return HICON::default();
        }
        std::ptr::copy_nonoverlapping(icon.pixels.as_ptr(), bits.cast::<u32>(), icon.pixels.len());

        // A zero monochrome mask lets the 32-bit colour bitmap's alpha channel
        // define the transparent pixels and antialiased edges.
        let mask_bitmap = CreateBitmap(icon.width as i32, icon.height as i32, 1, 1, None);
        let icon_info = ICONINFO {
            fIcon: TRUE,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask_bitmap,
            hbmColor: color_bitmap,
        };
        let result = CreateIconIndirect(&icon_info).unwrap_or_default();

        let _ = DeleteObject(mask_bitmap.into());
        let _ = DeleteObject(color_bitmap.into());
        let _ = DeleteDC(memory_dc);
        ReleaseDC(None, screen_dc);
        result
    }
}

fn remove_id(hwnd: HWND, id: u32) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = id;
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn remove_registered_theme_icons(hwnd: HWND) {
    let mut registered = REGISTERED_THEME_ICON_IDS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for id in registered.drain(..) {
        remove_id(hwnd, id);
    }
}

/// Register or refresh the single persistent application tray icon.
pub fn sync(hwnd: HWND, tooltip: &str) {
    remove_registered_theme_icons(hwnd);
    let hicon = load_app_icon();
    if hicon.is_invalid() {
        return;
    }

    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = APP_TRAY_ICON_ID;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = hicon;
        copy_to_tip(tooltip, &mut nid.szTip);

        // NIM_ADD succeeds on first registration. If the icon is already
        // present, NIM_MODIFY refreshes its image, callback and tooltip.
        if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
        let _ = DestroyIcon(hicon);
    }
}

/// Register Theme Studio roots as independent notification-area icons. The
/// shell owns their order and overflow placement just like every other app icon.
pub fn sync_themed(hwnd: HWND, icons: &[ThemedTrayIcon]) {
    remove_id(hwnd, APP_TRAY_ICON_ID);
    let mut refreshed_ids = Vec::with_capacity(icons.len());
    for icon in icons {
        let hicon = create_themed_icon(icon);
        if hicon.is_invalid() {
            continue;
        }
        let id = themed_icon_id(icon.surface_index);
        unsafe {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = id;
            nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = WM_APP_TRAY;
            nid.hIcon = hicon;
            copy_to_tip(&icon.tooltip, &mut nid.szTip);
            if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
            let _ = DestroyIcon(hicon);
        }
        refreshed_ids.push(id);
    }

    let mut registered = REGISTERED_THEME_ICON_IDS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for id in registered
        .iter()
        .copied()
        .filter(|id| !refreshed_ids.contains(id))
    {
        remove_id(hwnd, id);
    }
    *registered = refreshed_ids;
}

/// Show a Windows balloon notification from the application tray icon.
pub fn notify_balloon(hwnd: HWND, title: &str, message: &str) {
    let icon_id = REGISTERED_THEME_ICON_IDS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .first()
        .copied()
        .unwrap_or(APP_TRAY_ICON_ID);
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = icon_id;
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_WARNING;
        copy_wide(title, &mut nid.szInfoTitle);
        copy_wide(message, &mut nid.szInfo);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Remove the application tray icon from the shell.
pub fn remove_all(hwnd: HWND) {
    remove_id(hwnd, APP_TRAY_ICON_ID);
    remove_registered_theme_icons(hwnd);
}

/// Interpret a tray callback message and return the action to take.
pub fn handle_message(lparam: LPARAM) -> TrayAction {
    let mouse_msg = lparam.0 as u32;
    match mouse_msg {
        WM_LBUTTONUP | WM_LBUTTONDBLCLK => TrayAction::OpenDashboard,
        WM_RBUTTONUP | WM_CONTEXTMENU => TrayAction::ShowContextMenu,
        _ => TrayAction::None,
    }
}

fn copy_wide<const N: usize>(value: &str, buffer: &mut [u16; N]) {
    let wide: Vec<u16> = value.encode_utf16().collect();
    let mut len = wide.len().min(N - 1);
    if len > 0 && (0xD800..=0xDBFF).contains(&wide[len - 1]) {
        len -= 1;
    }
    buffer[..len].copy_from_slice(&wide[..len]);
    buffer[len] = 0;
}

fn copy_to_tip(value: &str, tooltip: &mut [u16; 128]) {
    copy_wide(value, tooltip);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_buttons_have_distinct_actions() {
        assert!(matches!(
            handle_message(LPARAM(WM_LBUTTONUP as isize)),
            TrayAction::OpenDashboard
        ));
        assert!(matches!(
            handle_message(LPARAM(WM_LBUTTONDBLCLK as isize)),
            TrayAction::OpenDashboard
        ));
        assert!(matches!(
            handle_message(LPARAM(WM_RBUTTONUP as isize)),
            TrayAction::ShowContextMenu
        ));
        assert!(matches!(
            handle_message(LPARAM(WM_CONTEXTMENU as isize)),
            TrayAction::ShowContextMenu
        ));
    }

    #[test]
    fn theme_icon_ids_do_not_overlap_the_application_icon() {
        assert_ne!(themed_icon_id(0), APP_TRAY_ICON_ID);
        assert_ne!(themed_icon_id(42), themed_icon_id(43));
    }
}
