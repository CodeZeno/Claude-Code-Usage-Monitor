use super::*;

/// Main window procedure
pub(super) unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let _ = BeginPaint(hwnd, &mut ps);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DISPLAYCHANGE | WM_DPICHANGED_MSG | WM_SETTINGCHANGE => {
            if msg == WM_DPICHANGED_MSG {
                let new_dpi = (wparam.0 & 0xFFFF) as u32;
                CURRENT_DPI.store(new_dpi, Ordering::Relaxed);
            }
            if msg == WM_SETTINGCHANGE {
                check_theme_change();
                check_language_change();
            }
            refresh_dpi();
            position_at_taskbar();
            render_layered();
            sync_tray_icon(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            let timer_id = wparam.0;
            match timer_id {
                TIMER_POLL => {
                    let auth_watch = {
                        let state = lock_state();
                        state.as_ref().map(|s| {
                            (
                                s.auth_error_paused_polling,
                                s.auth_watch_mode,
                                s.auth_watch_snapshot.clone(),
                            )
                        })
                    };
                    match auth_watch {
                        Some((true, watch_mode, previous_snapshot)) => {
                            let current_snapshot = poller::credential_watch_snapshot(watch_mode);
                            if current_snapshot != previous_snapshot {
                                let mut state = lock_state();
                                if let Some(s) = state.as_mut() {
                                    if s.auth_error_paused_polling
                                        && s.auth_watch_mode == watch_mode
                                    {
                                        s.auth_watch_snapshot = current_snapshot;
                                    }
                                }
                                drop(state);
                                request_poll(hwnd);
                            }
                        }
                        Some((false, _, _)) => {
                            request_scheduled_poll(hwnd);
                        }
                        None => {}
                    }
                }
                TIMER_COUNTDOWN => {
                    render_layered();
                    sync_tray_icon(hwnd);
                    schedule_countdown_timer();
                }
                TIMER_CLOCK => {
                    render_layered();
                    let refresh_tray = lock_state()
                        .as_ref()
                        .is_some_and(|state| state.tray_theme_uses_current_time);
                    if refresh_tray {
                        sync_tray_icon(hwnd);
                    }
                    schedule_clock_timer();
                }
                TIMER_RESET_POLL => {
                    let should_poll = {
                        let state = lock_state();
                        state
                            .as_ref()
                            .map(|s| !s.auth_error_paused_polling)
                            .unwrap_or(false)
                    };
                    if should_poll {
                        request_scheduled_poll(hwnd);
                    }
                }
                TIMER_UPDATE_CHECK => {
                    begin_update_check(hwnd, false);
                }
                TIMER_WINDOW_STATE => {
                    sync_theme_window_visibility();
                }
                TIMER_MOUSE_CLICK => {
                    let _ = KillTimer(Some(hwnd), TIMER_MOUSE_CLICK);
                    let pending = lock_state()
                        .as_mut()
                        .and_then(|state| state.pending_mouse_click.take());
                    if let Some(pending) = pending {
                        let _ = dispatch_mouse_event(
                            pending.surface_index,
                            &pending.object_id,
                            MouseEventKind::Click,
                        );
                    }
                }
                TIMER_TRAY_HOVER => {
                    clear_tray_mouse_hover_if_left(hwnd);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_USAGE_UPDATED => {
            check_theme_change();
            check_language_change();
            render_layered();
            schedule_countdown_timer();
            schedule_clock_timer();
            suppress_tray_reposition_for(Duration::from_millis(
                TRAY_ICON_UPDATE_REPOSITION_SUPPRESS_MS,
            ));
            sync_tray_icon(hwnd);
            LRESULT(0)
        }
        WM_APP_SETTINGS_UPDATED => {
            reload_external_settings(hwnd);
            LRESULT(0)
        }
        WM_APP_REFRESH_NOW => {
            request_poll(hwnd);
            LRESULT(0)
        }
        WM_APP_OPEN_DASHBOARD => {
            crate::dashboard::show(hwnd);
            LRESULT(0)
        }
        WM_APP_QUIT => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_APP_UPDATE_CHECK_COMPLETE => {
            schedule_auto_update_check(hwnd);
            LRESULT(0)
        }
        WM_SETCURSOR if set_surface_cursor(hwnd) => LRESULT(1),
        WM_SETCURSOR => DefWindowProcW(hwnd, msg, wparam, lparam),
        WM_MOUSEMOVE => {
            let is_dragging = {
                let state = lock_state();
                state.as_ref().map(|s| s.dragging).unwrap_or(false)
            };
            if is_dragging {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let move_target = {
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

                    let taskbar_hwnd = s.taskbar_hwnd.map(SendHwnd::to_hwnd);
                    let embedded = s.embedded;
                    let hwnd_val = s.hwnd.to_hwnd();

                    // Clamp: don't go past left edge of taskbar
                    if let Some(taskbar_hwnd) = taskbar_hwnd {
                        if let Some(taskbar_rect) = native_interop::get_taskbar_rect(taskbar_hwnd) {
                            let mut tray_left = taskbar_rect.right;
                            if let Some(tray_hwnd) =
                                native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd")
                            {
                                if let Some(tray_rect) =
                                    native_interop::get_window_rect_safe(tray_hwnd)
                                {
                                    tray_left = tray_rect.left;
                                }
                            }
                            let widget_width = total_widget_width_for_state(s);
                            let max_offset = (tray_left - taskbar_rect.left - widget_width).max(0);
                            if new_offset > max_offset {
                                new_offset = max_offset;
                            }

                            s.tray_offset = new_offset;

                            let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
                            let anchor_top = taskbar_rect.top;
                            let anchor_height = taskbar_height;
                            let widget_height = total_widget_height_for_state(s);
                            let y = compute_anchor_y(anchor_top, anchor_height, widget_height);
                            let x = if embedded {
                                tray_left - taskbar_rect.left - widget_width - new_offset
                            } else {
                                tray_left - widget_width - new_offset
                            };
                            Some((
                                hwnd_val,
                                embedded,
                                x,
                                y,
                                taskbar_rect.top,
                                widget_width,
                                widget_height,
                            ))
                        } else {
                            s.tray_offset = new_offset;
                            None
                        }
                    } else {
                        s.tray_offset = new_offset;
                        None
                    }
                };

                if let Some((hwnd_val, embedded, x, y, taskbar_top, widget_width, widget_height)) =
                    move_target
                {
                    if embedded {
                        native_interop::move_window(
                            hwnd_val,
                            x,
                            y - taskbar_top,
                            widget_width,
                            widget_height,
                        );
                    } else {
                        native_interop::move_window(hwnd_val, x, y, widget_width, widget_height);
                    }
                }
            } else {
                update_mouse_hover(hwnd, lparam);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            clear_mouse_hover(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                dispatch_double_click(hwnd, surface, object);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                let _ = dispatch_mouse_event(surface, &object, MouseEventKind::RightClick);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let suppressed = {
                let mut state = lock_state();
                state.as_mut().is_some_and(|state| {
                    let suppressed = state.suppress_next_left_up;
                    state.suppress_next_left_up = false;
                    suppressed
                })
            };
            if suppressed {
                return LRESULT(0);
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let drag_result = {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    if s.dragging {
                        s.dragging = false;
                        Some((s.taskbar_index, s.drag_start_client_x))
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if let Some((current_taskbar_index, drag_start_client_x)) = drag_result {
                let _ = ReleaseCapture();
                if let Some((target_index, target_taskbar)) = taskbar_at_point(pt) {
                    if target_index != current_taskbar_index {
                        let new_offset = offset_for_drop_point(
                            target_taskbar.hwnd,
                            target_taskbar.rect,
                            pt,
                            drag_start_client_x,
                        );
                        {
                            let mut state = lock_state();
                            if let Some(s) = state.as_mut() {
                                s.tray_offset = new_offset;
                            }
                        }
                        if attach_to_taskbar(hwnd, target_index) {
                            position_at_taskbar();
                            render_layered();
                        }
                    }
                }
                save_state_settings();
            } else if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                schedule_or_dispatch_click(hwnd, surface, object);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = wparam.0 as u16;
            match id {
                IDM_DASHBOARD => {
                    crate::dashboard::show(hwnd);
                }
                1 => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.force_notify_auth_error = true;
                        }
                    }
                    render_layered();
                    request_poll(hwnd);
                }
                IDM_VERSION_ACTION => {
                    let (install_channel, release) = {
                        let state = lock_state();
                        match state.as_ref() {
                            Some(s) => (
                                s.install_channel,
                                match &s.update_status {
                                    UpdateStatus::Available(release) => Some(release.clone()),
                                    _ => None,
                                },
                            ),
                            None => (InstallChannel::Portable, None),
                        }
                    };

                    match install_channel {
                        InstallChannel::Winget => {
                            if release.is_some() {
                                begin_winget_update(hwnd);
                            } else {
                                begin_update_check(hwnd, true);
                            }
                        }
                        InstallChannel::Portable => {
                            if let Some(release) = release {
                                begin_update_apply(hwnd, release);
                            } else {
                                begin_update_check(hwnd, true);
                            }
                        }
                    }
                }
                2 => {
                    let hook = {
                        let state = lock_state();
                        state.as_ref().and_then(|s| s.win_event_hook)
                    };
                    if let Some(h) = hook {
                        native_interop::unhook_win_event(h.to_hook());
                    }
                    crate::dashboard::close_existing();
                    let _ = DestroyWindow(hwnd);
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
                    SetTimer(Some(hwnd), TIMER_POLL, new_interval, None);
                }
                id if ProviderId::from_native_menu_command_id(id).is_some() => {
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            let provider = ProviderId::from_native_menu_command_id(id)
                                .expect("provider menu command was matched above");
                            s.providers.toggle(provider);
                        }
                    }
                    save_state_settings();
                    position_at_taskbar();
                    render_layered();
                    sync_tray_icon(hwnd);
                    request_poll(hwnd);
                }
                id if id == IDM_LANG_SYSTEM || language_from_menu_command_id(id).is_some() => {
                    let language_override = if id == IDM_LANG_SYSTEM {
                        None
                    } else {
                        language_from_menu_command_id(id)
                    };
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            apply_language_to_state(s, language_override);
                        }
                    }
                    save_state_settings();
                    render_layered();
                }
                _ => {}
            }
            LRESULT(0)
        }
        _ if msg == WM_APP_TRAY => {
            let tray_message = lparam.0 as u32;
            if let Some(surface_index) = tray_icon::themed_surface_index(wparam.0 as u32) {
                let root_id = lock_state().as_ref().and_then(|state| {
                    state
                        .active_theme
                        .as_ref()
                        .and_then(|theme| theme.surfaces.get(surface_index))
                        .map(|surface| surface.id.clone())
                });
                if let Some(root_id) = root_id {
                    match tray_message {
                        WM_MOUSEMOVE => {
                            update_tray_mouse_hover(hwnd, surface_index, root_id);
                            return LRESULT(0);
                        }
                        WM_LBUTTONUP => {
                            let suppressed = {
                                let mut state = lock_state();
                                state.as_mut().is_some_and(|state| {
                                    let suppressed = state.suppress_next_left_up;
                                    state.suppress_next_left_up = false;
                                    suppressed
                                })
                            };
                            if suppressed {
                                return LRESULT(0);
                            }
                            if mouse_handler_exists(surface_index, &root_id, MouseEventKind::Click)
                            {
                                schedule_or_dispatch_click(hwnd, surface_index, root_id);
                            } else if !mouse_handler_exists(
                                surface_index,
                                &root_id,
                                MouseEventKind::DoubleClick,
                            ) {
                                crate::dashboard::show(hwnd);
                            }
                            return LRESULT(0);
                        }
                        WM_LBUTTONDBLCLK => {
                            if mouse_handler_exists(
                                surface_index,
                                &root_id,
                                MouseEventKind::DoubleClick,
                            ) {
                                dispatch_double_click(hwnd, surface_index, root_id);
                            } else {
                                crate::dashboard::show(hwnd);
                            }
                            return LRESULT(0);
                        }
                        WM_RBUTTONUP | WM_CONTEXTMENU => {
                            if !dispatch_mouse_event(
                                surface_index,
                                &root_id,
                                MouseEventKind::RightClick,
                            ) {
                                show_context_menu_document(hwnd, None, None);
                            }
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                }
            }
            match tray_icon::handle_message(lparam) {
                tray_icon::TrayAction::OpenDashboard => {
                    crate::dashboard::show(hwnd);
                }
                tray_icon::TrayAction::ShowContextMenu => {
                    show_context_menu_document(hwnd, None, None);
                }
                tray_icon::TrayAction::None => {}
            }
            LRESULT(0)
        }
        _ if msg == taskbar_created_message() => {
            // Explorer discards notification icons when it restarts. Floating
            // and tray-icon-only themes keep their owner HWND, so restore the
            // registrations when the shell broadcasts its return.
            sync_tray_icon(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            crate::dashboard::close_existing();
            crate::desktop_compositor::clear();
            let (hook, desktop_windows) = {
                let mut state = lock_state();
                match state.as_mut() {
                    Some(state) => (
                        state.win_event_hook,
                        std::mem::take(&mut state.desktop_hwnds),
                    ),
                    None => (None, Vec::new()),
                }
            };
            if let Some(h) = hook {
                native_interop::unhook_win_event(h.to_hook());
            }
            for window in desktop_windows.into_iter().flatten() {
                let _ = DestroyWindow(window.to_hwnd());
            }
            tray_icon::remove_all(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
