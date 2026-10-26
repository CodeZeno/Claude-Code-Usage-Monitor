//! The controller owns application lifetime; Explorer owns only replaceable surfaces.
use super::*;
use std::cell::Cell;

pub(super) const WM_APP_RECONCILE: u32 = WM_APP + 16;
const TIMER_RECONCILE: usize = 16;
const WM_APP_SHELL_CHANGED: u32 = WM_APP + 17;
const WM_APP_SHELL_READY: u32 = WM_APP + 18;

thread_local! {
    static RECONCILING: Cell<bool> = const { Cell::new(false) };
    static DESTROYING_SURFACE: Cell<bool> = const { Cell::new(false) };
    static LAST_HOSTS: std::cell::RefCell<Vec<isize>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// No state lock: callers include poll workers and reentrant shell callbacks.
pub(super) fn controller_for(fallback: HWND) -> HWND {
    let controller = CONTROLLER_HWND.load(Ordering::Relaxed);
    if controller == 0 {
        fallback
    } else {
        SendHwnd(controller).to_hwnd()
    }
}

pub(super) unsafe fn set_controller_timer(
    hwnd: Option<HWND>,
    id: usize,
    interval: u32,
    callback: TIMERPROC,
) -> usize {
    SetTimer(hwnd.map(controller_for), id, interval, callback)
}

pub(super) unsafe fn kill_controller_timer(
    hwnd: Option<HWND>,
    id: usize,
) -> windows::core::Result<()> {
    KillTimer(hwnd.map(controller_for), id)
}

pub(super) fn schedule_reconcile(controller: HWND) {
    unsafe {
        set_controller_timer(Some(controller), TIMER_RECONCILE, 80, None);
    }
}

pub(super) unsafe extern "system" fn controller_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CLOSE | WM_APP_QUIT => {
            diagnose::log("controller: application shutdown requested");
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            // Withdraw the published target before destroying surfaces, so their
            // destruction notifications cannot schedule another recovery.
            CONTROLLER_HWND.store(0, Ordering::Relaxed);
            widget_tooltip::hide();
            crate::dashboard::close_existing();
            crate::desktop_compositor::clear();
            let (hook, windows) = {
                let mut state = lock_state();
                match state.take() {
                    Some(state) => (
                        state.win_event_hook,
                        std::iter::once(state.hwnd)
                            .chain(state.mirror_hwnds)
                            .chain(state.desktop_hwnds.into_iter().flatten())
                            .collect::<Vec<_>>(),
                    ),
                    None => (None, Vec::new()),
                }
            };
            if let Some(hook) = hook {
                native_interop::unhook_win_event(hook.to_hook());
            }
            for window in windows {
                destroy_surface(window.to_hwnd());
            }
            tray_icon::remove_all(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TIMER_RECONCILE => {
            let _ = KillTimer(Some(hwnd), TIMER_RECONCILE);
            reconcile_surfaces();
            LRESULT(0)
        }
        WM_APP_RECONCILE => {
            // The periodic watchdog only schedules work if hosts or surfaces
            // changed. Ordinary ticks do not repaint or touch tray registration.
            if hosts_changed() || surfaces_missing() {
                schedule_reconcile(hwnd);
            }
            LRESULT(0)
        }
        WM_APP_TRAY => {
            // Shell_NotifyIcon can synchronously call back while STATE is held.
            // Queue the callback before any attempt to resolve the UI surface.
            let _ = PostMessageW(
                Some(hwnd),
                native_interop::WM_APP_TRAY_DISPATCH,
                wparam,
                lparam,
            );
            LRESULT(0)
        }
        WM_SETTINGCHANGE | WM_DISPLAYCHANGE => {
            let _ = PostMessageW(
                Some(hwnd),
                WM_APP_SHELL_CHANGED,
                WPARAM(msg as usize),
                LPARAM(0),
            );
            LRESULT(0)
        }
        WM_APP_SHELL_CHANGED => dispatch_to_surface(wparam.0 as u32, WPARAM(0), LPARAM(0)),
        WM_TIMER | WM_COMMAND => dispatch_to_surface(msg, wparam, lparam),
        _ if msg == taskbar_created_message() => {
            let _ = PostMessageW(Some(hwnd), WM_APP_SHELL_READY, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        WM_APP_SHELL_READY => {
            sync_tray_icon(hwnd);
            schedule_reconcile(hwnd);
            LRESULT(0)
        }
        _ if (WM_APP..WM_APP_RECONCILE).contains(&msg) => dispatch_to_surface(msg, wparam, lparam),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn dispatch_to_surface(msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Reuse UI actions with the current surface, never a captured old HWND.
    // Work that outlives this call always targets controller_for(hwnd).
    let surface = lock_state()
        .as_ref()
        .map(|state| state.hwnd.to_hwnd())
        .unwrap_or_default();
    wnd_proc(surface, msg, wparam, lparam)
}

pub(super) unsafe fn create_primary_surface() -> HWND {
    let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
    let class = native_interop::wide_str("ClaudeCodeUsageSurface");
    RegisterClassW(&WNDCLASSW {
        style: CS_DBLCLKS,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance.into(),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    });
    CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
        PCWSTR(class.as_ptr()),
        PCWSTR::null(),
        WS_POPUP,
        0,
        0,
        1,
        1,
        None,
        None,
        Some(instance.into()),
        None,
    )
    .unwrap_or_default()
}

pub(super) fn surface_destroyed(hwnd: HWND) {
    widget_tooltip::hide_for(hwnd);
    crate::desktop_compositor::remove(hwnd);
    if DESTROYING_SURFACE.get() {
        return;
    }
    diagnose::log_recovery(format!(
        "surface destroyed hwnd={hwnd:?}; controller will reconcile"
    ));
    let controller = controller_for(HWND::default());
    if !controller.is_invalid() {
        // Do not take STATE or recreate windows inside Explorer's DestroyWindow.
        // The notification is deferred until the current native call returns.
        schedule_reconcile(controller);
    }
}

pub(super) unsafe fn destroy_surface(hwnd: HWND) {
    let previous = DESTROYING_SURFACE.replace(true);
    if IsWindow(Some(hwnd)).as_bool() {
        let _ = DestroyWindow(hwnd);
    }
    DESTROYING_SURFACE.set(previous);
}

pub(super) fn surface_alive(hwnd: HWND) -> bool {
    let mut process_id = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
    }
    (unsafe { IsWindow(Some(hwnd)).as_bool() })
        && process_id == std::process::id()
        && native_interop::window_class_name(hwnd).is_some_and(|name| {
            matches!(
                name.as_str(),
                "ClaudeCodeUsageSurface" | "CCUMThemeMirror" | "CCUMDesktopSurface"
            )
        })
}

pub(super) fn surface_has_class(hwnd: HWND, class: &str) -> bool {
    surface_alive(hwnd) && native_interop::window_class_name(hwnd).as_deref() == Some(class)
}

fn surfaces_missing() -> bool {
    lock_state().as_ref().is_some_and(|state| {
        let regular_missing = std::iter::once(state.hwnd)
            .chain(state.mirror_hwnds.iter().copied())
            .chain(state.desktop_hwnds.iter().flatten().copied())
            .any(|window| !surface_alive(window.to_hwnd()));
        regular_missing
            || state.active_theme.as_ref().is_some_and(|theme| {
                state.mirror_hwnds.len() + 1 < theme.surfaces.len()
                    || theme.surfaces.iter().enumerate().any(|(index, surface)| {
                        surface
                            .placement
                            .nest
                            .resolve(surface.placement.reference.region)
                            == SurfaceNest::Desktop
                            && state.desktop_hwnds.get(index).is_none_or(Option::is_none)
                    })
            })
    })
}

fn hosts_changed() -> bool {
    let mut hosts = Vec::new();
    for display in native_interop::find_monitors() {
        hosts.extend([
            display.handle.0 as isize,
            display.rect.left as isize,
            display.rect.top as isize,
            display.rect.right as isize,
            display.rect.bottom as isize,
        ]);
    }
    hosts.push(-1);
    for taskbar in native_interop::find_taskbars() {
        hosts.extend([
            taskbar.hwnd.0 as isize,
            taskbar.rect.left as isize,
            taskbar.rect.top as isize,
            taskbar.rect.right as isize,
            taskbar.rect.bottom as isize,
        ]);
    }
    hosts.push(-2);
    let has_desktop = lock_state().as_ref().is_some_and(|state| {
        state.active_theme.as_ref().is_some_and(|theme| {
            theme.surfaces.iter().any(|surface| {
                surface
                    .placement
                    .nest
                    .resolve(surface.placement.reference.region)
                    == SurfaceNest::Desktop
            })
        })
    });
    if has_desktop {
        if let Some(host) = native_interop::find_desktop_host() {
            hosts.extend([host.parent.0 as isize, host.insert_after.0 as isize]);
        }
    }
    LAST_HOSTS.with_borrow_mut(|previous| {
        if *previous == hosts {
            false
        } else {
            *previous = hosts;
            true
        }
    })
}

pub(super) fn reconcile_surfaces() {
    if RECONCILING.replace(true) {
        return;
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            RECONCILING.set(false);
        }
    }
    let _reset = Reset;
    refresh_theme_host_geometry();
    sync_surface_windows();
    let hwnd = lock_state().as_ref().map(|state| state.hwnd.to_hwnd());
    let Some(hwnd) = hwnd else {
        return;
    };
    if lock_state().as_ref().is_some_and(taskbar_lock_applies) {
        redock_locked_widget(hwnd);
    }
    position_at_taskbar();
    render_layered();
    sync_tray_icon(controller_for(hwnd));
}

#[cfg(test)]
mod tests;
