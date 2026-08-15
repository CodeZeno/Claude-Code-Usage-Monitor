use super::*;

pub fn format_template(template: &str, context: &DataContext) -> String {
    let mut output = String::new();
    let chars: Vec<char> = template.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '{' && index + 1 < chars.len() && chars[index + 1] == '{' {
            output.push('{');
            index += 2;
            continue;
        }
        if chars[index] != '{' {
            output.push(chars[index]);
            index += 1;
            continue;
        }
        let Some(relative_end) = chars[index + 1..].iter().position(|c| *c == '}') else {
            output.push(chars[index]);
            index += 1;
            continue;
        };
        let end = index + 1 + relative_end;
        let token: String = chars[index + 1..end].iter().collect();
        let (expression, format) = token.rsplit_once(':').unwrap_or((&token, "0.##"));
        let expression = expression.trim();
        let format = format.trim();
        if let Some(value) = context.get_string(expression) {
            output.push_str(value);
        } else if format.eq_ignore_ascii_case("usage_line") {
            output.push_str(&format_usage_line(expression, context).unwrap_or_else(|| "--".into()));
        } else if format.eq_ignore_ascii_case("usage_badge") {
            output
                .push_str(&format_usage_badge(expression, context).unwrap_or_else(|| "--".into()));
        } else {
            match evaluate(expression, context) {
                Ok(value) => output.push_str(&format_value(value, format, context)),
                Err(_) => output.push_str("--"),
            }
        }
        index = end + 1;
    }
    output
}

/// Resolve and rasterize a custom theme without mutating the live application.
/// The same output is used by the studio preview and by the desktop widget.
#[cfg(test)]
pub fn render_theme(theme: &ThemeDocument, data: Option<&AppUsageData>) -> RenderedTheme {
    render_theme_surface_with_runtime(theme, 0, data, ThemeRuntime::default())
}

pub fn render_theme_surface_with_runtime(
    theme: &ThemeDocument,
    surface_index: usize,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
) -> RenderedTheme {
    render_theme_surface_with_runtime_at_scale(theme, surface_index, data, runtime, 1.0)
}

/// Rasterize a theme surface at a physical-pixel scale while keeping theme
/// expressions and object geometry in their 96-DPI logical coordinate space.
pub fn render_theme_surface_with_runtime_at_scale(
    theme: &ThemeDocument,
    surface_index: usize,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
    scale: f64,
) -> RenderedTheme {
    let Some(surface) = theme.surfaces.get(surface_index) else {
        return RenderedTheme {
            width: 1,
            height: 1,
            pixels: vec![0],
            warnings: vec![format!("Root object {surface_index} does not exist")],
        };
    };
    let scale = normalized_render_scale(scale);
    let mut warnings = Vec::new();
    let (logical_width, logical_height) =
        resolve_object_size(surface, data, runtime, &mut warnings);
    let width = scaled_render_dimension(logical_width, scale);
    let height = scaled_render_dimension(logical_height, scale);
    let resolved_canvas = Canvas {
        width: logical_width,
        width_expression: Some(surface.width.clone()),
        height: logical_height,
        height_expression: Some(surface.height.clone()),
        background: surface.background.canvas_paint(),
    };
    let context = DataContext::from_usage_with_runtime(data, &resolved_canvas, runtime);
    let mut pixels = vec![0u32; width as usize * height as usize];
    let root_layer = ResolvedObject {
        source: surface,
        x: 0.0,
        y: 0.0,
        width: logical_width as f64,
        height: logical_height as f64,
        parent_width: logical_width as f64,
        parent_height: logical_height as f64,
        opacity: 1.0,
        rotation: evaluate(&surface.rotation.0, &context).unwrap_or(0.0),
        clip: Vec::new(),
    };
    render_resolved_object(
        &mut pixels,
        width,
        height,
        &root_layer,
        &context,
        scale,
        &mut warnings,
    );

    let (resolved, layer_warnings) =
        resolve_objects_for(surface, &resolved_canvas, &surface.children, data, runtime);
    warnings.extend(layer_warnings);
    for object in resolved {
        render_resolved_object(
            &mut pixels,
            width,
            height,
            &object,
            &context,
            scale,
            &mut warnings,
        );
    }
    let visibility = theme
        .surfaces
        .get(surface_index)
        .map(|surface| match evaluate(&surface.visibility.0, &context) {
            Ok(value) if value.is_finite() => value.clamp(0.0, 100.0) / 100.0,
            Ok(_) => {
                warnings.push(format!(
                    "{}.visibility did not produce a finite value",
                    surface.name
                ));
                1.0
            }
            Err(error) => {
                warnings.push(format!("{}.visibility: {error}", surface.name));
                1.0
            }
        })
        .unwrap_or(1.0);
    if visibility < 1.0 {
        for pixel in &mut pixels {
            let scale = |component: u32| ((component as f64 * visibility).round() as u32).min(255);
            *pixel = (scale(*pixel >> 24) << 24)
                | (scale((*pixel >> 16) & 0xff) << 16)
                | (scale((*pixel >> 8) & 0xff) << 8)
                | scale(*pixel & 0xff);
        }
    }
    RenderedTheme {
        width,
        height,
        pixels,
        warnings,
    }
}

pub(super) fn normalized_render_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale.clamp(0.25, 8.0)
    } else {
        1.0
    }
}

pub(super) fn scaled_render_dimension(logical: u32, scale: f64) -> u32 {
    (logical as f64 * scale).round().clamp(1.0, 8192.0) as u32
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_resolved_object(
    target: &mut [u32],
    target_width: u32,
    target_height: u32,
    object: &ResolvedObject<'_>,
    context: &DataContext,
    scale: f64,
    warnings: &mut Vec<String>,
) {
    let object_width = (object.width * scale).round().max(1.0) as u32;
    let object_height = (object.height * scale).round().max(1.0) as u32;
    if object_width > 8192 || object_height > 8192 {
        warnings.push(format!("{} is too large to render", object.source.name));
        return;
    }
    let mut local = vec![0u32; object_width as usize * object_height as usize];
    let object_context = context.clone().with_object(object);
    render_object_background(
        object,
        &object_context,
        scale,
        &mut local,
        object_width,
        object_height,
        warnings,
    );
    render_object_content(
        object,
        &object_context,
        scale,
        &mut local,
        object_width,
        object_height,
        warnings,
    );
    render_object_border(
        object,
        &object_context,
        scale,
        &mut local,
        object_width,
        object_height,
    );
    composite_object(
        target,
        target_width,
        target_height,
        &local,
        object_width,
        object_height,
        (object.x * scale).round(),
        (object.y * scale).round(),
        object.rotation,
        object.opacity,
        scale,
        &object.clip,
    );
}

pub(super) fn resolve_objects_for<'a>(
    root: &SceneObject,
    canvas: &Canvas,
    layers: &'a [SceneObject],
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
) -> (Vec<ResolvedObject<'a>>, Vec<String>) {
    let context = DataContext::from_usage_with_runtime(data, canvas, runtime);
    let mut resolved = Vec::new();
    let mut warnings = Vec::new();
    let mut cache = vec![None; layers.len()];
    for index in 0..layers.len() {
        let Some(geometry) = resolve_geometry(
            index,
            root,
            canvas,
            layers,
            &context,
            &mut cache,
            &mut Vec::new(),
            &mut warnings,
        ) else {
            continue;
        };
        let object = &layers[index];
        resolved.push(ResolvedObject {
            source: object,
            x: geometry.x,
            y: geometry.y,
            width: geometry.width,
            height: geometry.height,
            parent_width: geometry.parent_width,
            parent_height: geometry.parent_height,
            opacity: geometry.opacity,
            rotation: geometry.rotation,
            clip: geometry.clip,
        });
    }
    (resolved, warnings)
}

/// Return the topmost interactive layer at a logical canvas point. Non-
/// interactive layers intentionally do not block layers beneath them, which
/// lets visual decoration sit above a larger deliberate hit area.
pub fn hit_test_mouse_event(
    theme: &ThemeDocument,
    surface_index: usize,
    x: f64,
    y: f64,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
) -> Option<String> {
    let surface = theme.surfaces.get(surface_index)?;
    if !surface_should_render(theme, surface_index, data, runtime) {
        return None;
    }
    let (width, height) = resolve_object_size(surface, data, runtime, &mut Vec::new());
    let canvas = Canvas {
        width,
        width_expression: Some(surface.width.clone()),
        height,
        height_expression: Some(surface.height.clone()),
        background: surface.background.canvas_paint(),
    };
    let (resolved, _) = resolve_objects_for(surface, &canvas, &surface.children, data, runtime);
    for object in resolved.into_iter().rev() {
        if object.opacity <= 0.0
            || object.width <= 0.0
            || object.height <= 0.0
            || object
                .source
                .mouse_events
                .as_ref()
                .is_none_or(MouseEvents::is_empty)
        {
            continue;
        }
        let bounds = ClipRegion {
            x: object.x,
            y: object.y,
            width: object.width,
            height: object.height,
            rotation: object.rotation,
        };
        if point_in_clip((x, y), bounds)
            && object
                .clip
                .iter()
                .all(|region| point_in_clip((x, y), *region))
        {
            return Some(object.source.id.clone());
        }
    }

    let root_is_visible = DataContext::from_usage_with_runtime(data, &canvas, runtime);
    let root_is_visible = evaluate(&surface.visibility.0, &root_is_visible)
        .is_ok_and(|value| value.is_finite() && value > 0.0);
    if root_is_visible
        && surface
            .mouse_events
            .as_ref()
            .is_some_and(|events| !events.is_empty())
        && point_in_clip(
            (x, y),
            ClipRegion {
                x: 0.0,
                y: 0.0,
                width: width as f64,
                height: height as f64,
                rotation: evaluate(
                    &surface.rotation.0,
                    &DataContext::from_usage_with_runtime(data, &canvas, runtime),
                )
                .unwrap_or(0.0),
            },
        )
    {
        Some(surface.id.clone())
    } else {
        None
    }
}

/// Resolve one SceneObject into canvas coordinates, including every parent object's
/// position and anchor. Theme Studio uses this for accurate selection handles.
pub fn resolve_object_bounds_with_runtime(
    theme: &ThemeDocument,
    surface_index: usize,
    object_index: usize,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
) -> Option<(f64, f64, f64, f64)> {
    let surface = theme.surfaces.get(surface_index)?;
    if object_index >= surface.children.len() {
        return None;
    }
    let mut warnings = Vec::new();
    let (width, height) = resolve_object_size(surface, data, runtime, &mut warnings);
    let canvas = Canvas {
        width,
        width_expression: Some(surface.width.clone()),
        height,
        height_expression: Some(surface.height.clone()),
        background: surface.background.canvas_paint(),
    };
    let context = DataContext::from_usage_with_runtime(data, &canvas, runtime);
    let mut cache = vec![None; surface.children.len()];
    let geometry = resolve_geometry(
        object_index,
        surface,
        &canvas,
        &surface.children,
        &context,
        &mut cache,
        &mut Vec::new(),
        &mut Vec::new(),
    )?;
    Some((geometry.x, geometry.y, geometry.width, geometry.height))
}

pub fn resolve_surface_size(
    theme: &ThemeDocument,
    surface_index: usize,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
) -> (u32, u32) {
    theme
        .surfaces
        .get(surface_index)
        .map(|surface| resolve_object_size(surface, data, runtime, &mut Vec::new()))
        .unwrap_or((theme.canvas.width.max(1), theme.canvas.height.max(1)))
}

pub(super) fn resolve_object_size(
    object: &SceneObject,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
    warnings: &mut Vec<String>,
) -> (u32, u32) {
    let fallback = Canvas::default();
    let context = DataContext::from_usage_with_runtime(data, &fallback, runtime);
    let mut resolve = |label: &str, expression: &Expression, fallback: u32| match evaluate(
        &expression.0,
        &context,
    ) {
        Ok(value) if value.is_finite() => value.round().clamp(1.0, 8192.0) as u32,
        Ok(_) => {
            warnings.push(format!(
                "{}.{} did not produce a finite value",
                object.name, label
            ));
            fallback
        }
        Err(error) => {
            warnings.push(format!("{}.{}: {error}", object.name, label));
            fallback
        }
    };
    (
        resolve("width", &object.width, fallback.width),
        resolve("height", &object.height, fallback.height),
    )
}

#[derive(Clone, Copy, Debug)]
pub struct ResolvedPlacementValues {
    pub offset_x: i32,
    pub offset_y: i32,
}

pub fn resolve_surface_placement(
    theme: &ThemeDocument,
    surface_index: usize,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
) -> ResolvedPlacementValues {
    let Some(surface) = theme.surfaces.get(surface_index) else {
        return ResolvedPlacementValues {
            offset_x: theme.placement.offset_x,
            offset_y: theme.placement.offset_y,
        };
    };
    let (width, height) = resolve_surface_size(theme, surface_index, data, runtime);
    let canvas = Canvas {
        width,
        width_expression: Some(surface.width.clone()),
        height,
        height_expression: Some(surface.height.clone()),
        background: surface.background.canvas_paint(),
    };
    let context = DataContext::from_usage_with_runtime(data, &canvas, runtime);
    let number = |expression: &Option<Expression>, fallback: i32| {
        expression
            .as_ref()
            .and_then(|expression| evaluate(&expression.0, &context).ok())
            .filter(|value| value.is_finite())
            .map(|value| value.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
            .unwrap_or(fallback)
    };
    ResolvedPlacementValues {
        offset_x: number(
            &surface.placement.offset_x_expression,
            surface.placement.offset_x,
        ),
        offset_y: number(
            &surface.placement.offset_y_expression,
            surface.placement.offset_y,
        ),
    }
}

pub fn surface_should_render(
    theme: &ThemeDocument,
    surface_index: usize,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
) -> bool {
    let Some(surface) = theme.surfaces.get(surface_index) else {
        return surface_index == 0;
    };
    let (width, height) = resolve_surface_size(theme, surface_index, data, runtime);
    let canvas = Canvas {
        width,
        width_expression: Some(surface.width.clone()),
        height,
        height_expression: Some(surface.height.clone()),
        background: surface.background.canvas_paint(),
    };
    let context = DataContext::from_usage_with_runtime(data, &canvas, runtime);
    evaluate(&surface.render.0, &context).is_ok_and(|value| value.is_finite() && value != 0.0)
}

#[derive(Clone, Debug)]
pub(super) struct ObjectGeometry {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    parent_width: f64,
    parent_height: f64,
    opacity: f64,
    rotation: f64,
    clip: Vec<ClipRegion>,
    child_clip: Vec<ClipRegion>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_geometry(
    index: usize,
    root: &SceneObject,
    canvas: &Canvas,
    layers: &[SceneObject],
    context: &DataContext,
    cache: &mut [Option<ObjectGeometry>],
    stack: &mut Vec<usize>,
    warnings: &mut Vec<String>,
) -> Option<ObjectGeometry> {
    if let Some(value) = &cache[index] {
        return Some(value.clone());
    }
    if stack.contains(&index) {
        warnings.push(format!(
            "{}: object hierarchy contains a cycle",
            layers[index].name
        ));
        return None;
    }
    stack.push(index);
    let object = &layers[index];
    match evaluate(&object.render.0, context) {
        Ok(0.0) => {
            stack.pop();
            return None;
        }
        Ok(value) if value.is_finite() => {}
        Ok(_) => {
            warnings.push(format!(
                "{}.render did not produce a finite value",
                object.name
            ));
            stack.pop();
            return None;
        }
        Err(error) => {
            warnings.push(format!("{}.render: {error}", object.name));
            stack.pop();
            return None;
        }
    }
    let mut parent_index = None;
    let parent = if let Some(parent_id) = &object.parent {
        if parent_id == &root.id {
            ObjectGeometry {
                x: 0.0,
                y: 0.0,
                width: canvas.width as f64,
                height: canvas.height as f64,
                parent_width: canvas.width as f64,
                parent_height: canvas.height as f64,
                opacity: 1.0,
                rotation: evaluate(&root.rotation.0, context).unwrap_or(0.0),
                clip: Vec::new(),
                child_clip: Vec::new(),
            }
        } else {
            let Some(found_parent_index) = layers
                .iter()
                .position(|candidate| &candidate.id == parent_id)
            else {
                warnings.push(format!(
                    "{}: parent object '{}' was not found",
                    object.name, parent_id
                ));
                stack.pop();
                return None;
            };
            parent_index = Some(found_parent_index);
            resolve_geometry(
                found_parent_index,
                root,
                canvas,
                layers,
                context,
                cache,
                stack,
                warnings,
            )?
        }
    } else {
        ObjectGeometry {
            x: 0.0,
            y: 0.0,
            width: canvas.width as f64,
            height: canvas.height as f64,
            parent_width: canvas.width as f64,
            parent_height: canvas.height as f64,
            opacity: 1.0,
            rotation: evaluate(&root.rotation.0, context).unwrap_or(0.0),
            clip: Vec::new(),
            child_clip: Vec::new(),
        }
    };
    stack.pop();
    let mut object_context = context.clone();
    object_context.insert("parent.width", parent.width);
    object_context.insert("parent.height", parent.height);
    let mut value = |name: &str, expression: &Expression, fallback: f64| match evaluate(
        &expression.0,
        &object_context,
    ) {
        Ok(value) if value.is_finite() => value,
        Ok(_) => {
            warnings.push(format!(
                "{}.{} did not produce a finite value",
                object.name, name
            ));
            fallback
        }
        Err(error) => {
            warnings.push(format!("{}.{}: {error}", object.name, name));
            fallback
        }
    };
    let offset_x = value("x", &object.x, 0.0);
    let offset_y = value("y", &object.y, 0.0);
    let width = value("width", &object.width, 1.0).max(0.0);
    let height = value("height", &object.height, 1.0).max(0.0);
    let parent_object = parent_index
        .map(|parent_index| &layers[parent_index])
        .unwrap_or(root);
    let managed_layout = (parent_object.layout != ChildLayout::Freeform).then_some((
        parent_object.layout,
        parent_object.align,
        &parent_object.gap,
    ));
    let (local_x, local_y) = if let Some((layout, align, gap)) = managed_layout {
        let gap = value("parent.gap", gap, 0.0).max(0.0);
        let mut cursor = 0.0;
        for sibling_index in 0..index {
            let same_parent = match parent_index {
                Some(parent_index) => {
                    layers[sibling_index].parent.as_deref()
                        == Some(layers[parent_index].id.as_str())
                }
                None => layers[sibling_index]
                    .parent
                    .as_deref()
                    .is_none_or(|parent_id| parent_id == root.id),
            };
            if !same_parent {
                continue;
            }
            let sibling = &layers[sibling_index];
            let sibling_renders = evaluate(&sibling.render.0, &object_context)
                .is_ok_and(|render| render.is_finite() && render != 0.0);
            if !sibling_renders {
                continue;
            }
            let sibling_size = match layout {
                ChildLayout::Row => evaluate(&sibling.width.0, &object_context).unwrap_or(0.0),
                ChildLayout::Column => evaluate(&sibling.height.0, &object_context).unwrap_or(0.0),
                ChildLayout::Freeform => 0.0,
            }
            .max(0.0);
            cursor += sibling_size + gap;
        }
        match layout {
            ChildLayout::Row => {
                let cross = match align {
                    ChildAlignment::Start => 0.0,
                    ChildAlignment::Center => (parent.height - height) / 2.0,
                    ChildAlignment::End => parent.height - height,
                };
                (cursor + offset_x, cross + offset_y)
            }
            ChildLayout::Column => {
                let cross = match align {
                    ChildAlignment::Start => 0.0,
                    ChildAlignment::Center => (parent.width - width) / 2.0,
                    ChildAlignment::End => parent.width - width,
                };
                (cross + offset_x, cursor + offset_y)
            }
            ChildLayout::Freeform => unreachable!(),
        }
    } else {
        let x = match object.anchor.horizontal {
            ObjectHorizontalAnchor::Left => offset_x,
            ObjectHorizontalAnchor::Center => (parent.width - width) / 2.0 + offset_x,
            ObjectHorizontalAnchor::Right => parent.width - width - offset_x,
        };
        let y = match object.anchor.vertical {
            ObjectVerticalAnchor::Top => offset_y,
            ObjectVerticalAnchor::Center => (parent.height - height) / 2.0 + offset_y,
            ObjectVerticalAnchor::Bottom => parent.height - height - offset_y,
        };
        (x, y)
    };
    let parent_center = (
        parent.x + parent.width / 2.0,
        parent.y + parent.height / 2.0,
    );
    let local_center = (
        parent.x + local_x + width / 2.0,
        parent.y + local_y + height / 2.0,
    );
    let angle = parent.rotation.to_radians();
    let (sin, cos) = angle.sin_cos();
    let dx = local_center.0 - parent_center.0;
    let dy = local_center.1 - parent_center.1;
    let world_center = (
        parent_center.0 + dx * cos - dy * sin,
        parent_center.1 + dx * sin + dy * cos,
    );
    let x = world_center.0 - width / 2.0;
    let y = world_center.1 - height / 2.0;
    let rotation = parent.rotation + value("rotation", &object.rotation, 0.0);
    let clip = parent.child_clip.clone();
    let mut child_clip = clip.clone();
    child_clip.push(ClipRegion {
        x,
        y,
        width,
        height,
        rotation,
    });
    let geometry = ObjectGeometry {
        x,
        y,
        width,
        height,
        parent_width: parent.width,
        parent_height: parent.height,
        opacity: parent.opacity
            * (value("visibility", &object.visibility, 100.0).clamp(0.0, 100.0) / 100.0),
        rotation,
        clip,
        child_clip,
    };
    cache[index] = Some(geometry.clone());
    Some(geometry)
}

pub(super) fn render_object_background(
    object: &ResolvedObject<'_>,
    context: &DataContext,
    scale: f64,
    pixels: &mut [u32],
    width: u32,
    height: u32,
    warnings: &mut Vec<String>,
) {
    let radius = evaluate(&object.source.corner_radius.0, context).unwrap_or(0.0) * scale;
    match &object.source.background {
        LayerBackground::None => {}
        LayerBackground::Colour { colour } => {
            fill_rounded(pixels, width, height, colour.resolve(context), radius);
        }
        LayerBackground::Gradient { start, end, angle } => {
            let angle = match evaluate(&angle.0, context) {
                Ok(value) if value.is_finite() => value,
                Ok(_) => {
                    warnings.push(format!(
                        "{}.background.gradient.angle did not produce a finite value",
                        object.source.name
                    ));
                    0.0
                }
                Err(error) => {
                    warnings.push(format!(
                        "{}.background.gradient.angle: {error}",
                        object.source.name
                    ));
                    0.0
                }
            };
            fill_linear_gradient(
                pixels,
                width,
                height,
                start.resolve(context),
                end.resolve(context),
                angle,
            );
            clip_to_rounded_rectangle(pixels, width, height, radius);
        }
        LayerBackground::Image { path, fit } => {
            let path = resolve_asset_path(path);
            match load_image_cached(&path) {
                Ok(image) => {
                    render_image(pixels, width, height, &image, *fit);
                    clip_to_rounded_rectangle(pixels, width, height, radius);
                }
                Err(error) => warnings.push(format!(
                    "{}: unable to load {} ({error})",
                    object.source.name,
                    path.display()
                )),
            }
        }
    }
}

pub(super) fn render_object_border(
    object: &ResolvedObject<'_>,
    context: &DataContext,
    scale: f64,
    pixels: &mut [u32],
    width: u32,
    height: u32,
) {
    if let Some(border) = &object.source.border {
        let radius = evaluate(&object.source.corner_radius.0, context).unwrap_or(0.0) * scale;
        let border_width = (evaluate(&border.width.0, context).unwrap_or(1.0) * scale).max(1.0);
        stroke_rounded_rectangle(
            pixels,
            width,
            height,
            border.color.resolve(context),
            radius,
            border_width,
        );
    }
}

pub(super) fn render_object_content(
    object: &ResolvedObject<'_>,
    context: &DataContext,
    scale: f64,
    pixels: &mut [u32],
    width: u32,
    height: u32,
    warnings: &mut Vec<String>,
) {
    match &object.source.content {
        SceneContent::None => {}
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
            let radius = evaluate(&corner_radius.0, context).unwrap_or(0.0) * scale;
            let gap = evaluate(&segment_gap.0, context).unwrap_or(2.0) * scale;
            let segments = match segments_expression {
                Some(expression) => match evaluate(&expression.0, context) {
                    Ok(value) if value.is_finite() => value.round().clamp(0.0, 1000.0) as u16,
                    Ok(_) => {
                        warnings.push(format!(
                            "{}.segments did not produce a finite value",
                            object.source.name
                        ));
                        *segments
                    }
                    Err(error) => {
                        warnings.push(format!("{}.segments: {error}", object.source.name));
                        *segments
                    }
                },
                None => *segments,
            };
            if segments > 1 {
                draw_progress(
                    pixels,
                    width,
                    height,
                    track.resolve(context),
                    radius,
                    1.0,
                    *direction,
                    segments,
                    gap,
                );
            } else {
                fill_rounded(pixels, width, height, track.resolve(context), radius);
            }
            let amount = (evaluate(&value.0, context).unwrap_or(0.0) / 100.0).clamp(0.0, 1.0);
            draw_progress(
                pixels,
                width,
                height,
                fill.resolve(context),
                radius,
                amount,
                *direction,
                segments,
                gap,
            );
        }
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
            let text = format_template(template, context);
            let size = (evaluate(&font_size.0, context).unwrap_or(13.0) * scale).max(1.0);
            let contrast = evaluate(&contrast.0, context)
                .unwrap_or(1.4)
                .clamp(0.25, 4.0);
            render_text_mask(
                pixels,
                width,
                height,
                &text,
                font_family,
                size,
                weight.gdi_weight(),
                *rendering,
                contrast,
                *align,
                color.resolve(context),
            );
        }
    }
}

pub(super) fn resolve_asset_path(path: &str) -> PathBuf {
    let expanded = if let Some(rest) = path.strip_prefix("%APPDATA%") {
        std::env::var("APPDATA")
            .map(|root| PathBuf::from(root).join(rest.trim_start_matches(['\\', '/'])))
            .unwrap_or_else(|_| PathBuf::from(path))
    } else {
        PathBuf::from(path)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        themes_directory().join(expanded)
    }
}

pub(super) fn load_image_cached(
    path: &Path,
) -> Result<Arc<image::DynamicImage>, image::ImageError> {
    type ImageCache = HashMap<PathBuf, (Option<std::time::SystemTime>, Arc<image::DynamicImage>)>;
    static CACHE: OnceLock<Mutex<ImageCache>> = OnceLock::new();
    let modified = std::fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((cached_modified, image)) = cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(path)
    {
        if *cached_modified == modified {
            return Ok(image.clone());
        }
    }
    let image = Arc::new(image::open(path)?);
    cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(path.to_path_buf(), (modified, image.clone()));
    Ok(image)
}

pub(super) fn render_image(
    target: &mut [u32],
    width: u32,
    height: u32,
    source: &image::DynamicImage,
    fit: ImageFit,
) {
    use image::GenericImageView;
    let (source_width, source_height) = source.dimensions();
    if source_width == 0 || source_height == 0 {
        return;
    }
    let (draw_width, draw_height) = match fit {
        ImageFit::Stretch => (width, height),
        ImageFit::Original => (source_width.min(width), source_height.min(height)),
        ImageFit::Contain | ImageFit::Cover => {
            let sx = width as f64 / source_width as f64;
            let sy = height as f64 / source_height as f64;
            let factor = if matches!(fit, ImageFit::Contain) {
                sx.min(sy)
            } else {
                sx.max(sy)
            };
            (
                (source_width as f64 * factor).round().max(1.0) as u32,
                (source_height as f64 * factor).round().max(1.0) as u32,
            )
        }
    };
    let resized = source
        .resize_exact(
            draw_width,
            draw_height,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    let offset_x = (width as i64 - draw_width as i64) / 2;
    let offset_y = (height as i64 - draw_height as i64) / 2;
    for y in 0..height {
        for x in 0..width {
            let sx = x as i64 - offset_x;
            let sy = y as i64 - offset_y;
            if sx < 0 || sy < 0 || sx >= draw_width as i64 || sy >= draw_height as i64 {
                continue;
            }
            let pixel = resized.get_pixel(sx as u32, sy as u32).0;
            let color = Rgba {
                r: pixel[0],
                g: pixel[1],
                b: pixel[2],
                a: pixel[3],
            };
            blend(
                &mut target[(y * width + x) as usize],
                premultiply(color),
                1.0,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_text_mask(
    target: &mut [u32],
    width: u32,
    height: u32,
    text: &str,
    font_family: &str,
    font_size: f64,
    font_weight: i32,
    rendering: FontRendering,
    contrast: f64,
    align: TextAlign,
    color: Rgba,
) {
    // windows-rs represents an empty UTF-16 Vec with a non-null dangling
    // sentinel. DrawTextW still probes the pointer when cchText is zero, so
    // calling it for empty output causes a native access violation.
    if text.is_empty() || width == 0 || height == 0 || target.is_empty() {
        return;
    }
    unsafe {
        let memory_dc = CreateCompatibleDC(HDC::default());
        if memory_dc.is_invalid() {
            return;
        }
        let bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(memory_dc, &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0)
            .unwrap_or_default();
        if bitmap.is_invalid() || bits.is_null() {
            let _ = DeleteDC(memory_dc);
            return;
        }
        let old_bitmap = SelectObject(memory_dc, bitmap);
        std::ptr::write_bytes(bits, 0, width as usize * height as usize * 4);
        let font_name: Vec<u16> = font_family.encode_utf16().chain(Some(0)).collect();
        let font = CreateFontW(
            -(font_size.round() as i32),
            0,
            0,
            0,
            font_weight,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            rendering.gdi_quality(),
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR::from_raw(font_name.as_ptr()),
        );
        let old_font = SelectObject(memory_dc, font);
        let _ = SetBkMode(memory_dc, TRANSPARENT);
        let _ = SetTextColor(memory_dc, COLORREF(0x00FF_FFFF));
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let alignment = match align {
            TextAlign::Left => DT_LEFT,
            TextAlign::Center => DT_CENTER,
            TextAlign::Right => DT_RIGHT,
        };
        let _ = DrawTextW(
            memory_dc,
            &mut wide,
            &mut rect,
            alignment | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        let mask = std::slice::from_raw_parts(bits as *const u32, width as usize * height as usize);
        for (destination, source) in target.iter_mut().zip(mask) {
            let coverage = text_mask_coverage(*source, rendering, contrast);
            if coverage > 0.0 {
                blend(destination, premultiply(color), coverage);
            }
        }
        SelectObject(memory_dc, old_font);
        let _ = DeleteObject(font);
        SelectObject(memory_dc, old_bitmap);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory_dc);
    }
}

pub(super) fn text_mask_coverage(pixel: u32, rendering: FontRendering, contrast: f64) -> f64 {
    let blue = (pixel & 0xFF) as f64;
    let green = ((pixel >> 8) & 0xFF) as f64;
    let red = ((pixel >> 16) & 0xFF) as f64;
    let intensity = match rendering {
        FontRendering::ClearType => (red + green + blue) / (3.0 * 255.0),
        FontRendering::Antialiased | FontRendering::Aliased => red.max(green).max(blue) / 255.0,
    };
    if rendering == FontRendering::Aliased {
        return (intensity > 0.0) as u8 as f64;
    }
    // GDI tunes grayscale antialiasing for direct drawing onto an opaque
    // surface. Reusing those gamma-adjusted values as linear per-pixel alpha
    // makes the fringe too opaque and small text look artificially bold.
    intensity.clamp(0.0, 1.0).powf(contrast.clamp(0.25, 4.0))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn composite_object(
    target: &mut [u32],
    target_width: u32,
    target_height: u32,
    source: &[u32],
    source_width: u32,
    source_height: u32,
    x: f64,
    y: f64,
    rotation_degrees: f64,
    opacity: f64,
    scale: f64,
    clip: &[ClipRegion],
) {
    let angle = rotation_degrees.to_radians();
    let (sin, cos) = angle.sin_cos();
    let cx = source_width as f64 / 2.0;
    let cy = source_height as f64 / 2.0;
    let corners = [(-cx, -cy), (cx, -cy), (-cx, cy), (cx, cy)];
    let rotated: Vec<(f64, f64)> = corners
        .iter()
        .map(|(px, py)| (px * cos - py * sin + x + cx, px * sin + py * cos + y + cy))
        .collect();
    let min_x = rotated
        .iter()
        .map(|p| p.0)
        .fold(f64::INFINITY, f64::min)
        .floor()
        .max(0.0) as i32;
    let max_x = rotated
        .iter()
        .map(|p| p.0)
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .min(target_width as f64) as i32;
    let min_y = rotated
        .iter()
        .map(|p| p.1)
        .fold(f64::INFINITY, f64::min)
        .floor()
        .max(0.0) as i32;
    let max_y = rotated
        .iter()
        .map(|p| p.1)
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil()
        .min(target_height as f64) as i32;
    for dy in min_y..max_y {
        for dx in min_x..max_x {
            let point = (dx as f64 + 0.5, dy as f64 + 0.5);
            let logical_point = (point.0 / scale, point.1 / scale);
            if clip
                .iter()
                .any(|region| !point_in_clip(logical_point, *region))
            {
                continue;
            }
            let rx = dx as f64 + 0.5 - x - cx;
            let ry = dy as f64 + 0.5 - y - cy;
            let sx = rx * cos + ry * sin + cx;
            let sy = -rx * sin + ry * cos + cy;
            if sx >= 0.0 && sy >= 0.0 && sx < source_width as f64 && sy < source_height as f64 {
                let source_pixel =
                    source[sy.floor() as usize * source_width as usize + sx.floor() as usize];
                blend(
                    &mut target[dy as usize * target_width as usize + dx as usize],
                    source_pixel,
                    opacity,
                );
            }
        }
    }
}

pub(super) fn point_in_clip(point: (f64, f64), clip: ClipRegion) -> bool {
    let center = (clip.x + clip.width / 2.0, clip.y + clip.height / 2.0);
    let angle = (-clip.rotation).to_radians();
    let (sin, cos) = angle.sin_cos();
    let dx = point.0 - center.0;
    let dy = point.1 - center.1;
    let local_x = dx * cos - dy * sin + clip.width / 2.0;
    let local_y = dx * sin + dy * cos + clip.height / 2.0;
    local_x >= 0.0 && local_y >= 0.0 && local_x < clip.width && local_y < clip.height
}

pub(super) fn fill_rounded(pixels: &mut [u32], width: u32, height: u32, color: Rgba, radius: f64) {
    let radius = radius.clamp(0.0, width.min(height) as f64 / 2.0);
    let source = premultiply(color);
    if radius <= 0.0 {
        for pixel in pixels {
            blend(pixel, source, 1.0);
        }
        return;
    }
    for y in 0..height {
        for x in 0..width {
            let dx = if x as f64 + 0.5 < radius {
                radius - (x as f64 + 0.5)
            } else if x as f64 + 0.5 > width as f64 - radius {
                x as f64 + 0.5 - (width as f64 - radius)
            } else {
                0.0
            };
            let dy = if y as f64 + 0.5 < radius {
                radius - (y as f64 + 0.5)
            } else if y as f64 + 0.5 > height as f64 - radius {
                y as f64 + 0.5 - (height as f64 - radius)
            } else {
                0.0
            };
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend(&mut pixels[(y * width + x) as usize], source, coverage);
            }
        }
    }
}

pub(super) fn fill_linear_gradient(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    start: Rgba,
    end: Rgba,
    angle_degrees: f64,
) {
    let angle = angle_degrees.to_radians();
    let direction_x = angle.cos();
    let direction_y = angle.sin();
    let half_width = width.saturating_sub(1) as f64 / 2.0;
    let half_height = height.saturating_sub(1) as f64 / 2.0;
    let extent = (direction_x.abs() * half_width + direction_y.abs() * half_height).max(0.5);
    let interpolate = |from: u8, to: u8, amount: f64| {
        (from as f64 + (to as f64 - from as f64) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    for y in 0..height {
        for x in 0..width {
            let projection =
                (x as f64 - half_width) * direction_x + (y as f64 - half_height) * direction_y;
            let amount = ((projection + extent) / (extent * 2.0)).clamp(0.0, 1.0);
            let color = Rgba {
                r: interpolate(start.r, end.r, amount),
                g: interpolate(start.g, end.g, amount),
                b: interpolate(start.b, end.b, amount),
                a: interpolate(start.a, end.a, amount),
            };
            blend(
                &mut pixels[(y * width + x) as usize],
                premultiply(color),
                1.0,
            );
        }
    }
}

pub(super) fn clip_to_rounded_rectangle(pixels: &mut [u32], width: u32, height: u32, radius: f64) {
    if radius <= 0.0 {
        return;
    }
    let mut mask = vec![0u32; pixels.len()];
    fill_rounded(
        &mut mask,
        width,
        height,
        Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        },
        radius,
    );
    for (pixel, mask) in pixels.iter_mut().zip(mask) {
        let coverage = mask >> 24;
        let scale = |component: u32| (component * coverage + 127) / 255;
        *pixel = (scale(*pixel >> 24) << 24)
            | (scale((*pixel >> 16) & 0xff) << 16)
            | (scale((*pixel >> 8) & 0xff) << 8)
            | scale(*pixel & 0xff);
    }
}

pub(super) fn stroke_rounded_rectangle(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    color: Rgba,
    radius: f64,
    stroke_width: f64,
) {
    let mut outer = vec![0u32; pixels.len()];
    fill_rounded(&mut outer, width, height, color, radius);
    if width as f64 > stroke_width * 2.0 && height as f64 > stroke_width * 2.0 {
        let inner_width = (width as f64 - stroke_width * 2.0).max(1.0) as u32;
        let inner_height = (height as f64 - stroke_width * 2.0).max(1.0) as u32;
        let mut inner = vec![0u32; inner_width as usize * inner_height as usize];
        let erase = Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        };
        fill_rounded(
            &mut inner,
            inner_width,
            inner_height,
            erase,
            (radius - stroke_width).max(0.0),
        );
        let offset = stroke_width.round() as i32;
        for iy in 0..inner_height {
            for ix in 0..inner_width {
                let inner_coverage = inner[(iy * inner_width + ix) as usize] >> 24;
                if inner_coverage != 0 {
                    let outer_index = ((iy as i32 + offset) as u32 * width
                        + (ix as i32 + offset) as u32)
                        as usize;
                    let keep = 255 - inner_coverage;
                    let pixel = outer[outer_index];
                    let scale = |component: u32| (component * keep + 127) / 255;
                    outer[outer_index] = (scale(pixel >> 24) << 24)
                        | (scale((pixel >> 16) & 0xff) << 16)
                        | (scale((pixel >> 8) & 0xff) << 8)
                        | scale(pixel & 0xff);
                }
            }
        }
    }
    for (destination, source) in pixels.iter_mut().zip(outer) {
        if source != 0 {
            blend(destination, source, 1.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_progress(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    color: Rgba,
    radius: f64,
    amount: f64,
    direction: ProgressDirection,
    segments: u16,
    gap: f64,
) {
    let mut fill = vec![0u32; pixels.len()];
    fill_rounded(&mut fill, width, height, color, radius);
    let count = segments as u32;
    for y in 0..height {
        for x in 0..width {
            let progress = match direction {
                ProgressDirection::LeftToRight => (x + 1) as f64 / width as f64,
                ProgressDirection::RightToLeft => (width - x) as f64 / width as f64,
                ProgressDirection::TopToBottom => (y + 1) as f64 / height as f64,
                ProgressDirection::BottomToTop => (height - y) as f64 / height as f64,
            };
            let mut visible = progress <= amount;
            if visible && count > 1 {
                let extent = if matches!(
                    direction,
                    ProgressDirection::LeftToRight | ProgressDirection::RightToLeft
                ) {
                    width
                } else {
                    height
                };
                let position = if matches!(
                    direction,
                    ProgressDirection::LeftToRight | ProgressDirection::RightToLeft
                ) {
                    x
                } else {
                    y
                };
                let segment_extent = extent as f64 / count as f64;
                visible = (position as f64 % segment_extent) < (segment_extent - gap).max(1.0);
            }
            if visible {
                let index = (y * width + x) as usize;
                blend(&mut pixels[index], fill[index], 1.0);
            }
        }
    }
}

pub(super) fn premultiply(color: Rgba) -> u32 {
    let a = color.a as u32;
    let r = color.r as u32 * a / 255;
    let g = color.g as u32 * a / 255;
    let b = color.b as u32 * a / 255;
    (a << 24) | (r << 16) | (g << 8) | b
}

pub(super) fn blend(destination: &mut u32, source: u32, opacity: f64) {
    let opacity = opacity.clamp(0.0, 1.0);
    let sa = (((source >> 24) & 0xFF) as f64 * opacity).round() as u32;
    if sa == 0 {
        return;
    }
    let sr = (((source >> 16) & 0xFF) as f64 * opacity).round() as u32;
    let sg = (((source >> 8) & 0xFF) as f64 * opacity).round() as u32;
    let sb = ((source & 0xFF) as f64 * opacity).round() as u32;
    let da = (*destination >> 24) & 0xFF;
    let dr = (*destination >> 16) & 0xFF;
    let dg = (*destination >> 8) & 0xFF;
    let db = *destination & 0xFF;
    let inverse = 255 - sa;
    let oa = sa + da * inverse / 255;
    let or = sr + dr * inverse / 255;
    let og = sg + dg * inverse / 255;
    let ob = sb + db * inverse / 255;
    *destination = (oa << 24) | (or << 16) | (og << 8) | ob;
}
