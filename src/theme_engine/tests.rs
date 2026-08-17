use super::*;

#[test]
fn managed_asset_paths_stay_inside_the_asset_directory() {
    assert_eq!(managed_asset_file_name("assets/logo.png"), Some("logo.png"));
    assert_eq!(managed_asset_file_name("logo.png"), None);
    assert_eq!(managed_asset_file_name("assets/../logo.png"), None);
    assert_eq!(managed_asset_file_name("assets/folder/logo.png"), None);
    assert_eq!(managed_asset_file_name("assets\\logo.png"), None);
}

#[test]
fn theme_asset_usage_includes_roots_and_child_layers() {
    let mut theme = ThemeDocument::starter();
    let path = "assets/logo.png".to_string();
    theme.surfaces[0].background = LayerBackground::Image {
        path: path.clone(),
        fit: ImageFit::Contain,
    };
    theme.surfaces[0].children[0].background = LayerBackground::Image {
        path: path.clone(),
        fit: ImageFit::Contain,
    };
    assert_eq!(theme_asset_usage(&theme, &path), 2);
    assert_eq!(theme_asset_usage(&theme, "assets/other.png"), 0);
}

#[test]
fn removing_an_asset_clears_root_and_child_layer_references() {
    let mut theme = ThemeDocument::starter();
    let path = "assets/logo.png";
    theme.surfaces[0].background = LayerBackground::Image {
        path: path.into(),
        fit: ImageFit::Contain,
    };
    theme.surfaces[0].children[0].background = LayerBackground::Image {
        path: path.into(),
        fit: ImageFit::Contain,
    };

    assert_eq!(remove_asset_references(&mut theme, path), 2);
    assert!(matches!(
        theme.surfaces[0].background,
        LayerBackground::None
    ));
    assert!(matches!(
        theme.surfaces[0].children[0].background,
        LayerBackground::None
    ));
}

#[test]
fn legacy_image_content_becomes_an_image_background_without_tint() {
    let object: SceneObject = serde_json::from_value(serde_json::json!({
        "id": "legacy-image",
        "name": "Legacy image",
        "content": {
            "type": "image",
            "path": "assets/logo.png",
            "fit": "cover",
            "tint": { "color": "#80FF0000" }
        }
    }))
    .unwrap();
    assert!(matches!(object.content, SceneContent::None));
    assert!(matches!(
        &object.background,
        LayerBackground::Image { path, fit }
            if path == "assets/logo.png" && *fit == ImageFit::Cover
    ));

    let encoded = serde_json::to_value(&object).unwrap();
    assert_eq!(encoded["background"]["type"], "image");
    assert_eq!(encoded["content"]["type"], "none");
    assert!(encoded.get("tint").is_none());
    assert!(encoded["background"].get("tint").is_none());
}

#[test]
fn legacy_shape_content_becomes_layer_appearance() {
    let object: SceneObject = serde_json::from_value(serde_json::json!({
        "id": "legacy-shape",
        "name": "Legacy shape",
        "content": {
            "type": "shape",
            "shape": "rectangle",
            "fill": { "color": "#FF123456" },
            "stroke": { "color": { "color": "#FFFFFFFF" }, "width": "2" },
            "corner_radius": "8"
        }
    }))
    .unwrap();
    assert!(matches!(object.content, SceneContent::None));
    assert!(matches!(
        &object.background,
        LayerBackground::Colour { colour } if colour.color == "#FF123456"
    ));
    assert_eq!(object.border.as_ref().unwrap().width.0, "2");
    assert_eq!(object.corner_radius.0, "8");
}

#[test]
fn rounded_background_clip_masks_image_corners() {
    let mut pixels = vec![0xFFFF_FFFF; 100];
    clip_to_rounded_rectangle(&mut pixels, 10, 10, 5.0);
    assert!((pixels[0] >> 24) < 255);
    assert_eq!(pixels[5 * 10 + 5] >> 24, 255);
}

#[test]
fn rounded_stroke_preserves_partial_coverage_on_its_inner_curve() {
    let mut pixels = vec![0; 20 * 20];
    stroke_rounded_rectangle(
        &mut pixels,
        20,
        20,
        Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        },
        10.0,
        3.0,
    );

    let inner_curve_alpha = pixels[4 * 20 + 5] >> 24;
    assert!(
        (1..255).contains(&inner_curve_alpha),
        "inner curve should be antialiased, got alpha {inner_curve_alpha}"
    );
}

#[test]
fn gradient_angles_run_clockwise_from_left_to_right() {
    let black = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    let white = Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    let mut horizontal = vec![0; 3];
    fill_linear_gradient(&mut horizontal, 3, 1, black, white, 0.0);
    assert_eq!(horizontal[0], 0xFF00_0000);
    assert_eq!(horizontal[2], 0xFFFF_FFFF);

    let mut vertical = vec![0; 3];
    fill_linear_gradient(&mut vertical, 1, 3, black, white, 90.0);
    assert_eq!(vertical[0], 0xFF00_0000);
    assert_eq!(vertical[2], 0xFFFF_FFFF);
}

#[test]
fn gradient_background_round_trips() {
    let mut object = SceneObject::object("gradient", "Gradient");
    object.background = LayerBackground::Gradient {
        start: Paint::new("#FF0000FF"),
        end: Paint::new("#0000FFFF"),
        angle: Expression("45".into()),
    };
    let encoded = serde_json::to_value(&object).unwrap();
    assert_eq!(encoded["background"]["type"], "gradient");
    let decoded: SceneObject = serde_json::from_value(encoded).unwrap();
    assert!(matches!(
        decoded.background,
        LayerBackground::Gradient { start, end, angle }
            if start.color == "#FF0000FF"
                && end.color == "#0000FFFF"
                && angle.0 == "45"
    ));
}

#[test]
fn text_mask_preserves_solid_pixels_and_reduces_antialiased_edges() {
    assert_eq!(
        text_mask_coverage(0x0000_0000, FontRendering::Antialiased, 1.4),
        0.0
    );
    assert_eq!(
        text_mask_coverage(0x00FF_FFFF, FontRendering::Antialiased, 1.4),
        1.0
    );

    let raw_midpoint = 128.0 / 255.0;
    let corrected_midpoint = text_mask_coverage(0x0080_8080, FontRendering::Antialiased, 1.4);
    assert!(corrected_midpoint > 0.0);
    assert!(corrected_midpoint < raw_midpoint);

    assert_eq!(
        text_mask_coverage(0x0080_8080, FontRendering::Aliased, 4.0),
        1.0
    );
    let clear_type_subpixel = text_mask_coverage(0x00FF_0000, FontRendering::ClearType, 1.0);
    assert!((clear_type_subpixel - (1.0 / 3.0)).abs() < 0.001);
    assert!(
        text_mask_coverage(0x0080_8080, FontRendering::Antialiased, 2.0)
            < text_mask_coverage(0x0080_8080, FontRendering::Antialiased, 1.0)
    );
}

#[test]
fn expressions_support_data_math_and_functions() {
    let mut context = DataContext::default();
    context.insert("claude.session.percentage", 73.4);
    assert_eq!(
        evaluate("clamp(claude.session.percentage * 2, 0, 100)", &context).unwrap(),
        100.0
    );
    assert_eq!(evaluate("round((73.4 + 2.6) / 2)", &context).unwrap(), 38.0);
    assert_eq!(
        evaluate(
            "if(claude.session.percentage >= 70 && claude.session.percentage < 80, 1, 0)",
            &context
        )
        .unwrap(),
        1.0
    );
    assert_eq!(evaluate("lerp(10, 20, 0.25)", &context).unwrap(), 12.5);
    let context = DataContext::from_usage(None, &Canvas::default());
    assert_eq!(evaluate("true", &context).unwrap(), 1.0);
    assert_eq!(evaluate("false", &context).unwrap(), 0.0);
}

#[test]
fn host_dimensions_are_available_to_root_size_expressions() {
    let mut theme = ThemeDocument::starter();
    theme.surfaces[0].width = Expression("host.width".into());
    theme.surfaces[0].height = Expression("min(46, host.height)".into());
    let runtime = ThemeRuntime::default().with_host_dimensions(220, 30);

    assert_eq!(resolve_surface_size(&theme, 0, None, runtime), (220, 30));
    let rendered = render_theme_surface_with_runtime_at_scale(&theme, 0, None, runtime, 1.25);
    assert_eq!((rendered.width, rendered.height), (275, 38));
    assert!(rendered.warnings.is_empty());
}

#[test]
fn templates_apply_numeric_character_formats() {
    let mut context = DataContext::default();
    context.insert("claude.session.percentage", 73.45);
    context.insert("reset", 4.75 * 86_400.0);
    assert_eq!(
        format_template("Used {claude.session.percentage:0.0}%", &context),
        "Used 73.5%"
    );
    assert_eq!(format_template("{reset:duration_short}", &context), "4d");
}

#[test]
fn application_version_is_available_to_templates_and_numeric_expressions() {
    let context = DataContext::from_usage(None, &Canvas::default());
    assert_eq!(
        format_template("v{app.version}", &context),
        format!("v{}", env!("CARGO_PKG_VERSION"))
    );
    let major = env!("CARGO_PKG_VERSION")
        .split('.')
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0);
    assert_eq!(evaluate("app.version.major", &context).unwrap(), major);
}

#[test]
fn usage_lines_handle_loading_errors_missing_resets_and_language() {
    let canvas = Canvas::default();
    let loading = DataContext::from_usage_with_runtime(
        None,
        &canvas,
        ThemeRuntime::default()
            .with_poll_state(false, false)
            .with_language(LanguageId::from_code("nl").unwrap()),
    );
    assert_eq!(format_template("{i18n.session_window}", &loading), "5u");
    assert_eq!(
        format_template("{claude.session:usage_line}", &loading),
        "--"
    );
    assert_eq!(
        format_template("{claude.session:usage_badge}", &loading),
        "--"
    );

    let failed = DataContext::from_usage_with_runtime(
        None,
        &canvas,
        ThemeRuntime::default().with_poll_state(false, true),
    );
    assert_eq!(format_template("{claude.session:usage_line}", &failed), "!");
    assert_eq!(
        format_template("{claude.session:usage_badge}", &failed),
        "!"
    );

    let usage = AppUsageData::from_iter([(
        ProviderId::Claude,
        crate::models::UsageData {
            session: crate::models::UsageSection {
                percentage: 25.0,
                resets_at: None,
            },
            weekly: crate::models::UsageSection::default(),
            weekly_label: None,
        },
    )]);
    let ready = DataContext::from_usage_with_runtime(
        Some(&usage),
        &canvas,
        ThemeRuntime::default().with_language(LanguageId::from_code("ko").unwrap()),
    );
    assert_eq!(
        format_template("{claude.session:usage_line}", &ready),
        "25%"
    );
    assert_eq!(
        format_template("{claude.session:usage_badge}", &ready),
        "25%"
    );
    assert_eq!(
        format_template("{reset:duration_short}", &{
            let mut context = ready.clone();
            context.insert("reset", 3_600.0);
            context
        }),
        "1시간"
    );
    assert_eq!(format_template("{codex.session:usage_line}", &ready), "!");
}

#[test]
fn built_in_classic_uses_149_geometry() {
    let theme = ThemeDocument::starter();
    assert_eq!(theme.id, CLASSIC_THEME_ID);
    assert_eq!(theme.name, "Classic v1");
    assert_eq!(theme.validate(), Vec::<String>::new());
    for (runtime, expected_width) in [
        (ThemeRuntime::new(true, false, false), 217),
        (ThemeRuntime::new(true, true, false), 285),
        (ThemeRuntime::new(true, true, true), 375),
    ] {
        assert_eq!(
            resolve_surface_size(&theme, 0, None, runtime),
            (expected_width, 46)
        );
    }
}

#[test]
fn starter_theme_round_trips_and_validates() {
    let theme = ThemeDocument::starter();
    assert!(theme.validate().is_empty());
    let segments = theme.surfaces[0]
        .children
        .iter()
        .filter_map(|object| match object.content {
            SceneContent::Progress { segments, .. } => Some(segments),
            _ => None,
        })
        .collect::<Vec<_>>();
    // Classic contains separate light and dark progress layers so the
    // 1.4.9 palette follows the taskbar mode without runtime recolouring.
    assert_eq!(segments, vec![10; 20]);
    assert!(theme.surfaces[0]
        .children
        .iter()
        .any(|object| object.id == "provider-row"
            && object.layout == ChildLayout::Row
            && matches!(object.content, SceneContent::None)));
    let json = serde_json::to_string(&theme).unwrap();
    assert!(json.contains("\"render\""));
    assert!(json.contains("\"visibility\""));
    assert!(!json.contains("visible_when"));
    assert!(!json.contains("clip_contents"));
    let decoded: ThemeDocument = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.surfaces.len(), theme.surfaces.len());
    assert_eq!(
        decoded.surfaces[0].children.len(),
        theme.surfaces[0].children.len()
    );
}

#[test]
fn schema_only_serializes_placement_fields_for_the_relevant_level() {
    let mut theme = ThemeDocument::starter();
    theme.surfaces[0].placement.reference = ReferenceTarget {
        region: ReferenceRegion::Monitor,
        display: 1,
    };
    let json = serde_json::to_value(&theme).unwrap();
    let root = &json["surfaces"][0];
    assert!(root.get("parent").is_none());
    assert!(root.get("anchor").is_none());
    assert!(root.get("x").is_none());
    assert!(root.get("y").is_none());
    assert!(root.get("locked").is_none());
    assert!(root.get("locked_expression").is_none());
    assert_eq!(root["placement"]["reference"]["region"], "monitor");
    assert_eq!(root["placement"]["reference"]["display"], 1);
    assert!(root["placement"].get("anchor").is_none());
    assert!(root["placement"].get("monitor").is_none());
    assert!(root["placement"].get("duplicate_on_all_monitors").is_none());
    assert!(root["placement"]
        .get("click_through_transparent_pixels")
        .is_none());
    assert!(root.get("clip_children").is_none());
    let child = &root["children"][0];
    assert!(child.get("placement").is_none());
    assert!(child.get("locked").is_none());
    assert!(child.get("width").is_some());
    assert!(child.get("content").is_some());
}

#[test]
fn generated_studio_defaults_are_replaceable_but_named_user_themes_are_not() {
    let mut theme = ThemeDocument::starter();
    theme.id = "midnight-glass".into();
    theme.name = "Midnight Glass".into();
    assert!(theme.is_obsolete_studio_starter());
    theme.name = "My Midnight Glass".into();
    assert!(!theme.is_obsolete_studio_starter());
}

#[test]
fn colors_support_rgb_and_rgba() {
    assert_eq!(
        parse_color("#112233"),
        Some(Rgba {
            r: 0x11,
            g: 0x22,
            b: 0x33,
            a: 255
        })
    );
    assert_eq!(
        parse_color("#11223380"),
        Some(Rgba {
            r: 0x11,
            g: 0x22,
            b: 0x33,
            a: 0x80
        })
    );
}

#[test]
fn theme_schema_does_not_serialize_descriptions() {
    let source = serde_json::json!({
        "schema_version": THEME_SCHEMA_VERSION,
        "id": "without-description",
        "name": "Without description",
        "description": "This legacy field should be ignored",
        "surfaces": serde_json::to_value(ThemeDocument::starter().surfaces).unwrap(),
    });
    let theme: ThemeDocument = serde_json::from_value(source).unwrap();
    assert!(serde_json::to_value(theme)
        .unwrap()
        .get("description")
        .is_none());
}

#[test]
fn reset_stats_and_duration_formats_are_available_to_every_provider() {
    let reset = std::time::SystemTime::now() + std::time::Duration::from_secs(5_430);
    let usage = crate::models::AppUsageData::from_iter([(
        ProviderId::Claude,
        crate::models::UsageData {
            session: crate::models::UsageSection {
                percentage: 25.0,
                resets_at: Some(reset),
            },
            weekly: crate::models::UsageSection::default(),
            weekly_label: None,
        },
    )]);
    let context = DataContext::from_usage(Some(&usage), &Canvas::default());
    assert!(evaluate("claude.session.reset.seconds", &context).unwrap() > 5_400.0);
    assert_eq!(
        format_template("{claude.session.reset.seconds:duration}", &context),
        "1h 30m"
    );
    assert_eq!(evaluate("codex.available", &context).unwrap(), 0.0);
}

#[test]
fn provider_specific_long_window_labels_are_available_to_templates() {
    let usage = crate::models::AppUsageData::from_iter([(
        ProviderId::OpenCode,
        crate::models::UsageData {
            weekly_label: Some("30d".into()),
            ..Default::default()
        },
    )]);
    let context = DataContext::from_usage(Some(&usage), &Canvas::default());
    assert_eq!(format_template("{opencode.weekly.label}", &context), "30d");
    assert_eq!(format_template("{claude.weekly.label}", &context), "7d");
}

#[test]
fn starter_theme_renders_transparent_pixels_at_declared_size() {
    let theme = ThemeDocument::starter();
    let rendered = render_theme(&theme, None);
    assert_eq!((rendered.width, rendered.height), (217, 46));
    assert_eq!(rendered.pixels.len(), 217 * 46);
    assert!(rendered.pixels.iter().any(|pixel| pixel >> 24 > 0));
    assert_eq!(rendered.pixels[0] >> 24, 0);
    let track_alpha = (30..139)
        .map(|x| rendered.pixels[10 * rendered.width as usize + x] >> 24)
        .collect::<Vec<_>>();
    assert!(track_alpha.iter().filter(|alpha| **alpha == 0).count() >= 9);
    assert!(track_alpha.iter().filter(|alpha| **alpha > 0).count() >= 70);
    assert!(rendered.warnings.is_empty());
}

#[test]
fn empty_text_output_is_a_safe_noop() {
    let mut pixels = vec![0x7F12_3456; 16];
    let original = pixels.clone();
    render_text_mask(
        &mut pixels,
        4,
        4,
        "",
        "Segoe UI",
        12.0,
        400,
        FontRendering::Antialiased,
        1.0,
        TextAlign::Left,
        Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        },
    );
    assert_eq!(pixels, original);
}

#[test]
fn theme_surfaces_rasterize_at_requested_dpi_scales() {
    let theme = ThemeDocument::starter();
    let runtime = ThemeRuntime::new(true, false, false);
    for (scale, width, height) in [
        (1.0, 217, 46),
        (1.25, 271, 58),
        (1.5, 326, 69),
        (2.0, 434, 92),
    ] {
        let rendered = render_theme_surface_with_runtime_at_scale(&theme, 0, None, runtime, scale);
        assert_eq!((rendered.width, rendered.height), (width, height));
        assert_eq!(rendered.pixels.len(), (width * height) as usize);
        assert!(rendered.warnings.is_empty());
    }
}

#[test]
fn segmented_progress_reserves_gaps_only_between_segments() {
    let mask = (0..34)
        .map(|position| segmented_position_visible(position, 34, 5, 1.0))
        .collect::<Vec<_>>();

    assert_eq!(mask.iter().filter(|visible| **visible).count(), 30);
    assert_eq!(
        mask.iter()
            .enumerate()
            .filter_map(|(position, visible)| (!visible).then_some(position))
            .collect::<Vec<_>>(),
        vec![6, 13, 20, 27]
    );
    assert!(mask[0]);
    assert!(mask[33]);
}

#[test]
fn segmented_progress_preserves_both_rounded_outer_edges() {
    let mut pixels = vec![0; 34 * 12];
    draw_progress(
        &mut pixels,
        34,
        12,
        Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        },
        2.0,
        1.0,
        ProgressDirection::LeftToRight,
        5,
        1.0,
    );

    assert_ne!(pixels[6 * 34], 0);
    assert_ne!(pixels[6 * 34 + 33], 0);
    for gap in [6, 13, 20, 27] {
        assert_eq!(pixels[6 * 34 + gap], 0);
    }
}

#[test]
fn invalid_render_scales_fall_back_to_one() {
    let theme = ThemeDocument::starter();
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let rendered = render_theme_surface_with_runtime_at_scale(
            &theme,
            0,
            None,
            ThemeRuntime::default(),
            scale,
        );
        assert_eq!((rendered.width, rendered.height), (217, 46));
    }
}

#[test]
fn child_objects_resolve_against_parent_anchors_and_clip() {
    let mut theme = ThemeDocument::starter();
    let surface = &mut theme.surfaces[0];
    let mut parent = SceneObject::object("parent", "Parent with text content");
    parent.visibility = 50.0.into();
    parent.x = 20.0.into();
    parent.y = 10.0.into();
    parent.width = 100.0.into();
    parent.height = 50.0.into();
    parent.content = SceneContent::Text {
        template: "Container content".into(),
        font_family: "Segoe UI".into(),
        font_size: 12.0.into(),
        weight: FontWeight::Regular,
        rendering: FontRendering::Antialiased,
        contrast: default_font_contrast(),
        align: TextAlign::Left,
        color: Paint::new("#FFFFFFFF"),
    };
    let mut child = SceneObject::object("child", "Child");
    child.visibility = 50.0.into();
    child.parent = Some("parent".into());
    child.anchor = ObjectAnchor {
        horizontal: ObjectHorizontalAnchor::Right,
        vertical: ObjectVerticalAnchor::Bottom,
    };
    child.x = 5.0.into();
    child.y = 5.0.into();
    child.width = 10.0.into();
    child.height = 10.0.into();
    child.background = LayerBackground::Colour {
        colour: Paint::new("#FFFFFFFF"),
    };
    surface.children = vec![parent, child];
    let (width, height) =
        resolve_object_size(surface, None, ThemeRuntime::default(), &mut Vec::new());
    let canvas = Canvas {
        width,
        width_expression: Some(surface.width.clone()),
        height,
        height_expression: Some(surface.height.clone()),
        background: surface.background.canvas_paint(),
    };
    let (layers, warnings) = resolve_objects_for(
        surface,
        &canvas,
        &surface.children,
        None,
        ThemeRuntime::default(),
    );
    assert!(warnings.is_empty());
    assert_eq!(layers.len(), 2);
    assert_eq!((layers[1].x, layers[1].y), (105.0, 45.0));
    assert_eq!(layers[1].opacity, 0.25);
    assert_eq!(layers[1].clip.len(), 1);
    assert_eq!(
        resolve_object_bounds_with_runtime(&theme, 0, 1, None, ThemeRuntime::default()),
        Some((105.0, 45.0, 10.0, 10.0))
    );
}

#[test]
fn legacy_padding_is_ignored_and_offsets_control_child_position() {
    let mut theme = ThemeDocument::starter();
    let surface = &mut theme.surfaces[0];
    let mut parent = SceneObject::object("parent", "Parent");
    parent.x = 20.0.into();
    parent.y = 10.0.into();
    parent.width = 100.0.into();
    parent.height = 50.0.into();
    let mut child = SceneObject::object("child", "Child");
    child.parent = Some("parent".into());
    child.anchor = ObjectAnchor {
        horizontal: ObjectHorizontalAnchor::Right,
        vertical: ObjectVerticalAnchor::Bottom,
    };
    child.x = 3.0.into();
    child.y = 4.0.into();
    child.width = 10.0.into();
    child.height = 10.0.into();
    surface.children = vec![parent, child];

    assert_eq!(
        resolve_object_bounds_with_runtime(&theme, 0, 1, None, ThemeRuntime::default()),
        Some((107.0, 46.0, 10.0, 10.0))
    );

    let legacy: SceneObject = serde_json::from_value(serde_json::json!({
        "id": "legacy",
        "name": "Legacy",
        "padding": { "top": "9", "right": "8", "bottom": "7", "left": "6" }
    }))
    .unwrap();
    assert!(serde_json::to_value(legacy)
        .unwrap()
        .get("padding")
        .is_none());
}

#[test]
fn parent_rotation_rotates_child_position_and_clip() {
    let mut theme = ThemeDocument::starter();
    let surface = &mut theme.surfaces[0];
    surface.width = 200.0.into();
    surface.height = 200.0.into();
    let mut parent = SceneObject::object("parent", "Rotated parent");
    parent.x = 50.0.into();
    parent.y = 50.0.into();
    parent.width = 100.0.into();
    parent.height = 100.0.into();
    parent.rotation = 90.0.into();
    let mut child = SceneObject::object("child", "Child");
    child.parent = Some("parent".into());
    child.width = 20.0.into();
    child.height = 10.0.into();
    surface.children = vec![parent, child];

    let bounds =
        resolve_object_bounds_with_runtime(&theme, 0, 1, None, ThemeRuntime::default()).unwrap();
    assert!((bounds.0 - 135.0).abs() < 0.001);
    assert!((bounds.1 - 55.0).abs() < 0.001);
    let rendered = render_theme(&theme, None);
    assert!(rendered.warnings.is_empty());
}

#[test]
fn provider_variables_follow_settings_instead_of_poll_availability() {
    let runtime = ThemeRuntime::new(false, true, true);
    let context = DataContext::from_usage_with_runtime(None, &Canvas::default(), runtime);
    assert_eq!(evaluate("providers.count", &context).unwrap(), 2.0);
    assert_eq!(evaluate("providers.claude.enabled", &context).unwrap(), 0.0);
    assert_eq!(evaluate("providers.codex.enabled", &context).unwrap(), 1.0);
    assert_eq!(evaluate("codex.available", &context).unwrap(), 0.0);
    assert!(matches!(
        evaluate("system.dark", &context).unwrap(),
        0.0 | 1.0
    ));
    assert_eq!(
        evaluate("ceil(10 / max(1, providers.count))", &context).unwrap(),
        5.0
    );
}

#[test]
fn render_collapses_layout_while_zero_visibility_keeps_space() {
    let mut theme = ThemeDocument::starter();
    let runtime = ThemeRuntime::new(true, true, false);
    let surface = &theme.surfaces[0];
    let claude = surface
        .children
        .iter()
        .position(|object| object.id == "claude-provider")
        .unwrap();
    let codex = surface
        .children
        .iter()
        .position(|object| object.id == "codex-provider")
        .unwrap();

    theme.surfaces[0].children[claude].visibility = 0.0.into();
    assert_eq!(
        resolve_object_bounds_with_runtime(&theme, 0, codex, None, runtime).map(|bounds| bounds.0),
        Some(164.0)
    );

    theme.surfaces[0].children[claude].render = 0.0.into();
    assert_eq!(
        resolve_object_bounds_with_runtime(&theme, 0, codex, None, runtime).map(|bounds| bounds.0),
        Some(41.0)
    );
}

#[test]
fn surface_render_and_visibility_are_expression_driven() {
    let mut theme = ThemeDocument::starter();
    theme.surfaces[0].render = Expression("providers.codex.enabled".into());
    assert!(!surface_should_render(
        &theme,
        0,
        None,
        ThemeRuntime::new(true, false, false)
    ));
    assert!(surface_should_render(
        &theme,
        0,
        None,
        ThemeRuntime::new(false, true, false)
    ));

    theme.surfaces[0].render = Expression("true".into());
    theme.surfaces[0].visibility = 50.0.into();
    let rendered = render_theme_surface_with_runtime(&theme, 0, None, ThemeRuntime::default());
    let max_alpha = rendered
        .pixels
        .iter()
        .map(|pixel| pixel >> 24)
        .max()
        .unwrap_or(0);
    assert!((1..=128).contains(&max_alpha));
}

#[test]
fn surface_placement_offsets_are_expression_driven() {
    let mut theme = ThemeDocument::starter();
    let surface = &mut theme.surfaces[0];
    surface.width = 200.0.into();
    surface.placement.offset_x_expression = Some(Expression("canvas.width / 2".into()));
    surface.placement.offset_y_expression = Some(Expression("-5".into()));
    let resolved = resolve_surface_placement(&theme, 0, None, ThemeRuntime::new(true, true, false));
    assert_eq!(resolved.offset_x, 100);
    assert_eq!(resolved.offset_y, -5);
}

#[test]
fn starter_adapts_width_segments_and_collapsed_provider_rows() {
    let theme = ThemeDocument::starter();
    let surface = &theme.surfaces[0];
    let index = |id: &str| {
        surface
            .children
            .iter()
            .position(|object| object.id == id)
            .unwrap()
    };
    for (runtime, width, segments) in [
        (ThemeRuntime::new(true, false, false), 217, 10),
        (ThemeRuntime::new(false, true, false), 217, 10),
        (ThemeRuntime::new(false, false, true), 217, 10),
        (ThemeRuntime::new(true, true, false), 285, 5),
        (ThemeRuntime::new(true, false, true), 285, 5),
        (ThemeRuntime::new(false, true, true), 285, 5),
        (ThemeRuntime::new(true, true, true), 375, 4),
        (
            ThemeRuntime::from_providers(ProviderSet::from_enabled([ProviderId::OpenCode])),
            245,
            10,
        ),
        (
            ThemeRuntime::from_providers(ProviderSet::from_enabled([ProviderId::Cursor])),
            245,
            10,
        ),
        (
            ThemeRuntime::from_providers(ProviderSet::from_enabled(ProviderId::ALL)),
            545,
            2,
        ),
    ] {
        assert_eq!(resolve_surface_size(&theme, 0, None, runtime), (width, 46));
        let (canvas_width, canvas_height) = resolve_surface_size(&theme, 0, None, runtime);
        let canvas = Canvas {
            width: canvas_width,
            width_expression: Some(surface.width.clone()),
            height: canvas_height,
            height_expression: Some(surface.height.clone()),
            background: surface.background.canvas_paint(),
        };
        let context = DataContext::from_usage_with_runtime(None, &canvas, runtime);
        assert_eq!(
            evaluate("ceil(10 / max(1, providers.count))", &context).unwrap() as u16,
            segments
        );
        let rendered = render_theme_surface_with_runtime(&theme, 0, None, runtime);
        assert_eq!((rendered.width, rendered.height), (width, 46));
        assert!(rendered.warnings.is_empty());
    }

    let codex_only = ThemeRuntime::new(false, true, false);
    assert_eq!(
        resolve_object_bounds_with_runtime(&theme, 0, index("codex-provider"), None, codex_only,)
            .map(|bounds| bounds.0),
        Some(41.0)
    );
    assert!(resolve_object_bounds_with_runtime(
        &theme,
        0,
        index("claude-provider"),
        None,
        codex_only,
    )
    .is_none());

    let claude_and_antigravity = ThemeRuntime::new(true, false, true);
    assert_eq!(
        resolve_object_bounds_with_runtime(
            &theme,
            0,
            index("antigravity-provider"),
            None,
            claude_and_antigravity,
        )
        .map(|bounds| bounds.0),
        Some(164.0)
    );
}

#[test]
fn each_surface_renders_at_its_own_size() {
    let mut theme = ThemeDocument::starter();
    let mut second = theme.surfaces[0].clone();
    second.id = "secondary".into();
    second.width = 80.0.into();
    second.height = 40.0.into();
    theme.surfaces.push(second);
    let surface_index = theme.surfaces.len() - 1;
    let rendered =
        render_theme_surface_with_runtime(&theme, surface_index, None, ThemeRuntime::default());
    assert_eq!((rendered.width, rendered.height), (80, 40));
    assert!(theme.validate().is_empty());
}

#[test]
fn starter_has_a_taskbar_widget_and_provider_tray_icons() {
    let theme = ThemeDocument::starter();
    assert!(theme.is_builtin_classic());
    assert_eq!(theme.surfaces[0].placement.nest, SurfaceNest::Taskbar);
    assert_eq!(
        theme.surfaces[0].placement.reference.region,
        ReferenceRegion::SystemTray
    );
    assert_eq!(theme.surfaces.len(), 6);
    assert!(theme.surfaces[1..]
        .iter()
        .all(|surface| surface.placement.nest == SurfaceNest::TrayIcon));
    assert_eq!(
        theme.surfaces[0].mouse_events.as_ref().unwrap().right_click,
        "show_context_menu(\"classic-v1\")"
    );
    assert!(theme.surfaces[1..].iter().all(|surface| {
        let events = surface.mouse_events.as_ref().unwrap();
        events.click == "toggle(\"main\", render)"
            && events.double_click == events.click
            && events.right_click == "show_context_menu(\"classic-v1\")"
    }));
}

#[test]
fn migrated_theme_is_writable_and_only_moves_the_taskbar_surface() {
    let theme = ThemeDocument::migrated_from_legacy(Some((2, -96)), false);
    assert_eq!(theme.id, "migrated-theme");
    assert_eq!(theme.name, "Migrated Theme");
    assert!(!theme.is_builtin_classic());
    assert_eq!(theme.surfaces[0].placement.reference.display, 2);
    assert_eq!(theme.surfaces[0].placement.offset_x, -96);
    assert_eq!(theme.surfaces[0].render.0, "0");
    assert!(theme.surfaces[1..]
        .iter()
        .all(|surface| surface.placement.nest == SurfaceNest::TrayIcon));
    assert!(theme.validate().is_empty());
}

#[test]
fn hidden_legacy_widget_creates_an_unplaced_hidden_copy() {
    let theme = ThemeDocument::migrated_from_legacy(None, false);
    let classic = ThemeDocument::starter();
    assert_eq!(theme.surfaces[0].render.0, "0");
    assert_eq!(theme.surfaces[0].placement, classic.surfaces[0].placement);
    assert!(theme.validate().is_empty());
}

#[test]
fn starter_tray_icons_follow_enabled_providers() {
    let theme = ThemeDocument::starter();
    let runtime = ThemeRuntime::new(false, true, false);
    assert!(!surface_should_render(&theme, 1, None, runtime));
    assert!(surface_should_render(&theme, 2, None, runtime));
    assert!(!surface_should_render(&theme, 3, None, runtime));
    let rendered = render_theme_surface_with_runtime(&theme, 2, None, runtime);
    assert_eq!((rendered.width, rendered.height), (64, 64));
    assert!(rendered.pixels.iter().any(|pixel| pixel >> 24 > 0));
}

#[test]
fn built_in_themes_are_valid_and_cannot_be_saved_as_editable_themes() {
    assert_eq!(BUILTIN_THEME_SOURCES.len(), 1);
    assert_eq!(BUILTIN_THEME_SOURCES[0].0, CLASSIC_THEME_ID);
    assert!(REMOVED_BUILTIN_THEME_IDS
        .iter()
        .all(|id| !is_builtin_theme_id(id)));
    let mut ids = std::collections::HashSet::new();
    for (expected_id, source) in BUILTIN_THEME_SOURCES {
        let mut theme: ThemeDocument = serde_json::from_str(source).unwrap();
        assert_eq!(&theme.id, expected_id);
        assert!(ids.insert(theme.id.clone()));
        theme.prepare_runtime();
        assert!(theme.is_builtin());
        assert!(theme.validate().is_empty(), "{}", theme.name);
        for surface_index in 0..theme.surfaces.len() {
            let rendered = render_theme_surface_with_runtime(
                &theme,
                surface_index,
                None,
                ThemeRuntime::new(true, true, true),
            );
            assert!(
                rendered.warnings.is_empty(),
                "{} / {}: {:?}",
                theme.name,
                theme.surfaces[surface_index].name,
                rendered.warnings
            );
            assert!(rendered.width > 0 && rendered.height > 0);
        }
        assert!(save_theme(&theme).unwrap_err().contains("read-only"));
    }

    let error = save_theme(&ThemeDocument::starter()).unwrap_err();
    assert!(error.contains("read-only"));

    let mut duplicate = ThemeDocument::starter();
    duplicate.id = "classic-copy".into();
    assert!(!duplicate.is_builtin());
    assert!(!duplicate.is_builtin_classic());
}

#[test]
fn bundled_minecraft_theme_is_valid_editable_and_uses_dashboard_v2() {
    assert_eq!(BUNDLED_EDITABLE_THEME_SOURCES.len(), 1);
    let (expected_id, source) = BUNDLED_EDITABLE_THEME_SOURCES[0];
    let mut theme: ThemeDocument = serde_json::from_str(source).unwrap();
    assert_eq!(expected_id, MINECRAFT_THEME_ID);
    assert_eq!(theme.id, MINECRAFT_THEME_ID);
    assert!(!theme.is_builtin());
    theme.prepare_runtime();
    assert!(theme.validate().is_empty());
    assert_eq!(
        theme.surfaces[0]
            .mouse_events
            .as_ref()
            .map(|events| events.right_click.as_str()),
        Some("show_context_menu(\"dashboard-v2\")")
    );

    for (file_name, bytes) in BUNDLED_THEME_ASSETS {
        assert!(file_name.starts_with("minecraft-"));
        assert!(image::load_from_memory(bytes).is_ok());
        assert_eq!(theme_asset_usage(&theme, &format!("assets/{file_name}")), 1);
    }
}

#[test]
fn minecraft_context_menu_migration_is_targeted_and_one_time() {
    let mut minecraft: ThemeDocument =
        serde_json::from_str(BUNDLED_EDITABLE_THEME_SOURCES[0].1).unwrap();
    minecraft.surfaces[0]
        .mouse_events
        .as_mut()
        .unwrap()
        .right_click = "show_context_menu(\"classic-test\")".into();
    assert!(migrate_minecraft_context_menu(&mut minecraft));
    assert_eq!(
        minecraft.surfaces[0]
            .mouse_events
            .as_ref()
            .unwrap()
            .right_click,
        "show_context_menu(\"dashboard-v2\")"
    );
    assert!(!migrate_minecraft_context_menu(&mut minecraft));

    minecraft.id = "user-theme".into();
    minecraft.surfaces[0]
        .mouse_events
        .as_mut()
        .unwrap()
        .right_click = "show_context_menu(\"classic-test\")".into();
    assert!(!migrate_minecraft_context_menu(&mut minecraft));
}

#[test]
fn bundled_minecraft_install_preserves_user_edits() {
    let root = std::env::temp_dir().join(format!(
        "claude-code-usage-monitor-minecraft-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let themes = root.join("themes");
    let assets = themes.join("assets");
    ensure_bundled_editable_themes(&themes, &assets).unwrap();

    let theme_path = themes.join(format!("{MINECRAFT_THEME_ID}.json"));
    let mut installed = load_theme(&theme_path).unwrap();
    assert!(!installed.is_builtin());
    installed.name = "My Minecraft".into();
    crate::app_settings::write_json_atomic(&theme_path, &installed).unwrap();

    ensure_bundled_editable_themes(&themes, &assets).unwrap();
    assert_eq!(load_theme(&theme_path).unwrap().name, "My Minecraft");
    for (file_name, _) in BUNDLED_THEME_ASSETS {
        assert!(assets.join(file_name).is_file());
    }

    std::fs::remove_file(&theme_path).unwrap();
    ensure_bundled_editable_themes(&themes, &assets).unwrap();
    assert!(!theme_path.exists());

    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn theme_deletion_is_limited_to_managed_writable_themes() {
    let managed = themes_directory().join("deletable-theme.json");
    assert!(is_managed_theme_path(&managed));
    assert!(!is_managed_theme_path(Path::new("external-theme.json")));
    assert!(delete_theme(Path::new("external-theme.json"))
        .unwrap_err()
        .contains("Theme Studio"));

    let classic = ensure_starter_theme().unwrap();
    assert!(delete_theme(&classic)
        .unwrap_err()
        .contains("cannot be deleted"));
    assert!(classic.exists());
}

#[test]
fn themes_without_a_nest_migrate_from_their_reference_region() {
    let mut json = serde_json::to_value(ThemeDocument::starter()).unwrap();
    json["surfaces"][0]["placement"] = serde_json::json!({
        "reference": { "region": "system_tray", "display": 0 }
    });
    let mut taskbar_theme: ThemeDocument = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(taskbar_theme.surfaces[0].placement.nest, SurfaceNest::Auto);
    taskbar_theme.prepare_runtime();
    assert_eq!(
        taskbar_theme.surfaces[0].placement.nest,
        SurfaceNest::Taskbar
    );

    json["surfaces"][0]["placement"]["reference"]["region"] =
        serde_json::Value::String("monitor".into());
    let mut monitor_theme: ThemeDocument = serde_json::from_value(json).unwrap();
    monitor_theme.prepare_runtime();
    assert_eq!(
        monitor_theme.surfaces[0].placement.nest,
        SurfaceNest::Floating
    );
}

#[test]
fn nest_and_reference_are_independent() {
    let mut theme = ThemeDocument::starter();
    theme.surfaces[0].placement.nest = SurfaceNest::Desktop;
    theme.surfaces[0].placement.reference.region = ReferenceRegion::SystemTray;
    theme.prepare_runtime();
    assert_eq!(theme.surfaces[0].placement.nest, SurfaceNest::Desktop);
    assert_eq!(
        theme.surfaces[0].placement.reference.region,
        ReferenceRegion::SystemTray
    );

    let placement: Placement = serde_json::from_value(serde_json::json!({
        "layer": "floating",
        "reference": { "region": "taskbar", "display": 0 }
    }))
    .unwrap();
    assert_eq!(placement.nest, SurfaceNest::Floating);
    assert_eq!(placement.reference.region, ReferenceRegion::Taskbar);
}

#[test]
fn unsupported_theme_schemas_are_rejected_instead_of_migrated() {
    let mut theme = ThemeDocument::starter();
    theme.schema_version = THEME_SCHEMA_VERSION + 1;
    theme.prepare_runtime();
    assert_eq!(theme.schema_version, THEME_SCHEMA_VERSION + 1);
    assert!(theme
        .validate()
        .iter()
        .any(|error| error.contains("not supported")));
}

#[test]
fn mouse_action_parser_preserves_nested_value_expressions() {
    let actions = parse_mouse_actions(
        "show_dashboard(); toggle_dashboard(); show_context_menu()\n\
             set(\"details\", height, max(40, parent.height / 2))\n\
             increase(self.width, 10)\ndecrease(\"details\", rotation, 5)\n\
             toggle(self.render)\nreset(\"details\", width)",
    )
    .unwrap();
    assert_eq!(actions.len(), 8);
    assert!(matches!(
        &actions[3],
        MouseAction::Set { value, .. } if value.0 == "max(40, parent.height / 2)"
    ));
    assert!(matches!(actions[4], MouseAction::Increase { .. }));
    assert!(matches!(actions[5], MouseAction::Decrease { .. }));
    assert!(parse_mouse_actions("increase(self.render, 1)")
        .unwrap_err()
        .contains("numeric property"));
    assert!(matches!(
        parse_mouse_actions("show_context_menu(\"My Menu\")")
            .unwrap()
            .as_slice(),
        [MouseAction::ShowContextMenu { menu: Some(menu) }] if menu == "My Menu"
    ));
}

#[test]
fn mouse_actions_validate_targets_and_runtime_values() {
    let mut surface = SceneObject::root(
        "surface",
        "Surface",
        200.0.into(),
        100.0.into(),
        Placement::default(),
    );
    let mut button = SceneObject::object("button", "Button");
    button.parent = Some(surface.id.clone());
    button.mouse_events = Some(MouseEvents {
        click: "set(\"missing\", height, 50)".into(),
        ..Default::default()
    });
    surface.children.push(button);
    let mut theme = ThemeDocument::starter();
    theme.surfaces = vec![surface];
    let context = DataContext::from_usage(None, &Canvas::default());
    let errors = validate_mouse_action_script(
        &theme.surfaces[0].children[0]
            .mouse_events
            .as_ref()
            .unwrap()
            .click,
        &theme,
        0,
        "button",
        &context,
    );
    assert!(errors.iter().any(|error| error.contains("does not exist")));
}

#[test]
fn action_overrides_are_runtime_only_and_reset_restores_saved_expression() {
    let mut theme = ThemeDocument::starter();
    theme.id = "mouse-actions".into();
    let surface = &mut theme.surfaces[0];
    let target_id = surface.children[0].id.clone();
    let original = surface.children[0].height.clone();
    let mut overrides = HashMap::new();
    execute_mouse_actions(
        &theme,
        0,
        &target_id,
        "set(self.height, 123)",
        None,
        ThemeRuntime::default(),
        &mut overrides,
    )
    .unwrap();
    let effective = apply_mouse_action_overrides(&theme, &overrides);
    assert_eq!(effective.surfaces[0].children[0].height.0, "123");
    assert_eq!(theme.surfaces[0].children[0].height, original);
    execute_mouse_actions(
        &theme,
        0,
        &target_id,
        "reset(self.height)",
        None,
        ThemeRuntime::default(),
        &mut overrides,
    )
    .unwrap();
    assert!(overrides.is_empty());
}

#[test]
fn tray_root_can_toggle_the_main_root_on_another_surface() {
    let theme = ThemeDocument::starter();
    let tray_id = theme.surfaces[1].id.clone();
    let click = theme.surfaces[1]
        .mouse_events
        .as_ref()
        .unwrap()
        .click
        .clone();
    let mut overrides = HashMap::new();
    execute_mouse_actions(
        &theme,
        1,
        &tray_id,
        &click,
        None,
        ThemeRuntime::default(),
        &mut overrides,
    )
    .unwrap();

    let key = MouseActionOverrideKey {
        surface_index: 0,
        object_id: "main".into(),
        property: MouseActionProperty::Render,
    };
    assert_eq!(overrides.get(&key).map(|value| value.0.as_str()), Some("0"));
    assert!(!surface_should_render(
        &apply_mouse_action_overrides(&theme, &overrides),
        0,
        None,
        ThemeRuntime::default()
    ));
}

#[test]
fn increase_and_decrease_accumulate_from_the_effective_value() {
    let mut theme = ThemeDocument::starter();
    theme.id = "mouse-adjustments".into();
    let target_id = theme.surfaces[0].children[0].id.clone();
    theme.surfaces[0].children[0].width = 100.0.into();
    let mut overrides = HashMap::new();
    for script in [
        "increase(self.width, 10)",
        "increase(self.width, 10)",
        "decrease(self.width, 5)",
    ] {
        execute_mouse_actions(
            &theme,
            0,
            &target_id,
            script,
            None,
            ThemeRuntime::default(),
            &mut overrides,
        )
        .unwrap();
    }
    let effective = apply_mouse_action_overrides(&theme, &overrides);
    assert_eq!(effective.surfaces[0].children[0].width.0, "115");
    assert_eq!(theme.surfaces[0].children[0].width.0, "100");
}

#[test]
fn dashboard_and_context_menu_actions_emit_ordered_effects() {
    let theme = ThemeDocument::starter();
    let self_id = theme.surfaces[0].id.clone();
    let effects = execute_mouse_actions(
        &theme,
        0,
        &self_id,
        "show_dashboard(); toggle_dashboard(); show_context_menu()",
        None,
        ThemeRuntime::default(),
        &mut HashMap::new(),
    )
    .unwrap();
    assert_eq!(
        effects,
        vec![
            MouseActionEffect::ShowDashboard,
            MouseActionEffect::ToggleDashboard,
            MouseActionEffect::ShowContextMenu(None),
        ]
    );
}

#[test]
fn hit_testing_selects_the_topmost_interactive_layer() {
    let mut theme = ThemeDocument::starter();
    theme.id = "mouse-hit-test".into();
    let surface = &mut theme.surfaces[0];
    surface.width = 100.0.into();
    surface.height = 100.0.into();
    surface.children.clear();
    for id in ["bottom", "top"] {
        let mut object = SceneObject::object(id, id);
        object.parent = Some(surface.id.clone());
        object.width = 50.0.into();
        object.height = 50.0.into();
        object.mouse_events = Some(MouseEvents {
            click: "show_dashboard()".into(),
            ..Default::default()
        });
        surface.children.push(object);
    }
    assert_eq!(
        hit_test_mouse_event(&theme, 0, 25.0, 25.0, None, ThemeRuntime::default()),
        Some("top".into())
    );
}
