use super::*;

pub(super) fn toggle_labeled_control(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut bool,
    language: LanguageId,
) -> bool {
    let mut changed = false;
    labeled(ui, label, |ui| {
        let width = inspector_control_width(ui);
        changed = ui
            .allocate_ui_with_layout(
                egui::vec2(width, CONTROL_HEIGHT),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    Toggle::new(value)
                        .labels(language.text("Enabled"), language.text("Disabled"))
                        .show(ui)
                },
            )
            .inner
            .changed();
    });
    changed
}

pub(super) fn template_has_expression(template: &str) -> bool {
    let mut remaining = template;
    while let Some(start) = remaining.find('{') {
        remaining = &remaining[start + 1..];
        if remaining.starts_with('{') {
            remaining = &remaining[1..];
            continue;
        }
        return remaining.contains('}');
    }
    false
}

pub(super) fn text_template_editor_control(
    ui: &mut egui::Ui,
    id: egui::Id,
    template: &mut String,
    context: &DataContext,
    width: f32,
) -> bool {
    let has_expression = template_has_expression(template);
    let preview = if has_expression {
        theme_engine::format_template(template, context)
    } else {
        template.clone()
    };
    let backup_id = id.with("pre-expression-value");
    let action = helper_preview_field(
        ui,
        id,
        &preview,
        width,
        has_expression,
        "text helper",
        egui::Align::Min,
    );
    if action.remove {
        *template = ui
            .data_mut(|data| data.remove_temp::<String>(backup_id))
            .unwrap_or(preview);
        return false;
    }
    if action.open && !has_expression {
        ui.data_mut(|data| data.insert_temp(backup_id, template.clone()));
    }
    action.open
}

pub(super) fn numeric_expression_control(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    value: &mut Expression,
    context: &DataContext,
) -> bool {
    let is_simple = value
        .0
        .trim()
        .parse::<f64>()
        .is_ok_and(|number| number.is_finite());
    let stored_expression_mode = ui.data_mut(|data| data.get_temp::<bool>(id));
    let expression_mode = stored_expression_mode.unwrap_or(!is_simple) || !is_simple;
    if expression_mode && stored_expression_mode != Some(true) {
        ui.data_mut(|data| data.insert_temp(id, true));
    }

    let backup_id = id.with("pre-expression-value");
    let mut edit_clicked = false;
    let preview = expression_mode.then(|| {
        theme_engine::evaluate(&value.0, context)
            .ok()
            .filter(|number| number.is_finite())
            .map(format_number_for_ui)
            .unwrap_or_else(|| "Invalid expression".into())
    });
    // Keep DragValue's live keyboard buffer tied to this exact property.
    ui.push_id(id, |ui| {
        labeled(ui, label, |ui| {
            let available_width = inspector_control_width(ui);
            let action = crate::ui::components::compound_field::expression_or_value(
                ui,
                id,
                available_width,
                preview.as_deref().map(|value| (value, egui::Align::Center)),
                "an expression",
                |ui, width| {
                    let mut number = value.0.trim().parse::<f64>().unwrap_or_default();
                    if NumberField::new(&mut number).show(ui, width).changed() {
                        value.0 = format_number_for_ui(number);
                    }
                },
            );
            match action {
                crate::ui::components::compound_field::CompoundFieldAction::Open => {
                    if !expression_mode {
                        ui.data_mut(|data| data.insert_temp(backup_id, value.0.clone()));
                        ui.data_mut(|data| data.insert_temp(id, true));
                    }
                    edit_clicked = true;
                }
                crate::ui::components::compound_field::CompoundFieldAction::Remove => {
                    value.0 = ui
                        .data_mut(|data| data.remove_temp::<String>(backup_id))
                        .unwrap_or_else(|| {
                            let resolved = theme_engine::evaluate(&value.0, context)
                                .ok()
                                .filter(|number| number.is_finite())
                                .unwrap_or_default();
                            format_number_for_ui(resolved)
                        });
                    ui.data_mut(|data| data.insert_temp(id, false));
                    edit_clicked = false;
                }
                crate::ui::components::compound_field::CompoundFieldAction::None => {}
            }
        });
    });
    edit_clicked
}

pub(super) fn render_controls(
    ui: &mut egui::Ui,
    id: egui::Id,
    render: &mut Expression,
    visibility: &mut Expression,
    context: &DataContext,
    language: LanguageId,
) -> Option<ExpressionField> {
    let edit_render = expression_control(
        ui,
        id.with("render"),
        language.text("Render"),
        render,
        ExpressionControlKind::Boolean,
        context,
        language,
    );
    let edit_visibility = expression_control(
        ui,
        id.with("visibility"),
        language.text("Visibility"),
        visibility,
        ExpressionControlKind::Percentage,
        context,
        language,
    );
    if edit_render {
        Some(ExpressionField::Render)
    } else if edit_visibility {
        Some(ExpressionField::Visibility)
    } else {
        None
    }
}

pub(super) fn expression_control(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    value: &mut Expression,
    kind: ExpressionControlKind,
    context: &DataContext,
    language: LanguageId,
) -> bool {
    let is_simple = match kind {
        ExpressionControlKind::Boolean => {
            matches!(
                value.0.trim().to_ascii_lowercase().as_str(),
                "0" | "1" | "false" | "true"
            )
        }
        ExpressionControlKind::Percentage => value
            .0
            .trim()
            .parse::<f32>()
            .is_ok_and(|number| number.is_finite() && (0.0..=100.0).contains(&number)),
    };
    let stored_expression_mode = ui.data_mut(|data| data.get_temp::<bool>(id));
    let expression_mode = stored_expression_mode.unwrap_or(!is_simple) || !is_simple;
    if expression_mode && stored_expression_mode != Some(true) {
        ui.data_mut(|data| data.insert_temp(id, true));
    }

    let backup_id = id.with("pre-expression-value");
    let mut edit_clicked = false;
    let preview = expression_mode.then(|| {
        theme_engine::evaluate(&value.0, context)
            .ok()
            .filter(|number| number.is_finite())
            .map(|number| match kind {
                ExpressionControlKind::Boolean => {
                    if number != 0.0 {
                        language.text("True").into()
                    } else {
                        language.text("False").into()
                    }
                }
                ExpressionControlKind::Percentage => {
                    format!("{}%", format_number_for_ui(number))
                }
            })
            .unwrap_or_else(|| language.text("Invalid expression").into())
    });
    ui.push_id(id, |ui| {
        labeled(ui, label, |ui| {
            // Measure from the actual pane clip edge rather than the row's desired
            // width. Reserve the Inspector's overlaid scrollbar gutter plus a small
            // DPI safety margin so the trailing icon and its border remain visible.
            let available_width = inspector_control_width(ui);
            let action = crate::ui::components::compound_field::expression_or_value(
                ui,
                id,
                available_width,
                preview.as_deref().map(|value| {
                    (
                        value,
                        if matches!(kind, ExpressionControlKind::Percentage) {
                            egui::Align::Center
                        } else {
                            egui::Align::Min
                        },
                    )
                }),
                language.text("an expression"),
                |ui, width| match kind {
                    ExpressionControlKind::Boolean => {
                        let mut enabled =
                            matches!(value.0.trim().to_ascii_lowercase().as_str(), "1" | "true");
                        let before = enabled;
                        Dropdown::from_id_salt(id.with("boolean"))
                            .width(width)
                            .selected_text(if enabled {
                                language.text("True")
                            } else {
                                language.text("False")
                            })
                            .show_ui(ui, |ui| {
                                dropdown_selectable_value(
                                    ui,
                                    &mut enabled,
                                    true,
                                    language.text("True"),
                                );
                                dropdown_selectable_value(
                                    ui,
                                    &mut enabled,
                                    false,
                                    language.text("False"),
                                );
                            });
                        if enabled != before {
                            value.0 = if enabled { "true" } else { "false" }.into();
                        }
                    }
                    ExpressionControlKind::Percentage => {
                        let mut percentage = value.0.trim().parse::<f32>().unwrap_or(100.0);
                        if percentage_slider(ui, &mut percentage, width).changed() {
                            value.0 = format!("{percentage:.0}");
                        }
                    }
                },
            );
            match action {
                crate::ui::components::compound_field::CompoundFieldAction::Open => {
                    if !expression_mode {
                        ui.data_mut(|data| data.insert_temp(backup_id, value.0.clone()));
                        ui.data_mut(|data| data.insert_temp(id, true));
                    }
                    edit_clicked = true;
                }
                crate::ui::components::compound_field::CompoundFieldAction::Remove => {
                    value.0 = ui
                        .data_mut(|data| data.remove_temp::<String>(backup_id))
                        .unwrap_or_else(|| match kind {
                            ExpressionControlKind::Boolean => "true".into(),
                            ExpressionControlKind::Percentage => "100".into(),
                        });
                    ui.data_mut(|data| data.insert_temp(id, false));
                    edit_clicked = false;
                }
                crate::ui::components::compound_field::CompoundFieldAction::None => {}
            }
        });
    });
    edit_clicked
}

pub(super) fn placement_offset_expression_control(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    value: &mut i32,
    expression: &mut Option<Expression>,
    context: &DataContext,
) -> bool {
    let mut edit_clicked = false;
    let preview = expression.as_ref().map(|formula| {
        theme_engine::evaluate(&formula.0, context)
            .ok()
            .filter(|number| number.is_finite())
            .map(format_number_for_ui)
            .unwrap_or_else(|| "Invalid expression".into())
    });
    ui.push_id(id, |ui| {
        labeled(ui, label, |ui| {
            let available_width = inspector_control_width(ui);
            match crate::ui::components::compound_field::expression_or_value(
                ui,
                id,
                available_width,
                preview.as_deref().map(|value| (value, egui::Align::Center)),
                "an expression",
                |ui, width| {
                    NumberField::new(value).show(ui, width);
                },
            ) {
                crate::ui::components::compound_field::CompoundFieldAction::Open => {
                    if expression.is_none() {
                        *expression = Some(Expression(value.to_string()));
                    }
                    edit_clicked = true;
                }
                crate::ui::components::compound_field::CompoundFieldAction::Remove => {
                    *expression = None;
                }
                crate::ui::components::compound_field::CompoundFieldAction::None => {}
            }
        });
    });
    edit_clicked
}

pub(super) fn segment_count_expression_control(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    value: &mut u16,
    expression: &mut Option<Expression>,
    context: &DataContext,
) -> bool {
    let mut edit_clicked = false;
    let preview = expression.as_ref().map(|formula| {
        theme_engine::evaluate(&formula.0, context)
            .ok()
            .filter(|number| number.is_finite())
            .map(|number| format!("{number:.0}"))
            .unwrap_or_else(|| "Invalid expression".into())
    });
    ui.push_id(id, |ui| {
        labeled(ui, label, |ui| {
            let available_width = inspector_control_width(ui);
            match crate::ui::components::compound_field::expression_or_value(
                ui,
                id,
                available_width,
                preview.as_deref().map(|value| (value, egui::Align::Center)),
                "an expression",
                |ui, width| {
                    NumberField::new(value).range(0..=100).show(ui, width);
                },
            ) {
                crate::ui::components::compound_field::CompoundFieldAction::Open => {
                    if expression.is_none() {
                        *expression = Some(Expression(value.to_string()));
                    }
                    edit_clicked = true;
                }
                crate::ui::components::compound_field::CompoundFieldAction::Remove => {
                    *expression = None;
                }
                crate::ui::components::compound_field::CompoundFieldAction::None => {}
            }
        });
    });
    edit_clicked
}

pub(super) fn paint_control(ui: &mut egui::Ui, label: &str, paint: &mut Paint) {
    labeled(ui, label, |ui| {
        let width = inspector_control_width(ui);
        crate::ui::components::color_picker::color_string_field(ui, &mut paint.color, width);
    });
}

pub(super) fn asset_grid(
    ui: &mut egui::Ui,
    assets: &[theme_engine::ManagedAsset],
    thumbnails: &HashMap<String, egui::TextureHandle>,
    filter: &str,
    selected_path: &mut Option<String>,
    theme: &ThemeDocument,
    language: LanguageId,
) -> Option<String> {
    let needle = filter.trim().to_ascii_lowercase();
    let visible = assets
        .iter()
        .filter(|asset| needle.is_empty() || asset.name.to_ascii_lowercase().contains(&needle))
        .collect::<Vec<_>>();
    if visible.is_empty() {
        ui.label(if assets.is_empty() {
            language.text("No images have been added yet. Use Add image to create the library.")
        } else {
            language.text("No assets match this search.")
        });
        return None;
    }

    let mut activated = None;
    ui.horizontal_wrapped(|ui| {
        for asset in visible {
            let selected = selected_path.as_deref() == Some(asset.relative_path.as_str());
            let response = asset_card(
                ui,
                asset,
                thumbnails.get(&asset.relative_path),
                selected,
                theme_engine::theme_asset_usage(theme, &asset.relative_path),
                language,
            );
            if response.clicked() {
                *selected_path = Some(asset.relative_path.clone());
            }
            if response.double_clicked() {
                *selected_path = Some(asset.relative_path.clone());
                activated = Some(asset.relative_path.clone());
            }
        }
    });
    activated
}

pub(super) fn asset_card(
    ui: &mut egui::Ui,
    asset: &theme_engine::ManagedAsset,
    texture: Option<&egui::TextureHandle>,
    selected: bool,
    usage: usize,
    language: LanguageId,
) -> egui::Response {
    let details = if usage == 0 {
        format!(
            "{} × {}  •  {}",
            asset.width,
            asset.height,
            format_asset_size(asset.bytes)
        )
    } else {
        format!(
            "{} × {}  •  {} {}×",
            asset.width,
            asset.height,
            language.text("Used"),
            usage
        )
    };
    crate::ui::components::asset::asset_card(
        ui,
        &asset.name,
        &details,
        &asset.relative_path,
        texture,
        selected,
    )
}

pub(super) fn format_asset_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
pub(super) fn anchor_point_picker(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    horizontal: &mut HorizontalAnchor,
    vertical: &mut VerticalAnchor,
) {
    ui.push_id(id, |ui| {
        let mut selected = AnchorPoint::new(
            match horizontal {
                HorizontalAnchor::Left => 0,
                HorizontalAnchor::Center => 1,
                HorizontalAnchor::Right => 2,
            },
            match vertical {
                VerticalAnchor::Top => 0,
                VerticalAnchor::Center => 1,
                VerticalAnchor::Bottom => 2,
            },
        );
        if AnchorPointPicker::new(&mut selected)
            .width(inspector_control_width(ui))
            .show(ui)
            .changed()
        {
            *horizontal = [
                HorizontalAnchor::Left,
                HorizontalAnchor::Center,
                HorizontalAnchor::Right,
            ][selected.column];
            *vertical = [
                VerticalAnchor::Top,
                VerticalAnchor::Center,
                VerticalAnchor::Bottom,
            ][selected.row];
        }
    });
}

pub(super) fn object_anchor_picker(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    anchor: &mut ObjectAnchor,
) {
    ui.push_id(id, |ui| {
        let mut selected = AnchorPoint::new(
            match anchor.horizontal {
                ObjectHorizontalAnchor::Left => 0,
                ObjectHorizontalAnchor::Center => 1,
                ObjectHorizontalAnchor::Right => 2,
            },
            match anchor.vertical {
                ObjectVerticalAnchor::Top => 0,
                ObjectVerticalAnchor::Center => 1,
                ObjectVerticalAnchor::Bottom => 2,
            },
        );
        if AnchorPointPicker::new(&mut selected)
            .width(inspector_control_width(ui))
            .show(ui)
            .changed()
        {
            anchor.horizontal = [
                ObjectHorizontalAnchor::Left,
                ObjectHorizontalAnchor::Center,
                ObjectHorizontalAnchor::Right,
            ][selected.column];
            anchor.vertical = [
                ObjectVerticalAnchor::Top,
                ObjectVerticalAnchor::Center,
                ObjectVerticalAnchor::Bottom,
            ][selected.row];
        }
    });
}
pub(super) fn appearance_inspector(
    ui: &mut egui::Ui,
    id: egui::Id,
    context: &DataContext,
    object: &mut SceneObject,
    asset_requested: &mut bool,
    language: LanguageId,
) -> Option<ExpressionField> {
    let mut requested_expression = None;
    let background_labels = [
        language.text("None"),
        language.text("Solid colour"),
        language.text("Gradient"),
        language.text("Image"),
    ];
    let mut background_kind = match &object.background {
        LayerBackground::None => 0,
        LayerBackground::Colour { .. } => 1,
        LayerBackground::Gradient { .. } => 2,
        LayerBackground::Image { .. } => 3,
    };
    let previous_background_kind = background_kind;
    labeled(ui, language.text("Background"), |ui| {
        Dropdown::from_id_salt(id.with("background-type"))
            .width(inspector_control_width(ui))
            .selected_text(background_labels[background_kind])
            .show_ui(ui, |ui| {
                for (value, label) in background_labels.into_iter().enumerate() {
                    dropdown_selectable_value(ui, &mut background_kind, value, label);
                }
            });
    });
    if background_kind != previous_background_kind {
        object.background = match background_kind {
            1 => LayerBackground::Colour {
                colour: Paint::new("#FFFFFFFF"),
            },
            2 => LayerBackground::Gradient {
                start: Paint::new("#FFFFFFFF"),
                end: Paint::new("#000000FF"),
                angle: 0.0.into(),
            },
            3 => LayerBackground::Image {
                path: String::new(),
                fit: ImageFit::Contain,
            },
            _ => LayerBackground::None,
        };
    }
    match &mut object.background {
        LayerBackground::None => {}
        LayerBackground::Colour { colour } => paint_control(ui, language.text("Colour"), colour),
        LayerBackground::Gradient { start, end, angle } => {
            paint_control(ui, language.text("Start"), start);
            paint_control(ui, language.text("End"), end);
            if numeric_expression_control(
                ui,
                id.with("background-gradient-angle"),
                language.text("Angle"),
                angle,
                context,
            ) {
                requested_expression = Some(ExpressionField::BackgroundGradientAngle);
            }
        }
        LayerBackground::Image { path, fit } => {
            labeled(ui, language.text("Asset"), |ui| {
                let label = Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.is_empty())
                    .unwrap_or(language.text("Choose asset..."));
                let hover_text = if path.is_empty() {
                    language.text("Open the asset library").to_owned()
                } else {
                    format!("{label}\n{}", language.text("Open the asset library"))
                };
                let width = inspector_control_width(ui);
                if leading_icon_button(ui, LucideIcon::Image, label, width)
                    .on_hover_text(hover_text)
                    .clicked()
                {
                    *asset_requested = true;
                }
            });
            labeled(ui, language.text("Fit"), |ui| {
                Dropdown::from_id_salt(id.with("background-image-fit"))
                    .width(inspector_control_width(ui))
                    .selected_text(image_fit_name(language, *fit))
                    .show_ui(ui, |ui| {
                        for value in [
                            ImageFit::Contain,
                            ImageFit::Cover,
                            ImageFit::Stretch,
                            ImageFit::Original,
                        ] {
                            dropdown_selectable_value(
                                ui,
                                fit,
                                value,
                                image_fit_name(language, value),
                            );
                        }
                    });
            });
        }
    }
    let mut border_enabled = object.border.is_some();
    if toggle_labeled_control(ui, language.text("Border"), &mut border_enabled, language) {
        object.border = border_enabled.then(|| theme_engine::Stroke {
            color: Paint::new("#FFFFFFFF"),
            width: 1.0.into(),
        });
    }
    if let Some(border) = &mut object.border {
        paint_control(ui, language.text("Border colour"), &mut border.color);
        if numeric_expression_control(
            ui,
            id.with("border-width"),
            language.text("Border width"),
            &mut border.width,
            context,
        ) {
            requested_expression = Some(ExpressionField::ObjectBorderWidth);
        }
    }
    if numeric_expression_control(
        ui,
        id.with("object-corner-radius"),
        language.text("Corner radius"),
        &mut object.corner_radius,
        context,
    ) {
        requested_expression = Some(ExpressionField::ObjectCornerRadius);
    }
    requested_expression
}

pub(super) fn mouse_events_inspector(
    ui: &mut egui::Ui,
    id: egui::Id,
    object: &mut SceneObject,
    language: LanguageId,
) -> Option<MouseEventField> {
    let mut enabled = object.mouse_events.is_some();
    if toggle_labeled_control(ui, language.text("Mouse events"), &mut enabled, language) {
        object.mouse_events = enabled.then(MouseEvents::default);
    }
    let Some(events) = &mut object.mouse_events else {
        return None;
    };
    let mut requested = None;
    for (field, label) in [
        (MouseEventField::Click, language.text("Click")),
        (MouseEventField::DoubleClick, language.text("Double click")),
        (MouseEventField::RightClick, language.text("Right click")),
        (MouseEventField::MouseEnter, language.text("Mouse enter")),
        (MouseEventField::MouseLeave, language.text("Mouse leave")),
    ] {
        let value = events.handler_mut(field.kind());
        labeled(ui, label, |ui| {
            let preview = value.lines().next().unwrap_or_default();
            let action = helper_preview_field(
                ui,
                id.with(label),
                preview,
                inspector_control_width(ui),
                !value.trim().is_empty(),
                language.text("action helper"),
                egui::Align::Min,
            );
            if action.remove {
                value.clear();
            }
            if action.open {
                requested = Some(field);
            }
        });
    }
    requested
}

pub(super) fn layer_properties_inspector(
    ui: &mut egui::Ui,
    id: egui::Id,
    context: &DataContext,
    object: &mut SceneObject,
    language: LanguageId,
) -> Option<LayerInspectorRequest> {
    let mut requested = None;
    let content_labels = [
        language.text("None (Container)"),
        language.text("Text"),
        language.text("Data bar"),
    ];
    let mut kind = match object.content {
        SceneContent::None => 0,
        SceneContent::Text { .. } => 1,
        SceneContent::Progress { .. } => 2,
    };
    let previous_kind = kind;
    labeled(ui, language.text("Content type"), |ui| {
        Dropdown::from_id_salt(id.with("content-type"))
            .width(inspector_control_width(ui))
            .selected_text(content_labels[kind])
            .show_ui(ui, |ui| {
                for (value, label) in content_labels.into_iter().enumerate() {
                    dropdown_selectable_value(ui, &mut kind, value, label);
                }
            });
    });
    if kind != previous_kind {
        object.content = match kind {
            1 => SceneContent::Text {
                template: "Text".into(),
                font_family: "Segoe UI Variable Text".into(),
                font_size: 16.0.into(),
                weight: FontWeight::Regular,
                rendering: FontRendering::Antialiased,
                contrast: 1.4.into(),
                align: TextAlign::Left,
                color: Paint::new("#FFFFFFFF"),
            },
            2 => SceneContent::Progress {
                value: Expression("claude.session.percentage".into()),
                direction: ProgressDirection::LeftToRight,
                fill: Paint::new("#D97757FF"),
                track: Paint::new("#FFFFFF24"),
                corner_radius: 6.0.into(),
                segments: 0,
                segments_expression: None,
                segment_gap: 2.0.into(),
            },
            _ => SceneContent::None,
        };
    }
    if let Some(content_request) = content_inspector(
        ui,
        id.with("content"),
        context,
        &mut object.content,
        language,
    ) {
        requested = Some(content_request);
    }
    labeled(ui, language.text("Children layout"), |ui| {
        Dropdown::from_id_salt(id.with("child-layout"))
            .width(inspector_control_width(ui))
            .selected_text(child_layout_name(language, object.layout))
            .show_ui(ui, |ui| {
                for value in [ChildLayout::Freeform, ChildLayout::Row, ChildLayout::Column] {
                    dropdown_selectable_value(
                        ui,
                        &mut object.layout,
                        value,
                        child_layout_name(language, value),
                    );
                }
            });
    });
    if object.layout != ChildLayout::Freeform {
        labeled(ui, language.text("Cross-axis"), |ui| {
            Dropdown::from_id_salt(id.with("child-alignment"))
                .width(inspector_control_width(ui))
                .selected_text(child_alignment_name(language, object.align))
                .show_ui(ui, |ui| {
                    for value in [
                        ChildAlignment::Start,
                        ChildAlignment::Center,
                        ChildAlignment::End,
                    ] {
                        dropdown_selectable_value(
                            ui,
                            &mut object.align,
                            value,
                            child_alignment_name(language, value),
                        );
                    }
                });
        });
        if numeric_expression_control(
            ui,
            id.with("gap"),
            language.text("Gap"),
            &mut object.gap,
            context,
        ) {
            requested = Some(LayerInspectorRequest::Expression(ExpressionField::ChildGap));
        }
    }
    requested
}

pub(super) fn content_inspector(
    ui: &mut egui::Ui,
    id: egui::Id,
    context: &DataContext,
    content: &mut SceneContent,
    language: LanguageId,
) -> Option<LayerInspectorRequest> {
    let mut requested = None;
    match content {
        SceneContent::None => {}
        SceneContent::Text {
            template,
            font_family,
            font_size,
            weight,
            rendering,
            contrast,
            align,
            color,
        } => {
            labeled(ui, language.text("Text"), |ui| {
                let width = inspector_control_width(ui);
                if text_template_editor_control(
                    ui,
                    id.with("text-template"),
                    template,
                    context,
                    width,
                ) {
                    requested = Some(LayerInspectorRequest::TextTemplate);
                }
            });
            labeled(ui, language.text("Font"), |ui| {
                searchable_dropdown(
                    ui,
                    id.with("font-family"),
                    font_family,
                    installed_font_families(),
                    inspector_control_width(ui),
                    language.text("Type to filter fonts"),
                    language.text("No matching fonts"),
                );
            });
            if numeric_expression_control(
                ui,
                id.with("font-size"),
                language.text("Size"),
                font_size,
                context,
            ) {
                requested = Some(LayerInspectorRequest::Expression(
                    ExpressionField::TextFontSize,
                ));
            }
            labeled(ui, language.text("Weight"), |ui| {
                Dropdown::from_id_salt("weight")
                    .width(inspector_control_width(ui))
                    .selected_text(font_weight_name(language, *weight))
                    .show_ui(ui, |ui| {
                        for v in [
                            FontWeight::Light,
                            FontWeight::Regular,
                            FontWeight::Medium,
                            FontWeight::Semibold,
                            FontWeight::Bold,
                        ] {
                            dropdown_selectable_value(ui, weight, v, font_weight_name(language, v));
                        }
                    });
            });
            labeled(ui, language.text("Rendering"), |ui| {
                Dropdown::from_id_salt("font_rendering")
                    .width(inspector_control_width(ui))
                    .selected_text(font_rendering_name(language, *rendering))
                    .show_ui(ui, |ui| {
                        for value in [
                            FontRendering::Antialiased,
                            FontRendering::ClearType,
                            FontRendering::Aliased,
                        ] {
                            dropdown_selectable_value(
                                ui,
                                rendering,
                                value,
                                font_rendering_name(language, value),
                            );
                        }
                    });
            });
            if numeric_expression_control(
                ui,
                id.with("font-contrast"),
                language.text("Edge contrast"),
                contrast,
                context,
            ) {
                requested = Some(LayerInspectorRequest::Expression(
                    ExpressionField::TextFontContrast,
                ));
            }
            labeled(ui, language.text("Align"), |ui| {
                Dropdown::from_id_salt("text_align")
                    .width(inspector_control_width(ui))
                    .selected_text(text_align_name(language, *align))
                    .show_ui(ui, |ui| {
                        for v in [TextAlign::Left, TextAlign::Center, TextAlign::Right] {
                            dropdown_selectable_value(ui, align, v, text_align_name(language, v));
                        }
                    });
            });
            paint_control(ui, language.text("Colour"), color);
        }
        SceneContent::Progress {
            value,
            direction,
            fill,
            track,
            corner_radius,
            segments,
            segments_expression,
            segment_gap,
        } => {
            if numeric_expression_control(
                ui,
                id.with("value"),
                language.text("Value"),
                value,
                context,
            ) {
                requested = Some(LayerInspectorRequest::Expression(
                    ExpressionField::ProgressValue,
                ));
            }
            labeled(ui, language.text("Direction"), |ui| {
                Dropdown::from_id_salt("progress_direction")
                    .width(inspector_control_width(ui))
                    .selected_text(progress_direction_name(language, *direction))
                    .show_ui(ui, |ui| {
                        for v in [
                            ProgressDirection::LeftToRight,
                            ProgressDirection::RightToLeft,
                            ProgressDirection::BottomToTop,
                            ProgressDirection::TopToBottom,
                        ] {
                            dropdown_selectable_value(
                                ui,
                                direction,
                                v,
                                progress_direction_name(language, v),
                            );
                        }
                    });
            });
            paint_control(ui, language.text("Fill"), fill);
            paint_control(ui, language.text("Track"), track);
            if numeric_expression_control(
                ui,
                id.with("radius"),
                language.text("Radius"),
                corner_radius,
                context,
            ) {
                requested = Some(LayerInspectorRequest::Expression(
                    ExpressionField::ProgressCornerRadius,
                ));
            }
            if segment_count_expression_control(
                ui,
                id.with("segments"),
                language.text("Segments"),
                segments,
                segments_expression,
                context,
            ) {
                requested = Some(LayerInspectorRequest::Expression(
                    ExpressionField::ProgressSegments,
                ));
            }
            if numeric_expression_control(
                ui,
                id.with("segment-gap"),
                language.text("Segment gap"),
                segment_gap,
                context,
            ) {
                requested = Some(LayerInspectorRequest::Expression(
                    ExpressionField::ProgressSegmentGap,
                ));
            }
        }
    }
    requested
}
pub(super) fn image_fit_name(language: LanguageId, value: ImageFit) -> &'static str {
    match value {
        ImageFit::Contain => language.text("Contain"),
        ImageFit::Cover => language.text("Cover"),
        ImageFit::Stretch => language.text("Stretch"),
        ImageFit::Original => language.text("Original"),
    }
}

pub(super) fn child_layout_name(language: LanguageId, value: ChildLayout) -> &'static str {
    match value {
        ChildLayout::Freeform => language.text("Freeform"),
        ChildLayout::Row => language.text("Row"),
        ChildLayout::Column => language.text("Column"),
    }
}

pub(super) fn child_alignment_name(language: LanguageId, value: ChildAlignment) -> &'static str {
    match value {
        ChildAlignment::Start => language.text("Start"),
        ChildAlignment::Center => language.text("Center"),
        ChildAlignment::End => language.text("End"),
    }
}

pub(super) fn font_weight_name(language: LanguageId, value: FontWeight) -> &'static str {
    match value {
        FontWeight::Light => language.text("Light"),
        FontWeight::Regular => language.text("Regular"),
        FontWeight::Medium => language.text("Medium"),
        FontWeight::Semibold => language.text("Semibold"),
        FontWeight::Bold => language.text("Bold"),
    }
}

pub(super) fn font_rendering_name(language: LanguageId, value: FontRendering) -> &'static str {
    match value {
        FontRendering::Antialiased => language.text("Antialiased"),
        FontRendering::ClearType => "ClearType",
        FontRendering::Aliased => language.text("Aliased"),
    }
}

pub(super) fn text_align_name(language: LanguageId, value: TextAlign) -> &'static str {
    match value {
        TextAlign::Left => language.text("Left"),
        TextAlign::Center => language.text("Center"),
        TextAlign::Right => language.text("Right"),
    }
}

pub(super) fn progress_direction_name(
    language: LanguageId,
    value: ProgressDirection,
) -> &'static str {
    match value {
        ProgressDirection::LeftToRight => language.text("Left to right"),
        ProgressDirection::RightToLeft => language.text("Right to left"),
        ProgressDirection::BottomToTop => language.text("Bottom to top"),
        ProgressDirection::TopToBottom => language.text("Top to bottom"),
    }
}

pub(super) fn reference_target_name(language: LanguageId, target: ReferenceTarget) -> String {
    let region = match target.region {
        ReferenceRegion::Monitor => language.text("Monitor"),
        ReferenceRegion::Taskbar => language.text("Taskbar"),
        ReferenceRegion::SystemTray => language.text("System Tray"),
    };
    format!(
        "{region} ({} {})",
        language.text("Display"),
        target.display + 1
    )
}
pub(super) fn surface_nest_name(language: LanguageId, nest: SurfaceNest) -> &'static str {
    match nest {
        SurfaceNest::Auto => language.text("Automatic"),
        SurfaceNest::Taskbar => language.text("Taskbar"),
        SurfaceNest::TrayIcon => language.text("Tray Icon"),
        SurfaceNest::Desktop => language.text("Desktop"),
        SurfaceNest::Floating => language.text("Floating"),
    }
}
pub(super) fn interval_name(language: LanguageId, value: u32) -> &'static str {
    match value {
        POLL_1_MIN => language.text("Every minute"),
        POLL_5_MIN => language.text("Every 5 minutes"),
        POLL_15_MIN => language.text("Every 15 minutes"),
        POLL_1_HOUR => language.text("Every hour"),
        _ => language.text("Custom"),
    }
}
