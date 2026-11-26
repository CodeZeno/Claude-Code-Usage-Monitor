//! Mouse events on scene objects: parsing the action language, validating
//! scripts and executing actions against a surface.

use super::*;

/// Optional, side-effecting mouse handlers for a scene object. Handler text is
/// parsed as a small action language rather than by the numeric expression
/// evaluator, keeping ordinary theme expressions deterministic and read-only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseEvents {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub click: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub double_click: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub right_click: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mouse_enter: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mouse_leave: String,
}

impl MouseEvents {
    pub fn is_empty(&self) -> bool {
        self.click.trim().is_empty()
            && self.double_click.trim().is_empty()
            && self.right_click.trim().is_empty()
            && self.mouse_enter.trim().is_empty()
            && self.mouse_leave.trim().is_empty()
    }

    pub fn handler(&self, event: MouseEventKind) -> &str {
        match event {
            MouseEventKind::Click => &self.click,
            MouseEventKind::DoubleClick => &self.double_click,
            MouseEventKind::RightClick => &self.right_click,
            MouseEventKind::MouseEnter => &self.mouse_enter,
            MouseEventKind::MouseLeave => &self.mouse_leave,
        }
    }

    pub fn handler_mut(&mut self, event: MouseEventKind) -> &mut String {
        match event {
            MouseEventKind::Click => &mut self.click,
            MouseEventKind::DoubleClick => &mut self.double_click,
            MouseEventKind::RightClick => &mut self.right_click,
            MouseEventKind::MouseEnter => &mut self.mouse_enter,
            MouseEventKind::MouseLeave => &mut self.mouse_leave,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEventKind {
    Click,
    DoubleClick,
    RightClick,
    MouseEnter,
    MouseLeave,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseActionProperty {
    Render,
    Visibility,
    X,
    Y,
    Width,
    Height,
    Rotation,
}

impl MouseActionProperty {
    pub const ALL: [Self; 7] = [
        Self::Render,
        Self::Visibility,
        Self::X,
        Self::Y,
        Self::Width,
        Self::Height,
        Self::Rotation,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Render => "render",
            Self::Visibility => "visibility",
            Self::X => "x",
            Self::Y => "y",
            Self::Width => "width",
            Self::Height => "height",
            Self::Rotation => "rotation",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "render" => Some(Self::Render),
            "visibility" => Some(Self::Visibility),
            "x" => Some(Self::X),
            "y" => Some(Self::Y),
            "width" => Some(Self::Width),
            "height" => Some(Self::Height),
            "rotation" => Some(Self::Rotation),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MouseActionTarget {
    SelfObject,
    Object(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MouseAction {
    ShowDashboard,
    ToggleDashboard,
    OpenUrl {
        url: String,
    },
    ShowContextMenu {
        menu: Option<String>,
    },
    Set {
        target: MouseActionTarget,
        property: MouseActionProperty,
        value: Expression,
    },
    Toggle {
        target: MouseActionTarget,
        property: MouseActionProperty,
    },
    Increase {
        target: MouseActionTarget,
        property: MouseActionProperty,
        amount: Expression,
    },
    Decrease {
        target: MouseActionTarget,
        property: MouseActionProperty,
        amount: Expression,
    },
    Reset {
        target: MouseActionTarget,
        property: MouseActionProperty,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MouseActionOverrideKey {
    pub surface_index: usize,
    pub object_id: String,
    pub property: MouseActionProperty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MouseActionEffect {
    ShowDashboard,
    ToggleDashboard,
    OpenUrl(String),
    ShowContextMenu(Option<String>),
}

/// Parse the deliberately small, line-or-semicolon separated action language.
/// Supported forms include `show_dashboard()`, `toggle_dashboard()`,
/// `open_url("https://example.com")`, `show_context_menu()`,
/// `show_context_menu("menu-id")`, `set(self.render, false)`,
/// `increase("layer-id", height, 10)`, `decrease(self.width, 5)`,
/// `toggle(self.render)`, and `reset("layer-id", height)`.
pub fn parse_mouse_actions(source: &str) -> Result<Vec<MouseAction>, String> {
    let statements = split_action_statements(source)?;
    let mut actions = Vec::with_capacity(statements.len());
    for statement in statements {
        let open = statement
            .find('(')
            .ok_or_else(|| format!("Expected an action call: {statement}"))?;
        if !statement.ends_with(')') {
            return Err(format!("Expected ')' at the end of: {statement}"));
        }
        let name = statement[..open].trim().to_ascii_lowercase();
        let args = split_action_arguments(&statement[open + 1..statement.len() - 1])?;
        match name.as_str() {
            "show_dashboard" if args.is_empty() => actions.push(MouseAction::ShowDashboard),
            "show_dashboard" => return Err("show_dashboard() does not take arguments".into()),
            "toggle_dashboard" if args.is_empty() => actions.push(MouseAction::ToggleDashboard),
            "toggle_dashboard" => return Err("toggle_dashboard() does not take arguments".into()),
            "open_url" if args.len() == 1 => {
                let url = parse_quoted_action_string(&args[0], "URL")?;
                if !crate::context_menu::supported_url(&url) {
                    return Err("open_url URL must start with http:// or https://".into());
                }
                actions.push(MouseAction::OpenUrl { url });
            }
            "open_url" => return Err("open_url expects one quoted URL".into()),
            "show_context_menu" if args.is_empty() => {
                actions.push(MouseAction::ShowContextMenu { menu: None })
            }
            "show_context_menu" if args.len() == 1 => {
                actions.push(MouseAction::ShowContextMenu {
                    menu: Some(parse_quoted_action_string(
                        &args[0],
                        "context menu id or name",
                    )?),
                });
            }
            "show_context_menu" => {
                return Err("show_context_menu expects zero or one quoted menu id or name".into())
            }
            "set" | "increase" | "decrease" => {
                let (target, property, value) = match args.as_slice() {
                    [target_property, value] => {
                        let (target, property) = parse_target_property(target_property)?;
                        (target, property, value.as_str())
                    }
                    [target, property, value] => (
                        parse_action_target(target)?,
                        parse_action_property(property)?,
                        value.as_str(),
                    ),
                    _ => return Err(
                        format!(
                            "{name} expects {name}(target.property, value) or {name}(target, property, value)"
                        ),
                    ),
                };
                if value.trim().is_empty() {
                    return Err(format!("{name} requires a value expression"));
                }
                if name != "set" && property == MouseActionProperty::Render {
                    return Err(format!(
                        "{name} requires a numeric property; use toggle(..., render) for Render"
                    ));
                }
                let value = Expression(value.trim().to_string());
                match name.as_str() {
                    "set" => actions.push(MouseAction::Set {
                        target,
                        property,
                        value,
                    }),
                    "increase" => actions.push(MouseAction::Increase {
                        target,
                        property,
                        amount: value,
                    }),
                    "decrease" => actions.push(MouseAction::Decrease {
                        target,
                        property,
                        amount: value,
                    }),
                    _ => unreachable!(),
                }
            }
            "toggle" | "reset" => {
                let (target, property) = match args.as_slice() {
                    [target_property] => parse_target_property(target_property)?,
                    [target, property] => (
                        parse_action_target(target)?,
                        parse_action_property(property)?,
                    ),
                    _ => {
                        return Err(format!(
                            "{name} expects {name}(target.property) or {name}(target, property)"
                        ))
                    }
                };
                if name == "toggle" {
                    if property != MouseActionProperty::Render {
                        return Err("toggle currently supports the render property only".into());
                    }
                    actions.push(MouseAction::Toggle { target, property });
                } else {
                    actions.push(MouseAction::Reset { target, property });
                }
            }
            _ => return Err(theme_language::unknown_action_message(&name)),
        }
    }
    Ok(actions)
}

fn split_action_statements(source: &str) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for character in source.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if quoted && character == '\\' {
            current.push(character);
            escaped = true;
            continue;
        }
        if character == '"' {
            quoted = !quoted;
            current.push(character);
            continue;
        }
        if !quoted {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| "Unexpected ')' in action script".to_string())?
                }
                ';' | '\n' | '\r' if depth == 0 => {
                    if !current.trim().is_empty() {
                        result.push(current.trim().to_string());
                    }
                    current.clear();
                    continue;
                }
                _ => {}
            }
        }
        current.push(character);
    }
    if quoted {
        return Err("Unterminated quoted layer id in action script".into());
    }
    if depth != 0 {
        return Err("Unclosed '(' in action script".into());
    }
    if !current.trim().is_empty() {
        result.push(current.trim().to_string());
    }
    Ok(result)
}

fn split_action_arguments(source: &str) -> Result<Vec<String>, String> {
    if source.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for character in source.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if quoted && character == '\\' {
            current.push(character);
            escaped = true;
            continue;
        }
        if character == '"' {
            quoted = !quoted;
            current.push(character);
            continue;
        }
        if !quoted {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| "Unexpected ')' in action arguments".to_string())?
                }
                ',' if depth == 0 => {
                    result.push(current.trim().to_string());
                    current.clear();
                    continue;
                }
                _ => {}
            }
        }
        current.push(character);
    }
    if quoted || depth != 0 {
        return Err("Unclosed quote or parenthesis in action arguments".into());
    }
    result.push(current.trim().to_string());
    Ok(result)
}

/// How text inserted into an action value must be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionValue {
    /// Directly in an action script.
    Bare,
    /// Inside the quoted script of a context menu `layer_actions("…")`, where
    /// quotes and backslashes are escaped.
    Quoted,
}

/// Whether byte offset `caret` in an action script falls in the value of a
/// `set`, `increase` or `decrease` action, where any expression is accepted.
/// The script inside a context menu `layer_actions("…")` is searched too.
pub fn action_value_at(source: &str, caret: usize) -> Option<ActionValue> {
    let (name, args, index) = action_argument_at(source, caret)?;
    match name.as_str() {
        "set" | "increase" | "decrease" => {
            (index > 0 && index + 1 == args.len()).then_some(ActionValue::Bare)
        }
        "layer_actions" => {
            let argument = args[index].clone();
            let open = argument.start + source[argument.clone()].find('"')?;
            let close = source[open + 1..argument.end]
                .rfind('"')
                .map_or(argument.end, |offset| open + 1 + offset);
            if caret <= open || caret > close {
                return None;
            }
            let before = unescape_action_string(&source[open + 1..caret])?;
            let script = before.clone() + &unescape_action_string(&source[caret..close])?;
            action_value_at(&script, before.len()).map(|_| ActionValue::Quoted)
        }
        _ => None,
    }
}

/// The action call around byte offset `caret`: its lowercase name, the byte
/// ranges of its arguments, and the index of the argument holding the caret.
fn action_argument_at(
    source: &str,
    caret: usize,
) -> Option<(String, Vec<std::ops::Range<usize>>, usize)> {
    let mut statement = 0;
    let mut open = None;
    let mut args = Vec::new();
    let mut argument = 0;
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in source.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            _ if quoted => {}
            '(' => {
                if depth == 0 {
                    open = Some(index);
                    argument = index + 1;
                }
                depth += 1;
            }
            ',' if depth == 1 => {
                args.push(argument..index);
                argument = index + 1;
            }
            ')' if depth == 1 => {
                args.push(argument..index);
                depth = 0;
                if caret <= index {
                    break;
                }
            }
            ')' => depth = depth.saturating_sub(1),
            ';' | '\n' | '\r' if depth == 0 => {
                if caret <= index {
                    break;
                }
                statement = index + 1;
                open = None;
                args.clear();
            }
            _ => {}
        }
    }
    let open = open.filter(|open| *open >= statement && *open < caret)?;
    if depth > 0 {
        args.push(argument..source.len());
    }
    let index = args
        .iter()
        .position(|range| range.start <= caret && caret <= range.end)?;
    let name = source[statement..open].trim().to_ascii_lowercase();
    Some((name, args, index))
}

/// Reverses the escaping of a quoted action string, or `None` when `source`
/// ends partway through an escape.
fn unescape_action_string(source: &str) -> Option<String> {
    let mut result = String::with_capacity(source.len());
    let mut characters = source.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            result.push(characters.next()?);
        } else {
            result.push(character);
        }
    }
    Some(result)
}

fn parse_target_property(source: &str) -> Result<(MouseActionTarget, MouseActionProperty), String> {
    let (target, property) = source
        .rsplit_once('.')
        .ok_or_else(|| format!("Expected target.property, got: {source}"))?;
    Ok((
        parse_action_target(target)?,
        parse_action_property(property)?,
    ))
}

fn parse_action_target(source: &str) -> Result<MouseActionTarget, String> {
    let source = source.trim();
    if source.eq_ignore_ascii_case("self") {
        return Ok(MouseActionTarget::SelfObject);
    }
    if source.len() >= 2 && source.starts_with('"') && source.ends_with('"') {
        let value = source[1..source.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
        if value.trim().is_empty() {
            return Err("Layer id cannot be empty".into());
        }
        return Ok(MouseActionTarget::Object(value));
    }
    Err(format!("Use self or a quoted layer id, got: {source}"))
}

fn parse_quoted_action_string(source: &str, label: &str) -> Result<String, String> {
    let source = source.trim();
    if source.len() >= 2 && source.starts_with('"') && source.ends_with('"') {
        let value = source[1..source.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
        if !value.trim().is_empty() {
            return Ok(value);
        }
    }
    Err(format!("Use a non-empty quoted {label}, got: {source}"))
}

fn parse_action_property(source: &str) -> Result<MouseActionProperty, String> {
    MouseActionProperty::parse(source).ok_or_else(|| {
        format!(
            "Unknown layer property '{}'; use render, visibility, x, y, width, height, or rotation",
            source.trim()
        )
    })
}

fn action_target_id<'a>(target: &'a MouseActionTarget, self_id: &'a str) -> &'a str {
    match target {
        MouseActionTarget::SelfObject => self_id,
        MouseActionTarget::Object(id) => id,
    }
}

fn surface_object_by_id<'a>(surface: &'a SceneObject, id: &str) -> Option<&'a SceneObject> {
    std::iter::once(surface)
        .chain(surface.children.iter())
        .find(|object| object.id.eq_ignore_ascii_case(id))
}

fn surface_object_by_id_mut<'a>(
    surface: &'a mut SceneObject,
    id: &str,
) -> Option<&'a mut SceneObject> {
    if surface.id.eq_ignore_ascii_case(id) {
        return Some(surface);
    }
    surface
        .children
        .iter_mut()
        .find(|object| object.id.eq_ignore_ascii_case(id))
}

fn mouse_action_target<'a>(
    theme: &'a ThemeDocument,
    source_surface_index: usize,
    self_id: &str,
    target: &MouseActionTarget,
) -> Result<(usize, &'a SceneObject), String> {
    let object_id = action_target_id(target, self_id);
    let source_surface = theme
        .surfaces
        .get(source_surface_index)
        .ok_or_else(|| format!("Surface {source_surface_index} does not exist"))?;
    if matches!(target, MouseActionTarget::SelfObject) {
        let object = surface_object_by_id(source_surface, object_id)
            .ok_or_else(|| format!("Layer '{object_id}' does not exist"))?;
        return Ok((source_surface_index, object));
    }
    if let Some(object) = surface_object_by_id(source_surface, object_id) {
        return Ok((source_surface_index, object));
    }
    let mut matches = theme
        .surfaces
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != source_surface_index)
        .filter_map(|(index, surface)| {
            surface_object_by_id(surface, object_id).map(|object| (index, object))
        });
    let Some(found) = matches.next() else {
        return Err(format!("Layer '{object_id}' does not exist in this theme"));
    };
    if matches.next().is_some() {
        return Err(format!(
            "Layer id '{object_id}' is ambiguous across multiple root surfaces"
        ));
    }
    Ok(found)
}

fn mouse_property_expression(object: &SceneObject, property: MouseActionProperty) -> &Expression {
    match property {
        MouseActionProperty::Render => &object.render,
        MouseActionProperty::Visibility => &object.visibility,
        MouseActionProperty::X => &object.x,
        MouseActionProperty::Y => &object.y,
        MouseActionProperty::Width => &object.width,
        MouseActionProperty::Height => &object.height,
        MouseActionProperty::Rotation => &object.rotation,
    }
}

fn set_mouse_property_expression(
    object: &mut SceneObject,
    property: MouseActionProperty,
    expression: Expression,
) {
    *match property {
        MouseActionProperty::Render => &mut object.render,
        MouseActionProperty::Visibility => &mut object.visibility,
        MouseActionProperty::X => &mut object.x,
        MouseActionProperty::Y => &mut object.y,
        MouseActionProperty::Width => &mut object.width,
        MouseActionProperty::Height => &mut object.height,
        MouseActionProperty::Rotation => &mut object.rotation,
    } = expression;
}

pub fn validate_mouse_action_script(
    source: &str,
    theme: &ThemeDocument,
    source_surface_index: usize,
    self_id: &str,
    context: &DataContext,
) -> Vec<String> {
    let actions = match parse_mouse_actions(source) {
        Ok(actions) => actions,
        Err(error) => return vec![error],
    };
    let mut errors = Vec::new();
    for action in actions {
        let (target, property, value) = match action {
            MouseAction::ShowDashboard
            | MouseAction::ToggleDashboard
            | MouseAction::OpenUrl { .. }
            | MouseAction::ShowContextMenu { .. } => continue,
            MouseAction::Set {
                target,
                property,
                value,
            } => (target, property, Some(value)),
            MouseAction::Increase {
                target,
                property,
                amount,
            }
            | MouseAction::Decrease {
                target,
                property,
                amount,
            } => (target, property, Some(amount)),
            MouseAction::Toggle { target, property } | MouseAction::Reset { target, property } => {
                (target, property, None)
            }
        };
        let (target_surface_index, target_object) =
            match mouse_action_target(theme, source_surface_index, self_id, &target) {
                Ok(target) => target,
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            };
        let target_is_root = theme
            .surfaces
            .get(target_surface_index)
            .is_some_and(|surface| surface.id.eq_ignore_ascii_case(&target_object.id));
        if target_is_root && matches!(property, MouseActionProperty::X | MouseActionProperty::Y) {
            errors.push(format!(
                "{}.{} cannot be changed because root layers use screen placement",
                target_object.name,
                property.name()
            ));
        }
        if let Some(value) = value {
            if let Err(error) = evaluate(&value.0, context) {
                errors.push(format!(
                    "{}.{} value: {error}",
                    target_object.name,
                    property.name()
                ));
            }
        }
    }
    errors
}

pub fn mouse_event_script<'a>(
    theme: &'a ThemeDocument,
    surface_index: usize,
    object_id: &str,
    event: MouseEventKind,
) -> Option<&'a str> {
    let surface = theme.surfaces.get(surface_index)?;
    let object = surface_object_by_id(surface, object_id)?;
    object
        .mouse_events
        .as_ref()
        .map(|events| events.handler(event).trim())
        .filter(|handler| !handler.is_empty())
}

pub fn apply_mouse_action_overrides(
    theme: &ThemeDocument,
    overrides: &HashMap<MouseActionOverrideKey, Expression>,
) -> ThemeDocument {
    let mut effective = theme.clone();
    for (key, expression) in overrides {
        let Some(surface) = effective.surfaces.get_mut(key.surface_index) else {
            continue;
        };
        let is_root = surface.id.eq_ignore_ascii_case(&key.object_id);
        if is_root
            && matches!(
                key.property,
                MouseActionProperty::X | MouseActionProperty::Y
            )
        {
            continue;
        }
        if let Some(object) = surface_object_by_id_mut(surface, &key.object_id) {
            set_mouse_property_expression(object, key.property, expression.clone());
        }
    }
    effective.prepare_runtime();
    effective
}

pub(super) fn mouse_action_object_context(
    theme: &ThemeDocument,
    surface_index: usize,
    object_id: &str,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
) -> Result<DataContext, String> {
    let surface = theme
        .surfaces
        .get(surface_index)
        .ok_or_else(|| format!("Surface {surface_index} does not exist"))?;
    let (width, height) = resolve_surface_content_size(theme, surface_index, data, runtime);
    let canvas = Canvas {
        width,
        width_expression: Some(surface.width.clone()),
        height,
        height_expression: Some(surface.height.clone()),
        background: surface.background.canvas_paint(),
    };
    let context = DataContext::from_usage_with_runtime(data, &canvas, runtime);
    if surface.id.eq_ignore_ascii_case(object_id) {
        return Ok(context);
    }
    let (resolved, _) = resolve_objects_for(surface, &canvas, &surface.children, data, runtime);
    Ok(resolved
        .iter()
        .find(|object| object.source.id.eq_ignore_ascii_case(object_id))
        .map_or(context.clone(), |object| context.with_object(object)))
}

struct MouseActionExecutionContext<'a> {
    theme: &'a ThemeDocument,
    surface_index: usize,
    self_id: &'a str,
    data: Option<&'a AppUsageData>,
    runtime: ThemeRuntime,
}

impl MouseActionExecutionContext<'_> {
    fn adjust_property(
        &self,
        target: &MouseActionTarget,
        property: MouseActionProperty,
        amount: &Expression,
        direction: f64,
        overrides: &mut HashMap<MouseActionOverrideKey, Expression>,
    ) -> Result<(), String> {
        let effective = apply_mouse_action_overrides(self.theme, overrides);
        let (target_surface_index, object) =
            mouse_action_target(&effective, self.surface_index, self.self_id, target)?;
        let surface = &effective.surfaces[target_surface_index];
        let object_id = object.id.clone();
        if object.id == surface.id
            && matches!(property, MouseActionProperty::X | MouseActionProperty::Y)
        {
            return Err(format!(
                "{}.{} cannot be changed because root layers use screen placement",
                object.name,
                property.name()
            ));
        }
        let context = mouse_action_object_context(
            &effective,
            target_surface_index,
            &object_id,
            self.data,
            self.runtime,
        )?;
        let current = evaluate(&mouse_property_expression(object, property).0, &context)?;
        let amount = evaluate(&amount.0, &context)?;
        let adjusted = current + amount * direction;
        if !adjusted.is_finite() {
            return Err(format!(
                "{}.{} adjustment did not produce a finite value",
                object.name,
                property.name()
            ));
        }
        overrides.insert(
            MouseActionOverrideKey {
                surface_index: target_surface_index,
                object_id,
                property,
            },
            Expression(format_number(adjusted)),
        );
        Ok(())
    }
}

pub fn execute_mouse_actions(
    theme: &ThemeDocument,
    surface_index: usize,
    self_id: &str,
    source: &str,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
    overrides: &mut HashMap<MouseActionOverrideKey, Expression>,
) -> Result<Vec<MouseActionEffect>, String> {
    let actions = parse_mouse_actions(source)?;
    let execution = MouseActionExecutionContext {
        theme,
        surface_index,
        self_id,
        data,
        runtime,
    };
    let mut effects = Vec::new();
    for action in actions {
        match action {
            MouseAction::ShowDashboard => effects.push(MouseActionEffect::ShowDashboard),
            MouseAction::ToggleDashboard => effects.push(MouseActionEffect::ToggleDashboard),
            MouseAction::OpenUrl { url } => effects.push(MouseActionEffect::OpenUrl(url)),
            MouseAction::ShowContextMenu { menu } => {
                effects.push(MouseActionEffect::ShowContextMenu(menu))
            }
            MouseAction::Set {
                target,
                property,
                value,
            } => {
                let (target_surface_index, object) =
                    mouse_action_target(theme, surface_index, self_id, &target)?;
                let surface = &theme.surfaces[target_surface_index];
                let object_id = object.id.clone();
                if object.id == surface.id
                    && matches!(property, MouseActionProperty::X | MouseActionProperty::Y)
                {
                    return Err(format!(
                        "{}.{} cannot be changed because root layers use screen placement",
                        object.name,
                        property.name()
                    ));
                }
                overrides.insert(
                    MouseActionOverrideKey {
                        surface_index: target_surface_index,
                        object_id,
                        property,
                    },
                    value,
                );
            }
            MouseAction::Reset { target, property } => {
                let (target_surface_index, object) =
                    mouse_action_target(theme, surface_index, self_id, &target)?;
                overrides.remove(&MouseActionOverrideKey {
                    surface_index: target_surface_index,
                    object_id: object.id.clone(),
                    property,
                });
            }
            MouseAction::Increase {
                target,
                property,
                amount,
            } => execution.adjust_property(&target, property, &amount, 1.0, overrides)?,
            MouseAction::Decrease {
                target,
                property,
                amount,
            } => execution.adjust_property(&target, property, &amount, -1.0, overrides)?,
            MouseAction::Toggle { target, property } => {
                let effective = apply_mouse_action_overrides(theme, overrides);
                let (target_surface_index, object) =
                    mouse_action_target(&effective, surface_index, self_id, &target)?;
                let surface = &effective.surfaces[target_surface_index];
                let object_id = object.id.clone();
                let (width, height) =
                    resolve_surface_content_size(&effective, target_surface_index, data, runtime);
                let canvas = Canvas {
                    width,
                    width_expression: Some(surface.width.clone()),
                    height,
                    height_expression: Some(surface.height.clone()),
                    background: surface.background.canvas_paint(),
                };
                let context = DataContext::from_usage_with_runtime(data, &canvas, runtime);
                let current = evaluate(&mouse_property_expression(object, property).0, &context)
                    .unwrap_or(0.0);
                overrides.insert(
                    MouseActionOverrideKey {
                        surface_index: target_surface_index,
                        object_id,
                        property,
                    },
                    Expression(if current == 0.0 { "1" } else { "0" }.into()),
                );
            }
        }
    }
    Ok(effects)
}
