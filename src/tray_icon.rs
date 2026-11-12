use std::sync::Mutex;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::UI::Shell::{
    ExtractIconExW, Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_GUID, NIF_ICON, NIF_INFO,
    NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    NOTIFYICONIDENTIFIER,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::native_interop::WM_APP_TRAY;

const APP_TRAY_ICON_ID: u32 = 1;
const THEME_TRAY_ICON_ID_BASE: u32 = 1_000;

mod settings;
pub(crate) use settings::cleanup_legacy_entries;
mod provider_artwork;

const APP_NAME: &str = "Claude Code Usage Monitor";

pub fn surface_provider(surface_id: &str) -> Option<crate::providers::ProviderId> {
    let key = surface_id.strip_suffix("-tray-icon")?;
    crate::providers::PROVIDER_DESCRIPTORS
        .iter()
        .find(|provider| provider.key == key)
        .map(|provider| provider.id)
}

pub fn surface_label(surface_id: &str, surface_name: &str) -> String {
    let name = surface_provider(surface_id)
        .map(|provider| {
            if provider == crate::providers::ProviderId::Claude {
                "Claude"
            } else {
                provider.descriptor().display_name
            }
        })
        .unwrap_or(surface_name);
    format!("{APP_NAME}: {name}")
}

unsafe fn update_icon(nid: &NOTIFYICONDATAW) -> bool {
    Shell_NotifyIconW(NIM_MODIFY, nid).as_bool() || Shell_NotifyIconW(NIM_ADD, nid).as_bool()
}

fn labelled_tooltip(label: &str, usage: &str) -> String {
    if usage.trim().is_empty() || usage == label {
        label.to_owned()
    } else {
        format!("{label}\n{usage}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrayIdentity {
    callback_id: u32,
    guid: GUID,
}

static REGISTERED_THEME_ICONS: Mutex<Vec<TrayIdentity>> = Mutex::new(Vec::new());

// These namespace seeds and the encoding are a persisted shell identity contract.
// Never use DefaultHasher, array positions, display names, tooltips or versions.
fn icon_guid(kind: &str, key: &str) -> GUID {
    let mut high = 0x936e_7a42_1cd5_48bfu64;
    let mut low = 0xa4d3_509b_82f6_017eu64;
    for byte in kind.bytes().chain([0]).chain(key.bytes()) {
        high = (high ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        low = (low ^ u64::from(byte))
            .wrapping_mul(0x100000001b3)
            .rotate_left(7);
    }
    GUID::from_u128((u128::from(high) << 64) | u128::from(low))
}

fn app_identity() -> TrayIdentity {
    TrayIdentity {
        callback_id: APP_TRAY_ICON_ID,
        guid: icon_guid("application", "main"),
    }
}

fn theme_identity(surface_index: usize, surface_id: &str) -> TrayIdentity {
    TrayIdentity {
        callback_id: themed_icon_id(surface_index),
        // Root IDs already identify provider icons across bundled themes, and
        // distinguish custom roots (including separately bound account icons).
        guid: icon_guid("surface", &surface_id.to_ascii_lowercase()),
    }
}

fn icon_data(hwnd: HWND, identity: TrayIdentity) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: identity.callback_id,
        uFlags: NIF_GUID,
        guidItem: identity.guid,
        ..Default::default()
    }
}

/// Rasterized Theme Studio root to expose as a genuine notification-area icon.
pub struct ThemedTrayIcon {
    pub surface_index: usize,
    pub surface_id: String,
    pub label: String,
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
    let identity = REGISTERED_THEME_ICONS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .find(|icon| icon.callback_id == themed_icon_id(surface_index))
        .copied();
    let Some(identity) = identity else {
        return false;
    };
    unsafe {
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: hwnd,
            uID: identity.callback_id,
            guidItem: identity.guid,
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

fn remove_id(hwnd: HWND, identity: TrayIdentity) {
    unsafe {
        let nid = icon_data(hwnd, identity);
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn remove_registered_theme_icons(hwnd: HWND) {
    let mut registered = REGISTERED_THEME_ICONS
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
        let mut nid = icon_data(hwnd, app_identity());
        nid.uFlags |= NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_APP_TRAY;
        nid.hIcon = hicon;
        copy_to_tip(&labelled_tooltip(APP_NAME, tooltip), &mut nid.szTip);

        // Ordinary refreshes modify the existing icon; add it on startup or
        // after Explorer has restarted and lost the live registration.
        update_icon(&nid);
        let _ = DestroyIcon(hicon);
    }
}

/// Register Theme Studio roots as independent notification-area icons. The
/// shell owns their order and overflow placement just like every other app icon.
pub fn sync_themed(hwnd: HWND, icons: &[ThemedTrayIcon]) {
    remove_id(hwnd, app_identity());
    let desired: Vec<_> = icons
        .iter()
        .map(|icon| theme_identity(icon.surface_index, &icon.surface_id))
        .collect();
    let mut registered = REGISTERED_THEME_ICONS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // Withdraw changed callback mappings before adding anything: an old root
    // must not delete a new icon that has taken its former array position.
    // Re-add moved roots with the same GUID so Explorer retains preferences.
    registered.retain(|identity| {
        if desired.contains(identity) {
            true
        } else {
            remove_id(hwnd, *identity);
            false
        }
    });
    for icon in icons {
        let hicon = create_themed_icon(icon);
        if hicon.is_invalid() {
            continue;
        }
        let identity = theme_identity(icon.surface_index, &icon.surface_id);
        unsafe {
            let mut nid = icon_data(hwnd, identity);
            nid.uFlags |= NIF_ICON | NIF_MESSAGE | NIF_TIP;
            nid.uCallbackMessage = WM_APP_TRAY;
            nid.hIcon = hicon;
            copy_to_tip(
                &labelled_tooltip(&icon.label, &icon.tooltip),
                &mut nid.szTip,
            );
            let success = update_icon(&nid);
            if success && !registered.contains(&identity) {
                registered.push(identity);
            }
            let _ = DestroyIcon(hicon);
        }
    }
    drop(registered);
    // Windows Settings keeps its own PNG snapshot. Keep it independent from
    // the live, usage-dependent HICON sent to Explorer above.
    let snapshots: Vec<_> = icons
        .iter()
        .filter_map(|icon| {
            let provider = surface_provider(&icon.surface_id)?;
            let png = provider_artwork::provider_snapshot(provider)?;
            Some((
                theme_identity(icon.surface_index, &icon.surface_id).guid,
                png,
            ))
        })
        .collect();
    settings::sync_provider_snapshots(&snapshots);
}

/// Show a Windows balloon notification from the application tray icon.
pub fn notify_balloon(hwnd: HWND, title: &str, message: &str) {
    let identity = REGISTERED_THEME_ICONS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .first()
        .copied()
        .unwrap_or_else(app_identity);
    unsafe {
        let mut nid = icon_data(hwnd, identity);
        nid.uFlags |= NIF_INFO;
        nid.dwInfoFlags = NIIF_WARNING;
        copy_wide(title, &mut nid.szInfoTitle);
        copy_wide(message, &mut nid.szInfo);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Remove the application tray icon from the shell.
pub fn remove_all(hwnd: HWND) {
    remove_id(hwnd, app_identity());
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
    fn provider_labels_identify_each_icon_and_custom_roots_keep_their_name() {
        assert_eq!(
            surface_label("claude-tray-icon", "Ignored"),
            "Claude Code Usage Monitor: Claude"
        );
        assert_eq!(
            surface_label("codex-tray-icon", "Ignored"),
            "Claude Code Usage Monitor: Codex"
        );
        assert_eq!(
            surface_label("codex-work", "Codex (Work)"),
            "Claude Code Usage Monitor: Codex (Work)"
        );
    }

    #[test]
    fn shell_identity_survives_surface_reordering_and_theme_switches() {
        let mut theme = crate::theme_engine::ThemeDocument::starter();
        let index = theme
            .surfaces
            .iter()
            .position(|s| s.id == "codex-tray-icon")
            .unwrap();
        let original = theme_identity(index, &theme.surfaces[index].id);
        theme.surfaces.swap(0, index);
        theme.surfaces[0].name = "Renamed Codex account icon".into();
        theme.id = "custom-copy".into();
        let moved = theme_identity(0, &theme.surfaces[0].id);
        assert_eq!(original.guid, moved.guid);
        assert_ne!(original.callback_id, moved.callback_id);
        assert_ne!(
            original, moved,
            "moving a root must replace its callback mapping"
        );
        assert_ne!(
            original.guid,
            theme_identity(index, "claude-tray-icon").guid
        );
        assert_ne!(
            original.guid,
            theme_identity(index, "codex-work-tray-icon").guid
        );
        assert_ne!(original.guid, app_identity().guid);
    }

    #[test]
    fn all_shell_operations_use_the_persistent_guid() {
        let identity = theme_identity(8, "codex-tray-icon");
        // Pin the persisted value so refactors and upgrades cannot silently
        // create another Settings entry for an existing root.
        assert_eq!(
            identity.guid,
            GUID::from_u128(0x2ad06097_b0fe_0e3a_8790_c274861a5ced)
        );
        let data = icon_data(HWND::default(), identity);
        assert_eq!(data.guidItem, identity.guid);
        assert_eq!(data.uID, identity.callback_id);
        assert_eq!(data.uFlags, NIF_GUID);
        assert_eq!(identity.guid, theme_identity(6, "CODEX-TRAY-ICON").guid);
    }

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
