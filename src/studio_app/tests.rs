use super::*;

#[test]
fn configured_fonts_render_fallback_text_and_lucide_icons() {
    let context = egui::Context::default();
    configure_style(&context, LanguageId::English);

    let _ = context.run_ui(egui::RawInput::default(), |ui| {
        let fallback = ui.ctx().fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                "Fallback — ‘ready’".into(),
                egui::FontId::new(14.0, egui::FontFamily::Name("ui-fallback".into())),
                egui::Color32::WHITE,
            )
        });
        assert!(fallback.size().x > 0.0);

        for language in LanguageId::ALL {
            let native_name = language.native_name();
            assert_eq!(
                language_name(LanguageId::English, language.code()),
                native_name
            );
            let native_label = ui.ctx().fonts_mut(|fonts| {
                fonts.layout_no_wrap(
                    native_name.into(),
                    egui::FontId::new(14.0, egui::FontFamily::Proportional),
                    egui::Color32::WHITE,
                )
            });
            assert!(native_label.size().x > 0.0);
        }

        let icon = ui.ctx().fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                format!(
                    "{}{}",
                    LucideIcon::X.unicode(),
                    LucideIcon::ChevronRight.unicode()
                ),
                egui::FontId::new(14.0, egui::FontFamily::Name("lucide".into())),
                egui::Color32::WHITE,
            )
        });
        assert!(icon.size().x > 0.0);

        let native_menu = ui.ctx().fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                "Native menu".into(),
                egui::FontId::new(12.0, egui::FontFamily::Name("native-menu".into())),
                egui::Color32::BLACK,
            )
        });
        assert!(native_menu.size().x > 0.0);
    });
}

#[test]
fn colorref_conversion_uses_windows_bgr_byte_order() {
    assert_eq!(
        colorref_to_egui(0x0033_2211),
        egui::Color32::from_rgb(0x11, 0x22, 0x33)
    );
}

#[test]
fn native_context_menu_metrics_are_valid_at_common_dpi_scales() {
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let appearance = NativeContextMenuAppearance::detect(scale);
        assert!(appearance.font.size >= 9.0);
        assert!(appearance.row_height > appearance.font.size);
        assert!(appearance.left_gutter > 0.0);
        assert!(appearance.frame_inset > 0.0);
    }
}

#[test]
fn context_menu_preview_submenu_opens_on_hover() {
    let context = egui::Context::default();
    let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
    let preview_bounds = std::cell::Cell::new(egui::Rect::NOTHING);
    let open_submenu_state_ids = std::cell::RefCell::new(Vec::new());
    let mut input = egui::RawInput {
        screen_rect: Some(screen_rect),
        ..Default::default()
    };
    input
        .events
        .push(egui::Event::PointerMoved(egui::pos2(50.0, 20.0)));
    let mut popup_rect = None;
    let _ = context.run_ui(input, |ui| {
        let (_, popup) = preview_context_menu_submenu(
            ui,
            egui::Button::new("").min_size(egui::vec2(120.0, 24.0)),
            &preview_bounds,
            &open_submenu_state_ids,
            |ui| {
                ui.label("Submenu");
            },
        );
        popup_rect = popup.map(|popup| popup.response.rect);
    });
    let popup_rect = popup_rect.expect("submenu should open over its parent button");

    let mut input = egui::RawInput {
        screen_rect: Some(screen_rect),
        ..Default::default()
    };
    input.events.push(egui::Event::PointerMoved(egui::pos2(
        popup_rect.left() + 1.0,
        popup_rect.center().y,
    )));
    let mut remained_open = false;
    let _ = context.run_ui(input, |ui| {
        let (_, popup) = preview_context_menu_submenu(
            ui,
            egui::Button::new("").min_size(egui::vec2(120.0, 24.0)),
            &preview_bounds,
            &open_submenu_state_ids,
            |ui| {
                ui.label("Submenu");
            },
        );
        remained_open = popup.is_some();
    });
    assert!(
        remained_open,
        "submenu should remain open at its overlapping edge"
    );

    let mut input = egui::RawInput {
        screen_rect: Some(screen_rect),
        ..Default::default()
    };
    input
        .events
        .push(egui::Event::PointerMoved(popup_rect.center()));
    remained_open = false;
    let _ = context.run_ui(input, |ui| {
        let (_, popup) = preview_context_menu_submenu(
            ui,
            egui::Button::new("").min_size(egui::vec2(120.0, 24.0)),
            &preview_bounds,
            &open_submenu_state_ids,
            |ui| {
                ui.label("Submenu");
            },
        );
        remained_open = popup.is_some();
    });
    assert!(
        remained_open,
        "submenu should remain open under the pointer"
    );

    let mut input = egui::RawInput {
        screen_rect: Some(screen_rect),
        ..Default::default()
    };
    let outside = egui::pos2(350.0, 250.0);
    input.events.extend([
        egui::Event::PointerMoved(outside),
        egui::Event::PointerButton {
            pos: outside,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos: outside,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    remained_open = true;
    let _ = context.run_ui(input, |ui| {
        preview_context_menu_submenu(
            ui,
            egui::Button::new("").min_size(egui::vec2(120.0, 24.0)),
            &preview_bounds,
            &open_submenu_state_ids,
            |ui| {
                ui.label("Submenu");
            },
        );
    });
    let unrelated_popup_id = egui::Id::new("unrelated-dropdown-popup");
    egui::Popup::open_id(&context, unrelated_popup_id);
    collapse_context_menu_preview_on_outside_click(
        &context,
        preview_bounds.get(),
        &open_submenu_state_ids.borrow(),
    );
    assert!(
        egui::Popup::is_id_open(&context, unrelated_popup_id),
        "an outside preview click must not close unrelated UI popups"
    );
    let input = egui::RawInput {
        screen_rect: Some(screen_rect),
        ..Default::default()
    };
    let _ = context.run_ui(input, |ui| {
        let (_, popup) = preview_context_menu_submenu(
            ui,
            egui::Button::new("").min_size(egui::vec2(120.0, 24.0)),
            &preview_bounds,
            &open_submenu_state_ids,
            |ui| {
                ui.label("Submenu");
            },
        );
        remained_open = popup.is_some();
    });
    assert!(!remained_open, "an outside click should close the submenu");
}

#[test]
fn legacy_menu_theme_colors_are_normalized_to_the_native_fluent_palette() {
    let colors = normalize_native_menu_theme_colors(NativeMenuThemeColors {
        background: egui::Color32::from_rgb(240, 240, 240),
        text: egui::Color32::BLACK,
        disabled_text: egui::Color32::from_rgb(109, 109, 109),
        highlight: egui::Color32::from_rgb(0, 120, 215),
        highlight_text: egui::Color32::WHITE,
        border: egui::Color32::from_rgb(100, 100, 100),
    });
    assert_eq!(colors.background, egui::Color32::from_rgb(249, 249, 249));
    assert_eq!(colors.text, egui::Color32::from_rgb(31, 31, 31));
    assert_eq!(colors.border, egui::Color32::from_rgb(229, 229, 229));
}

#[test]
fn context_menu_preview_marks_a_rendered_target_layer_as_checked() {
    let mut app = app_with_surfaces(vec![root("main")]);
    let action = ContextMenuAction::ToggleLayerRender {
        target: "main".into(),
    };
    assert!(context_menu_preview_action_checked(
        &action,
        &app.settings,
        app.startup_enabled,
        &app.theme,
        app.usage.as_ref(),
        app.theme_runtime(),
    ));

    app.theme.surfaces[0].render = 0.0.into();
    assert!(!context_menu_preview_action_checked(
        &action,
        &app.settings,
        app.startup_enabled,
        &app.theme,
        app.usage.as_ref(),
        app.theme_runtime(),
    ));
}

#[test]
fn studio_preview_uses_cached_poll_failure_state_instead_of_stale_values() {
    let mut app = app_with_surfaces(vec![root("main")]);
    app.usage = Some(AppUsageData::from_iter([(
        crate::providers::ProviderId::Codex,
        crate::models::UsageData {
            session: crate::models::UsageSection {
                percentage: 7.0,
                resets_at: None,
            },
            weekly: Default::default(),
            weekly_label: None,
            ..Default::default()
        },
    )]));
    app.usage_poll_ok = false;
    app.usage_has_error = true;
    let context = DataContext::from_usage_with_runtime(
        app.usage.as_ref(),
        &Canvas::default(),
        app.theme_runtime(),
    );
    assert_eq!(
        theme_engine::format_template("{codex.session:usage_line}", &context),
        "!"
    );
}

fn root(id: &str) -> SceneObject {
    SceneObject::root(id, id, 200.0.into(), 100.0.into(), Placement::default())
}

fn object(id: &str, parent: Option<&str>) -> SceneObject {
    let mut object = SceneObject::object(id, id);
    object.parent = parent.map(str::to_string);
    object
}

fn app_with_surfaces(surfaces: Vec<SceneObject>) -> StudioApp {
    let mut theme = ThemeDocument::starter();
    theme.id = "test-theme".into();
    theme.surfaces = surfaces;
    theme.prepare_runtime();
    let history_snapshot = theme.clone();
    StudioApp {
        owner: 0,
        page: Page::Studio,
        settings: SettingsFile::default(),
        startup_enabled: false,
        theme,
        theme_path: None,
        selection: Selection::Surface(0),
        preview: None,
        preview_dirty: true,
        usage: None,
        usage_poll_ok: false,
        usage_has_error: false,
        last_cache_read: Instant::now(),
        dirty: false,
        live_apply: DEFAULT_LIVE_APPLY,
        zoom: 1.0,
        undo_stack: Vec::new(),
        redo_stack: Vec::new(),
        history_snapshot,
        scene_width: DEFAULT_SCENE_WIDTH,
        inspector_width: DEFAULT_INSPECTOR_WIDTH,
        hovered_scene_item: None,
        expression_helper: None,
        action_helper: None,
        text_template_helper: None,
        preview_mouse_overrides: HashMap::new(),
        preview_hover_target: None,
        preview_pending_click: None,
        asset_picker: None,
        asset_thumbnails: HashMap::new(),
        asset_page_filter: String::new(),
        asset_page_selected: None,
        asset_error: None,
        settings_error: None,
        theme_error: None,
        pending_unsaved_action: None,
        asset_delete_confirmation: None,
        new_theme_name: None,
        duplicate_theme_name: None,
        delete_theme_confirmation: None,
        context_menu: context_menu::classic_context_menu(),
        context_menu_path: None,
        context_menu_dirty: false,
        context_menu_selection: None,
        context_menu_action_helper: None,
        delete_context_menu_confirmation: None,
    }
}

#[test]
fn inspector_controls_use_a_consistent_maximum_width() {
    assert_eq!(crate::ui::components::layout::control_width(180.0), 180.0);
    assert_eq!(crate::ui::components::layout::control_width(500.0), 180.0);
}

#[test]
fn context_menu_items_can_be_dragged_into_and_out_of_groups() {
    let mut items = vec![
        ContextMenuItem::submenu("tools", "Tools", vec![]),
        ContextMenuItem::action("site", "Website", ContextMenuAction::OpenDashboard),
    ];
    let mut selection = Some(vec![1]);
    assert!(move_context_menu_item_to(
        &mut items,
        &[1],
        &ContextMenuDropTarget::Into(vec![0]),
        &mut selection,
    ));
    assert_eq!(selection, Some(vec![0, 0]));
    assert!(move_context_menu_item_to(
        &mut items,
        &[0, 0],
        &ContextMenuDropTarget::After(vec![0]),
        &mut selection,
    ));
    assert_eq!(selection, Some(vec![1]));
    assert_eq!(items[1].id, "site");
    assert!(move_context_menu_item_to(
        &mut items,
        &[1],
        &ContextMenuDropTarget::Before(vec![0]),
        &mut selection,
    ));
    assert_eq!(selection, Some(vec![0]));
    assert_eq!(items[0].id, "site");
}

#[test]
fn context_menu_item_ids_are_unique_across_nested_items() {
    let items = vec![ContextMenuItem::submenu(
        "tools",
        "Tools",
        vec![ContextMenuItem::action(
            "action-1",
            "Action",
            ContextMenuAction::OpenDashboard,
        )],
    )];
    assert_eq!(next_context_menu_item_id(&items, "action"), "action-2");
}

#[test]
fn context_menu_action_scripts_round_trip_every_action_kind() {
    let actions = [
        ContextMenuAction::OpenDashboard,
        ContextMenuAction::Refresh,
        ContextMenuAction::SetUpdateFrequency {
            seconds: POLL_5_MIN_SECONDS,
        },
        ContextMenuAction::ToggleProvider {
            provider: ContextMenuProvider::Codex,
        },
        ContextMenuAction::ToggleStartup,
        ContextMenuAction::ToggleWidget,
        ContextMenuAction::SetLanguage {
            language: "en-US".into(),
        },
        ContextMenuAction::CheckForUpdates,
        ContextMenuAction::ToggleLayerRender {
            target: "main".into(),
        },
        ContextMenuAction::LayerActions {
            actions: "increase(self, width, 10)".into(),
        },
        ContextMenuAction::OpenUrl {
            url: "https://example.com/a?q=1".into(),
        },
        ContextMenuAction::Exit,
    ];
    for action in actions {
        let script = context_menu_action_script(&action);
        assert_eq!(parse_context_menu_action_script(&script), Ok(action));
    }
}

#[test]
fn context_menu_action_script_rejects_multiple_or_unsafe_actions() {
    assert!(parse_context_menu_action_script("refresh()\nexit()").is_err());
    assert!(parse_context_menu_action_script("open_url(\"mailto:test@example.com\")").is_err());
    assert!(parse_context_menu_action_script("open_url(\"file:///temp\")").is_err());
    assert!(parse_context_menu_action_script("toggle_provider(unknown)").is_err());
    assert!(parse_context_menu_action_script("reset_position()").is_err());
    assert_eq!(
        parse_context_menu_action_script("set_update_frequency(300000)"),
        Ok(ContextMenuAction::SetUpdateFrequency { seconds: 300 })
    );
}

#[test]
fn text_helper_catalog_only_builds_valid_template_tokens() {
    let context = DataContext::from_usage(None, &Canvas::default());
    for value in TEXT_TEMPLATE_VALUES {
        for format in text_template_formats(value.kind) {
            let token = text_template_token(value.expression, *format);
            assert!(
                theme_engine::validate_template(&token, &context).is_empty(),
                "invalid helper token: {token}"
            );
        }
    }
    assert_eq!(
        text_template_token("claude.session.percentage", TextTemplateFormat::Percentage),
        "{claude.session.percentage:percent}"
    );
}

#[test]
fn text_helper_only_marks_real_template_tokens_as_expressions() {
    assert!(!template_has_expression("Plain text"));
    assert!(!template_has_expression("A literal {{ brace"));
    assert!(!template_has_expression("An incomplete {value"));
    assert!(template_has_expression(
        "Used {claude.session.percentage:percent}"
    ));
}

#[test]
fn duplicating_layers_remaps_mouse_action_targets() {
    let mut source = object("source", None);
    source.mouse_events = Some(MouseEvents {
        click: "set(\"target\", render, false)".into(),
        ..Default::default()
    });
    let target = object("target", None);
    let mut objects = vec![source, target];
    remap_scene_ids(&mut objects, &std::collections::HashSet::new(), true);
    let handler = &objects[0].mouse_events.as_ref().unwrap().click;
    assert!(!handler.contains("\"target\""));
    assert!(handler.contains(&format!("\"{}\"", objects[1].id)));
}

#[test]
fn blank_theme_starts_writable_with_one_empty_surface() {
    let theme = blank_theme("New theme");

    assert_eq!(theme.name, "New theme");
    assert!(!theme.is_builtin());
    assert_eq!(theme.surfaces.len(), 1);
    assert!(matches!(theme.surfaces[0].content, SceneContent::None));
    assert!(theme.surfaces[0].children.is_empty());
    assert!(theme.validate().is_empty());
}

#[test]
fn searchable_dropdown_filter_is_case_insensitive_and_matches_partial_names() {
    let matches = crate::ui::components::searchable_dropdown::option_matches;
    assert!(matches("Segoe UI Variable Text", "segoe ui"));
    assert!(matches("Consolas", ""));
    assert!(!matches("Consolas", "arial"));
}

#[test]
fn live_apply_starts_disabled() {
    const { assert!(!DEFAULT_LIVE_APPLY) };
}

#[test]
fn dirty_theme_defers_theme_switch_until_the_user_decides() {
    let mut app = app_with_surfaces(vec![root("alpha")]);
    let requested = PathBuf::from("another-theme.json");
    app.dirty = true;

    app.request_activate_theme(requested.clone());

    assert_eq!(
        app.pending_unsaved_action,
        Some(PendingUnsavedAction::ActivateTheme(requested))
    );
    assert_eq!(app.theme.surfaces[0].name, "alpha");
}

#[test]
fn dirty_theme_defers_new_theme_until_the_user_decides() {
    let mut app = app_with_surfaces(vec![root("alpha")]);
    app.dirty = true;

    app.request_new_theme();

    assert_eq!(
        app.pending_unsaved_action,
        Some(PendingUnsavedAction::NewTheme)
    );
    assert!(app.new_theme_name.is_none());
}

#[test]
fn save_failure_remains_dirty_and_surfaces_the_error() {
    let mut app = app_with_surfaces(vec![root("")]);
    app.dirty = true;

    assert!(!app.save_theme());
    assert!(app.dirty);
    assert!(app
        .theme_error
        .as_deref()
        .is_some_and(|error| error.contains("Unable to save theme")));
}

#[test]
fn add_layer_inserts_beside_the_selected_root() {
    let mut app = app_with_surfaces(vec![root("alpha"), root("beta")]);
    app.selection = Selection::Surface(0);

    app.add_layer();

    assert_eq!(app.theme.surfaces.len(), 3);
    assert_eq!(app.theme.surfaces[0].name, "alpha");
    assert_eq!(app.theme.surfaces[2].name, "beta");
    assert_eq!(app.selection, Selection::Surface(1));
}

#[test]
fn add_layer_inserts_beside_the_selected_child_at_the_same_level() {
    let mut surface = root("surface");
    surface.children = vec![
        object("parent", None),
        object("selected", Some("parent")),
        object("existing-sibling", Some("parent")),
    ];
    let mut app = app_with_surfaces(vec![surface]);
    app.selection = Selection::Object(0, 1);

    app.add_layer();

    assert_eq!(app.theme.surfaces[0].children.len(), 4);
    assert_eq!(
        app.theme.surfaces[0].children[2].parent.as_deref(),
        Some("parent")
    );
    assert_eq!(app.theme.surfaces[0].children[3].name, "existing-sibling");
    assert_eq!(app.selection, Selection::Object(0, 2));
}

#[test]
fn add_layer_falls_back_to_the_top_for_an_invalid_selection() {
    let mut app = app_with_surfaces(vec![root("alpha"), root("beta")]);
    app.selection = Selection::Surface(usize::MAX);

    app.add_layer();

    assert_eq!(app.theme.surfaces[1].name, "alpha");
    assert_eq!(app.selection, Selection::Surface(0));
}

#[test]
fn surfaces_can_become_children_and_be_promoted_again() {
    let alpha = root("alpha");
    let mut beta = root("beta");
    beta.children.push(object("beta-child", None));
    let mut app = app_with_surfaces(vec![alpha, beta]);

    assert!(app.apply_scene_drop(
        Selection::Surface(1),
        SceneDropTarget::Into(Selection::Surface(0)),
    ));
    assert_eq!(app.theme.surfaces.len(), 1);
    let beta_index = app.theme.surfaces[0]
        .children
        .iter()
        .position(|object| object.name == "beta")
        .unwrap();
    let beta_id = app.theme.surfaces[0].children[beta_index].id.clone();
    assert_eq!(app.theme.surfaces[0].children[beta_index].parent, None);
    assert!(app.theme.surfaces[0]
        .children
        .iter()
        .any(|object| object.name == "beta-child"
            && object.parent.as_deref() == Some(beta_id.as_str())));
    assert!(app.theme.validate().is_empty());

    assert!(app.apply_scene_drop(Selection::Object(0, beta_index), SceneDropTarget::RootAt(1),));
    assert_eq!(app.theme.surfaces.len(), 2);
    assert_eq!(app.theme.surfaces[1].name, "beta");
    assert!(app.theme.surfaces[1]
        .children
        .iter()
        .any(|object| object.name == "beta-child" && object.parent.is_none()));
    assert!(app.theme.validate().is_empty());
}

#[test]
fn scene_drop_reparents_reorders_and_rejects_cycles() {
    let mut surface = root("surface");
    surface.children = vec![object("a", None), object("b", None), object("c", None)];
    let mut app = app_with_surfaces(vec![surface]);

    assert!(app.apply_scene_drop(
        Selection::Object(0, 1),
        SceneDropTarget::Into(Selection::Object(0, 0)),
    ));
    let a = app.theme.surfaces[0]
        .children
        .iter()
        .position(|object| object.name == "a")
        .unwrap();
    let b = app.theme.surfaces[0]
        .children
        .iter()
        .position(|object| object.name == "b")
        .unwrap();
    assert_eq!(
        app.theme.surfaces[0].children[b].parent.as_deref(),
        Some("a")
    );
    assert!(!app.apply_scene_drop(
        Selection::Object(0, a),
        SceneDropTarget::Into(Selection::Object(0, b)),
    ));

    let c = app.theme.surfaces[0]
        .children
        .iter()
        .position(|object| object.name == "c")
        .unwrap();
    assert!(app.apply_scene_drop(
        Selection::Object(0, b),
        SceneDropTarget::After(Selection::Object(0, c)),
    ));
    let names: Vec<&str> = app.theme.surfaces[0]
        .children
        .iter()
        .map(|object| object.name.as_str())
        .collect();
    assert_eq!(names, vec!["a", "c", "b"]);
    assert!(app.theme.surfaces[0].children[2].parent.is_none());
    assert!(app.theme.validate().is_empty());
}

#[test]
fn duplicate_and_delete_include_the_selected_subtree() {
    let mut surface = root("surface");
    surface.children = vec![object("a", None), object("b", Some("a"))];
    let mut app = app_with_surfaces(vec![surface]);
    app.selection = Selection::Object(0, 0);

    assert!(app.duplicate_selection());
    assert_eq!(app.theme.surfaces[0].children.len(), 4);
    assert!(app.theme.validate().is_empty());
    assert!(app.delete_selection());
    assert_eq!(app.theme.surfaces[0].children.len(), 2);
    assert!(app.theme.validate().is_empty());
}

#[test]
fn scene_preview_colors_preserve_alpha_and_choose_readable_icons() {
    let mut light = Paint::new("#FFFFFFFF");
    light.opacity = 0.5.into();
    let light = scene_paint_color(&light);
    assert_eq!(light.a(), 128);
    assert_eq!(
        scene_icon_contrast_color(egui::Color32::WHITE),
        egui::Color32::BLACK
    );

    let dark = scene_paint_color(&Paint::new("#101214FF"));
    assert_eq!(scene_icon_contrast_color(dark), egui::Color32::WHITE);
    assert_eq!(
        scene_icon_contrast_color(egui::Color32::TRANSPARENT),
        egui::Color32::WHITE
    );

    let mut layout_object = object("layout", None);
    assert_eq!(
        scene_object_icon(&layout_object).unicode(),
        LucideIcon::SquareDashed.unicode()
    );
    layout_object.layout = ChildLayout::Row;
    assert_eq!(
        scene_object_icon(&layout_object).unicode(),
        LucideIcon::SquareDashedText.unicode()
    );
    layout_object.layout = ChildLayout::Column;
    assert_eq!(
        scene_object_icon(&layout_object).unicode(),
        LucideIcon::SquareDashedKanban.unicode()
    );

    let text_paint = Paint::new("#4A90E2FF");
    layout_object.content = SceneContent::Text {
        template: "Text".into(),
        font_family: "Segoe UI".into(),
        font_size: 16.0.into(),
        weight: FontWeight::Regular,
        rendering: FontRendering::Antialiased,
        contrast: 1.0.into(),
        align: TextAlign::Left,
        color: text_paint.clone(),
    };
    assert_eq!(
        scene_object_icon_color(&layout_object, egui::Color32::WHITE),
        scene_paint_color(&text_paint)
    );
}

#[test]
fn scene_preview_stretches_the_entire_layer_background() {
    let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(24.0, 24.0));
    let stretch = scene_preview_stretch(bounds, egui::vec2(250.0, 46.0));
    assert!((stretch.x - 0.096).abs() < 0.001);
    assert!((stretch.y - 24.0 / 46.0).abs() < 0.001);
    assert_ne!(stretch.x, stretch.y);

    let outline = scene_preview_outline(bounds, 23.0 * stretch.x, 23.0 * stretch.y);
    let outline_bounds = egui::Rect::from_points(&outline);
    assert!((outline_bounds.width() - bounds.width()).abs() < 0.001);
    assert!((outline_bounds.height() - bounds.height()).abs() < 0.001);

    let start = egui::Color32::GREEN;
    let end = egui::Color32::RED;
    assert_eq!(
        scene_preview_gradient_color(bounds.left_center(), bounds, start, end, 0.0),
        start
    );
    assert_eq!(
        scene_preview_gradient_color(bounds.right_center(), bounds, start, end, 0.0),
        end
    );
}
