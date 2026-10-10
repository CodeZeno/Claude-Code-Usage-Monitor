use super::low_usage_alarm::mono_for;
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
        last_update_unix: None,
        last_fresh_unix: None,
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
        auto_ejected_for_capacity: false,
        auto_ejected_origin: None,
        auto_ejected_host: None,
        lock_taskbar: false,
        is_switching_window_style: false,
        is_snapped: false,
        placement_override: Some(placement),
        floating_card_opacity: None,
        hide_until_hover: None,
        edge_display: None,
        low_usage_alarm: true,
        alarm: Default::default(),
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
fn theme_selection_releases_the_previous_roots_temporary_fallback() {
    for nest in [
        SurfaceNest::Desktop,
        SurfaceNest::TrayIcon,
        SurfaceNest::Floating,
        SurfaceNest::Taskbar,
    ] {
        let mut state = ejected_state();
        let mut next = ThemeDocument::starter();
        next.id = "new-theme".into();
        next.surfaces[0].placement.nest = nest;
        next.surfaces[0].placement.offset_x = 400;
        next.surfaces[0].placement.offset_y = 300;
        next.prepare_runtime();
        let authored = next.surfaces[0].placement.clone();
        replace_active_theme(&mut state, next, Some("new-theme.json".into()));
        assert!(!state.auto_ejected);
        assert!(!state.auto_ejected_for_capacity);
        assert!(state.auto_ejected_origin.is_none());
        assert!(state.auto_ejected_host.is_none());
        assert_eq!(
            effective_theme_from_state(&state).unwrap().surfaces[0].placement,
            authored
        );
    }
}

#[test]
fn carried_forward_readings_do_not_advance_the_fresh_time() {
    let mut state = state_for(ThemeDocument::starter(), placement("taskbar"));
    let reading = |stale| {
        AppUsageData::from_iter([(
            ProviderId::Claude,
            crate::models::UsageData {
                stale,
                ..Default::default()
            },
        )])
    };
    state.note_fresh(&reading(true));
    assert_eq!(state.last_fresh_unix, None);
    state.note_fresh(&reading(false));
    assert!(state.last_fresh_unix.is_some());
}

#[test]
fn failed_accounts_merged_with_old_readings_do_not_advance_the_fresh_time() {
    let profile = |id: &str| crate::accounts::AccountProfile {
        id: id.into(),
        name: id.into(),
        enabled: true,
        ..Default::default()
    };
    let account = |id: &str, ok: bool| crate::models::AccountUsage {
        provider: ProviderId::Claude,
        profile: profile(id),
        source_signature: "sig".into(),
        source_path: None,
        usage: ok.then(crate::models::UsageData::default),
        error: (!ok).then_some(poller::PollError::NetworkError),
        selected: false,
    };
    let update = |a: bool, b: bool| {
        let mut data = AppUsageData::default();
        data.accounts.push(account("a", a));
        data.accounts.push(account("b", b));
        data
    };
    let mut settings = crate::accounts::AccountSettings::default();
    settings.claude.profiles = vec![profile("a"), profile("b")];
    settings.claude.selected = "a".into();
    let mut state = state_for(ThemeDocument::starter(), placement("taskbar"));
    state.apply_progress(update(true, true), &settings);
    let first = state.last_fresh_unix.expect("a successful poll stamps");
    state.last_fresh_unix = Some(first - 600);
    // Both fail, delivered one account at a time: old readings are carried
    // forward (stale) but the fresh time must not move.
    let mut only_a = AppUsageData::default();
    only_a.accounts.push(account("a", false));
    let merged = state.apply_progress(only_a, &settings);
    assert_eq!(state.last_fresh_unix, Some(first - 600));
    let mut only_b = AppUsageData::default();
    only_b.accounts.push(account("b", false));
    state.apply_progress(only_b, &settings);
    assert_eq!(state.last_fresh_unix, Some(first - 600));
    // The merge still holds another account's old non-stale reading.
    assert!(merged.has_fresh_reading());
    // One success moves it.
    let mut a_ok = AppUsageData::default();
    a_ok.accounts.push(account("a", true));
    state.apply_progress(a_ok, &settings);
    assert!(state.last_fresh_unix.unwrap() > first - 600);
}

fn ejected_state() -> AppState {
    let mut state = state_for(ThemeDocument::starter(), placement("taskbar"));
    state.placement_override = None;
    state.active_theme_path = Some("current.json".into());
    state.auto_ejected = true;
    state.auto_ejected_for_capacity = true;
    state.auto_ejected_origin = Some(POINT { x: 100, y: 100 });
    state.auto_ejected_host = Some(app_settings::FloatingHost {
        theme_id: state.active_theme.as_ref().unwrap().id.clone(),
        surface_id: "main".into(),
        width: 62,
        height: 1080,
    });
    state
}

#[test]
fn same_theme_refresh_preserves_fallback_until_the_configured_root_changes() {
    let mut state = ejected_state();
    let current = state.active_theme.clone().unwrap();
    replace_active_theme(&mut state, current.clone(), Some("current.json".into()));
    update_configured_placement(&mut state, None, 0, 0);
    assert!(state.auto_ejected && state.auto_ejected_for_capacity);
    assert_eq!(state.auto_ejected_origin.unwrap().x, 100);
    assert!(state.auto_ejected_host.is_some());
    // Editing an existing file must take effect even with the same theme id.
    let mut edited = current.clone();
    edited.surfaces[0].placement.nest = SurfaceNest::Desktop;
    replace_active_theme(&mut state, edited, Some("current.json".into()));
    assert!(!state.auto_ejected);
    let mut state = ejected_state();
    let mut edited = current.clone();
    edited.surfaces[0].placement.offset_y += 1;
    replace_active_theme(&mut state, edited, None);
    assert!(!state.auto_ejected);
    let mut state = ejected_state();
    replace_active_theme(&mut state, current, Some("duplicate-id.json".into()));
    assert!(!state.auto_ejected);
}

#[test]
fn explicit_placement_changes_take_precedence_over_temporary_fallback() {
    for nest in ["taskbar", "floating"] {
        let mut state = ejected_state();
        let mut saved = placement(nest);
        saved.screen_x = 400;
        saved.screen_y = 300;
        update_configured_placement(&mut state, Some(saved.clone()), 0, 0);
        assert!(!state.auto_ejected);
        assert_eq!(state.placement_override, Some(saved.clone()));
        // Selecting another theme retains the user's explicit placement.
        let mut next = ThemeDocument::starter();
        next.id = "new-theme".into();
        replace_active_theme(&mut state, next, None);
        assert_eq!(state.placement_override, Some(saved));
    }
    for (index, offset) in [(1, 0), (0, 50)] {
        let mut state = ejected_state();
        update_configured_placement(&mut state, None, index, offset);
        assert!(!state.auto_ejected);
    }
}

#[test]
fn taskbar_lock_restores_auto_ejection_without_discarding_dock_placement() {
    let mut saved = placement("taskbar");
    saved.screen_x = 640;
    saved.tray_offset = 80;
    let mut state = state_for(ThemeDocument::starter(), saved.clone());
    state.auto_ejected = true;
    state.auto_ejected_origin = Some(POINT { x: 640, y: 900 });
    state.auto_ejected_host = Some(app_settings::FloatingHost {
        theme_id: "classic".into(),
        surface_id: "main".into(),
        width: 1920,
        height: 46,
    });
    assert!(!clear_locked_auto_ejection(&mut state));
    assert!(state.auto_ejected);
    assert!(state.auto_ejected_origin.is_some());
    assert!(state.auto_ejected_host.is_some());

    state.lock_taskbar = true;
    assert!(clear_locked_auto_ejection(&mut state));
    assert!(!state.auto_ejected);
    assert!(state.auto_ejected_origin.is_none());
    assert!(state.auto_ejected_host.is_none());
    assert_eq!(state.placement_override, Some(saved));
    assert!(!clear_locked_auto_ejection(&mut state));
}

#[test]
fn taskbar_lock_follows_configured_host_during_drag_and_auto_ejection() {
    for nest in [
        SurfaceNest::Taskbar,
        SurfaceNest::TrayIcon,
        SurfaceNest::Desktop,
        SurfaceNest::Floating,
    ] {
        let mut theme = ThemeDocument::starter();
        theme.surfaces[0].placement.nest = nest;
        let mut state = state_for(theme, placement("taskbar"));
        state.placement_override = None;
        for locked in [false, true] {
            state.lock_taskbar = locked;
            // A drag temporarily detaches the HWND; it must retain its host policy.
            state.dragging = true;
            state.embedded = false;
            assert_eq!(
                taskbar_lock_applies(&state),
                locked && nest == SurfaceNest::Taskbar
            );
            state.dragging = false;
            state.auto_ejected = true;
            assert_eq!(
                taskbar_lock_applies(&state),
                locked && nest == SurfaceNest::Taskbar
            );
        }
    }
}

#[test]
fn taskbar_lock_respects_saved_host_overrides_and_legacy_auto_hosts() {
    let mut theme = ThemeDocument::starter();
    theme.surfaces[0].placement.nest = SurfaceNest::Floating;
    let mut state = state_for(theme, placement("taskbar"));
    state.lock_taskbar = true;
    assert!(taskbar_lock_applies(&state));
    state.active_theme.as_mut().unwrap().surfaces[0]
        .placement
        .nest = SurfaceNest::Taskbar;
    state.placement_override = Some(placement("floating"));
    assert!(!taskbar_lock_applies(&state));
    state.placement_override = None;
    let surface = &mut state.active_theme.as_mut().unwrap().surfaces[0];
    surface.placement.nest = SurfaceNest::Auto;
    surface.placement.reference.region = ReferenceRegion::SystemTray;
    assert!(taskbar_lock_applies(&state));
    state.active_theme.as_mut().unwrap().surfaces[0]
        .placement
        .reference
        .region = ReferenceRegion::Monitor;
    assert!(!taskbar_lock_applies(&state));
}

#[test]
fn taskbar_lock_leaves_intentional_floating_placement_unchanged() {
    let saved = placement("floating");
    let mut state = state_for(ThemeDocument::starter(), saved.clone());
    state.lock_taskbar = true;
    assert!(!taskbar_lock_applies(&state));
    assert!(!clear_locked_auto_ejection(&mut state));
    assert_eq!(state.placement_override, Some(saved));
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

#[test]
fn tray_movement_does_not_cause_app_collision_when_docked() {
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
    // 500px of free space between apps and tray (apps end at 1100, tray starts at 1600)
    let occupancy = collision_fixture(taskbar, Some(tray), Some(1100));
    let docked = positioning::dock_placement(0, 0, 1.0, true);
    let target =
        positioning::surface_screen_rect(&docked, 200, 46, 1.0, monitor, Some(taskbar), Some(tray));
    assert_eq!((target.left, target.right), (1400, 1600));

    // When docked next to tray, overlaps_app_controls is false
    assert!(!occupancy.overlaps_app_controls(target));

    // When tray expands left by 50px (tray starts at 1550)
    let expanded_tray = RECT {
        left: 1550,
        ..taskbar
    };
    let occupancy_expanded = collision_fixture(taskbar, Some(expanded_tray), Some(1100));
    // Even before the widget repositions (target is still 1400..1600, overlapping expanded tray 1550..1600),
    // it does NOT collide with app controls!
    assert!(!occupancy_expanded.overlaps_app_controls(target));

    // Only when app buttons expand into the widget area (e.g. apps reach 1450)
    let crowded_occupancy = collision_fixture(taskbar, Some(expanded_tray), Some(1450));
    assert!(crowded_occupancy.overlaps_app_controls(target));
}

#[test]
fn smart_anchoring_remains_stationary_on_tray_change_and_clamps_when_pushed() {
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
    let initial_tray = RECT {
        left: 1600,
        ..taskbar
    };

    // User placed widget at fixed position x = 1200 on taskbar (width = 200)
    let taskbar_placement = positioning::taskbar_dock_placement(0, 1200, 1.0, true);

    // 1. Initial positioning: widget is at 1200..1400, tray is at 1600..1920
    let rect1 = positioning::surface_screen_rect(
        &taskbar_placement,
        200,
        46,
        1.0,
        monitor,
        Some(taskbar),
        Some(initial_tray),
    );
    assert_eq!((rect1.left, rect1.right), (1200, 1400));

    // 2. Tray shifts left or right without reaching the widget (e.g. tray is at 1450 or 1700)
    let shifted_tray = RECT {
        left: 1450,
        ..taskbar
    };
    let rect2 = positioning::surface_screen_rect(
        &taskbar_placement,
        200,
        46,
        1.0,
        monitor,
        Some(taskbar),
        Some(shifted_tray),
    );
    // Widget stays completely stationary at 1200!
    assert_eq!((rect2.left, rect2.right), (1200, 1400));

    // 3. Tray expands so much that it pushes the widget (tray at 1350, max_x = 1150)
    let encroaching_tray = RECT {
        left: 1350,
        ..taskbar
    };
    let rect3 = positioning::surface_screen_rect(
        &taskbar_placement,
        200,
        46,
        1.0,
        monitor,
        Some(taskbar),
        Some(encroaching_tray),
    );
    // Protective clamp shifts widget left to 1150
    assert_eq!((rect3.left, rect3.right), (1150, 1350));

    // 4. Tray shrinks back to 1600: widget automatically returns to its original 1200 position!
    let restored_rect = positioning::surface_screen_rect(
        &taskbar_placement,
        200,
        46,
        1.0,
        monitor,
        Some(taskbar),
        Some(initial_tray),
    );
    assert_eq!((restored_rect.left, restored_rect.right), (1200, 1400));
}

#[test]
fn smart_anchoring_tray_snapped_follows_tray_movement() {
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
    let initial_tray = RECT {
        left: 1600,
        ..taskbar
    };

    // When tray_offset == 0 (snapped to tray), dock_placement anchors to ReferenceRegion::SystemTray
    let tray_snapped = positioning::dock_placement(0, 0, 1.0, true);

    let rect1 = positioning::surface_screen_rect(
        &tray_snapped,
        200,
        46,
        1.0,
        monitor,
        Some(taskbar),
        Some(initial_tray),
    );
    assert_eq!((rect1.left, rect1.right), (1400, 1600));

    // When tray moves to 1550, snapped widget moves with it to 1350..1550
    let moved_tray = RECT {
        left: 1550,
        ..taskbar
    };
    let rect2 = positioning::surface_screen_rect(
        &tray_snapped,
        200,
        46,
        1.0,
        monitor,
        Some(taskbar),
        Some(moved_tray),
    );
    assert_eq!((rect2.left, rect2.right), (1350, 1550));
}

#[test]
fn smart_anchoring_vertical_taskbar_anchors_to_top_edge_and_clamps() {
    let monitor = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    let vertical_taskbar = RECT {
        left: 0,
        top: 0,
        right: 60,
        bottom: 1080,
    };
    let tray = RECT {
        left: 0,
        top: 900,
        right: 60,
        bottom: 1080,
    };

    // User docked widget at top offset y = 300 on a vertical taskbar
    let placement = positioning::taskbar_dock_placement(0, 300, 1.0, false);
    let rect = positioning::surface_screen_rect(
        &placement,
        50,
        100,
        1.0,
        monitor,
        Some(vertical_taskbar),
        Some(tray),
    );
    // Top edge must be precisely at 300, not 200 (shifted by height)
    assert_eq!(rect.top, 300);
    assert_eq!(rect.bottom, 400);

    // If tray expands upward to top = 350, clamp keeps widget from overlapping tray (max_y = 350 - 100 = 250)
    let expanded_tray = RECT {
        left: 0,
        top: 350,
        right: 60,
        bottom: 1080,
    };
    let clamped = positioning::surface_screen_rect(
        &placement,
        50,
        100,
        1.0,
        monitor,
        Some(vertical_taskbar),
        Some(expanded_tray),
    );
    assert_eq!(clamped.top, 250);
    assert_eq!(clamped.bottom, 350);
}

#[test]
fn smart_anchoring_clamp_does_not_panic_when_taskbar_is_crowded_or_tiny() {
    let monitor = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    let taskbar = RECT {
        left: 0,
        top: 1032,
        right: 1920,
        bottom: 1080,
    };
    // Severe crowding: tray is at 100, widget width is 200 -> max_x = -100, which is < taskbar.left (0)
    let crowded_tray = RECT {
        left: 100,
        top: 1032,
        right: 1920,
        bottom: 1080,
    };
    let placement = positioning::taskbar_dock_placement(0, 500, 1.0, true);
    let rect = positioning::surface_screen_rect(
        &placement,
        200,
        46,
        1.0,
        monitor,
        Some(taskbar),
        Some(crowded_tray),
    );
    // Must not panic with min > max, clamps safely to taskbar.left (0)
    assert_eq!(rect.left, 0);
}

#[test]
fn floating_taskbar_reference_is_not_clamped_to_tray() {
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
    let mut placement = positioning::taskbar_dock_placement(0, 1700, 1.0, true);
    placement.nest = SurfaceNest::Floating;
    let rect = positioning::surface_screen_rect(
        &placement,
        200,
        46,
        1.0,
        monitor,
        Some(taskbar),
        Some(tray),
    );
    assert_eq!(rect.left, 1700);
}

#[test]
fn authored_taskbar_placements_keep_their_position_near_the_tray() {
    let monitor = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    for horizontal in [true, false] {
        let taskbar = if horizontal {
            RECT {
                top: 1032,
                ..monitor
            }
        } else {
            RECT {
                right: 60,
                ..monitor
            }
        };
        let tray = if horizontal {
            RECT {
                left: 1600,
                ..taskbar
            }
        } else {
            RECT {
                top: 900,
                ..taskbar
            }
        };
        // Exercise both edge alignment and the same anchors used by drag docking.
        for edge_aligned in [true, false] {
            let placement: theme_engine::Placement = serde_json::from_value(serde_json::json!({
                "reference": { "region": "taskbar", "display": 0 },
                "nest": "taskbar",
                "horizontal": if horizontal && edge_aligned { "right" } else { "left" },
                "vertical": if horizontal || edge_aligned { "bottom" } else { "top" },
                "surface_horizontal": if horizontal && edge_aligned { "right" } else { "left" },
                "surface_vertical": if horizontal || edge_aligned { "bottom" } else { "top" },
                "offset_x": if horizontal && !edge_aligned { 1720 } else { 0 },
                "offset_y": if !horizontal && !edge_aligned { 980 } else { 0 }
            }))
            .unwrap();
            for tray in [Some(tray), None] {
                let rect = positioning::surface_screen_rect(
                    &placement,
                    if horizontal { 200 } else { 50 },
                    if horizontal { 46 } else { 100 },
                    1.0,
                    monitor,
                    Some(taskbar),
                    tray,
                );
                if horizontal {
                    assert_eq!((rect.left, rect.right), (1720, 1920));
                } else {
                    assert_eq!((rect.top, rect.bottom), (980, 1080));
                }
            }
        }
    }
}

#[test]
fn drag_taskbar_clamp_is_not_saved_in_themes() {
    let dragged = positioning::taskbar_dock_placement(0, 1700, 1.0, true);
    assert!(dragged.clamp_taskbar_drag);
    let json = serde_json::to_value(&dragged).unwrap();
    assert!(json.get("clamp_taskbar_drag").is_none());
    let loaded: theme_engine::Placement = serde_json::from_value(json).unwrap();
    assert!(!loaded.clamp_taskbar_drag);
}

#[test]
fn smart_anchoring_fractional_dpi_scaling() {
    let monitor = RECT {
        left: 0,
        top: 0,
        right: 2560,
        bottom: 1440,
    };
    let taskbar = RECT {
        left: 0,
        top: 1392,
        right: 2560,
        bottom: 1440,
    };
    let tray = RECT {
        left: 2100,
        ..taskbar
    };

    // 1.25x scaling, screen offset = 1000 physical px
    let placement = positioning::taskbar_dock_placement(0, 1000, 1.25, true);
    let rect = positioning::surface_screen_rect(
        &placement,
        250,
        46,
        1.25,
        monitor,
        Some(taskbar),
        Some(tray),
    );
    assert_eq!(rect.left, 1000);
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

#[test]
fn auto_hiding_roots_ignore_saved_drags_and_follow_display_settings() {
    let mut top_bar: ThemeDocument =
        serde_json::from_str(include_str!("../themes/top-bar.json")).unwrap();
    top_bar.prepare_runtime();
    let authored = top_bar.surfaces[0].placement.clone();
    // A drag saved under another theme must not pull the bar into the taskbar
    // or leave it floating mid-screen.
    for nest in ["taskbar", "floating"] {
        let mut saved = placement(nest);
        saved.screen_x = 400;
        saved.screen_y = 300;
        let mut state = state_for(top_bar.clone(), saved);
        assert_eq!(
            effective_theme_from_state(&state).unwrap().surfaces[0].placement,
            authored,
            "{nest}"
        );
        state.edge_display = Some(1);
        let moved = effective_theme_from_state(&state).unwrap();
        assert_eq!(moved.surfaces[0].placement.reference.display, 1);
        assert_eq!(moved.placement.reference.display, 1);
        assert_eq!(
            moved.surfaces[0].placement.auto_hide_edge(),
            authored.auto_hide_edge()
        );

        // Hiding turned off, it stays at its edge and still ignores the drag.
        state.hide_until_hover = Some(false);
        let pinned = effective_theme_from_state(&state).unwrap();
        assert!(!pinned.surfaces[0].placement.auto_hide);
        assert_eq!(pinned.surfaces[0].placement.edge(), authored.edge());
        assert_eq!(
            pinned.surfaces[0].placement.offset_x, authored.offset_x,
            "{nest}"
        );
        assert_eq!(
            pinned.surfaces[0].placement.offset_y, authored.offset_y,
            "{nest}"
        );
        assert_eq!(pinned.surfaces[0].placement.nest, authored.nest, "{nest}");
    }
    // Other themes keep their saved drag placement and ignore the settings.
    let mut state = state_for(ThemeDocument::starter(), placement("taskbar"));
    let before = effective_theme_from_state(&state).unwrap().surfaces[0]
        .placement
        .clone();
    state.hide_until_hover = Some(true);
    state.edge_display = Some(1);
    assert_eq!(
        effective_theme_from_state(&state).unwrap().surfaces[0].placement,
        before
    );
}

#[test]
fn an_offset_floating_root_that_never_hides_still_honours_a_saved_drag() {
    // The Top Bar's docking, authored without hiding and with an offset.
    let mut theme: ThemeDocument =
        serde_json::from_str(include_str!("../themes/top-bar.json")).unwrap();
    theme.surfaces[0].placement.offset_y = 20;
    theme.surfaces[0].placement.auto_hide = false;
    theme.prepare_runtime();
    let authored = theme.surfaces[0].placement.clone();
    assert!(authored.edge().is_some() && authored.auto_hide_edge().is_none());
    let mut saved = placement("floating");
    saved.screen_x = 400;
    saved.screen_y = 300;
    let state = state_for(theme, saved);
    assert_ne!(
        effective_theme_from_state(&state).unwrap().surfaces[0].placement,
        authored
    );
}

#[test]
fn the_low_usage_alarm_sounds_once_holds_the_top_bar_open_and_can_be_snoozed_or_turned_off() {
    use crate::models::{UsageData, UsageSection};
    let mut top_bar: ThemeDocument =
        serde_json::from_str(include_str!("../themes/top-bar.json")).unwrap();
    top_bar.prepare_runtime();
    let usage = |percentage| UsageData {
        session: UsageSection {
            available: true,
            percentage,
            resets_at: None,
        },
        ..Default::default()
    };
    let data = |readings: &[(ProviderId, f64)]| {
        Some(AppUsageData::from_iter(readings.iter().map(
            |(provider, percentage)| (*provider, usage(*percentage)),
        )))
    };
    // What the render path does: update the alarm, then ask the theme.
    let forced = |state: &AppState| {
        let theme = effective_theme_from_state(state).unwrap();
        theme_engine::surface_force_reveal(
            &theme,
            0,
            state.data.as_ref(),
            theme_runtime_from_state(state),
        )
    };
    let mut state = state_for(top_bar, placement("floating"));
    state.providers = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]);

    state.data = data(&[(ProviderId::Claude, 94.0)]);
    assert!(
        !update_low_usage_alarm(&mut state),
        "6% left is not an alarm"
    );
    assert!(!forced(&state));
    state.data = data(&[(ProviderId::Claude, 95.0)]);
    assert!(update_low_usage_alarm(&mut state), "5% left sounds");
    assert!(forced(&state));
    for _ in 0..3 {
        assert!(
            !update_low_usage_alarm(&mut state),
            "later polls stay quiet"
        );
        assert!(forced(&state));
    }

    // Snoozed, the bar may hide while that limit stays low.
    state.alarm.snooze(Instant::now());
    assert!(!update_low_usage_alarm(&mut state));
    assert!(!forced(&state));
    // Another limit crossing during the snooze alarms again.
    state.data = data(&[(ProviderId::Claude, 95.0), (ProviderId::Codex, 97.0)]);
    let later = std::time::SystemTime::now() + Duration::from_secs(120);
    assert!(update_low_usage_alarm_at(
        &mut state,
        later,
        mono_for(later)
    ));
    assert!(forced(&state));

    // Providers publishing one by one within a poll: the second crossing 3 s
    // after the first still holds the bar open but plays no second sound.
    state.data = data(&[(ProviderId::Claude, 10.0), (ProviderId::Codex, 10.0)]);
    update_low_usage_alarm_at(&mut state, later, mono_for(later));
    state.data = data(&[(ProviderId::Claude, 96.0), (ProviderId::Codex, 10.0)]);
    let t = later + Duration::from_secs(70);
    assert!(
        update_low_usage_alarm_at(&mut state, t, mono_for(t)),
        "first result"
    );
    state.data = data(&[(ProviderId::Claude, 96.0), (ProviderId::Codex, 97.0)]);
    let t3 = t + Duration::from_secs(3);
    assert!(!update_low_usage_alarm_at(&mut state, t3, mono_for(t3)));
    assert!(forced(&state));
    state.data = data(&[(ProviderId::Claude, 96.0), (ProviderId::Codex, 10.0)]);
    let t4 = t + Duration::from_secs(4);
    update_low_usage_alarm_at(&mut state, t4, mono_for(t4));
    state.data = data(&[(ProviderId::Claude, 96.0), (ProviderId::Codex, 97.0)]);
    let t120 = t + Duration::from_secs(120);
    assert!(update_low_usage_alarm_at(&mut state, t120, mono_for(t120)));
    state.data = data(&[(ProviderId::Claude, 95.0), (ProviderId::Codex, 97.0)]);

    // No data for a moment (a poll gap) is not a recovery: still held open,
    // and the same low readings returning stay silent.
    state.data = None;
    assert!(!update_low_usage_alarm(&mut state));
    assert!(forced(&state));
    state.data = data(&[(ProviderId::Claude, 95.0), (ProviderId::Codex, 97.0)]);
    assert!(!update_low_usage_alarm(&mut state));
    assert!(forced(&state));

    // After the resets it hides normally again.
    state.data = data(&[(ProviderId::Claude, 3.0), (ProviderId::Codex, 0.0)]);
    assert!(!update_low_usage_alarm(&mut state));
    assert!(!forced(&state));

    // Turned off: no sound and never held open.
    state.low_usage_alarm = false;
    state.data = data(&[(ProviderId::Claude, 99.0)]);
    assert!(!update_low_usage_alarm(&mut state));
    assert!(!forced(&state));
}

/// A usage cache from a report of a silent alarm: Codex weekly at 96% used
/// (4% left) with no five-hour window, Claude far from its limits. Times are
/// placeholders `@N@`, seconds after now, filled in by `sample_cache` so the
/// fixture never ages past the clock the alarm reads.
const SAMPLE_CACHE: &str = r#"{
  "updated_unix": @0@,
  "poll_ok": true,
  "data": {
    "claude_code": {
      "session": {"available": true, "percentage": 55.0, "resets_at": {"secs_since_epoch": @5400@, "nanos_since_epoch": 0}},
      "weekly": {"available": true, "percentage": 10.0, "resets_at": {"secs_since_epoch": @280800@, "nanos_since_epoch": 0}},
      "limits": [
        {"key": "iguana_necktie", "kind": "iguana_necktie", "label": "iguana necktie", "model": null, "model_id": null, "scope": null, "is_active": false, "usage": {"available": true, "percentage": 0.0, "resets_at": {"secs_since_epoch": @2257000@, "nanos_since_epoch": 0}}},
        {"key": "session", "kind": "session", "label": "session", "model": null, "model_id": null, "scope": null, "is_active": true, "usage": {"available": true, "percentage": 55.0, "resets_at": {"secs_since_epoch": @5400@, "nanos_since_epoch": 0}}},
        {"key": "weekly_all", "kind": "weekly_all", "label": "weekly all", "model": null, "model_id": null, "scope": null, "is_active": false, "usage": {"available": true, "percentage": 10.0, "resets_at": {"secs_since_epoch": @280800@, "nanos_since_epoch": 0}}},
        {"key": "weekly_scoped_fable", "kind": "weekly_scoped", "label": "Fable", "model": "Fable", "model_id": null, "scope": {"model": {"display_name": "Fable", "id": null}, "surface": null}, "is_active": false, "usage": {"available": true, "percentage": 0.0, "resets_at": {"secs_since_epoch": @280801@, "nanos_since_epoch": 0}}}
      ]
    },
    "codex": {
      "session": {"available": false, "percentage": 0.0, "resets_at": null},
      "weekly": {"available": true, "percentage": 96.0, "resets_at": {"secs_since_epoch": @343000@, "nanos_since_epoch": 0}}
    },
    "accounts": [
      {
        "provider": "claude",
        "profile": {"id": "default", "name": "Default", "config_dir": "", "credentials_path": "", "enabled": true},
        "source_signature": "",
        "usage": {
          "session": {"available": true, "percentage": 55.0, "resets_at": {"secs_since_epoch": @5400@, "nanos_since_epoch": 0}},
          "weekly": {"available": true, "percentage": 10.0, "resets_at": {"secs_since_epoch": @280800@, "nanos_since_epoch": 0}},
          "limits": [
            {"key": "iguana_necktie", "kind": "iguana_necktie", "label": "iguana necktie", "model": null, "model_id": null, "scope": null, "is_active": false, "usage": {"available": true, "percentage": 0.0, "resets_at": {"secs_since_epoch": @2257000@, "nanos_since_epoch": 0}}},
            {"key": "session", "kind": "session", "label": "session", "model": null, "model_id": null, "scope": null, "is_active": true, "usage": {"available": true, "percentage": 55.0, "resets_at": {"secs_since_epoch": @5400@, "nanos_since_epoch": 0}}},
            {"key": "weekly_all", "kind": "weekly_all", "label": "weekly all", "model": null, "model_id": null, "scope": null, "is_active": false, "usage": {"available": true, "percentage": 10.0, "resets_at": {"secs_since_epoch": @280800@, "nanos_since_epoch": 0}}},
            {"key": "weekly_scoped_fable", "kind": "weekly_scoped", "label": "Fable", "model": "Fable", "model_id": null, "scope": {"model": {"display_name": "Fable", "id": null}, "surface": null}, "is_active": false, "usage": {"available": true, "percentage": 0.0, "resets_at": {"secs_since_epoch": @280801@, "nanos_since_epoch": 0}}}
          ]
        },
        "error": null,
        "selected": true
      },
      {
        "provider": "codex",
        "profile": {"id": "default", "name": "Default", "config_dir": "", "credentials_path": "", "enabled": true},
        "source_signature": "",
        "usage": {
          "session": {"available": false, "percentage": 0.0, "resets_at": null},
          "weekly": {"available": true, "percentage": 96.0, "resets_at": {"secs_since_epoch": @343000@, "nanos_since_epoch": 0}}
        },
        "error": null,
        "selected": true
      }
    ]
  }
}"#;

/// `SAMPLE_CACHE` with every time placed relative to `now`.
fn sample_cache(now: u64) -> String {
    let mut cache = SAMPLE_CACHE.to_string();
    for offset in [0, 5400, 280800, 280801, 2257000, 343000] {
        cache = cache.replace(&format!("@{offset}@"), &(now + offset).to_string());
    }
    cache
}

#[test]
fn the_sample_cache_has_no_absolute_times_left() {
    let cache = sample_cache(2_000_000_000);
    assert!(!cache.contains('@'), "every placeholder is filled");
    assert!(cache.contains("2000005400"), "times follow the given clock");
    assert!(
        !cache.contains("1791"),
        "no absolute epoch from a real cache"
    );
}

/// Settings for it: Claude and Codex on, counting what is used, no alarm setting
/// saved (so it is on), and a floating placement left by another theme.
const SAMPLE_SETTINGS: &str = r#"{
  "accounts": {
    "claude": {"profiles": [{"config_dir": "", "credentials_path": "", "enabled": true, "id": "default", "name": "Default"}], "selected": "default", "used_ids": ["default"]},
    "codex": {"profiles": [{"config_dir": "", "credentials_path": "", "enabled": true, "id": "default", "name": "Default"}], "selected": "default", "used_ids": ["default"]}
  },
  "custom_theme_enabled": true,
  "placement_override": {"floating_host": {"height": 48, "surface_id": "main", "theme_id": "compact-fluent-quad", "width": 1920}, "monitor_index": 1, "nest": "floating", "screen_x": 3529, "screen_y": 126, "tray_offset": 0},
  "poll_interval_ms": 60000,
  "show_antigravity": false,
  "show_claude_code": true,
  "show_codex": true,
  "show_cursor": false,
  "show_grok": false,
  "show_opencode": false,
  "usage_countdown": false
}"#;

/// Tests driving the app's global state and the auto-hide presenter run one at
/// a time.
static LIVE_APP: Mutex<()> = Mutex::new(());

unsafe extern "system" fn plain_window(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, message, wparam, lparam)
}

/// The app started with the sample settings and the Top Bar from the theme
/// library, before its first poll. Everything runs for real down to Windows:
/// the alarm is counted instead of played, and the window is a private one
/// that is only marked shown (see `auto_hide::show`).
struct LiveApp {
    window: HWND,
    _one_at_a_time: MutexGuard<'static, ()>,
}

impl LiveApp {
    fn start() -> Self {
        let guard = LIVE_APP.lock().unwrap_or_else(|error| error.into_inner());
        std::fs::write(app_settings::settings_path(), SAMPLE_SETTINGS).unwrap();
        let settings = load_settings();
        theme_engine::ensure_starter_theme().unwrap();
        let theme =
            theme_engine::load_theme(&theme_engine::themes_directory().join("top-bar.json"))
                .unwrap();
        let window = unsafe {
            let class = native_interop::wide_str("UsageMonitorAlarmCallSite");
            let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(plain_window),
                hInstance: instance.into(),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            });
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
            .unwrap()
        };
        let mut state = state_for(theme, settings.placement_override.clone().unwrap());
        state.hwnd = SendHwnd::from_hwnd(window);
        state.providers = settings.enabled_providers();
        state.accounts = settings.accounts.clone();
        state.low_usage_alarm = settings.low_usage_alarm;
        state.usage_countdown = settings.usage_countdown;
        state.is_dark = crate::theme::is_dark_mode();
        *lock_state() = Some(state);
        let app = Self {
            window,
            _one_at_a_time: guard,
        };
        render_layered();
        app
    }

    /// A poll delivers the sample reading, and the widget redraws as it does
    /// on WM_APP_USAGE_UPDATED.
    fn poll_result(&self) {
        let cache: app_settings::UsageCache =
            serde_json::from_str(&sample_cache(now_unix_secs())).unwrap();
        if let Some(state) = lock_state().as_mut() {
            state.data = Some(cache.data);
            state.last_poll_ok = true;
            state.last_update_unix = Some(now_unix_secs());
            state.last_fresh_unix = Some(now_unix_secs());
        }
        render_layered();
    }

    /// Let the hide-until-hover timer run, as the message loop would.
    fn run_timer(&self, duration: Duration) {
        let end = Instant::now() + duration;
        while Instant::now() < end {
            unsafe {
                wnd_proc(self.window, WM_TIMER, WPARAM(TIMER_AUTO_HIDE), LPARAM(0));
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    fn sounds() -> usize {
        low_usage_alarm::SOUNDED.with(std::cell::Cell::get)
    }

    /// The window's size on screen, and whether it lets clicks through.
    fn presented(&self) -> ((i32, i32), bool) {
        let rect = native_interop::get_window_rect_safe(self.window).unwrap();
        let ex_style = unsafe { GetWindowLongW(self.window, GWL_EXSTYLE) } as u32;
        (
            (rect.right - rect.left, rect.bottom - rect.top),
            ex_style & WS_EX_TRANSPARENT.0 != 0,
        )
    }

    /// The open bar's and the collapsed tab's sizes in device pixels.
    fn sizes(&self) -> ((i32, i32), (i32, i32)) {
        let state = lock_state();
        let state = state.as_ref().unwrap();
        let theme = effective_theme_from_state(state).unwrap();
        let scale = theme_surface_scale(&theme, 0);
        let runtime = theme_runtime_for_surface(&theme, 0, theme_runtime_from_state(state));
        let (width, height) =
            theme_engine::resolve_surface_size(&theme, 0, state.data.as_ref(), runtime);
        let handle = theme_engine::render_surface_handle(
            &theme,
            0,
            state.data.as_ref(),
            runtime,
            scale,
            theme_engine::SurfaceEdge::Top,
        );
        (
            (
                scaled_theme_dimension(width, scale),
                scaled_theme_dimension(height, scale),
            ),
            (handle.width as i32, handle.height as i32),
        )
    }

    /// Where a layer's centre is, in the window's device pixels.
    fn centre_of(&self, id: &str) -> Option<LPARAM> {
        let state = lock_state();
        let state = state.as_ref().unwrap();
        let theme = effective_theme_from_state(state).unwrap();
        let scale = theme_surface_scale(&theme, 0);
        let runtime = theme_runtime_for_surface(&theme, 0, theme_runtime_from_state(state));
        let index = theme.surfaces[0]
            .children
            .iter()
            .position(|layer| layer.id == id)
            .unwrap();
        let (x, y, width, height) = theme_engine::resolve_object_bounds_with_runtime(
            &theme,
            0,
            index,
            state.data.as_ref(),
            runtime,
        )?;
        let x = ((x + width / 2.0) * scale).round() as u32;
        let y = ((y + height / 2.0) * scale).round() as u32;
        Some(LPARAM(((y << 16) | x) as isize))
    }

    fn click(&self, at: LPARAM) {
        unsafe {
            wnd_proc(self.window, WM_LBUTTONDOWN, WPARAM(1), at);
            wnd_proc(self.window, WM_LBUTTONUP, WPARAM(0), at);
        }
    }
}

impl Drop for LiveApp {
    fn drop(&mut self) {
        *lock_state() = None;
        auto_hide::retain(&[]);
        auto_hide::show(self.window, false);
        unsafe {
            let _ = DestroyWindow(self.window);
        }
    }
}

#[test]
fn a_surface_on_a_display_that_is_gone_lands_on_the_first_display() {
    let mut top_bar: ThemeDocument =
        serde_json::from_str(include_str!("../themes/top-bar.json")).unwrap();
    top_bar.prepare_runtime();
    let mut rect_on = |display: usize| {
        top_bar.placement.reference.display = display;
        let (rect, _) = positioning::surface_target(&top_bar, 1.0).unwrap();
        (rect.left, rect.top, rect.right, rect.bottom)
    };
    let first = rect_on(0);
    assert_eq!(rect_on(native_interop::find_monitors().len() + 2), first);
}

#[test]
fn the_pointer_poll_sleeps_while_every_auto_hiding_surface_is_hidden_and_wakes_when_one_shows() {
    let app = LiveApp::start();
    app.run_timer(Duration::from_millis(400));
    assert_eq!(
        auto_hide::poll_interval(),
        Some(auto_hide::POLL_INTERVAL_MS)
    );
    assert!(auto_hide::take_timer(app.window), "running while collapsed");
    auto_hide::schedule(app.window);

    // A fullscreen app hides the bar: the poll stops once it is collapsed.
    auto_hide::show(app.window, false);
    app.run_timer(Duration::from_millis(100));
    assert_eq!(auto_hide::poll_interval(), None, "hidden and collapsed");
    assert!(!auto_hide::take_timer(app.window), "no timer while hidden");

    // The visibility timer shows it again and the poll comes back with it.
    sync_theme_window_visibility();
    assert_eq!(
        auto_hide::poll_interval(),
        Some(auto_hide::POLL_INTERVAL_MS)
    );
    assert!(auto_hide::take_timer(app.window), "timer restarted");
    auto_hide::schedule(app.window);

    // Hidden while still open: one more tick collapses it, then the poll stops.
    app.poll_result();
    app.run_timer(Duration::from_millis(500));
    auto_hide::show(app.window, false);
    assert!(auto_hide::poll_interval().is_some(), "still open");
    app.run_timer(Duration::from_millis(100));
    assert_eq!(auto_hide::poll_interval(), None);
    assert!(!auto_hide::take_timer(app.window));
}

#[test]
fn the_sample_reading_sounds_once_and_the_hide_timer_holds_the_top_bar_open() {
    let app = LiveApp::start();
    let (_, collapsed) = app.sizes();
    app.run_timer(Duration::from_millis(400));
    assert_eq!(app.presented(), (collapsed, true), "before any usage");
    assert_eq!(LiveApp::sounds(), 0);

    app.poll_result();
    assert_eq!(LiveApp::sounds(), 1, "Codex weekly has 4% left");
    let (open, collapsed) = app.sizes();
    assert!(open.1 > 10 * collapsed.1, "{open:?} {collapsed:?}");
    app.run_timer(Duration::from_millis(500));
    assert_eq!(app.presented(), (open, false), "held open and clickable");

    // Later polls with the same reading stay quiet and keep it open, well
    // past the moment it would slide away after a hover.
    for _ in 0..2 {
        app.poll_result();
        app.run_timer(auto_hide::LEAVE_DELAY + auto_hide::HIDE_DURATION);
    }
    assert_eq!(LiveApp::sounds(), 1);
    assert_eq!(app.presented(), (open, false));
}

#[test]
fn several_quotas_crossing_in_one_poll_play_one_sound() {
    use crate::models::{UsageData, UsageSection};
    let app = LiveApp::start();
    assert_eq!(LiveApp::sounds(), 0);
    let low = || UsageData {
        session: UsageSection {
            available: true,
            percentage: 97.0,
            resets_at: None,
        },
        weekly: UsageSection {
            available: true,
            percentage: 98.0,
            resets_at: None,
        },
        ..Default::default()
    };
    if let Some(state) = lock_state().as_mut() {
        state.data = Some(AppUsageData::from_iter([
            (ProviderId::Claude, low()),
            (ProviderId::Codex, low()),
        ]));
        state.last_poll_ok = true;
    }
    render_layered();
    assert_eq!(LiveApp::sounds(), 1, "four quotas crossed, one sound");
    render_layered();
    assert_eq!(LiveApp::sounds(), 1);
    drop(app);
}

#[test]
fn the_snooze_button_on_the_held_bar_lets_it_hide_exactly_like_the_menu_item() {
    for use_menu in [false, true] {
        let app = LiveApp::start();
        app.poll_result();
        app.run_timer(Duration::from_millis(500));
        let (open, collapsed) = app.sizes();
        assert_eq!(app.presented(), (open, false), "menu={use_menu}");
        let button = app
            .centre_of("snooze-button")
            .expect("the held bar shows the Snooze button");
        if use_menu {
            window_context_menu::execute_context_menu_action(
                app.window,
                ContextMenuAction::SnoozeAlarm,
                None,
            );
        } else {
            app.click(button);
        }
        let remaining = lock_state()
            .as_ref()
            .and_then(|state| state.alarm.snooze_remaining(Instant::now()))
            .expect("snoozed");
        assert!(
            remaining > low_usage_alarm::SNOOZE - Duration::from_secs(60),
            "{remaining:?}"
        );
        assert!(
            app.centre_of("snooze-button").is_none(),
            "hidden once snoozed"
        );
        app.run_timer(auto_hide::LEAVE_DELAY + auto_hide::HIDE_DURATION * 2);
        assert_eq!(app.presented(), (collapsed, true), "menu={use_menu}");
        // The same reading arriving again neither sounds nor reopens it.
        app.poll_result();
        app.run_timer(Duration::from_millis(300));
        assert_eq!(app.presented(), (collapsed, true), "menu={use_menu}");
        assert_eq!(LiveApp::sounds(), 1 + use_menu as usize);
    }
}
