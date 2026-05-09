use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::UI::Shell::{
    ExtractIconExW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::native_interop::{self, Color, WM_APP_TRAY};

const CLAUDE_TRAY_ICON_ID: u32 = 1;
const CODEX_TRAY_ICON_ID: u32 = 2;

/// Menu item ID for toggling widget visibility (used by window.rs context menu).
pub const IDM_TOGGLE_WIDGET: u16 = 50;

/// Actions the tray message handler can request from the main window.
pub enum TrayAction {
    None,
    ToggleWidget,
    ShowContextMenu,
}

#[derive(Clone, Copy)]
pub enum TrayIconKind {
    Claude,
    Codex,
}

pub struct TrayIconData {
    pub kind: TrayIconKind,
    pub percent: Option<f64>,
    pub tooltip: String,
}

impl TrayIconKind {
    fn id(self) -> u32 {
        match self {
            Self::Claude => CLAUDE_TRAY_ICON_ID,
            Self::Codex => CODEX_TRAY_ICON_ID,
        }
    }
}

fn lerp_channel(start: u8, end: u8, t: f64) -> u8 {
    (start as f64 + (end as f64 - start as f64) * t.clamp(0.0, 1.0)).round() as u8
}

fn lerp_color(start: Color, end: Color, t: f64) -> Color {
    Color::new(
        lerp_channel(start.r, end.r, t),
        lerp_channel(start.g, end.g, t),
        lerp_channel(start.b, end.b, t),
    )
}

fn interpolated_fill(percent: f64) -> Color {
    if percent <= 50.0 {
        return Color::from_hex("#D97757");
    }

    let stops = [
        (50.0, Color::from_hex("#D97757")),
        (70.0, Color::from_hex("#D08540")),
        (85.0, Color::from_hex("#CC8C20")),
        (95.0, Color::from_hex("#C45020")),
        (100.0, Color::from_hex("#B82020")),
    ];

    for pair in stops.windows(2) {
        let (start_pct, start_color) = pair[0];
        let (end_pct, end_color) = pair[1];
        if percent <= end_pct {
            let span = (end_pct - start_pct).max(f64::EPSILON);
            let t = (percent - start_pct) / span;
            return lerp_color(start_color, end_color, t);
        }
    }

    stops[stops.len() - 1].1
}

fn codex_fill(percent: f64) -> Color {
    if percent >= 90.0 {
        Color::from_hex("#FFFFFF")
    } else {
        Color::from_hex("#111111")
    }
}

/// Create a rounded-rectangle tray icon badge showing the usage percentage.
/// For Claude, `percent` = None uses the embedded app icon as the loading state.
/// For Codex, `percent` = None uses a black/white Codex placeholder badge.
pub fn create_icon(kind: TrayIconKind, percent: Option<f64>) -> HICON {
    if matches!(kind, TrayIconKind::Claude) && percent.is_none() {
        let app_icon = load_embedded_app_icon();
        if !app_icon.is_invalid() {
            return app_icon;
        }
    }

    let size = 64_i32;
    let margin = 0_i32;
    let radius = 2_i32;
    let outline = if matches!(kind, TrayIconKind::Codex) {
        3_i32
    } else {
        0_i32
    };

    let fill = match kind {
        TrayIconKind::Claude => interpolated_fill(percent.unwrap_or(0.0)),
        TrayIconKind::Codex => codex_fill(percent.unwrap_or(0.0)),
    };
    let text_col = match kind {
        TrayIconKind::Claude => Color::from_hex("#FFFFFF"),
        TrayIconKind::Codex if percent.unwrap_or(0.0) >= 90.0 => Color::from_hex("#111111"),
        TrayIconKind::Codex => Color::from_hex("#FFFFFF"),
    };
    let outline_col = match kind {
        TrayIconKind::Claude => fill,
        TrayIconKind::Codex if percent.unwrap_or(0.0) >= 90.0 => Color::from_hex("#111111"),
        TrayIconKind::Codex => Color::from_hex("#FFFFFF"),
    };

    let display_text = match percent {
        Some(p) => format!("{}", p.round().clamp(0.0, 999.0) as u32),
        None => match kind {
            TrayIconKind::Claude => String::new(),
            TrayIconKind::Codex => "C".to_string(),
        },
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
        let dib =
            CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();

        if dib.is_invalid() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(HWND::default(), screen_dc);
            return HICON::default();
        }

        let old_bmp = SelectObject(mem_dc, dib);

        // Zero-fill (transparent background)
        let pixel_data = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
        for px in pixel_data.iter_mut() {
            *px = 0;
        }

        // Draw rounded rectangle badge
        let null_pen = GetStockObject(NULL_PEN);
        let old_pen = SelectObject(mem_dc, null_pen);

        if outline > 0 {
            let br_outline = CreateSolidBrush(COLORREF(outline_col.to_colorref()));
            let old_brush = SelectObject(mem_dc, br_outline);
            let _ = RoundRect(
                mem_dc,
                margin,
                margin,
                size - margin + 1,
                size - margin + 1,
                (radius + 1) * 2,
                (radius + 1) * 2,
            );
            SelectObject(mem_dc, old_brush);
            let _ = DeleteObject(br_outline);
        }

        let br_fill = CreateSolidBrush(COLORREF(fill.to_colorref()));
        let old_brush = SelectObject(mem_dc, br_fill);
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

fn load_embedded_app_icon() -> HICON {
    unsafe {
        let mut exe_buf = [0u16; 260];
        let len = GetModuleFileNameW(None, &mut exe_buf) as usize;
        if len == 0 {
            return HICON::default();
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
            HICON::default()
        } else if !small_icon.is_invalid() {
            small_icon
        } else {
            large_icon
        }
    }
}

/// Show a Windows balloon notification from the tray icon.
/// Used to alert the user when re-authentication is required.
pub fn notify_balloon(hwnd: HWND, kind: TrayIconKind, title: &str, message: &str) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = kind.id();
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_WARNING;
        copy_wide(title, &mut nid.szInfoTitle);
        copy_wide_256(message, &mut nid.szInfo);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Copy a string into a fixed-size wide buffer (truncates to fit).
fn copy_wide<const N: usize>(s: &str, buf: &mut [u16; N]) {
    let wide: Vec<u16> = s.encode_utf16().collect();
    let len = wide.len().min(N - 1);
    buf[..len].copy_from_slice(&wide[..len]);
    buf[len] = 0;
}

/// Copy a string into a 256-wide buffer.
fn copy_wide_256(s: &str, buf: &mut [u16; 256]) {
    copy_wide(s, buf)
}

/// Register the tray icon with the shell.
pub fn add(hwnd: HWND, kind: TrayIconKind, percent: Option<f64>, tooltip: &str) {
    let hicon = create_icon(kind, percent);
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = kind.id();
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
pub fn update(hwnd: HWND, kind: TrayIconKind, percent: Option<f64>, tooltip: &str) {
    let hicon = create_icon(kind, percent);
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = kind.id();
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
pub fn remove(hwnd: HWND, kind: TrayIconKind) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = kind.id();
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

pub fn sync(hwnd: HWND, icons: &[TrayIconData]) {
    let show_claude = icons
        .iter()
        .find(|icon| matches!(icon.kind, TrayIconKind::Claude));
    let show_codex = icons
        .iter()
        .find(|icon| matches!(icon.kind, TrayIconKind::Codex));

    if let Some(icon) = show_claude {
        add(hwnd, icon.kind, icon.percent, &icon.tooltip);
        update(hwnd, icon.kind, icon.percent, &icon.tooltip);
    } else {
        remove(hwnd, TrayIconKind::Claude);
    }

    if let Some(icon) = show_codex {
        add(hwnd, icon.kind, icon.percent, &icon.tooltip);
        update(hwnd, icon.kind, icon.percent, &icon.tooltip);
    } else {
        remove(hwnd, TrayIconKind::Codex);
    }
}

pub fn remove_all(hwnd: HWND) {
    remove(hwnd, TrayIconKind::Claude);
    remove(hwnd, TrayIconKind::Codex);
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
