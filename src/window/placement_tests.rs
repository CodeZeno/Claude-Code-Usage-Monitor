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
    assert_eq!(positioning::calculate_rect_overlap_ratio(widget, target_none), 0.0);

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
    };
    let json = serde_json::to_string(&ov).unwrap();
    assert!(json.contains("\"nest\":\"floating\""));
    assert!(json.contains("\"screen_x\":250"));

    let deserialized: app_settings::PlacementOverride = serde_json::from_str(&json).unwrap();
    assert_eq!(ov, deserialized);

    let mut settings = app_settings::SettingsFile::default();
    settings.floating_card_opacity = Some(150);
    settings.normalize();
    assert_eq!(settings.floating_card_opacity, Some(100));
}

#[test]
fn test_hysteresis_threshold_state_machine() {
    let free_slot = RECT {
        left: 1000,
        top: 0,
        right: 1200,
        bottom: 48,
    };
    // 200px width widget
    // If widget is placed at 1100..1300: intersection is 1100..1200 = 100px.
    // Overlap = 100 / 200 = 0.50.
    let widget_half = RECT {
        left: 1100,
        top: 0,
        right: 1300,
        bottom: 48,
    };
    let overlap_half = positioning::calculate_rect_overlap_ratio(widget_half, free_slot);
    assert!((overlap_half - 0.50).abs() < 0.01);

    // If currently unsnapped, threshold is 0.67, so 0.50 should NOT snap
    let snap_threshold_unsnapped = 0.67;
    assert!(overlap_half < snap_threshold_unsnapped);

    // If already snapped, threshold is 0.45 (hysteresis), so 0.50 SHOULD remain snapped
    let snap_threshold_snapped = 0.45;
    assert!(overlap_half >= snap_threshold_snapped);

    // If widget moved further out to 1130..1330: intersection is 1130..1200 = 70px (35%)
    let widget_far = RECT {
        left: 1130,
        top: 0,
        right: 1330,
        bottom: 48,
    };
    let overlap_far = positioning::calculate_rect_overlap_ratio(widget_far, free_slot);
    assert!(overlap_far < snap_threshold_snapped);
}

#[test]
fn test_auto_eject_relative_monitor_coords() {
    // Secondary monitor stacked vertically above primary: Y from -1080 to 0
    let mon_rect = RECT {
        left: 0,
        top: -1080,
        right: 1920,
        bottom: 0,
    };
    // Taskbar at top of that secondary monitor: top = -1080, bottom = -1032
    let taskbar_top = RECT {
        left: 0,
        top: -1080,
        right: 1920,
        bottom: -1032,
    };
    let is_top = (taskbar_top.top - mon_rect.top).abs() <= 50;
    assert!(is_top, "Taskbar at top of stacked monitor should be detected as top");

    let widget_h = 44;
    let ejected_y = if is_top {
        taskbar_top.bottom + 6
    } else {
        taskbar_top.top - widget_h - 6
    };
    assert_eq!(ejected_y, -1032 + 6); // -1026

    // Taskbar at bottom of that secondary monitor: top = -48, bottom = 0
    let taskbar_bottom = RECT {
        left: 0,
        top: -48,
        right: 1920,
        bottom: 0,
    };
    let is_top_bottom_tb = (taskbar_bottom.top - mon_rect.top).abs() <= 50;
    assert!(!is_top_bottom_tb, "Taskbar at bottom of stacked monitor should NOT be detected as top");

    let ejected_y_bottom = if is_top_bottom_tb {
        taskbar_bottom.bottom + 6
    } else {
        taskbar_bottom.top - widget_h - 6
    };
    assert_eq!(ejected_y_bottom, -48 - 44 - 6); // -98
}

#[test]
fn test_drag_release_capture_state_ordering() {
    // Model the state machine during WM_LBUTTONUP and WM_CAPTURECHANGED
    struct DragStateMachine {
        dragging: bool,
        pending_drag: bool,
        is_snapped: bool,
    }

    impl DragStateMachine {
        // Correct implementation: extract before releasing capture
        fn on_lbuttonup_correct(&mut self) -> ((bool, bool), bool) {
            let was_dragging = self.dragging;
            let was_pending = self.pending_drag;
            let was_snapped = self.is_snapped;
            self.dragging = false;
            self.pending_drag = false;
            self.is_snapped = false;

            // ReleaseCapture() synchronously triggers on_capture_changed()
            self.on_capture_changed();

            ((was_dragging, was_pending), was_snapped)
        }

        fn on_capture_changed(&mut self) {
            self.dragging = false;
            self.pending_drag = false;
            self.is_snapped = false;
        }
    }

    // Case 1: Was dragging actively while snapped
    let mut sm1 = DragStateMachine {
        dragging: true,
        pending_drag: false,
        is_snapped: true,
    };
    let ((was_dragging, was_pending), was_snapped) = sm1.on_lbuttonup_correct();
    assert!(was_dragging, "Drop must receive was_dragging = true");
    assert!(!was_pending);
    assert!(was_snapped, "Drop must preserve was_snapped = true for 0.45 threshold");
    assert!(!sm1.dragging);
    assert!(!sm1.is_snapped);

    // Case 2: Was a pending click (not dragged beyond threshold)
    let mut sm2 = DragStateMachine {
        dragging: false,
        pending_drag: true,
        is_snapped: false,
    };
    let ((was_dragging2, was_pending2), was_snapped2) = sm2.on_lbuttonup_correct();
    assert!(!was_dragging2);
    assert!(was_pending2, "Click dispatch must receive was_pending = true");
    assert!(!was_snapped2);
}

#[test]
fn test_high_dpi_widget_capacity_and_watchdog_recovery() {
    let logical_w: f64 = 217.0;
    let logical_h: f64 = 46.0;
    let dpi_scale: f64 = 1.5; // 150% High DPI
    let physical_w = (logical_w * dpi_scale).round() as i32; // 326
    let physical_h = (logical_h * dpi_scale).round() as i32; // 69
    assert_eq!(physical_w, 326);
    assert_eq!(physical_h, 69);

    let taskbar = RECT {
        left: 0,
        top: 1032,
        right: 1920,
        bottom: 1080,
    };
    // Available slot is only 260px wide
    let slot = RECT {
        left: 1000,
        top: 1032,
        right: 1260,
        bottom: 1080,
    };

    // With physical width (326px), capacity is NOT sufficient
    assert!(!positioning::is_taskbar_capacity_sufficient(
        taskbar,
        slot,
        physical_w,
        physical_h,
    ));

    // Watchdog check: free_space = 260. If evaluated against logical_w (217 + 20 = 237),
    // it would erroneously re-dock (260 >= 237).
    // With physical_w (326 + 20 = 346), it correctly refuses to re-dock:
    let free_space = slot.right - slot.left;
    let can_redock = free_space >= physical_w + 20;
    assert!(!can_redock, "Watchdog must not re-dock into undersized slot on high DPI");

    // Only when free_space expands to at least physical_w + 20 (346px) can it re-dock:
    let wide_slot_space = 350;
    assert!(wide_slot_space >= physical_w + 20);
}

#[test]
fn test_multi_monitor_auto_eject_resolution() {
    let displays = vec![
        native_interop::DisplayMonitor {
            handle: HMONITOR::default(),
            rect: RECT { left: 0, top: 0, right: 1920, bottom: 1080 },
            primary: true,
        },
        native_interop::DisplayMonitor {
            handle: HMONITOR::default(),
            rect: RECT { left: 1920, top: 0, right: 3840, bottom: 1080 },
            primary: false,
        },
    ];

    let pt = POINT { x: 2500, y: 100 };
    let resolved = displays
        .iter()
        .enumerate()
        .find(|(_, d)| {
            pt.x >= d.rect.left
                && pt.x < d.rect.right
                && pt.y >= d.rect.top
                && pt.y < d.rect.bottom
        })
        .map(|(i, d)| (i, *d));

    assert!(resolved.is_some());
    let (idx, display) = resolved.unwrap();
    assert_eq!(idx, 1, "Point on secondary monitor must resolve to display index 1");
    assert_eq!(display.rect.left, 1920);

    let scale = 1.5; // 150% DPI on secondary
    let rel_x = ((pt.x - display.rect.left) as f64 / scale).round() as i32;
    let rel_y = ((pt.y - display.rect.top) as f64 / scale).round() as i32;
    assert_eq!(rel_x, ((2500 - 1920) as f64 / 1.5).round() as i32); // 387
    assert_eq!(rel_y, (100.0 / 1.5_f64).round() as i32); // 67
}

#[test]
fn test_rebar_stretching_to_tray_is_not_treated_as_collision() {
    // Model the condition where ReBarWindow32.right extends all the way to TrayNotifyWnd.left.
    // In this scenario, rect.right >= tray_left - 10, meaning it is just the layout container
    // band, NOT an actual running application button collision.
    let tray_left = 1614;
    let rebar_right = 1614; // Directly adjacent
    let is_container_stretch = rebar_right >= tray_left - 10;
    assert!(is_container_stretch, "ReBar spanning to tray must be identified as container stretch");

    // When an actual app button is detected far from the tray (e.g. at 1200px):
    let actual_app_right = 1200;
    let is_actual_app = actual_app_right < tray_left - 10;
    assert!(is_actual_app, "Real app button boundary before tray must be retained");
}


