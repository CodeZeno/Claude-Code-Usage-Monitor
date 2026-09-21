use super::*;

fn state_for(theme: ThemeDocument, placement: PlacementOverride) -> AppState {
    AppState {
        hwnd: SendHwnd::from_hwnd(HWND::default()),
        taskbar_hwnd: None,
        tray_notify_hwnd: None,
        win_event_hook: None,
        is_dark: false,
        embedded: false,
        language_override: None,
        language: LanguageId::English,
        install_channel: InstallChannel::Portable,
        providers: ProviderSet::default(),
        accounts: Default::default(),
        data: None,
        poll_interval_ms: POLL_15_MIN,
        retry_count: 0,
        force_notify_auth_error: false,
        auth_error_paused_polling: false,
        auth_watch_mode: poller::CredentialWatchMode::ActiveSource(ProviderId::Claude),
        auth_watch_snapshot: Vec::new(),
        last_poll_ok: false,
        last_poll_failure: None,
        update_status: UpdateStatus::Idle,
        last_update_check_unix: None,
        taskbar_index: 0,
        tray_offset: placement.tray_offset,
        dragging: false,
        pending_drag: false,
        drag_start_cursor: POINT::default(),
        drag_start_origin: POINT::default(),
        drag_start_client_x: 0,
        auto_ejected: false,
        auto_ejected_origin: None,
        auto_ejected_host: None,
        is_switching_window_style: false,
        is_snapped: false,
        placement_override: Some(placement),
        floating_card_opacity: None,
        window_state_timer_active: false,
        custom_theme_enabled: true,
        usage_countdown: false,
        active_theme_path: None,
        active_theme: Some(theme),
        theme_clock_interval: None,
        tray_theme_uses_current_time: false,
        mirror_hwnds: Vec::new(),
        desktop_hwnds: Vec::new(),
        mouse_action_overrides: HashMap::new(),
        hovered_mouse_layer: None,
        pending_mouse_click: None,
        suppress_next_left_up: false,
    }
}

fn placement(nest: &str) -> PlacementOverride {
    PlacementOverride {
        nest: nest.into(),
        monitor_index: 0,
        screen_x: 0,
        screen_y: 0,
        tray_offset: 0,
        floating_host: None,
    }
}

#[test]
fn floating_layout_survives_restart_repeated_drags_and_auto_ejection() {
    let mut theme = ThemeDocument::starter();
    theme.surfaces[0].height = theme_engine::Expression("host.height".into());
    let host = app_settings::FloatingHost {
        theme_id: theme.id.clone(),
        surface_id: theme.surfaces[0].id.clone(),
        width: 1920,
        height: 46,
    };
    let mut saved = placement("floating");
    saved.floating_host = Some(host.clone());
    let saved = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
    let mut state = state_for(theme, saved);
    for _ in 0..3 {
        let effective = effective_theme_from_state(&state).unwrap();
        let runtime = theme_runtime_for_surface(&effective, 0, theme_runtime_from_state(&state));
        assert_eq!(runtime.host_dimensions(), (1920, 46));
        assert_eq!(
            widget_frame_for_state(&state, None).height,
            scaled_theme_dimension(46, theme_surface_scale(&effective, 0))
        );
        let recaptured = floating_host_for_state(&state);
        assert_eq!(recaptured, Some(host.clone()));
        state.placement_override.as_mut().unwrap().floating_host = recaptured;
    }
    state.placement_override = Some(placement("taskbar"));
    state.auto_ejected_host = Some(host);
    state.auto_ejected = true;
    state.auto_ejected_origin = Some(POINT { x: 100, y: 100 });
    let effective = effective_theme_from_state(&state).unwrap();
    assert_eq!(
        effective.surfaces[0].placement.host_dimensions,
        Some((1920, 46))
    );
    assert!(theme_with_placement(&state, false).unwrap().surfaces[0]
        .placement
        .host_dimensions
        .is_none());
}

#[test]
fn taskbar_drop_maps_the_monitor_handle_instead_of_the_taskbar_index() {
    let displays = [
        native_interop::DisplayMonitor {
            handle: HMONITOR(std::ptr::dangling_mut()),
            primary: true,
            rect: RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
        },
        native_interop::DisplayMonitor {
            handle: HMONITOR(2usize as *mut _),
            primary: false,
            rect: RECT {
                left: -1920,
                top: 0,
                right: 0,
                bottom: 1080,
            },
        },
    ];
    let mut taskbars = [displays[0], displays[1]];
    taskbars.sort_by_key(|d| (d.rect.top, d.rect.left));
    assert_eq!(taskbars[0].handle, displays[1].handle);
    let index = positioning::monitor_index_for_handle(&displays, taskbars[0].handle).unwrap();
    assert_eq!(index, 1);
    assert_eq!(
        positioning::dock_placement(index, 0, 1.0, true)
            .reference
            .display,
        1
    );
    assert_eq!(
        positioning::monitor_index_for_handle(&displays, HMONITOR::default()),
        None
    );
}

#[test]
fn dock_override_replaces_authored_anchors_and_expressions_without_editing_the_theme() {
    let mut theme = ThemeDocument::starter();
    let p = &mut theme.surfaces[0].placement;
    p.nest = SurfaceNest::Floating;
    p.reference.region = ReferenceRegion::Monitor;
    p.horizontal = HorizontalAnchor::Right;
    p.vertical = VerticalAnchor::Top;
    p.offset_y = 100;
    p.offset_x_expression = Some(theme_engine::Expression("123".into()));
    p.offset_y_expression = Some(theme_engine::Expression("456".into()));
    theme.prepare_runtime();
    let authored = theme.clone();
    let state = state_for(
        theme,
        PlacementOverride {
            tray_offset: 100,
            ..placement("taskbar")
        },
    );
    let effective = effective_theme_from_state(&state).unwrap();
    let p = &effective.surfaces[0].placement;
    assert_eq!(p.reference.region, ReferenceRegion::SystemTray);
    assert_eq!(p.nest, SurfaceNest::Taskbar);
    assert_eq!(p.horizontal, HorizontalAnchor::Left);
    assert_eq!(p.surface_horizontal, Some(HorizontalAnchor::Right));
    assert_eq!(p.vertical, VerticalAnchor::Bottom);
    assert_eq!(p.offset_y, 0);
    assert!(p.offset_x_expression.is_none() && p.offset_y_expression.is_none());
    let resolved =
        theme_engine::resolve_surface_placement(&effective, 0, None, ThemeRuntime::default());
    assert_eq!((resolved.offset_x, resolved.offset_y), (p.offset_x, 0));
    assert_eq!(effective.placement, *p);
    assert_eq!(
        state.active_theme.as_ref().unwrap().surfaces[0].placement,
        authored.surfaces[0].placement
    );
    assert_eq!(
        effective.surfaces[1].placement,
        authored.surfaces[1].placement
    );
}

#[test]
fn floating_and_ejected_positions_clear_offset_expressions() {
    let mut theme = ThemeDocument::starter();
    theme.surfaces[0].placement.offset_x_expression =
        Some(theme_engine::Expression("99999".into()));
    theme.surfaces[0].placement.offset_y_expression =
        Some(theme_engine::Expression("99999".into()));
    let mut state = state_for(theme, placement("floating"));
    for ejected in [false, true] {
        state.auto_ejected = ejected;
        state.auto_ejected_origin = Some(POINT { x: 100, y: 100 });
        let effective = effective_theme_from_state(&state).unwrap();
        let p = &effective.surfaces[0].placement;
        assert_eq!(p.nest, SurfaceNest::Floating);
        assert!(p.offset_x_expression.is_none() && p.offset_y_expression.is_none());
        let resolved =
            theme_engine::resolve_surface_placement(&effective, 0, None, ThemeRuntime::default());
        assert_eq!(
            (resolved.offset_x, resolved.offset_y),
            (p.offset_x, p.offset_y)
        );
    }
}

#[test]
fn disconnected_monitor_positions_are_clamped_with_the_current_dpi_and_frame() {
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let monitor = RECT {
            left: -1920,
            top: -100,
            right: 0,
            bottom: 980,
        };
        let theme = ThemeDocument::starter();
        let frame = positioning::widget_frame(
            &theme,
            None,
            ThemeRuntime::default().with_nest(SurfaceNest::Floating),
            scale,
        );
        for point in [POINT { x: 3000, y: 2000 }, POINT { x: -4000, y: -3000 }] {
            let offset = positioning::clamped_floating_offset(point, monitor, &frame, scale);
            let x = monitor.left + (offset.x as f64 * scale).round() as i32;
            let y = monitor.top + (offset.y as f64 * scale).round() as i32;
            assert!(x >= monitor.left && x + frame.width <= monitor.right);
            assert!(y >= monitor.top && y + frame.height <= monitor.bottom);
        }
        let tiny = RECT {
            left: 0,
            top: 0,
            right: 20,
            bottom: 20,
        };
        let point =
            positioning::clamped_floating_offset(POINT { x: 100, y: 100 }, tiny, &frame, scale);
        assert_eq!((point.x, point.y), (0, 0));
    }
    let state = state_for(
        ThemeDocument::starter(),
        PlacementOverride {
            monitor_index: usize::MAX,
            screen_x: 99999,
            screen_y: 99999,
            ..placement("floating")
        },
    );
    let effective = effective_theme_from_state(&state).unwrap();
    let displays = native_interop::find_monitors();
    let display = displays[effective.placement.reference.display];
    let runtime = theme_runtime_for_surface(&effective, 0, theme_runtime_from_state(&state));
    let scale = monitor_scale(display);
    let frame = positioning::widget_frame(&effective, None, runtime, scale);
    let rect = positioning::surface_screen_rect(
        &effective.placement,
        frame.width,
        frame.height,
        scale,
        display.rect,
        None,
        None,
    );
    assert!(rect.left >= display.rect.left && rect.right <= display.rect.right);
    assert!(rect.top >= display.rect.top && rect.bottom <= display.rect.bottom);
}

#[test]
fn redocking_waits_until_the_saved_position_has_room_and_hysteresis() {
    let monitor = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    let taskbar = RECT {
        top: 1032,
        ..monitor
    };
    let tray = RECT {
        left: 1600,
        ..taskbar
    };
    let occupancy = collision_fixture(taskbar, Some(tray), Some(1100));
    let docked = positioning::dock_placement(0, 300, 1.0, true);
    let target =
        positioning::surface_screen_rect(&docked, 217, 46, 1.0, monitor, Some(taskbar), Some(tray));
    assert_eq!(target.left, 1083);
    assert!(occupancy.overlaps(target));
    assert!(!occupancy.can_restore(target, 0));
    assert!(!occupancy.can_restore(target, 20));
    for (app_end, can_return) in [(1083, false), (1064, false), (1063, true)] {
        let occupancy = collision_fixture(taskbar, Some(tray), Some(app_end));
        assert_eq!(occupancy.can_restore(target, 20), can_return);
    }
}

#[test]
fn visibility_timer_follows_drag_and_auto_ejection_instead_of_the_authored_theme() {
    let mut state = state_for(ThemeDocument::starter(), placement("taskbar"));
    assert!(!window_state_timer_required(&state));
    state.placement_override = Some(placement("floating"));
    assert!(window_state_timer_required(&state));
    state.placement_override = Some(placement("taskbar"));
    state.auto_ejected = true;
    state.auto_ejected_origin = Some(POINT { x: 100, y: 100 });
    assert!(window_state_timer_required(&state));
    state.auto_ejected = false;
    assert!(!window_state_timer_required(&state));
    state.active_theme.as_mut().unwrap().surfaces[1]
        .placement
        .nest = SurfaceNest::Floating;
    assert!(
        window_state_timer_required(&state),
        "a floating mirror still needs the timer"
    );
}

#[test]
fn mirror_registration_preserves_the_primary_host_and_embedding_state() {
    let mut state = state_for(ThemeDocument::starter(), placement("taskbar"));
    let primary = HWND(std::ptr::dangling_mut());
    let mirror = HWND(2usize as *mut _);
    let old_taskbar = HWND(3usize as *mut _);
    let new_taskbar = HWND(4usize as *mut _);
    state.hwnd = SendHwnd::from_hwnd(primary);
    state.taskbar_hwnd = Some(SendHwnd::from_hwnd(old_taskbar));
    state.tray_notify_hwnd = Some(SendHwnd::from_hwnd(old_taskbar));
    state.embedded = false;
    assert!(!positioning::record_primary_taskbar(
        &mut state,
        mirror,
        new_taskbar,
        None
    ));
    assert_eq!(state.taskbar_hwnd.unwrap().to_hwnd(), old_taskbar);
    assert_eq!(state.tray_notify_hwnd.unwrap().to_hwnd(), old_taskbar);
    assert!(!state.embedded);
    assert!(positioning::record_primary_taskbar(
        &mut state,
        primary,
        new_taskbar,
        None
    ));
    assert_eq!(state.taskbar_hwnd.unwrap().to_hwnd(), new_taskbar);
    assert!(state.embedded);
    assert!(state.tray_notify_hwnd.is_none());
}

#[test]
fn vertical_docking_uses_tray_top_and_a_vertical_saved_offset() {
    for left in [0, 1872, -1920] {
        let monitor = RECT {
            left: left.min(0),
            top: 0,
            right: left.max(0) + 1920,
            bottom: 1080,
        };
        let taskbar = RECT {
            left,
            top: 0,
            right: left + 48,
            bottom: 1080,
        };
        let tray = RECT {
            top: 900,
            ..taskbar
        };
        assert_eq!(
            collision_fixture(taskbar, None, None).free_slots(taskbar)[0].bottom,
            1080
        );
        let occupancy = collision_fixture(taskbar, Some(tray), Some(500));
        let slot = occupancy.free_slots(taskbar)[0];
        assert_eq!((slot.top, slot.bottom), (500, 900));
        assert!(positioning::is_taskbar_capacity_sufficient(
            taskbar, slot, 24, 100
        ));
        let placement = positioning::dock_placement(0, 200, 1.0, false);
        let target = positioning::surface_screen_rect(
            &placement,
            24,
            100,
            1.0,
            monitor,
            Some(taskbar),
            Some(tray),
        );
        assert_eq!((target.left, target.top, target.bottom), (left, 600, 700));
        assert!(!occupancy.overlaps(target));
        assert!(occupancy.can_restore(target, 20));
        let crowded = collision_fixture(taskbar, Some(tray), Some(590));
        assert!(!crowded.can_restore(target, 20));
    }
}

#[test]
fn a_taskbar_without_a_tray_anchors_at_its_trailing_edge() {
    let monitor = RECT {
        left: -1920,
        top: 0,
        right: 0,
        bottom: 1080,
    };
    let taskbar = RECT {
        top: 1032,
        ..monitor
    };
    let placement = positioning::dock_placement(0, 10, 1.0, true);
    let rect =
        positioning::surface_screen_rect(&placement, 217, 46, 1.0, monitor, Some(taskbar), None);
    assert_eq!((rect.left, rect.right), (-227, -10));
    assert_eq!((rect.top, rect.bottom), (1034, 1080));
}

// Fixture for the traditional leading-apps/trailing-tray arrangement. The
// production detector also supports independent groups and leading-side gaps.
fn collision_fixture(
    bounds: RECT,
    tray: Option<RECT>,
    app_end: Option<i32>,
) -> taskbar_collision::Occupancy {
    let mut occupied = Vec::new();
    if let Some(end) = app_end {
        let mut apps = bounds;
        if native_interop::is_taskbar_horizontal(bounds) {
            apps.right = end;
        } else {
            apps.bottom = end;
        }
        occupied.push(apps);
    }
    occupied.extend(tray);
    taskbar_collision::Occupancy {
        bounds,
        occupied,
        reserved: tray.into_iter().collect(),
    }
}
