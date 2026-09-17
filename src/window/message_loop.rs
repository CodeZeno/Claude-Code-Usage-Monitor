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
            refresh_theme_host_geometry();
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
                TIMER_TRAY_REPOSITION => {
                    let _ = KillTimer(Some(hwnd), TIMER_TRAY_REPOSITION);
                    refresh_theme_host_geometry();
                    position_at_taskbar();
                    render_layered();
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
        WM_LBUTTONDOWN => {
            unsafe {
                let _ = SetCapture(hwnd);
            }
            let mut pt = POINT::default();
            let _ = unsafe { GetCursorPos(&mut pt) };
            let rect = native_interop::get_window_rect_safe(hwnd).unwrap_or_default();
            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                s.pending_drag = true;
                s.is_snapped = false;
                s.drag_start_cursor = pt;
                s.drag_start_origin = POINT {
                    x: rect.left,
                    y: rect.top,
                };
                s.drag_start_mouse_x = pt.x;
                s.drag_start_client_x = pt.x - rect.left;
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let mut pt = POINT::default();
            let _ = unsafe { GetCursorPos(&mut pt) };

            let (should_start_drag, is_dragging) = {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    if s.pending_drag && !s.dragging {
                        let cx_drag = unsafe { GetSystemMetrics(SM_CXDRAG) };
                        let cy_drag = unsafe { GetSystemMetrics(SM_CYDRAG) };
                        let dx = (pt.x - s.drag_start_cursor.x).abs();
                        let dy = (pt.y - s.drag_start_cursor.y).abs();
                        if dx >= cx_drag || dy >= cy_drag {
                            (true, false)
                        } else {
                            (false, false)
                        }
                    } else {
                        (false, s.dragging)
                    }
                } else {
                    (false, false)
                }
            };

            if should_start_drag {
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.is_switching_window_style = true;
                    }
                }
                native_interop::make_popup(hwnd, true);
                unsafe {
                    let _ = SetCapture(hwnd);
                }
                {
                    let mut state = lock_state();
                    if let Some(s) = state.as_mut() {
                        s.is_switching_window_style = false;
                        s.dragging = true;
                        s.pending_drag = false;
                    }
                }
            }

            let is_now_dragging = should_start_drag || is_dragging;
            if is_now_dragging {
                let drag_info = {
                    let state = lock_state();
                    state.as_ref().map(|s| {
                        let widget_w = total_widget_width_for_state(s);
                        let widget_h = total_widget_height_for_state(s);
                        (
                            s.drag_start_client_x,
                            s.drag_start_cursor,
                            s.drag_start_origin,
                            widget_w,
                            widget_h,
                        )
                    })
                };

                if let Some((start_client_x, start_cursor, start_origin, widget_w, widget_h)) =
                    drag_info
                {
                    let origin_x = pt.x - start_client_x;
                    let origin_y = pt.y - (start_cursor.y - start_origin.y);
                    let virtual_rect = RECT {
                        left: origin_x,
                        top: origin_y,
                        right: origin_x + widget_w,
                        bottom: origin_y + widget_h,
                    };

                    let taskbars = native_interop::find_taskbars();
                    let target_taskbar = taskbars.into_iter().find(|tb| {
                        let tb_rect = tb.rect;
                        let extended = RECT {
                            left: tb_rect.left - 20,
                            top: tb_rect.top - 20,
                            right: tb_rect.right + 20,
                            bottom: tb_rect.bottom + 20,
                        };
                        pt.x >= extended.left
                            && pt.x <= extended.right
                            && pt.y >= extended.top
                            && pt.y <= extended.bottom
                    });

                    let mut snapped_pos = None;
                    let mut now_snapped = false;
                    if let Some(taskbar) = target_taskbar {
                        let free_dock_slot =
                            positioning::taskbar_free_dock_slot(taskbar.hwnd, taskbar.rect);
                        let capacity_ok = positioning::is_taskbar_capacity_sufficient(
                            taskbar.rect,
                            free_dock_slot,
                            widget_w,
                            widget_h,
                        );
                        if capacity_ok {
                            let was_snapped = {
                                let state = lock_state();
                                state.as_ref().map_or(false, |s| s.is_snapped)
                            };
                            let threshold = if was_snapped { 0.45 } else { 0.67 };
                            let overlap = positioning::calculate_rect_overlap_ratio(
                                virtual_rect,
                                free_dock_slot,
                            );
                            if overlap >= threshold {
                                now_snapped = true;
                                let is_horizontal =
                                    native_interop::is_taskbar_horizontal(taskbar.rect);
                                if is_horizontal {
                                    let snapped_x = origin_x
                                        .clamp(free_dock_slot.left, free_dock_slot.right - widget_w);
                                    let snapped_y = compute_anchor_y(
                                        taskbar.rect.top,
                                        taskbar.rect.bottom - taskbar.rect.top,
                                        widget_h,
                                    );
                                    snapped_pos = Some((snapped_x, snapped_y));
                                } else {
                                    let snapped_x = taskbar.rect.left;
                                    let snapped_y = origin_y
                                        .clamp(free_dock_slot.top, free_dock_slot.bottom - widget_h);
                                    snapped_pos = Some((snapped_x, snapped_y));
                                }
                            }
                        }
                    }

                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.is_snapped = now_snapped;
                        }
                    }

                    let (final_x, final_y) = snapped_pos.unwrap_or((origin_x, origin_y));
                    unsafe {
                        let _ = SetWindowPos(
                            hwnd,
                            Some(HWND_TOPMOST),
                            final_x,
                            final_y,
                            0,
                            0,
                            SWP_NOACTIVATE | SWP_NOSIZE,
                        );
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
            let (drag_ended, was_snapped) = {
                let mut state = lock_state();
                if let Some(s) = state.as_mut() {
                    let was_dragging = s.dragging;
                    let was_pending = s.pending_drag;
                    let was_snapped = s.is_snapped;
                    s.dragging = false;
                    s.pending_drag = false;
                    s.is_snapped = false;
                    ((was_dragging, was_pending), was_snapped)
                } else {
                    ((false, false), false)
                }
            };
            unsafe {
                let _ = ReleaseCapture();
            }
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
            let _ = unsafe { GetCursorPos(&mut pt) };

            if drag_ended.0 {
                let widget_rect = native_interop::get_window_rect_safe(hwnd).unwrap_or_default();
                let widget_w = (widget_rect.right - widget_rect.left).max(1);
                let widget_h = (widget_rect.bottom - widget_rect.top).max(1);

                let taskbars = native_interop::find_taskbars();
                let target_dock = taskbars.iter().enumerate().find_map(|(idx, tb)| {
                    let free_dock_slot = positioning::taskbar_free_dock_slot(tb.hwnd, tb.rect);
                    let capacity_ok = positioning::is_taskbar_capacity_sufficient(
                        tb.rect,
                        free_dock_slot,
                        widget_w,
                        widget_h,
                    );
                    if !capacity_ok {
                        return None;
                    }
                    let overlap =
                        positioning::calculate_rect_overlap_ratio(widget_rect, free_dock_slot);
                    let threshold = if was_snapped { 0.45 } else { 0.67 };
                    if overlap >= threshold {
                        Some((idx, tb, free_dock_slot))
                    } else {
                        None
                    }
                });

                if let Some((target_idx, taskbar, free_dock_slot)) = target_dock {
                    let tray_offset = (free_dock_slot.right - widget_rect.right).max(0);
                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.embedded = true;
                            s.is_snapped = false;
                            s.taskbar_hwnd = Some(SendHwnd::from_hwnd(taskbar.hwnd));
                            s.taskbar_index = target_idx;
                            s.auto_ejected = false;
                            s.auto_ejected_origin = None;
                            s.tray_offset = tray_offset;
                            s.placement_override = Some(PlacementOverride {
                                nest: "taskbar".into(),
                                monitor_index: target_idx,
                                screen_x: 0,
                                screen_y: 0,
                                tray_offset,
                            });
                        }
                    }
                    save_state_settings();
                    native_interop::embed_as_child(hwnd, taskbar.hwnd);
                    position_at_taskbar();
                    render_layered();
                } else {
                    let displays = native_interop::find_monitors();
                    let (monitor_idx, display) = displays
                        .iter()
                        .enumerate()
                        .find(|(_, d)| {
                            pt.x >= d.rect.left
                                && pt.x < d.rect.right
                                && pt.y >= d.rect.top
                                && pt.y < d.rect.bottom
                        })
                        .map(|(i, d)| (i, *d))
                        .unwrap_or_else(|| {
                            (
                                0,
                                displays.first().copied().unwrap_or(native_interop::DisplayMonitor {
                                    handle: HMONITOR::default(),
                                    rect: RECT {
                                        left: 0,
                                        top: 0,
                                        right: 1920,
                                        bottom: 1080,
                                    },
                                    primary: true,
                                }),
                            )
                        });

                    let clamped_x = widget_rect.left.clamp(
                        display.rect.left,
                        (display.rect.right - widget_w).max(display.rect.left),
                    );
                    let clamped_y = widget_rect.top.clamp(
                        display.rect.top,
                        (display.rect.bottom - widget_h).max(display.rect.top),
                    );

                    {
                        let mut state = lock_state();
                        if let Some(s) = state.as_mut() {
                            s.embedded = false;
                            s.is_snapped = false;
                            s.auto_ejected = false;
                            s.auto_ejected_origin = None;
                            s.placement_override = Some(PlacementOverride {
                                nest: "floating".into(),
                                monitor_index: monitor_idx,
                                screen_x: clamped_x,
                                screen_y: clamped_y,
                                tray_offset: 0,
                            });
                        }
                    }
                    save_state_settings();
                    unsafe {
                        let _ = SetWindowPos(
                            hwnd,
                            Some(HWND_TOPMOST),
                            clamped_x,
                            clamped_y,
                            widget_w,
                            widget_h,
                            SWP_NOACTIVATE,
                        );
                    }
                    render_layered();
                }
            } else if drag_ended.1 {
                if let Some((surface, object)) = mouse_target_at(hwnd, lparam) {
                    schedule_or_dispatch_click(hwnd, surface, object);
                }
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            let mut state = lock_state();
            if let Some(s) = state.as_mut() {
                if !s.is_switching_window_style {
                    s.dragging = false;
                    s.pending_drag = false;
                    s.is_snapped = false;
                }
            }
            LRESULT(0)
        }
        WM_APP_TASKBAR_COLLISION => {
            let action = wparam.0;
            let mut state = lock_state();
            let Some(s) = state.as_mut() else {
                return LRESULT(0);
            };
            if s.dragging {
                return LRESULT(0);
            }

            if action == 1 && !s.auto_ejected {
                s.auto_ejected = true;
                let widget_rect = native_interop::get_window_rect_safe(hwnd).unwrap_or_default();
                let widget_w = (widget_rect.right - widget_rect.left).max(1);
                let widget_h = (widget_rect.bottom - widget_rect.top).max(1);

                let taskbar_rect = s
                    .taskbar_hwnd
                    .and_then(|h| native_interop::get_window_rect_safe(h.to_hwnd()))
                    .unwrap_or_default();
                let is_horizontal = native_interop::is_taskbar_horizontal(taskbar_rect);

                let displays = native_interop::find_monitors();
                let mon = displays
                    .iter()
                    .find(|d| {
                        taskbar_rect.left >= d.rect.left
                            && taskbar_rect.right <= d.rect.right
                            && taskbar_rect.top >= d.rect.top
                            && taskbar_rect.bottom <= d.rect.bottom
                    })
                    .or_else(|| displays.first());
                let mon_rect = mon.map(|m| m.rect).unwrap_or(RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                });

                let pt = if is_horizontal {
                    let x = widget_rect.left;
                    let is_top = (taskbar_rect.top - mon_rect.top).abs() <= 50;
                    let y = if is_top {
                        taskbar_rect.bottom + 6
                    } else {
                        taskbar_rect.top - widget_h - 6
                    };
                    POINT { x, y }
                } else {
                    let y = widget_rect.top;
                    let is_left = (taskbar_rect.left - mon_rect.left).abs() <= 50;
                    let x = if is_left {
                        taskbar_rect.right + 6
                    } else {
                        taskbar_rect.left - widget_w - 6
                    };
                    POINT { x, y }
                };

                s.auto_ejected_origin = Some(pt);
                s.is_switching_window_style = true;
                native_interop::make_popup(hwnd, true);
                s.is_switching_window_style = false;

                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        pt.x,
                        pt.y,
                        widget_w,
                        widget_h,
                        SWP_NOACTIVATE,
                    );
                }
                diagnose::log("taskbar collision: auto-ejected widget to floating");
                drop(state);
                render_layered();
            } else if action == 0 && s.auto_ejected {
                s.auto_ejected = false;
                s.auto_ejected_origin = None;
                let taskbar_hwnd = s.taskbar_hwnd.map(|h| h.to_hwnd());
                s.is_switching_window_style = true;
                if let Some(tb) = taskbar_hwnd {
                    native_interop::embed_as_child(hwnd, tb);
                }
                s.is_switching_window_style = false;
                diagnose::log("taskbar collision resolved: re-docked widget to taskbar");
                drop(state);
                position_at_taskbar();
                render_layered();
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
            // Explorer can deliver this synchronously, including while a shell
            // call has re-entered our window procedure. Return before taking
            // STATE, opening windows, or calling back into Explorer.
            if let Err(error) = PostMessageW(
                Some(hwnd),
                native_interop::WM_APP_TRAY_DISPATCH,
                wparam,
                lparam,
            ) {
                diagnose::log_error("unable to queue tray callback", error);
            }
            LRESULT(0)
        }
        _ if msg == native_interop::WM_APP_TRAY_DISPATCH => {
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
            refresh_theme_host_geometry();
            // Explorer discards notification icons when it restarts. Floating
            // and tray-icon-only themes keep their owner HWND, so restore the
            // registrations when the shell broadcasts its return.
            sync_tray_icon(hwnd);
            render_layered();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_callbacks_return_while_state_is_locked_and_preserve_events() {
        // Model a shell call re-entering wnd_proc while the monitor owns STATE.
        // Keep the lock on this thread so a regression fails with a timeout
        // instead of permanently deadlocking the test process.
        let state = lock_state();
        let (completed, completion) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || unsafe {
            let class = native_interop::wide_str("STATIC");
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR::from_raw(class.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .expect("create isolated message-only test window");

            // Start with a themed hover: the old handler tries to acquire STATE
            // here. No dashboard or menu should ever be opened by this test.
            let events = [
                (1_000, WM_MOUSEMOVE),
                (1_042, WM_LBUTTONUP),
                (1_042, WM_LBUTTONDBLCLK),
                (1_042, WM_RBUTTONUP),
                (1, WM_LBUTTONUP),
                (1, WM_LBUTTONDBLCLK),
                (1, WM_RBUTTONUP),
                (1, WM_CONTEXTMENU),
            ];
            for (id, event) in events {
                assert_eq!(
                    wnd_proc(hwnd, WM_APP_TRAY, WPARAM(id), LPARAM(event as isize)).0,
                    0
                );
                let mut queued = MSG::default();
                let found = PeekMessageW(
                    &mut queued,
                    Some(hwnd),
                    native_interop::WM_APP_TRAY_DISPATCH,
                    native_interop::WM_APP_TRAY_DISPATCH,
                    PM_REMOVE,
                )
                .as_bool();
                if !found {
                    let _ = DestroyWindow(hwnd);
                    panic!("tray callback was processed inline instead of queued");
                }
                assert_eq!(queued.hwnd, hwnd);
                assert_eq!(queued.wParam.0, id);
                assert_eq!(queued.lParam.0, event as isize);
            }
            let _ = DestroyWindow(hwnd);
            completed.send(()).unwrap();
        });

        let result = completion.recv_timeout(Duration::from_secs(5));
        drop(state);
        worker.join().expect("tray callback test thread");
        result.expect("tray callbacks must return without waiting for STATE");
    }
}
