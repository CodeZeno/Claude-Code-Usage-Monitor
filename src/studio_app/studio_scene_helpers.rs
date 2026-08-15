use super::*;

pub(super) fn scene_row_contents(
    ui: &mut egui::Ui,
    object: &mut SceneObject,
    selection: Selection,
    preview_size: egui::Vec2,
    expanded: Option<bool>,
    editable: bool,
    language: LanguageId,
) -> SceneRowResponses {
    ui.horizontal(|ui| {
        let (leading_item, expand_button) = if let Some(expanded) = expanded {
            let expand_button = scene_tree_chevron(ui, expanded)
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(language.text("Expand or collapse"));
            let preview = scene_object_preview(ui, object, preview_size)
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(language.text("Select object"));
            (expand_button.clone() | preview, expand_button)
        } else {
            let preview = scene_object_preview(ui, object, preview_size)
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(language.text("Select object"));
            (preview.clone(), preview)
        };
        let row_width = ui.available_width();
        let (name, name_changed, drag_handle) = ui
            .allocate_ui_with_layout(
                egui::vec2(row_width, CONTROL_HEIGHT),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    let drag_handle =
                        crate::ui::components::tree_row::drag_handle(ui, editable, language);
                    if editable {
                        drag_handle.dnd_set_drag_payload(selection);
                    }
                    let label_width = ui.available_width().max(1.0);
                    let (name, name_changed) =
                        scene_name_editor(ui, object, selection, label_width, editable, language);
                    (name, name_changed, drag_handle)
                },
            )
            .inner;
        SceneRowResponses {
            item: leading_item | name.clone() | drag_handle.clone(),
            expand_button,
            drag_handle,
            name_changed,
        }
    })
    .inner
}

pub(super) fn inspector_heading(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(16.0)
            .strong()
            .color(egui::Color32::WHITE),
    );
}

pub(super) fn scene_tree_chevron(ui: &mut egui::Ui, expanded: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(27.0, CONTROL_HEIGHT), egui::Sense::click());
    crate::ui::components::icon::paint_centered_icon(
        ui,
        rect,
        if expanded {
            LucideIcon::ChevronDown
        } else {
            LucideIcon::ChevronRight
        },
        16.0,
        ui.style().interact(&response).text_color(),
    );
    response
}

pub(super) fn scene_name_editor(
    ui: &mut egui::Ui,
    object: &mut SceneObject,
    selection: Selection,
    width: f32,
    editable: bool,
    language: LanguageId,
) -> (egui::Response, bool) {
    let edit_id = ui.make_persistent_id(("scene-name-edit", selection));
    crate::ui::components::text_field::inline_rename(
        ui,
        &mut object.name,
        edit_id,
        width,
        editable,
        language,
    )
}

pub(super) fn scene_object_preview(
    ui: &mut egui::Ui,
    object: &SceneObject,
    source_size: egui::Vec2,
) -> egui::Response {
    let (allocated_rect, response) =
        ui.allocate_exact_size(egui::vec2(27.0, 24.0), egui::Sense::click());
    let preview_bounds = egui::Rect::from_min_size(
        egui::pos2(allocated_rect.left() + 3.0, allocated_rect.top()),
        egui::vec2(24.0, 24.0),
    );
    let stretch = scene_preview_stretch(preview_bounds, source_size);
    let source_radius = object
        .corner_radius
        .0
        .trim()
        .parse::<f32>()
        .unwrap_or(3.0)
        .clamp(0.0, source_size.x.min(source_size.y).max(0.0) / 2.0);
    let outline = scene_preview_outline(
        preview_bounds,
        source_radius * stretch.x,
        source_radius * stretch.y,
    );
    let background = match &object.background {
        LayerBackground::Colour { colour } => {
            let colour = scene_paint_color(colour);
            ui.painter().add(egui::Shape::convex_polygon(
                outline.clone(),
                colour,
                egui::Stroke::NONE,
            ));
            colour
        }
        LayerBackground::Gradient { start, end, angle } => {
            let start = scene_paint_color(start);
            let end = scene_paint_color(end);
            let angle = angle.0.trim().parse::<f32>().unwrap_or_default();
            paint_scene_preview_gradient(ui, preview_bounds, &outline, start, end, angle);
            lerp_scene_color(start, end, 0.5)
        }
        LayerBackground::None | LayerBackground::Image { .. } => egui::Color32::TRANSPARENT,
    };

    if let Some(border) = &object.border {
        let width =
            border.width.0.trim().parse::<f32>().unwrap_or(1.0) * ((stretch.x + stretch.y) * 0.5);
        let width = width.clamp(0.5, 3.0);
        ui.painter().add(egui::Shape::closed_line(
            outline,
            egui::Stroke::new(width, scene_paint_color(&border.color)),
        ));
    }

    let icon = scene_object_icon(object);
    ui.painter().text(
        preview_bounds.center(),
        egui::Align2::CENTER_CENTER,
        icon.unicode().to_string(),
        egui::FontId::new(16.0, egui::FontFamily::Name("lucide".into())),
        scene_object_icon_color(object, background),
    );
    response
}

pub(super) fn scene_preview_stretch(bounds: egui::Rect, source_size: egui::Vec2) -> egui::Vec2 {
    if !source_size.x.is_finite()
        || !source_size.y.is_finite()
        || source_size.x <= 0.0
        || source_size.y <= 0.0
    {
        return egui::Vec2::ONE;
    }
    egui::vec2(
        bounds.width() / source_size.x,
        bounds.height() / source_size.y,
    )
}

pub(super) fn scene_preview_outline(
    bounds: egui::Rect,
    radius_x: f32,
    radius_y: f32,
) -> Vec<egui::Pos2> {
    let radius_x = radius_x.clamp(0.0, bounds.width() / 2.0);
    let radius_y = radius_y.clamp(0.0, bounds.height() / 2.0);
    if radius_x < 0.01 || radius_y < 0.01 {
        return vec![
            bounds.left_top(),
            bounds.right_top(),
            bounds.right_bottom(),
            bounds.left_bottom(),
        ];
    }

    const ARC_STEPS: usize = 5;
    let corners = [
        (
            egui::pos2(bounds.left() + radius_x, bounds.top() + radius_y),
            std::f32::consts::PI,
        ),
        (
            egui::pos2(bounds.right() - radius_x, bounds.top() + radius_y),
            -std::f32::consts::FRAC_PI_2,
        ),
        (
            egui::pos2(bounds.right() - radius_x, bounds.bottom() - radius_y),
            0.0,
        ),
        (
            egui::pos2(bounds.left() + radius_x, bounds.bottom() - radius_y),
            std::f32::consts::FRAC_PI_2,
        ),
    ];
    let mut outline = Vec::with_capacity(corners.len() * (ARC_STEPS + 1));
    for (center, start_angle) in corners {
        for step in 0..=ARC_STEPS {
            let angle = start_angle + std::f32::consts::FRAC_PI_2 * step as f32 / ARC_STEPS as f32;
            outline.push(egui::pos2(
                center.x + angle.cos() * radius_x,
                center.y + angle.sin() * radius_y,
            ));
        }
    }
    outline
}

pub(super) fn lerp_scene_color(
    start: egui::Color32,
    end: egui::Color32,
    amount: f32,
) -> egui::Color32 {
    let amount = amount.clamp(0.0, 1.0);
    let channel =
        |start: u8, end: u8| (start as f32 + (end as f32 - start as f32) * amount).round() as u8;
    egui::Color32::from_rgba_unmultiplied(
        channel(start.r(), end.r()),
        channel(start.g(), end.g()),
        channel(start.b(), end.b()),
        channel(start.a(), end.a()),
    )
}

pub(super) fn scene_preview_gradient_color(
    position: egui::Pos2,
    bounds: egui::Rect,
    start: egui::Color32,
    end: egui::Color32,
    angle_degrees: f32,
) -> egui::Color32 {
    let radians = angle_degrees.to_radians();
    let direction = egui::vec2(radians.cos(), radians.sin());
    let half_extent =
        (direction.x.abs() * bounds.width() + direction.y.abs() * bounds.height()) * 0.5;
    let amount = if half_extent > 0.0 {
        0.5 + (position - bounds.center()).dot(direction) / (2.0 * half_extent)
    } else {
        0.5
    };
    lerp_scene_color(start, end, amount)
}

pub(super) fn paint_scene_preview_gradient(
    ui: &egui::Ui,
    bounds: egui::Rect,
    outline: &[egui::Pos2],
    start: egui::Color32,
    end: egui::Color32,
    angle_degrees: f32,
) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(
        bounds.center(),
        scene_preview_gradient_color(bounds.center(), bounds, start, end, angle_degrees),
    );
    for &position in outline {
        mesh.colored_vertex(
            position,
            scene_preview_gradient_color(position, bounds, start, end, angle_degrees),
        );
    }
    for index in 0..outline.len() {
        mesh.add_triangle(
            0,
            index as u32 + 1,
            ((index + 1) % outline.len()) as u32 + 1,
        );
    }
    ui.painter().add(egui::Shape::mesh(mesh));
}

pub(super) fn scene_object_icon(object: &SceneObject) -> LucideIcon {
    match &object.content {
        SceneContent::Text { .. } => LucideIcon::Type,
        SceneContent::Progress { .. } => LucideIcon::Gauge,
        SceneContent::None => {
            if matches!(&object.background, LayerBackground::Image { .. }) {
                LucideIcon::Image
            } else {
                match object.layout {
                    ChildLayout::Freeform => LucideIcon::SquareDashed,
                    ChildLayout::Row => LucideIcon::SquareDashedText,
                    ChildLayout::Column => LucideIcon::SquareDashedKanban,
                }
            }
        }
    }
}

pub(super) fn scene_object_icon_color(
    object: &SceneObject,
    background: egui::Color32,
) -> egui::Color32 {
    match &object.content {
        SceneContent::Text { color, .. } => scene_paint_color(color),
        SceneContent::None | SceneContent::Progress { .. } => scene_icon_contrast_color(background),
    }
}

pub(super) fn scene_paint_color(paint: &Paint) -> egui::Color32 {
    let Some(color) = theme_engine::parse_color(&paint.color) else {
        return egui::Color32::TRANSPARENT;
    };
    let opacity = paint
        .opacity
        .0
        .trim()
        .parse::<f32>()
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    egui::Color32::from_rgba_unmultiplied(
        color.r,
        color.g,
        color.b,
        ((color.a as f32) * opacity).round() as u8,
    )
}

pub(super) fn scene_icon_contrast_color(background: egui::Color32) -> egui::Color32 {
    let alpha = background.a() as f32 / 255.0;
    let composite = |channel: u8, base: f32| channel as f32 * alpha + base * (1.0 - alpha);
    let red = composite(background.r(), 35.0);
    let green = composite(background.g(), 37.0);
    let blue = composite(background.b(), 42.0);
    let luminance = 0.299 * red + 0.587 * green + 0.114 * blue;
    if luminance >= 150.0 {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    }
}

pub(super) fn toggle_scene_node(ui: &egui::Ui, id: egui::Id, default_open: bool) {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    state.toggle(ui);
    state.store(ui.ctx());
}

pub(super) fn scene_drop_from_response(
    ui: &mut egui::Ui,
    response: &egui::Response,
    target: Selection,
    _surface: usize,
) -> Option<(Selection, SceneDropTarget)> {
    let source = response.dnd_hover_payload::<Selection>()?;
    if *source == target {
        return None;
    }
    let pointer_y = ui.ctx().pointer_interact_pos()?.y;
    let relative_y = ((pointer_y - response.rect.top()) / response.rect.height()).clamp(0.0, 1.0);
    let target = match (target, relative_y) {
        (Selection::Surface(index), y) if y < 0.25 => SceneDropTarget::RootAt(index),
        (Selection::Surface(index), y) if y > 0.75 => SceneDropTarget::RootAt(index + 1),
        (selection @ Selection::Surface(_), _) => SceneDropTarget::Into(selection),
        (selection @ Selection::Object(_, _), y) if y < 0.25 => SceneDropTarget::Before(selection),
        (selection @ Selection::Object(_, _), y) if y > 0.75 => SceneDropTarget::After(selection),
        (selection, _) => SceneDropTarget::Into(selection),
    };

    match target {
        SceneDropTarget::RootAt(index) => {
            let y = if index == 0 || relative_y < 0.5 {
                response.rect.top()
            } else {
                response.rect.bottom()
            };
            ui.painter().line_segment(
                [
                    egui::pos2(response.rect.left(), y),
                    egui::pos2(response.rect.right(), y),
                ],
                egui::Stroke::new(2.0, accent()),
            );
        }
        SceneDropTarget::Before(_) | SceneDropTarget::After(_) => {
            let y = if matches!(target, SceneDropTarget::Before(_)) {
                response.rect.top()
            } else {
                response.rect.bottom()
            };
            ui.painter().line_segment(
                [
                    egui::pos2(response.rect.left(), y),
                    egui::pos2(response.rect.right(), y),
                ],
                egui::Stroke::new(2.0, accent()),
            );
        }
        SceneDropTarget::Into(_) => {
            ui.painter().rect_stroke(
                response.rect.shrink(1.0),
                4.0,
                egui::Stroke::new(2.0, accent()),
                egui::StrokeKind::Inside,
            );
        }
    }

    response
        .dnd_release_payload::<Selection>()
        .map(|source| (*source, target))
}

pub(super) fn format_expression_result(
    field: ExpressionField,
    value: f64,
    language: LanguageId,
) -> String {
    match field {
        ExpressionField::Render => {
            if value == 0.0 {
                format!(
                    "{} ({})",
                    language.text("False"),
                    format_number_for_ui(value)
                )
            } else {
                format!(
                    "{} ({})",
                    language.text("True"),
                    format_number_for_ui(value)
                )
            }
        }
        ExpressionField::Visibility => {
            let clamped = value.clamp(0.0, 100.0);
            if (clamped - value).abs() > f64::EPSILON {
                format!(
                    "{}% ({} {}%)",
                    format_number_for_ui(value),
                    language.text("rendered as"),
                    format_number_for_ui(clamped)
                )
            } else {
                format!("{}%", format_number_for_ui(value))
            }
        }
        ExpressionField::ObjectWidth | ExpressionField::ObjectHeight => {
            let resolved = value.round().clamp(1.0, 8192.0);
            if (resolved - value).abs() > f64::EPSILON {
                format!(
                    "{}px ({} {}px)",
                    format_number_for_ui(value),
                    language.text("rendered as"),
                    format_number_for_ui(resolved)
                )
            } else {
                format!("{}px", format_number_for_ui(resolved))
            }
        }
        _ => format_number_for_ui(value),
    }
}

pub(super) fn scene_object_expression(
    object: &SceneObject,
    field: ExpressionField,
) -> Option<String> {
    let expression = match field {
        ExpressionField::Render => Some(&object.render),
        ExpressionField::Visibility => Some(&object.visibility),
        ExpressionField::ObjectX => Some(&object.x),
        ExpressionField::ObjectY => Some(&object.y),
        ExpressionField::ObjectRotation => Some(&object.rotation),
        ExpressionField::ObjectWidth => Some(&object.width),
        ExpressionField::ObjectHeight => Some(&object.height),
        ExpressionField::ObjectCornerRadius => Some(&object.corner_radius),
        ExpressionField::ObjectBorderWidth => object.border.as_ref().map(|border| &border.width),
        ExpressionField::ChildGap => Some(&object.gap),
        ExpressionField::BackgroundGradientAngle => match &object.background {
            LayerBackground::Gradient { angle, .. } => Some(angle),
            _ => None,
        },
        ExpressionField::TextFontSize => match &object.content {
            SceneContent::Text { font_size, .. } => Some(font_size),
            _ => None,
        },
        ExpressionField::TextFontContrast => match &object.content {
            SceneContent::Text { contrast, .. } => Some(contrast),
            _ => None,
        },
        ExpressionField::ProgressValue => match &object.content {
            SceneContent::Progress { value, .. } => Some(value),
            _ => None,
        },
        ExpressionField::ProgressCornerRadius => match &object.content {
            SceneContent::Progress { corner_radius, .. } => Some(corner_radius),
            _ => None,
        },
        ExpressionField::ProgressSegments => match &object.content {
            SceneContent::Progress {
                segments_expression,
                ..
            } => segments_expression.as_ref(),
            _ => None,
        },
        ExpressionField::ProgressSegmentGap => match &object.content {
            SceneContent::Progress { segment_gap, .. } => Some(segment_gap),
            _ => None,
        },
        ExpressionField::PlacementOffsetX | ExpressionField::PlacementOffsetY => None,
    };
    expression.map(|expression| expression.0.clone())
}

pub(super) fn set_object_expression(
    object: &mut SceneObject,
    field: ExpressionField,
    expression: Expression,
) -> bool {
    let target = match field {
        ExpressionField::Render => &mut object.render,
        ExpressionField::Visibility => &mut object.visibility,
        ExpressionField::ObjectX => &mut object.x,
        ExpressionField::ObjectY => &mut object.y,
        ExpressionField::ObjectRotation => &mut object.rotation,
        ExpressionField::ObjectWidth => &mut object.width,
        ExpressionField::ObjectHeight => &mut object.height,
        ExpressionField::ObjectCornerRadius => &mut object.corner_radius,
        ExpressionField::ObjectBorderWidth => match &mut object.border {
            Some(border) => &mut border.width,
            None => return false,
        },
        ExpressionField::ChildGap => &mut object.gap,
        ExpressionField::BackgroundGradientAngle => match &mut object.background {
            LayerBackground::Gradient { angle, .. } => angle,
            _ => return false,
        },
        ExpressionField::TextFontSize => match &mut object.content {
            SceneContent::Text { font_size, .. } => font_size,
            _ => return false,
        },
        ExpressionField::TextFontContrast => match &mut object.content {
            SceneContent::Text { contrast, .. } => contrast,
            _ => return false,
        },
        ExpressionField::ProgressValue => match &mut object.content {
            SceneContent::Progress { value, .. } => value,
            _ => return false,
        },
        ExpressionField::ProgressCornerRadius => match &mut object.content {
            SceneContent::Progress { corner_radius, .. } => corner_radius,
            _ => return false,
        },
        ExpressionField::ProgressSegments => match &mut object.content {
            SceneContent::Progress {
                segments_expression,
                ..
            } => {
                *segments_expression = Some(expression);
                return true;
            }
            _ => return false,
        },
        ExpressionField::ProgressSegmentGap => match &mut object.content {
            SceneContent::Progress { segment_gap, .. } => segment_gap,
            _ => return false,
        },
        ExpressionField::PlacementOffsetX | ExpressionField::PlacementOffsetY => return false,
    };
    *target = expression;
    true
}
