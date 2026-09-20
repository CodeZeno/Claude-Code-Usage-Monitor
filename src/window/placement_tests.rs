use super::*;

#[test]
fn window_state_timer_is_only_needed_for_floating_surfaces() {
    let mut theme = ThemeDocument::starter();
    assert!(!theme_has_floating_surface(&theme));

    theme.surfaces[0].placement.nest = SurfaceNest::Floating;
    assert!(theme_has_floating_surface(&theme));

    theme.surfaces[0].placement.nest = SurfaceNest::Auto;
    theme.surfaces[0].placement.reference.region = ReferenceRegion::Monitor;
    assert!(theme_has_floating_surface(&theme));
}

#[test]
fn center_points_keep_a_surface_centered_as_it_resizes() {
    assert_eq!(aligned_origin(0, 1920, 300, 0.5, 0.5, 0), 810);
    assert_eq!(aligned_origin(0, 1920, 500, 0.5, 0.5, 0), 710);
}

#[test]
fn reference_left_to_surface_right_places_widget_before_reference() {
    assert_eq!(aligned_origin(1600, 320, 300, 0.0, 1.0, 0), 1300);
}

#[test]
fn negative_offsets_inset_right_bottom_anchored_desktop_surfaces() {
    assert_eq!(aligned_origin(0, 3440, 198, 1.0, 1.0, -26), 3216);
    assert_eq!(aligned_origin(0, 1440, 144, 1.0, 1.0, -54), 1242);
}

#[test]
fn theme_dimensions_scale_from_logical_to_physical_pixels() {
    assert_eq!(scaled_theme_dimension(217, 1.0), 217);
    assert_eq!(scaled_theme_dimension(217, 1.25), 271);
    assert_eq!(scaled_theme_dimension(217, 1.5), 326);
    assert_eq!(scaled_theme_dimension(46, 2.0), 92);
}

#[test]
fn physical_host_dimensions_are_normalized_to_logical_pixels() {
    assert_eq!(logical_host_dimension(38, 1.25), 30);
    assert_eq!(logical_host_dimension(46, 1.0), 46);
    assert_eq!(logical_host_dimension(92, 2.0), 46);
    assert_eq!(logical_host_dimension(30, 0.0), 30);
}

#[test]
fn legacy_physical_offset_becomes_a_leftward_logical_theme_offset() {
    assert_eq!(legacy_offset_to_theme_offset(120, 1.25), -96);
    assert_eq!(legacy_offset_to_theme_offset(120, 1.0), -120);
    assert_eq!(legacy_offset_to_theme_offset(-5, 1.0), 0);
    assert_eq!(legacy_offset_to_theme_offset(20, 0.0), -20);
}

#[test]
fn tray_widget_action_targets_a_custom_theme_root_without_a_main_id() {
    let mut theme = ThemeDocument::starter();
    theme.surfaces[0].id = "layer-62744-2".into();
    let (surface_index, root_id) = context_menu_widget_origin(&theme).unwrap();
    assert_eq!((surface_index, root_id.as_str()), (0, "layer-62744-2"));

    let runtime = ThemeRuntime::new(true, true, true);
    let mut overrides = HashMap::new();
    theme_engine::execute_mouse_actions(
        &theme,
        surface_index,
        &root_id,
        "toggle(self, render)",
        None,
        runtime,
        &mut overrides,
    )
    .unwrap();
    let hidden = theme_engine::apply_mouse_action_overrides(&theme, &overrides);
    assert!(!theme_engine::surface_should_render(
        &hidden,
        surface_index,
        None,
        runtime
    ));
}

#[test]
fn fullscreen_bounds_cover_the_monitor_but_maximized_work_area_does_not() {
    let monitor = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    assert!(rect_covers_monitor(monitor, monitor));
    assert!(rect_covers_monitor(
        RECT {
            left: -2,
            top: -2,
            right: 1922,
            bottom: 1082,
        },
        monitor,
    ));
    assert!(!rect_covers_monitor(
        RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        },
        monitor,
    ));
}

#[test]
fn tray_rect_changed_detects_all_edge_shifts() {
    let base = RECT {
        left: 1600,
        top: 0,
        right: 1920,
        bottom: 48,
    };
    assert!(!rect_changed(Some(base), Some(base)));
    assert!(!rect_changed(None, None));
    assert!(rect_changed(None, Some(base)));
    assert!(rect_changed(Some(base), None));

    // Left edge changes when icons appear or hide
    let expanded = RECT {
        left: 1576,
        top: 0,
        right: 1920,
        bottom: 48,
    };
    assert!(rect_changed(Some(base), Some(expanded)));

    let shrunk = RECT {
        left: 1624,
        top: 0,
        right: 1920,
        bottom: 48,
    };
    assert!(rect_changed(Some(base), Some(shrunk)));
}

#[test]
fn is_tray_event_source_identifies_tray_and_excludes_own_windows() {
    let dummy_our = HWND(0x1000 as _);
    let dummy_tray = HWND(0x2000 as _);
    let dummy_taskbar = HWND(0x3000 as _);
    let dummy_other = HWND(0x4000 as _);

    // Invalid HWND should be ignored
    assert!(!is_tray_event_source(
        HWND::default(),
        Some(dummy_tray),
        Some(dummy_taskbar),
        &[dummy_our],
    ));

    // Own windows must be excluded
    assert!(!is_tray_event_source(
        dummy_our,
        Some(dummy_tray),
        Some(dummy_taskbar),
        &[dummy_our],
    ));

    // TrayNotifyWnd itself is accepted
    assert!(is_tray_event_source(
        dummy_tray,
        Some(dummy_tray),
        Some(dummy_taskbar),
        &[dummy_our],
    ));

    // Taskbar itself is accepted
    assert!(is_tray_event_source(
        dummy_taskbar,
        Some(dummy_tray),
        Some(dummy_taskbar),
        &[dummy_our],
    ));

    // Unrelated top-level window is ignored
    assert!(!is_tray_event_source(
        dummy_other,
        Some(dummy_tray),
        Some(dummy_taskbar),
        &[dummy_our],
    ));
}

#[test]
fn test_calculate_rect_overlap_ratio() {
    let widget = RECT {
        left: 100,
        top: 0,
        right: 200,
        bottom: 50,
    }; // width 100, height 50, area 5000

    // Complete overlap
    let target_full = RECT {
        left: 50,
        top: 0,
        right: 250,
        bottom: 50,
    };
    assert!((positioning::calculate_rect_overlap_ratio(widget, target_full) - 1.0).abs() < 1e-4);

    // No overlap
    let target_none = RECT {
        left: 300,
        top: 0,
        right: 400,
        bottom: 50,
    };
    assert_eq!(
        positioning::calculate_rect_overlap_ratio(widget, target_none),
        0.0
    );

    // 70% overlap (width 70 overlap across full height 50)
    let target_70 = RECT {
        left: 130,
        top: 0,
        right: 300,
        bottom: 50,
    };
    let ratio = positioning::calculate_rect_overlap_ratio(widget, target_70);
    assert!((ratio - 0.70).abs() < 1e-4);
    assert!(ratio >= 0.67); // Triggers snap

    // 40% overlap
    let target_40 = RECT {
        left: 160,
        top: 0,
        right: 300,
        bottom: 50,
    };
    let ratio_40 = positioning::calculate_rect_overlap_ratio(widget, target_40);
    assert!((ratio_40 - 0.40).abs() < 1e-4);
    assert!(ratio_40 < 0.45); // Below hysteresis threshold
}

#[test]
fn test_is_taskbar_capacity_sufficient() {
    // Horizontal taskbar 1920x48
    let taskbar_h = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 48,
    };
    let free_slot_plenty = RECT {
        left: 1000,
        top: 0,
        right: 1500,
        bottom: 48,
    }; // width 500
    assert!(positioning::is_taskbar_capacity_sufficient(
        taskbar_h,
        free_slot_plenty,
        250,
        46
    ));

    let free_slot_crowded = RECT {
        left: 1400,
        top: 0,
        right: 1500,
        bottom: 48,
    }; // width 100 < widget_w 250
    assert!(!positioning::is_taskbar_capacity_sufficient(
        taskbar_h,
        free_slot_crowded,
        250,
        46
    ));

    // Vertical taskbar 48x1080
    let taskbar_v = RECT {
        left: 0,
        top: 0,
        right: 48,
        bottom: 1080,
    };
    let free_slot_v = RECT {
        left: 0,
        top: 200,
        right: 48,
        bottom: 800,
    };
    // Album widget width 250 cannot fit in 48px width
    assert!(!positioning::is_taskbar_capacity_sufficient(
        taskbar_v,
        free_slot_v,
        250,
        46
    ));
}

#[test]
fn test_placement_override_serialization_and_normalization() {
    let ov = app_settings::PlacementOverride {
        nest: "floating".into(),
        monitor_index: 1,
        screen_x: 250,
        screen_y: 120,
        tray_offset: 0,
        floating_host: None,
    };
    let json = serde_json::to_string(&ov).unwrap();
    assert!(json.contains("\"nest\":\"floating\""));
    assert!(json.contains("\"screen_x\":250"));

    let deserialized: app_settings::PlacementOverride = serde_json::from_str(&json).unwrap();
    assert_eq!(ov, deserialized);
    assert!(serde_json::from_str::<app_settings::PlacementOverride>(
        r#"{"nest":"floating","monitor_index":0}"#
    )
    .unwrap()
    .floating_host
    .is_none());

    let mut settings = app_settings::SettingsFile::default();
    settings.floating_card_opacity = Some(150);
    settings.normalize();
    assert_eq!(settings.floating_card_opacity, Some(100));
}

#[test]
fn snapping_uses_hysteresis_at_both_thresholds() {
    let slot = RECT {
        left: 0,
        top: 0,
        right: 100,
        bottom: 50,
    };
    for (left, snapped, expected) in [
        (33, false, true), // 67%: start snapping at the boundary.
        (34, false, false),
        (50, false, false),
        (50, true, true),
        (55, true, true), // 45%: remain snapped at the boundary.
        (56, true, false),
        (100, true, false),
    ] {
        let widget = RECT {
            left,
            top: 0,
            right: left + 100,
            bottom: 50,
        };
        assert_eq!(
            positioning::should_snap_to_slot(widget, slot, snapped),
            expected,
            "left={left}, previously snapped={snapped}"
        );
    }
}

#[test]
fn auto_ejection_uses_monitor_relative_edges() {
    let monitor = RECT {
        left: -1920,
        top: -1080,
        right: 0,
        bottom: 0,
    };
    let widget = RECT {
        left: -500,
        top: -400,
        right: -283,
        bottom: -354,
    };
    for (taskbar, expected) in [
        (
            RECT {
                left: -1920,
                top: -1080,
                right: 0,
                bottom: -1032,
            },
            (-500, -1026),
        ),
        (
            RECT {
                left: -1920,
                top: -48,
                right: 0,
                bottom: 0,
            },
            (-500, -100),
        ),
        (
            RECT {
                left: -1920,
                top: -1080,
                right: -1872,
                bottom: 0,
            },
            (-1866, -400),
        ),
        (
            RECT {
                left: -48,
                top: -1080,
                right: 0,
                bottom: 0,
            },
            (-271, -400),
        ),
    ] {
        let point = positioning::auto_eject_origin(widget, taskbar, monitor);
        assert_eq!((point.x, point.y), expected);
    }
}

#[test]
fn drag_release_snapshots_state_before_reentrant_capture_change() {
    use message_loop::{release_drag_capture_with, DragRelease};
    use std::cell::{Cell, RefCell};

    for original in [
        DragRelease {
            dragging: true,
            pending: false,
            snapped: true,
        },
        DragRelease {
            dragging: false,
            pending: true,
            snapped: false,
        },
        DragRelease::default(),
    ] {
        let live = RefCell::new(original);
        let release_calls = Cell::new(0);
        let result = release_drag_capture_with(
            || {
                let mut state = live.borrow_mut();
                let DragRelease {
                    dragging,
                    pending,
                    snapped,
                } = &mut *state;
                DragRelease::take(dragging, pending, snapped)
            },
            || {
                // ReleaseCapture can synchronously re-enter the window procedure.
                // Reborrow also proves the snapshot's guard has been dropped.
                assert_eq!(*live.borrow(), DragRelease::default());
                *live.borrow_mut() = DragRelease::default();
                release_calls.set(release_calls.get() + 1);
            },
        );
        assert_eq!(
            result, original,
            "capture change must not erase the drop/click"
        );
        assert_eq!(*live.borrow(), DragRelease::default());
        assert_eq!(release_calls.get(), 1);
    }
}

#[test]
fn high_dpi_dimensions_drive_capacity_and_watchdog_redocking() {
    let mut theme = ThemeDocument::starter();
    theme.surfaces[0].width = 217.0.into();
    theme.surfaces[0].height = 46.0.into();
    let frame = positioning::widget_frame(&theme, None, ThemeRuntime::default(), 1.5);
    let (width, height) = (frame.width, frame.height);
    assert_eq!((width, height), (326, 69));

    let taskbar = RECT {
        left: 0,
        top: 1032,
        right: 1920,
        bottom: 1080,
    };
    let slot = RECT {
        left: 1000,
        top: 1032,
        right: 1260,
        bottom: 1080,
    };
    assert!(!positioning::is_taskbar_capacity_sufficient(
        taskbar, slot, width, height
    ));
    assert!(!can_redock_at_tray(260, width));
    assert!(!can_redock_at_tray(345, width));
    assert!(can_redock_at_tray(346, width));
    assert!(can_redock_at_tray(350, width));
}

#[test]
fn floating_card_padding_is_excluded_from_docking_capacity() {
    let theme = ThemeDocument::starter();
    let docked = positioning::widget_frame(&theme, None, ThemeRuntime::default(), 1.5);
    let floating = positioning::widget_frame(
        &theme,
        None,
        ThemeRuntime::default().with_nest(SurfaceNest::Floating),
        1.5,
    );
    assert_eq!(floating.width, docked.width + 30);
    assert_eq!(floating.content_width, docked.width);
    assert_eq!(floating.height, docked.height);
    let content = floating.content_rect(POINT { x: 985, y: 900 });
    assert_eq!(content.left, 1000);
    assert_eq!(content.right, 1000 + docked.width);
    assert!(can_redock_at_tray(docked.width + 20, docked.width));
    assert!(!can_redock_at_tray(docked.width + 20, floating.width));
}

#[test]
fn floating_monitor_resolution_handles_boundaries_fallbacks_and_dpi() {
    let displays = [
        native_interop::DisplayMonitor {
            handle: HMONITOR::default(),
            rect: RECT {
                left: -1920,
                top: -1080,
                right: 0,
                bottom: 0,
            },
            primary: false,
        },
        native_interop::DisplayMonitor {
            handle: HMONITOR::default(),
            rect: RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            primary: true,
        },
        native_interop::DisplayMonitor {
            handle: HMONITOR::default(),
            rect: RECT {
                left: 1920,
                top: 0,
                right: 3840,
                bottom: 1080,
            },
            primary: false,
        },
    ];
    for (point, expected_index, expected_offset) in [
        (POINT { x: 2500, y: 100 }, 2, (387, 67)),
        (POINT { x: 1920, y: 0 }, 2, (0, 0)),
        (POINT { x: -1800, y: -930 }, 0, (80, 100)),
        (POINT { x: 0, y: 0 }, 1, (0, 0)),
    ] {
        let (index, monitor) = positioning::monitor_for_point(&displays, point);
        assert_eq!(index, expected_index);
        let offset = positioning::logical_monitor_offset(point, monitor.rect, 1.5);
        assert_eq!((offset.x, offset.y), expected_offset);
    }
    let outside = POINT { x: 9000, y: 9000 };
    assert_eq!(positioning::monitor_for_point(&displays, outside).0, 1);
    assert_eq!(positioning::monitor_for_point(&displays[..1], outside).0, 0);
    let (index, fallback) = positioning::monitor_for_point(&[], outside);
    assert_eq!(index, 0);
    assert_eq!(
        (
            fallback.rect.left,
            fallback.rect.top,
            fallback.rect.right,
            fallback.rect.bottom
        ),
        (0, 0, 1920, 1080)
    );
}

// Exercise the actual restored-rectangle check with a tray-adjacent widget.
fn can_redock_at_tray(free_space: i32, width: i32) -> bool {
    let taskbar = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 96,
    };
    let tray = RECT {
        left: free_space,
        ..taskbar
    };
    // A button at the leading edge supplies the 20px return clearance.
    let occupancy = taskbar_collision::Occupancy {
        bounds: taskbar,
        occupied: vec![
            RECT {
                left: -1,
                right: 0,
                ..taskbar
            },
            tray,
        ],
        reserved: vec![tray],
    };
    let widget = RECT {
        left: free_space - width,
        right: free_space,
        bottom: 69,
        ..taskbar
    };
    occupancy.can_restore(widget, 20)
}
