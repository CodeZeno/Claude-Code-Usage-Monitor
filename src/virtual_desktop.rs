//! Public-API-only Windows Virtual Desktop support.
//!
//! Only the documented `IVirtualDesktopManager` COM interface
//! (`shobjidl_core.idl`) is used here — never the undocumented
//! `IVirtualDesktopManagerInternal`/`IVirtualDesktopPinnedApps` interfaces,
//! and never `EVENT_SYSTEM_DESKTOPSWITCH`.
//!
//! ## Why this is app-level visibility synchronization, not native enforcement
//!
//! An earlier design tried to make Windows itself hide/show the widget by
//! calling `IVirtualDesktopManager::MoveWindowToDesktop` on the widget's own
//! `HWND` (or on an owner/anchor window for it) and relying on
//! `DWMWA_CLOAKED`/`IsWindowOnCurrentVirtualDesktop` to reflect the real,
//! live desktop-switch state. Confirmed live, repeatedly, against this
//! machine's real Windows session:
//!
//! - `GetWindowDesktopId` never resolves an id for a `WS_EX_TOOLWINDOW`
//!   window (this app's own popup style) — it always returns
//!   `TYPE_E_ELEMENTNOTFOUND`. It works for an ordinary
//!   `WS_OVERLAPPEDWINDOW`-style window instead.
//! - Even for a perfectly ordinary, unowned `WS_OVERLAPPEDWINDOW`,
//!   `MoveWindowToDesktop` succeeding and `IsWindowOnCurrentVirtualDesktop`
//!   correctly reflecting a desktop switch is *not stable*: a timestamped
//!   sampling audit (0/50/100/250/500/1000/2000ms after each switch, 3 round
//!   trips) showed the state settle to the correct value after switching
//!   away, then spontaneously revert to the wrong value roughly a second
//!   after switching back — with no further switch input at all.
//!
//! Given that instability, this module does not attempt to make the shell
//! itself cloak/uncloak the widget. Instead, `window.rs` polls
//! [`sense_current_desktop_id`] on a lightweight timer while
//! `virtual_desktop_scope` is `CurrentOnly`, compares the result against the
//! saved target desktop id, and drives the widget's own `ShowWindow`
//! calls — the same ones the "Show Widget" toggle already uses. The widget
//! itself is never moved to any desktop; it keeps its existing
//! `WS_EX_TOOLWINDOW` top-level style completely unchanged. See
//! `window::effective_widget_visible` for the visibility rule and
//! `window::TIMER_VIRTUAL_DESKTOP_SYNC` for the poll timer.
//!
//! ## Current Desktop Sensor
//!
//! [`sense_current_desktop_id`] tries three public-API methods in order,
//! from cheapest/most-common to most expensive:
//!
//! 1. [`current_desktop_id_via_foreground`] — `GetForegroundWindow`, then
//!    `GetWindowDesktopId` on it. Succeeds whenever some ordinary window
//!    (not this app's own tool-window popup) has focus, which is true most
//!    of the time in practice.
//! 2. [`current_desktop_id_via_enum_windows`] — `EnumWindows` over visible
//!    top-level windows, returning the first one for which
//!    `IsWindowOnCurrentVirtualDesktop` is true and `GetWindowDesktopId`
//!    resolves a non-null id. Used when nothing is focused (or focus is on
//!    this app's own popup).
//! 3. [`current_desktop_id_via_fresh_probe`] — as a last resort, creates a
//!    throwaway, off-screen (`x = y = -32000`), ordinary
//!    `WS_OVERLAPPEDWINDOW`, queries it, and destroys it immediately. Off
//!    screen and shown only for the few milliseconds it takes to query and
//!    destroy it, so it is not visually observable, but it is a normal
//!    window while it exists — this is why it is the last resort, not the
//!    primary method, and is never called on every poll tick in practice
//!    (methods 1/2 cover the overwhelming majority of polls).

use windows::core::GUID;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::IVirtualDesktopManager;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, EnumWindows, GetForegroundWindow, IsWindowVisible, ShowWindow,
    HMENU, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_OVERLAPPEDWINDOW, WS_POPUP,
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
/// UI thread before any other function in this module — every function here
/// (including the poll-timer-driven sensor) runs on that same thread.
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

fn is_null_guid(id: GUID) -> bool {
    id == GUID::from_u128(0)
}

/// Create a hidden, never-shown top-level tool window on whichever virtual
/// desktop is currently active — used as a safe, current-desktop menu owner
/// for the tray/context menu (see `window::show_context_menu`), never for
/// desktop sensing (`GetWindowDesktopId` does not track tool windows at
/// all; see [`current_desktop_id_via_fresh_probe`] for that).
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

/// The current virtual desktop's id, via the first public-API method (of
/// three) that succeeds — see the module docs for the full fallback chain
/// and the reasoning behind its ordering. `exclude` is this app's own main
/// `HWND`: since it is a `WS_EX_TOOLWINDOW` popup, `GetWindowDesktopId`
/// could never resolve an id for it anyway, but excluding it up front keeps
/// that failure mode explicit rather than incidental.
pub fn sense_current_desktop_id(exclude: HWND) -> Option<GUID> {
    if let Some(id) = current_desktop_id_via_foreground(exclude) {
        return Some(id);
    }
    if let Some(id) = current_desktop_id_via_enum_windows(exclude) {
        return Some(id);
    }
    current_desktop_id_via_fresh_probe()
}

/// Method 1: `GetForegroundWindow`, then the documented
/// `GetWindowDesktopId` on it. Cheap (two API calls, one COM object), and
/// covers the common case where some ordinary window has focus.
fn current_desktop_id_via_foreground(exclude: HWND) -> Option<GUID> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground == HWND::default() || foreground == exclude {
        return None;
    }
    let manager = create_manager()?;
    let id = unsafe { manager.GetWindowDesktopId(foreground) }.ok()?;
    if is_null_guid(id) {
        return None;
    }
    Some(id)
}

struct EnumCtx<'a> {
    manager: &'a IVirtualDesktopManager,
    exclude: HWND,
    found: Option<GUID>,
}

unsafe extern "system" fn enum_current_desktop_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut EnumCtx);
    if hwnd == ctx.exclude {
        return BOOL(1);
    }
    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }
    let Ok(on_current) = ctx.manager.IsWindowOnCurrentVirtualDesktop(hwnd) else {
        return BOOL(1);
    };
    if !on_current.as_bool() {
        return BOOL(1);
    }
    let Ok(id) = ctx.manager.GetWindowDesktopId(hwnd) else {
        return BOOL(1);
    };
    if is_null_guid(id) {
        return BOOL(1);
    }
    ctx.found = Some(id);
    BOOL(0) // stop enumeration — a match was found
}

/// Method 2: enumerate visible top-level windows via `EnumWindows`, and
/// return the first one for which the documented
/// `IsWindowOnCurrentVirtualDesktop` is true and `GetWindowDesktopId`
/// resolves a real (non-null) id. Used when method 1 finds nothing (e.g.
/// nothing is focused, or focus is on this app's own popup).
fn current_desktop_id_via_enum_windows(exclude: HWND) -> Option<GUID> {
    let manager = create_manager()?;
    let mut ctx = EnumCtx {
        manager: &manager,
        exclude,
        found: None,
    };
    unsafe {
        let _ = EnumWindows(
            Some(enum_current_desktop_window_proc),
            LPARAM(&mut ctx as *mut EnumCtx as isize),
        );
    }
    ctx.found
}

/// Create the pair of windows [`current_desktop_id_via_fresh_probe`] needs:
/// a hidden `owner`, and an ordinary (non-tool) top-level window owned by
/// it, positioned off-screen. `GetWindowDesktopId` only resolves an id for
/// a window the shell tracks per-desktop, which excludes `WS_EX_TOOLWINDOW`
/// windows entirely, so the probe must be an ordinary window style; owning
/// it by a hidden window and placing it off-screen keeps it out of the
/// taskbar, Alt+Tab, and the visible screen area for the few milliseconds
/// it exists. Caller destroys both, probe first.
fn create_offscreen_probe_windows() -> Option<(HWND, HWND)> {
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
            -32000,
            -32000,
            1,
            1,
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

/// Method 3 (last resort): see [`create_offscreen_probe_windows`] and the
/// module docs.
fn current_desktop_id_via_fresh_probe() -> Option<GUID> {
    let manager = create_manager()?;
    let (probe, owner) = create_offscreen_probe_windows()?;
    let result = unsafe { manager.GetWindowDesktopId(probe) };
    unsafe {
        let _ = DestroyWindow(probe);
        let _ = DestroyWindow(owner);
    }
    match result {
        Ok(id) if !is_null_guid(id) => Some(id),
        Ok(_) => None,
        Err(error) => {
            diagnose::log(format!(
                "virtual_desktop: GetWindowDesktopId(fresh probe) failed: {error:?}"
            ));
            None
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

    #[test]
    fn null_guid_detection_matches_zero_only() {
        assert!(is_null_guid(GUID::from_u128(0)));
        assert!(!is_null_guid(GUID::from_u128(1)));
    }

    /// Exercises the real COM/sensor path against the live desktop: not run
    /// by default (`cargo test -- --ignored`) since it needs an interactive
    /// Windows session. Uses only throwaway probe windows — it never
    /// touches the app's own main window or any running instance of it.
    #[test]
    #[ignore]
    fn live_sense_current_desktop_id_resolves_a_real_guid() {
        ensure_com_initialized();
        let id = sense_current_desktop_id(HWND::default())
            .expect("sense_current_desktop_id should resolve a real desktop id");
        assert!(!is_null_guid(id));
    }
}
