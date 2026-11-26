//! Validating scene objects, expressions, paints and templates when a theme
//! loads, with suggestions for likely fixes.

use super::*;

pub(super) fn validate_scene_mouse_events(
    errors: &mut Vec<String>,
    context: &DataContext,
    theme: &ThemeDocument,
    surface_index: usize,
    object: &SceneObject,
) {
    let Some(events) = &object.mouse_events else {
        return;
    };
    for (name, source) in [
        ("click", &events.click),
        ("double_click", &events.double_click),
        ("right_click", &events.right_click),
        ("mouse_enter", &events.mouse_enter),
        ("mouse_leave", &events.mouse_leave),
    ] {
        if source.trim().is_empty() {
            continue;
        }
        for error in validate_mouse_action_script(source, theme, surface_index, &object.id, context)
        {
            errors.push(format!("{}.mouse_events.{name}: {error}", object.name));
        }
    }
}

pub(super) fn validate_scene_object(
    errors: &mut Vec<String>,
    context: &DataContext,
    object: &SceneObject,
) {
    let mut object_context = context.clone();
    let gap = evaluate(&object.gap.0, context)
        .ok()
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
        .max(0.0);
    object_context.insert("this.gap", gap);
    for (property, expression) in [
        ("render", &object.render),
        ("visibility", &object.visibility),
        ("x", &object.x),
        ("y", &object.y),
        ("width", &object.width),
        ("height", &object.height),
        ("rotation", &object.rotation),
        ("corner_radius", &object.corner_radius),
        ("gap", &object.gap),
    ] {
        validate_expression(
            errors,
            if property == "gap" {
                context
            } else {
                &object_context
            },
            &format!("{}.{}", object.name, property),
            expression,
        );
    }
    match &object.background {
        LayerBackground::None => {}
        LayerBackground::Colour { colour } => validate_paint(
            errors,
            &object_context,
            &format!("{}.background.colour", object.name),
            colour,
        ),
        LayerBackground::Gradient { start, end, angle } => {
            validate_paint(
                errors,
                &object_context,
                &format!("{}.background.gradient.start", object.name),
                start,
            );
            validate_paint(
                errors,
                &object_context,
                &format!("{}.background.gradient.end", object.name),
                end,
            );
            validate_expression(
                errors,
                &object_context,
                &format!("{}.background.gradient.angle", object.name),
                angle,
            );
        }
        LayerBackground::Image { path, .. } => {
            if path.trim().is_empty() {
                errors.push(format!("{}.background.image: choose an image", object.name));
            }
        }
    }
    if let Some(border) = &object.border {
        validate_paint(
            errors,
            &object_context,
            &format!("{}.border", object.name),
            &border.color,
        );
        validate_expression(
            errors,
            &object_context,
            &format!("{}.border.width", object.name),
            &border.width,
        );
    }
    match &object.content {
        SceneContent::None => {}
        SceneContent::Text {
            template,
            font_size,
            contrast,
            color,
            ..
        } => {
            validate_expression(
                errors,
                &object_context,
                &format!("{}.font_size", object.name),
                font_size,
            );
            validate_expression(
                errors,
                &object_context,
                &format!("{}.contrast", object.name),
                contrast,
            );
            validate_paint(
                errors,
                &object_context,
                &format!("{}.color", object.name),
                color,
            );
            for error in validate_template(template, &object_context) {
                errors.push(format!("{}.template: {error}", object.name));
            }
        }
        SceneContent::Progress {
            value,
            fill,
            track,
            corner_radius,
            segment_gap,
            segments_expression,
            ..
        } => {
            for (name, expression) in [
                ("value", value),
                ("content.corner_radius", corner_radius),
                ("segment_gap", segment_gap),
            ] {
                validate_expression(
                    errors,
                    &object_context,
                    &format!("{}.{}", object.name, name),
                    expression,
                );
            }
            if let Some(expression) = segments_expression {
                validate_expression(
                    errors,
                    &object_context,
                    &format!("{}.segments", object.name),
                    expression,
                );
            }
            validate_paint(
                errors,
                &object_context,
                &format!("{}.fill", object.name),
                fill,
            );
            validate_paint(
                errors,
                &object_context,
                &format!("{}.track", object.name),
                track,
            );
        }
        SceneContent::ProviderMark { provider, color } => {
            if crate::providers::ProviderId::from_key(provider).is_none() {
                errors.push(format!(
                    "{}.provider: '{provider}' is not a provider key",
                    object.name
                ));
            }
            validate_paint(
                errors,
                &object_context,
                &format!("{}.color", object.name),
                color,
            );
        }
    }
}

pub(super) fn validate_expression(
    errors: &mut Vec<String>,
    context: &DataContext,
    label: &str,
    expression: &Expression,
) {
    if let Err(error) = evaluate(&expression.0, context) {
        errors.push(format!("{label}: {}", explain_error(error, context)));
    }
}

/// Adds the closest known name to an unknown-value error. Only validation does
/// this; evaluation runs every frame and keeps the short message.
fn explain_error(error: String, context: &DataContext) -> String {
    error
        .strip_prefix("Unknown value '")
        .and_then(|rest| rest.strip_suffix('\''))
        .map_or(error.clone(), |name| {
            theme_language::unknown_value_message(name, context)
        })
}

fn validate_paint(errors: &mut Vec<String>, context: &DataContext, label: &str, paint: &Paint) {
    if parse_color(&paint.color).is_none() {
        errors.push(format!(
            "{label}: '{}' is not #RRGGBB or #AARRGGBB",
            paint.color
        ));
    }
    if let Some(light) = &paint.light_color {
        if parse_color(light).is_none() {
            errors.push(format!(
                "{label}.light_color: '{light}' is not #RRGGBB or #AARRGGBB"
            ));
        }
    }
    validate_expression(errors, context, &format!("{label}.opacity"), &paint.opacity);
}

pub fn validate_template(template: &str, context: &DataContext) -> Vec<String> {
    let mut errors = Vec::new();
    let mut remaining = template;
    while let Some(start) = remaining.find('{') {
        remaining = &remaining[start + 1..];
        if remaining.starts_with('{') {
            remaining = &remaining[1..];
            continue;
        }
        let Some(end) = remaining.find('}') else {
            errors.push("missing closing brace".into());
            break;
        };
        let token = &remaining[..end];
        let (expression, format) = split_template_token(token);
        let expression = expression.trim();
        let format = format.trim();
        if context.get_string(expression).is_some()
            || (format.eq_ignore_ascii_case("usage_line")
                && format_usage_line(expression, context).is_some())
            || (format.eq_ignore_ascii_case("usage_badge")
                && format_usage_badge(expression, context).is_some())
        {
            remaining = &remaining[end + 1..];
            continue;
        }
        if let Err(error) = evaluate_value(expression, context) {
            errors.push(explain_error(error, context));
        } else if let Some(error) = theme_language::unknown_format_message(format) {
            errors.push(error);
        }
        remaining = &remaining[end + 1..];
    }
    errors
}
