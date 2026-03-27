use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

use crate::native_interop::{self, Color, WM_APP_TRAY};

const TRAY_ICON_ID: u32 = 1;

/// Menu item ID for toggling widget visibility (used by window.rs context menu).
pub const IDM_TOGGLE_WIDGET: u16 = 50;

/// Actions the tray message handler can request from the main window.
pub enum TrayAction {
    None,
    ToggleWidget,
    ShowContextMenu,
}

/// Create a rounded-rectangle tray icon badge showing the usage percentage.
/// `percent` = None means "no data" (gray "?"), Some(p) is the usage level.
pub fn create_icon(percent: Option<f64>) -> HICON {
    let size = 64_i32;
    let margin = 4_i32;
    let radius = 14_i32;
    let outline = 2_i32;

    let (fill, outline_col, text_col) = match percent {
        None => (
            Color::from_hex("#6c757d"),
            Color::from_hex("#495057"),
            Color::from_hex("#FFFFFF"),
        ),
        Some(p) if p < 50.0 => (
            Color::from_hex("#28a745"),
            Color::from_hex("#1e7e34"),
            Color::from_hex("#FFFFFF"),
        ),
        Some(p) if p < 75.0 => (
            Color::from_hex("#ffc107"),
            Color::from_hex("#e0a800"),
            Color::from_hex("#1a1a1a"),
        ),
        Some(p) if p < 90.0 => (
            Color::from_hex("#fd7e14"),
            Color::from_hex("#d9650a"),
            Color::from_hex("#FFFFFF"),
        ),
        _ => (
            Color::from_hex("#dc3545"),
            Color::from_hex("#bd2130"),
            Color::from_hex("#FFFFFF"),
        ),
    };

    let display_text = match percent {
        None => "?".to_string(),
        Some(p) => format!("{}", p as u32),
    };

    let font_h = match display_text.len() {
        1 => -50,
        2 => -42,
        _ => -30,
    };

    unsafe {
        let screen_dc = GetDC(HWND::default());
        let mem_dc = CreateCompatibleDC(screen_dc);

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .unwrap_or_default();

        if dib.is_invalid() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(HWND::default(), screen_dc);
            return HICON::default();
        }

        let old_bmp = SelectObject(mem_dc, dib);

        // Zero-fill (transparent background)
        let pixel_data =
            std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
        for px in pixel_data.iter_mut() {
            *px = 0;
        }

        // Draw rounded rectangle badge
        let null_pen = GetStockObject(NULL_PEN);
        let old_pen = SelectObject(mem_dc, null_pen);

        // Outer rounded rect = outline colour
        let br_outline = CreateSolidBrush(COLORREF(outline_col.to_colorref()));
        let old_brush = SelectObject(mem_dc, br_outline);
        let _ = RoundRect(
            mem_dc,
            margin,
            margin,
            size - margin + 1,
            size - margin + 1,
            radius * 2,
            radius * 2,
        );

        // Inner rounded rect = fill colour
        let br_fill = CreateSolidBrush(COLORREF(fill.to_colorref()));
        SelectObject(mem_dc, br_fill);
        let _ = RoundRect(
            mem_dc,
            margin + outline,
            margin + outline,
            size - margin - outline + 1,
            size - margin - outline + 1,
            (radius - 1) * 2,
            (radius - 1) * 2,
        );

        SelectObject(mem_dc, old_brush);
        SelectObject(mem_dc, old_pen);
        let _ = DeleteObject(br_outline);
        let _ = DeleteObject(br_fill);

        // Draw centered percentage text
        let font_name = native_interop::wide_str("Arial Bold");
        let font = CreateFontW(
            font_h,
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            ANTIALIASED_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR::from_raw(font_name.as_ptr()),
        );
        let old_font = SelectObject(mem_dc, font);
        let _ = SetBkMode(mem_dc, TRANSPARENT);
        let _ = SetTextColor(mem_dc, COLORREF(text_col.to_colorref()));

        let mut text_rect = RECT {
            left: margin,
            top: margin,
            right: size - margin,
            bottom: size - margin,
        };
        let mut text_wide: Vec<u16> = display_text.encode_utf16().collect();
        let _ = DrawTextW(
            mem_dc,
            &mut text_wide,
            &mut text_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );

        SelectObject(mem_dc, old_font);
        let _ = DeleteObject(font);

        // Set alpha: non-zero BGR pixel -> fully opaque; background stays transparent
        for px in pixel_data.iter_mut() {
            if *px != 0 {
                *px = (*px & 0x00FF_FFFF) | 0xFF00_0000;
            }
        }

        // Monochrome mask (per-pixel alpha from colour bitmap)
        let mask_bytes = vec![0u8; ((size * size + 7) / 8) as usize];
        let mask_bmp = CreateBitmap(
            size,
            size,
            1,
            1,
            Some(mask_bytes.as_ptr() as *const std::ffi::c_void),
        );

        let icon_info = ICONINFO {
            fIcon: TRUE,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask_bmp,
            hbmColor: dib,
        };
        let hicon = CreateIconIndirect(&icon_info).unwrap_or_default();

        let _ = DeleteObject(mask_bmp);
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND::default(), screen_dc);

        hicon
    }
}

/// Register the tray icon with the shell.
pub fn add(hwnd: HWND, percent: Option<f64>, tooltip: &str) {
    let hicon = create_icon(percent);
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_ICON_ID;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = hicon;
        copy_to_tip(tooltip, &mut nid.szTip);
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        if !hicon.is_invalid() {
            let _ = DestroyIcon(hicon);
        }
    }
}

/// Update the tray icon colour and tooltip to reflect current usage.
pub fn update(hwnd: HWND, percent: Option<f64>, tooltip: &str) {
    let hicon = create_icon(percent);
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_ICON_ID;
        nid.uFlags = NIF_ICON | NIF_TIP;
        nid.hIcon = hicon;
        copy_to_tip(tooltip, &mut nid.szTip);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        if !hicon.is_invalid() {
            let _ = DestroyIcon(hicon);
        }
    }
}

/// Remove the tray icon from the shell.
pub fn remove(hwnd: HWND) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = TRAY_ICON_ID;
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

/// Interpret a tray callback message and return the action to take.
pub fn handle_message(lparam: LPARAM) -> TrayAction {
    let mouse_msg = lparam.0 as u32;
    match mouse_msg {
        WM_LBUTTONUP => TrayAction::ToggleWidget,
        WM_RBUTTONUP => TrayAction::ShowContextMenu,
        _ => TrayAction::None,
    }
}

/// Copy a string into the fixed-size szTip field (max 127 chars + null).
fn copy_to_tip(s: &str, tip: &mut [u16; 128]) {
    let wide: Vec<u16> = s.encode_utf16().collect();
    let mut len = wide.len().min(127);
    // Don't leave a lone high surrogate at the truncation point
    if len > 0 && (0xD800..=0xDBFF).contains(&wide[len - 1]) {
        len -= 1;
    }
    tip[..len].copy_from_slice(&wide[..len]);
    tip[len] = 0;
}
