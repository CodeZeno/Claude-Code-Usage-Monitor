//! Public-API-only Windows Virtual Desktop support.
//!
//! Only the documented `IVirtualDesktopManager` COM interface
//! (`shobjidl_core.idl`) is used here — never the undocumented
//! `IVirtualDesktopManagerInternal`/`IVirtualDesktopPinnedApps` interfaces,
//! and never `EVENT_SYSTEM_DESKTOPSWITCH`. All calls in this module must run
//! on the UI thread: COM objects created here are apartment-threaded and are
//! never sent across threads.
//!
//! Two Win32 facts, confirmed live against a real Windows 11 session (not
//! just from documentation) before relying on them here:
//!
//! - `IVirtualDesktopManager::GetWindowDesktopId` only resolves a desktop id
//!   for a window the shell actually tracks per-desktop. A `WS_EX_TOOLWINDOW`
//!   window (this app's own main window's style) is never tracked and always
//!   returns `TYPE_E_ELEMENTNOTFOUND` — this is *why* an untouched
//!   `WS_EX_TOOLWINDOW` popup shows on every virtual desktop with no API
//!   calls at all, but it also means the "current active desktop" can only
//!   be queried through an ordinary (non-tool) window — see
//!   `active_desktop_probe_window`.
//! - `IVirtualDesktopManager::MoveWindowToDesktop`, unlike
//!   `GetWindowDesktopId`, *does* work on a `WS_EX_TOOLWINDOW` window (and
//!   `IsWindowOnCurrentVirtualDesktop` on it afterward correctly reflects
//!   the assignment) — so restricting the app's real main window to one
//!   desktop this way is safe.

use windows::core::GUID;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::IVirtualDesktopManager;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, ShowWindow, HMENU, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP,
};

use crate::diagnose;
use crate::native_interop;

/// `CLSID_VirtualDesktopManager` — `{AA509086-5CA9-4C25-8F95-589D3C07B48A}`.
/// Confirmed against the public `ShObjIdl.idl` coclass declaration
/// (`[ uuid(aa509086-5ca9-4c25-8f95-589d3c07b48a) ] coclass
/// VirtualDesktopManager`). The `windows` crate only generates a binding for
/// the interface itself, not this well-known class id, so it is defined
/// here rather than pulled in as a new dependency.
const CLSID_VIRTUAL_DESKTOP_MANAGER: GUID = GUID::from_u128(0xaa509086_5ca9_4c25_8f95_589d3c07b48a);

/// User-selected range of virtual desktops the widget should be displayed
/// on. Persisted in `SettingsFile`; `All` is the default and matches the
/// widget's historical behavior (a `WS_EX_TOOLWINDOW` top-level popup, which
/// Windows already shows on every virtual desktop without any explicit API
/// call).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VirtualDesktopScope {
    All,
    CurrentOnly,
}

impl Default for VirtualDesktopScope {
    fn default() -> Self {
        VirtualDesktopScope::All
    }
}

/// Ensure COM is initialized (single-threaded apartment) on the calling
/// thread. Idempotent — `CoInitializeEx` returning "already initialized"
/// (e.g. because `RoInitialize` already set the same apartment type for a
/// packaged build) is not treated as an error. Must be called once from the
/// UI thread before any other function in this module.
pub fn ensure_com_initialized() {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() {
            diagnose::log(format!("virtual_desktop: CoInitializeEx failed: {hr:?}"));
        }
    }
}

/// `CLSID_VirtualDesktopManager` is served in-process by `twinapi.dll` (only
/// an `InProcServer32` registration exists for it) — confirmed against the
/// live registry, not assumed from a `CLSCTX_LOCAL_SERVER` guess that
/// returns `REGDB_E_CLASSNOTREG`.
fn create_manager() -> Option<IVirtualDesktopManager> {
    unsafe {
        match CoCreateInstance::<_, IVirtualDesktopManager>(
            &CLSID_VIRTUAL_DESKTOP_MANAGER,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            Ok(manager) => Some(manager),
            Err(error) => {
                diagnose::log(format!(
                    "virtual_desktop: CoCreateInstance(VirtualDesktopManager) failed: {error:?}"
                ));
                None
            }
        }
    }
}

/// Create a hidden, never-shown top-level tool window on whichever virtual
/// desktop is currently active — used as a safe, current-desktop menu owner
/// for the tray/context menu (see `window::show_context_menu`), never for
/// `GetWindowDesktopId` (which does not track tool windows at all; see
/// `active_desktop_probe_window` for that).
///
/// Caller is responsible for destroying the returned window with
/// `DestroyWindow` once done with it.
pub fn create_probe_window() -> Option<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null()).ok()?;
        // "Static" is a system window class registered by user32 for every
        // GUI process, so this does not need its own `RegisterClassExW`.
        let class_name = native_interop::wide_str("Static");
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            hinstance,
            None,
        )
        .ok()
    }
}

/// Create the pair of windows `get_active_desktop_id` needs: a hidden
/// `owner`, and an ordinary (non-tool) top-level window owned by it —
/// `GetWindowDesktopId` only resolves an id for a window the shell tracks
/// per-desktop, which excludes `WS_EX_TOOLWINDOW` windows entirely.
/// Owning the visible probe by a hidden window keeps it out of the taskbar
/// and Alt+Tab (ordinary owned-window behavior) without needing
/// `WS_EX_TOOLWINDOW`, so the app's own real popup style is never used for
/// this query. Caller destroys both, probe first.
fn create_active_desktop_probe_windows() -> Option<(HWND, HWND)> {
    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null()).ok()?;
        let class_name = native_interop::wide_str("Static");

        let owner = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            hinstance,
            None,
        )
        .ok()?;

        let probe = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::null(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
            owner,
            HMENU::default(),
            hinstance,
            None,
        );
        let Ok(probe) = probe else {
            let _ = DestroyWindow(owner);
            return None;
        };
        let _ = ShowWindow(probe, SW_SHOWNOACTIVATE);
        Some((probe, owner))
    }
}

/// The virtual desktop id that is currently active, obtained via the
/// documented `IVirtualDesktopManager::GetWindowDesktopId` applied to a
/// freshly created, never-assigned probe window (see
/// `create_active_desktop_probe_windows`) rather than any undocumented "get
/// current desktop" API.
pub fn get_active_desktop_id() -> Option<GUID> {
    let manager = create_manager()?;
    let (probe, owner) = create_active_desktop_probe_windows()?;
    let result = unsafe { manager.GetWindowDesktopId(probe) };
    unsafe {
        let _ = DestroyWindow(probe);
        let _ = DestroyWindow(owner);
    }
    match result {
        Ok(id) => Some(id),
        Err(error) => {
            diagnose::log(format!(
                "virtual_desktop: GetWindowDesktopId(probe) failed: {error:?}"
            ));
            None
        }
    }
}

/// Assign `hwnd` to `desktop`, restricting it to that single virtual
/// desktop. Returns `false` (without panicking) if the Virtual Desktop COM
/// API is unavailable or the desktop id is no longer valid (e.g. the
/// desktop was deleted) — callers must fail open to the "all desktops"
/// behavior in that case, never leave the widget unexpectedly hidden. Works
/// on a `WS_EX_TOOLWINDOW` window (confirmed live), unlike
/// `GetWindowDesktopId`.
pub fn move_window_to_desktop(hwnd: HWND, desktop: GUID) -> bool {
    let Some(manager) = create_manager() else {
        return false;
    };
    match unsafe { manager.MoveWindowToDesktop(hwnd, &desktop) } {
        Ok(()) => true,
        Err(error) => {
            diagnose::log(format!(
                "virtual_desktop: MoveWindowToDesktop failed: {error:?}"
            ));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_defaults_to_all() {
        assert_eq!(VirtualDesktopScope::default(), VirtualDesktopScope::All);
    }

    #[test]
    fn scope_serializes_to_expected_snake_case() {
        assert_eq!(
            serde_json::to_string(&VirtualDesktopScope::All).unwrap(),
            "\"all\""
        );
        assert_eq!(
            serde_json::to_string(&VirtualDesktopScope::CurrentOnly).unwrap(),
            "\"current_only\""
        );
    }

    #[test]
    fn scope_round_trips_through_json() {
        for scope in [VirtualDesktopScope::All, VirtualDesktopScope::CurrentOnly] {
            let json = serde_json::to_string(&scope).unwrap();
            let decoded: VirtualDesktopScope = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, scope);
        }
    }

    #[test]
    fn clsid_matches_shobjidl_idl() {
        // `[ uuid(aa509086-5ca9-4c25-8f95-589d3c07b48a) ] coclass
        // VirtualDesktopManager` from the public Windows SDK's
        // ShObjIdl.idl — guards against the literal being edited by
        // accident.
        assert_eq!(
            CLSID_VIRTUAL_DESKTOP_MANAGER.to_u128(),
            0xaa509086_5ca9_4c25_8f95_589d3c07b48a
        );
    }

    /// Exercises the real COM path against the live desktop: not run by
    /// default (`cargo test -- --ignored`) since it needs an interactive
    /// Windows session, but uses only throwaway probe windows — it never
    /// touches the app's own main window or any running instance of it.
    #[test]
    #[ignore]
    fn live_get_active_desktop_and_move_a_tool_window_to_it() {
        ensure_com_initialized();
        let active = get_active_desktop_id()
            .expect("get_active_desktop_id should resolve a real desktop id");

        // Shaped like the app's real main window (WS_EX_TOOLWINDOW popup):
        // GetWindowDesktopId can't track this style, but MoveWindowToDesktop
        // must still succeed on it, and IsWindowOnCurrentVirtualDesktop must
        // reflect the assignment afterward.
        let probe =
            create_probe_window().expect("WS_EX_TOOLWINDOW probe window creation should succeed");
        let moved = move_window_to_desktop(probe, active);

        let manager = create_manager().expect("CoCreateInstance should succeed a second time");
        let is_on_current = unsafe { manager.IsWindowOnCurrentVirtualDesktop(probe) };

        unsafe {
            let _ = DestroyWindow(probe);
        }

        assert!(
            moved,
            "MoveWindowToDesktop on a WS_EX_TOOLWINDOW window to the already-active desktop \
             should succeed"
        );
        assert!(
            is_on_current.is_ok_and(|on_current| on_current.as_bool()),
            "IsWindowOnCurrentVirtualDesktop should confirm the assignment took effect"
        );
    }
}
