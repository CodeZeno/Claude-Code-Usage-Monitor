use super::*;

pub(super) fn surface_index_for_window(state: &AppState, hwnd: HWND) -> Option<usize> {
    if state.hwnd.to_hwnd() == hwnd {
        return Some(0);
    }
    if let Some(index) = state
        .mirror_hwnds
        .iter()
        .position(|window| window.to_hwnd() == hwnd)
    {
        return Some(index + 1);
    }
    state
        .desktop_hwnds
        .iter()
        .position(|window| window.is_some_and(|window| window.to_hwnd() == hwnd))
}

pub(super) fn mouse_client_point(lparam: LPARAM) -> (f64, f64) {
    let packed = lparam.0 as u32;
    let x = (packed as u16 as i16) as f64;
    let y = ((packed >> 16) as u16 as i16) as f64;
    (x, y)
}

pub(super) fn mouse_target_at(hwnd: HWND, lparam: LPARAM) -> Option<(usize, String)> {
    let state = lock_state();
    let state = state.as_ref()?;
    let surface_index = surface_index_for_window(state, hwnd)?;
    let theme = effective_theme_from_state(state)?;
    let scale = theme_surface_scale(&theme, surface_index).max(0.01);
    let runtime = theme_runtime_for_surface(&theme, surface_index, theme_runtime_from_state(state));
    let (x, y) = mouse_client_point(lparam);
    let object_id = theme_engine::hit_test_mouse_event(
        &theme,
        surface_index,
        x / scale,
        y / scale,
        state.data.as_ref(),
        runtime,
    )?;
    Some((surface_index, object_id))
}

pub(super) fn mouse_handler_exists(
    surface_index: usize,
    object_id: &str,
    event: MouseEventKind,
) -> bool {
    let state = lock_state();
    state.as_ref().is_some_and(|state| {
        state.active_theme.as_ref().is_some_and(|theme| {
            theme_engine::mouse_event_script(theme, surface_index, object_id, event).is_some()
        })
    })
}

pub(super) fn dispatch_mouse_event(
    surface_index: usize,
    object_id: &str,
    event: MouseEventKind,
) -> bool {
    let source = {
        let state = lock_state();
        let Some(state) = state.as_ref() else {
            return false;
        };
        let Some(theme) = state.active_theme.as_ref() else {
            return false;
        };
        let Some(source) = theme_engine::mouse_event_script(theme, surface_index, object_id, event)
            .map(str::to_string)
        else {
            return false;
        };
        source
    };
    execute_mouse_action_source(surface_index, object_id, &source)
}

pub(super) fn execute_mouse_action_source(
    surface_index: usize,
    object_id: &str,
    source: &str,
) -> bool {
    let result = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return false;
        };
        let Some(theme) = state.active_theme.clone() else {
            return false;
        };
        let data = state.data.clone();
        let runtime =
            theme_runtime_for_surface(&theme, surface_index, theme_runtime_from_state(state));
        theme_engine::execute_mouse_actions(
            &theme,
            surface_index,
            object_id,
            source,
            data.as_ref(),
            runtime,
            &mut state.mouse_action_overrides,
        )
    };
    match result {
        Ok(effects) => {
            render_layered();
            let owner = lock_state()
                .as_ref()
                .map(|state| state.hwnd.to_hwnd())
                .unwrap_or_default();
            sync_tray_icon(owner);
            for effect in effects {
                match effect {
                    MouseActionEffect::ShowDashboard => crate::dashboard::show(owner),
                    MouseActionEffect::ToggleDashboard => crate::dashboard::toggle(owner),
                    MouseActionEffect::ShowContextMenu(menu) => show_context_menu_document(
                        owner,
                        menu.as_deref(),
                        Some((surface_index, object_id.to_string())),
                    ),
                }
            }
            true
        }
        Err(error) => {
            diagnose::log(format!(
                "mouse action failed surface={surface_index} layer={object_id}: {error}"
            ));
            false
        }
    }
}

pub(super) fn schedule_or_dispatch_click(hwnd: HWND, surface_index: usize, object_id: String) {
    let has_click = mouse_handler_exists(surface_index, &object_id, MouseEventKind::Click);
    if !has_click {
        return;
    }
    if mouse_handler_exists(surface_index, &object_id, MouseEventKind::DoubleClick) {
        let owner = {
            let mut state = lock_state();
            let Some(state) = state.as_mut() else {
                return;
            };
            state.pending_mouse_click = Some(PendingMouseClick {
                surface_index,
                object_id,
            });
            state.hwnd.to_hwnd()
        };
        unsafe {
            let _ = KillTimer(Some(owner), TIMER_MOUSE_CLICK);
            SetTimer(
                Some(owner),
                TIMER_MOUSE_CLICK,
                GetDoubleClickTime().max(1),
                None,
            );
        }
    } else {
        let _ = dispatch_mouse_event(surface_index, &object_id, MouseEventKind::Click);
    }
    let _ = hwnd;
}

pub(super) fn dispatch_double_click(hwnd: HWND, surface_index: usize, object_id: String) {
    if !mouse_handler_exists(surface_index, &object_id, MouseEventKind::DoubleClick) {
        return;
    }
    let owner = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        state.pending_mouse_click = None;
        state.suppress_next_left_up = true;
        state.hwnd.to_hwnd()
    };
    unsafe {
        let _ = KillTimer(Some(owner), TIMER_MOUSE_CLICK);
    }
    let _ = dispatch_mouse_event(surface_index, &object_id, MouseEventKind::DoubleClick);
    let _ = hwnd;
}

pub(super) fn update_mouse_hover(hwnd: HWND, lparam: LPARAM) {
    unsafe {
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        let _ = TrackMouseEvent(&mut tracking);
    }
    for _ in 0..4 {
        let target = mouse_target_at(hwnd, lparam);
        let previous = {
            let mut state = lock_state();
            let Some(state) = state.as_mut() else {
                return;
            };
            if state.hovered_mouse_layer == target {
                return;
            }
            std::mem::replace(&mut state.hovered_mouse_layer, target.clone())
        };
        if let Some((surface, object)) = previous {
            let _ = dispatch_mouse_event(surface, &object, MouseEventKind::MouseLeave);
        }
        if let Some((surface, object)) = &target {
            let _ = dispatch_mouse_event(*surface, object, MouseEventKind::MouseEnter);
        }
    }
    diagnose::log("mouse enter/leave actions did not stabilize after four transitions");
    if let Some(state) = lock_state().as_mut() {
        state.hovered_mouse_layer = None;
    }
}

pub(super) fn clear_mouse_hover(hwnd: HWND) {
    let previous = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        let surface = surface_index_for_window(state, hwnd);
        if state
            .hovered_mouse_layer
            .as_ref()
            .is_none_or(|(hovered, _)| Some(*hovered) != surface)
        {
            return;
        }
        state.hovered_mouse_layer.take()
    };
    if let Some((surface, object)) = previous {
        let _ = dispatch_mouse_event(surface, &object, MouseEventKind::MouseLeave);
    }
}

pub(super) fn update_tray_mouse_hover(hwnd: HWND, surface_index: usize, object_id: String) {
    let target = Some((surface_index, object_id));
    let previous = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        if state.hovered_mouse_layer == target {
            unsafe {
                SetTimer(Some(hwnd), TIMER_TRAY_HOVER, 100, None);
            }
            return;
        }
        std::mem::replace(&mut state.hovered_mouse_layer, target.clone())
    };
    if let Some((surface, object)) = previous {
        let _ = dispatch_mouse_event(surface, &object, MouseEventKind::MouseLeave);
    }
    if let Some((surface, object)) = target {
        let _ = dispatch_mouse_event(surface, &object, MouseEventKind::MouseEnter);
    }
    unsafe {
        SetTimer(Some(hwnd), TIMER_TRAY_HOVER, 100, None);
    }
}

pub(super) fn clear_tray_mouse_hover_if_left(hwnd: HWND) {
    let hovered = {
        let state = lock_state();
        let Some(state) = state.as_ref() else {
            return;
        };
        state.hovered_mouse_layer.clone().map(|target| {
            let is_tray = state.active_theme.as_ref().is_some_and(|theme| {
                theme.surfaces.get(target.0).is_some_and(|surface| {
                    surface
                        .placement
                        .nest
                        .resolve(surface.placement.reference.region)
                        == SurfaceNest::TrayIcon
                })
            });
            (target, is_tray)
        })
    };
    let Some((target, true)) = hovered else {
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_TRAY_HOVER);
        }
        return;
    };
    if tray_icon::cursor_over_themed_icon(hwnd, target.0) {
        return;
    }
    let previous = lock_state().as_mut().and_then(|state| {
        (state.hovered_mouse_layer.as_ref() == Some(&target))
            .then(|| state.hovered_mouse_layer.take())
            .flatten()
    });
    unsafe {
        let _ = KillTimer(Some(hwnd), TIMER_TRAY_HOVER);
    }
    if let Some((surface, object)) = previous {
        let _ = dispatch_mouse_event(surface, &object, MouseEventKind::MouseLeave);
    }
}

pub(super) unsafe fn set_surface_cursor(hwnd: HWND) -> bool {
    let mut point = POINT::default();
    if GetCursorPos(&mut point).is_err() {
        return false;
    }
    let mut client = [point];
    MapWindowPoints(None, Some(hwnd), &mut client);
    let packed = ((client[0].y as u32 & 0xffff) << 16) | (client[0].x as u32 & 0xffff);
    let Some((surface, object)) = mouse_target_at(hwnd, LPARAM(packed as isize)) else {
        return false;
    };
    let clickable = [
        MouseEventKind::Click,
        MouseEventKind::DoubleClick,
        MouseEventKind::RightClick,
    ]
    .into_iter()
    .any(|event| mouse_handler_exists(surface, &object, event));
    if clickable {
        let cursor = LoadCursorW(None, IDC_HAND).unwrap_or_default();
        SetCursor(Some(cursor));
    }
    clickable
}
