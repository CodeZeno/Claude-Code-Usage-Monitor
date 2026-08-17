use super::*;

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
