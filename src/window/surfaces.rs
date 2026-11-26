//! Creating, embedding and destroying the windows that show each theme
//! surface: taskbar-hosted, floating, desktop and mirror windows.
#![allow(unsafe_code, reason = "calls Win32 APIs through the windows crate")]

use super::*;

pub(super) fn sync_surface_windows() {
    // SAFETY: sync_surface_windows runs on the UI thread, which owns and pumps the
    // surface windows; the creator uses only its own NUL-terminated buffers.
    sync_surface_windows_with(|| unsafe { create_desktop_surface_window() });
}

pub(super) fn sync_surface_windows_with(mut create_desktop: impl FnMut() -> HWND) {
    let primary = lock_state().as_ref().map(|state| state.hwnd.to_hwnd());
    let Some(primary) = primary else {
        return;
    };
    if !lifecycle::surface_has_class(primary, "ClaudeCodeUsageSurface") {
        // SAFETY: this runs on the UI thread, which owns and pumps the surfaces; the
        // creator uses only its own NUL-terminated class buffer and a WNDPROC.
        let replacement = unsafe { lifecycle::create_primary_surface() };
        if replacement.is_invalid() {
            diagnose::log_recovery("unable to recreate primary surface; will retry");
            return;
        }
        if let Some(state) = lock_state().as_mut() {
            state.hwnd = SendHwnd::from_hwnd(replacement);
            state.embedded = false;
            state.taskbar_hwnd = None;
            state.tray_notify_hwnd = None;
            state.dragging = false;
            state.pending_drag = false;
            state.auto_ejected = false;
            state.auto_ejected_for_capacity = false;
            state.auto_ejected_origin = None;
            state.auto_ejected_host = None;
            state.is_snapped = false;
            state.is_switching_window_style = false;
            state.pending_mouse_click = None;
            state.hovered_mouse_layer = None;
            set_window_title(replacement, state.language.strings());
        }
        diagnose::log_recovery(format!("recreated primary surface hwnd={replacement:?}"));
    }
    let mirrors = lock_state()
        .as_ref()
        .map(|state| state.mirror_hwnds.clone())
        .unwrap_or_default();
    for (index, mirror) in mirrors.into_iter().enumerate() {
        if !lifecycle::surface_has_class(mirror.to_hwnd(), "CCUMThemeMirror") {
            // SAFETY: this runs on the UI thread, which owns and pumps the surfaces; the
            // creator uses only its own NUL-terminated buffers and a WNDPROC.
            let replacement = unsafe { create_mirror_window() };
            // Window creation can re-enter; never index a list it may shrink.
            let orphaned = match lock_state()
                .as_mut()
                .and_then(|state| state.mirror_hwnds.get_mut(index))
            {
                Some(slot) => {
                    *slot = SendHwnd::from_hwnd(replacement);
                    false
                }
                None => true,
            };
            if orphaned {
                // SAFETY: `replacement` was created on this UI thread, which is required for
                // DestroyWindow; destroy_surface checks IsWindow first.
                unsafe { lifecycle::destroy_surface(replacement) };
            }
        }
    }
    let (desired_total, desktop_surfaces) = {
        let state = lock_state();
        state
            .as_ref()
            .map(|state| {
                let surfaces = state
                    .active_theme
                    .as_ref()
                    .map(|theme| {
                        theme
                            .surfaces
                            .iter()
                            .map(|surface| {
                                surface
                                    .placement
                                    .nest
                                    .resolve(surface.placement.reference.region)
                                    == SurfaceNest::Desktop
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                (surfaces.len().max(1), surfaces)
            })
            .unwrap_or_else(|| (1, Vec::new()))
    };
    let desired_mirrors = desired_total.saturating_sub(1);
    loop {
        let remove = {
            let mut state = lock_state();
            state.as_mut().and_then(|state| {
                if state.mirror_hwnds.len() > desired_mirrors {
                    state.mirror_hwnds.pop()
                } else {
                    None
                }
            })
        };
        match remove {
            // SAFETY: mirror surfaces are created on this UI thread, so DestroyWindow is
            // called from their owning thread; destroy_surface checks IsWindow first.
            Some(hwnd) => unsafe {
                lifecycle::destroy_surface(hwnd.to_hwnd());
            },
            None => break,
        }
    }
    while lock_state()
        .as_ref()
        .map(|state| state.mirror_hwnds.len())
        .unwrap_or(0)
        < desired_mirrors
    {
        // SAFETY: this runs on the UI thread, which owns and pumps the surfaces; the
        // creator uses only its own NUL-terminated buffers and a WNDPROC.
        let mirror = unsafe { create_mirror_window() };
        if mirror.is_invalid() {
            break;
        }
        if let Some(state) = lock_state().as_mut() {
            state.mirror_hwnds.push(SendHwnd::from_hwnd(mirror));
        }
    }

    let stale_desktop_windows = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        let removed = if state.desktop_hwnds.len() > desired_total {
            state.desktop_hwnds.split_off(desired_total)
        } else {
            Vec::new()
        };
        state.desktop_hwnds.resize_with(desired_total, || None);
        let mut stale = removed.into_iter().flatten().collect::<Vec<_>>();
        for (surface_index, window) in state.desktop_hwnds.iter_mut().enumerate() {
            let wanted = desktop_surfaces.get(surface_index) == Some(&true);
            let valid = window.is_some_and(|window| {
                lifecycle::surface_has_class(window.to_hwnd(), "CCUMDesktopSurface")
            });
            if !wanted || !valid {
                if let Some(window) = window.take() {
                    stale.push(window);
                }
            }
        }
        stale
    };
    for window in stale_desktop_windows {
        // SAFETY: desktop surfaces are created on this UI thread, so destroy_surface calls
        // DestroyWindow from their owning thread after checking IsWindow.
        unsafe {
            if lifecycle::surface_has_class(window.to_hwnd(), "CCUMDesktopSurface") {
                lifecycle::destroy_surface(window.to_hwnd());
            } else {
                crate::desktop_compositor::remove(window.to_hwnd());
            }
        }
    }
    for (surface_index, wanted) in desktop_surfaces.into_iter().enumerate() {
        if !wanted {
            continue;
        }
        let missing = lock_state()
            .as_ref()
            .and_then(|state| state.desktop_hwnds.get(surface_index))
            .is_none_or(Option::is_none);
        if !missing {
            continue;
        }
        let window = create_desktop();
        if window.is_invalid() {
            continue;
        }
        // SAFETY: ShowWindow takes the just-created surface HWND by value with no pointers.
        unsafe {
            let _ = ShowWindow(window, SW_HIDE);
        }
        if let Some(slot) = lock_state()
            .as_mut()
            .and_then(|state| state.desktop_hwnds.get_mut(surface_index))
        {
            *slot = Some(SendHwnd::from_hwnd(window));
        } else {
            // SAFETY: `window` was created on this UI thread, which is required for
            // DestroyWindow; destroy_surface checks IsWindow first.
            unsafe {
                lifecycle::destroy_surface(window);
            }
        }
    }
}

unsafe fn create_desktop_surface_window() -> HWND {
    let Some(desktop) = native_interop::find_desktop_host() else {
        return HWND::default();
    };
    create_desktop_surface_window_in(desktop)
}

pub(super) unsafe fn create_desktop_surface_window_in(
    desktop: native_interop::DesktopHost,
) -> HWND {
    let instance =
        GetModuleHandleW(PCWSTR::null()).expect("the running executable has a module handle");
    let class = native_interop::wide_str("CCUMDesktopSurface");
    let title = native_interop::wide_str("");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(mirror_wnd_proc),
        hInstance: HINSTANCE(instance.0),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszClassName: PCWSTR::from_raw(class.as_ptr()),
        ..Default::default()
    };
    RegisterClassExW(&wc);
    let previous_hosting = SetThreadDpiHostingBehavior(DPI_HOSTING_BEHAVIOR_MIXED);
    let previous_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE);
    let window = CreateWindowExW(
        WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE,
        PCWSTR::from_raw(class.as_ptr()),
        PCWSTR::from_raw(title.as_ptr()),
        WINDOW_STYLE(
            native_interop::WS_CHILD_STYLE | native_interop::WS_CLIPSIBLINGS_STYLE | WS_VISIBLE.0,
        ),
        0,
        0,
        198,
        144,
        Some(desktop.parent),
        None,
        Some(HINSTANCE(instance.0)),
        None,
    )
    .unwrap_or_default();
    let _ = SetThreadDpiAwarenessContext(previous_dpi);
    let _ = SetThreadDpiHostingBehavior(previous_hosting);
    if window.is_invalid() {
        diagnose::log("unable to create raised-desktop surface window");
    }
    window
}

unsafe fn create_mirror_window() -> HWND {
    let instance =
        GetModuleHandleW(PCWSTR::null()).expect("the running executable has a module handle");
    let class = native_interop::wide_str("CCUMThemeMirror");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_DBLCLKS,
        lpfnWndProc: Some(mirror_wnd_proc),
        hInstance: HINSTANCE(instance.0),
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszClassName: PCWSTR::from_raw(class.as_ptr()),
        ..Default::default()
    };
    RegisterClassExW(&wc);
    let title = native_interop::wide_str("Usage theme mirror");
    CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
        PCWSTR::from_raw(class.as_ptr()),
        PCWSTR::from_raw(title.as_ptr()),
        WS_POPUP,
        0,
        0,
        1,
        1,
        None,
        None,
        Some(HINSTANCE(instance.0)),
        None,
    )
    .unwrap_or_default()
}

pub(super) unsafe extern "system" fn mirror_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        // See the primary surface: DefWindowProc forwards both to the parent.
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL | WM_APPCOMMAND | WM_CONTEXTMENU => LRESULT(0),
        WM_SETCURSOR => {
            set_surface_cursor(hwnd);
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            update_mouse_hover(hwnd, lparam);
            LRESULT(0)
        }
        WM_MOUSEHOVER => {
            widget_tooltip::show(hwnd);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            clear_mouse_hover(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            widget_tooltip::hide();
            let suppressed = {
                let mut state = lock_state();
                state.as_mut().is_some_and(|state| {
                    let suppressed = state.suppress_next_left_up;
                    state.suppress_next_left_up = false;
                    suppressed
                })
            };
            if !suppressed {
                if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                    schedule_or_dispatch_click(hwnd, surface, object);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            widget_tooltip::hide();
            if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                dispatch_double_click(hwnd, surface, object);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            widget_tooltip::hide();
            if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                let _ = dispatch_mouse_event(surface, &object, MouseEventKind::RightClick);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut paint);
            let _ = EndPaint(hwnd, &paint);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DESTROY => {
            lifecycle::surface_destroyed(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
