//! Versioned custom theme documents, expression evaluation, and live data binding.
//!
//! The renderer deliberately consumes this module's resolved scene rather than the
//! persisted structs directly. That keeps malformed user input recoverable and
//! makes theme files portable across machines and future renderer revisions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::*;

use crate::localization::LanguageId;
use crate::models::AppUsageData;
use crate::providers::{ProviderId, ProviderSet, PROVIDER_DESCRIPTORS};

pub const THEME_SCHEMA_VERSION: u32 = 1;
pub const CLASSIC_THEME_ID: &str = "classic-usage-widget";
pub const MINECRAFT_THEME_ID: &str = "theme-minecraft";

const BUILTIN_THEME_SOURCES: &[(&str, &str)] = &[(
    CLASSIC_THEME_ID,
    include_str!("themes/classic-usage-widget.json"),
)];

/// Bundled starting points are copied into the managed library only when they
/// are missing. Their ids are deliberately excluded from `is_builtin_theme_id`
/// so users can edit, rename, export, or delete them in Theme Studio.
const BUNDLED_EDITABLE_THEME_SOURCES: &[(&str, &str)] = &[(
    MINECRAFT_THEME_ID,
    include_str!("themes/minecraft-codex.json"),
)];
const BUNDLED_EDITABLE_INSTALL_MARKER: &str = ".minecraft-theme-installed";

const BUNDLED_THEME_ASSETS: &[(&str, &[u8])] = &[
    (
        "minecraft-empty.png",
        include_bytes!("themes/assets/minecraft-empty.png"),
    ),
    (
        "minecraft-full.png",
        include_bytes!("themes/assets/minecraft-full.png"),
    ),
];

const REMOVED_BUILTIN_THEME_IDS: &[&str] = &[
    "mission-control",
    "neon-reactor",
    "rpg-party-hud",
    "quota-garden",
    "tokyo-data-skyline",
    "pixel-arcade",
    "minimal-signal",
    "quota-constellation",
    "quota-orrery",
    "terminal-ticker",
    "tactical-edge-hud",
];

fn is_builtin_theme_id(id: &str) -> bool {
    BUILTIN_THEME_SOURCES
        .iter()
        .any(|(builtin_id, _)| id == *builtin_id)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThemeDocument {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    /// Runtime adapter used by the native window positioning code. The saved
    /// schema stores these values on the root `SceneObject` instead.
    #[serde(skip)]
    pub canvas: Canvas,
    #[serde(skip)]
    pub placement: Placement,
    #[serde(skip)]
    pub children: Vec<SceneObject>,
    #[serde(default)]
    pub surfaces: Vec<SceneObject>,
}

/// One visual object in a theme. Root objects are native desktop surfaces and
/// have `placement`; descendants have `parent` and use `anchor`. Content,
/// styling, layout, geometry, and the ability to contain children are otherwise
/// identical at every level.
#[derive(Clone, Debug, Serialize)]
pub struct SceneObject {
    pub id: String,
    pub name: String,
    #[serde(default = "default_render")]
    pub render: Expression,
    #[serde(default = "default_visibility")]
    pub visibility: Expression,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    pub placement: Placement,
    #[serde(default, skip_serializing_if = "ObjectAnchor::is_default")]
    pub anchor: ObjectAnchor,
    #[serde(default, skip_serializing_if = "Expression::is_zero")]
    pub x: Expression,
    #[serde(default, skip_serializing_if = "Expression::is_zero")]
    pub y: Expression,
    #[serde(default = "default_layer_width")]
    pub width: Expression,
    #[serde(default = "default_layer_height")]
    pub height: Expression,
    #[serde(default, skip_serializing_if = "Expression::is_zero")]
    pub rotation: Expression,
    #[serde(default)]
    pub background: LayerBackground,
    #[serde(default)]
    pub border: Option<Stroke>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse_events: Option<MouseEvents>,
    #[serde(default)]
    pub corner_radius: Expression,
    #[serde(default)]
    pub layout: ChildLayout,
    #[serde(default)]
    pub align: ChildAlignment,
    #[serde(default)]
    pub gap: Expression,
    #[serde(default)]
    pub content: SceneContent,
    /// Descendants are stored as a flat ordered list on each root. Their
    /// `parent` ids describe the hierarchy while preserving global z-order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SceneObject>,
}

/// Resolved root dimensions used to build expression contexts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Canvas {
    #[serde(default = "default_canvas_width")]
    pub width: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_expression: Option<Expression>,
    #[serde(default = "default_canvas_height")]
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_expression: Option<Expression>,
    #[serde(default)]
    pub background: Paint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    #[serde(default)]
    pub reference: ReferenceTarget,
    /// Controls which native shell host owns a root surface. Older themes did
    /// not persist this value, so they are normalized from their reference
    /// region by `prepare_runtime`.
    #[serde(default = "legacy_surface_nest", alias = "layer")]
    pub nest: SurfaceNest,
    #[serde(default)]
    pub horizontal: HorizontalAnchor,
    #[serde(default)]
    pub vertical: VerticalAnchor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_horizontal: Option<HorizontalAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_vertical: Option<VerticalAnchor>,
    #[serde(default)]
    pub offset_x: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_x_expression: Option<Expression>,
    #[serde(default)]
    pub offset_y: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_y_expression: Option<Expression>,
}

impl Placement {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceTarget {
    #[serde(default)]
    pub region: ReferenceRegion,
    #[serde(default)]
    pub display: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceRegion {
    Monitor,
    Taskbar,
    #[default]
    SystemTray,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceNest {
    /// Transitional value used only while loading themes saved before native
    /// surface hosting was introduced.
    #[default]
    Auto,
    Taskbar,
    /// A genuine Windows notification-area icon. Explorer owns its position,
    /// overflow state, DPI scaling, and drag ordering; the root object supplies
    /// the icon artwork.
    TrayIcon,
    Desktop,
    Floating,
}

impl SurfaceNest {
    pub fn resolve(self, reference: ReferenceRegion) -> Self {
        match self {
            Self::Auto => match reference {
                ReferenceRegion::Taskbar | ReferenceRegion::SystemTray => Self::Taskbar,
                ReferenceRegion::Monitor => Self::Floating,
            },
            nest => nest,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HorizontalAnchor {
    Left,
    Center,
    #[default]
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerticalAnchor {
    Top,
    Center,
    #[default]
    Bottom,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SceneContent {
    #[default]
    None,
    Text {
        #[serde(default)]
        template: String,
        #[serde(default = "default_font_family")]
        font_family: String,
        #[serde(default = "default_font_size")]
        font_size: Expression,
        #[serde(default)]
        weight: FontWeight,
        #[serde(default)]
        rendering: FontRendering,
        #[serde(default = "default_font_contrast")]
        contrast: Expression,
        #[serde(default)]
        align: TextAlign,
        #[serde(default = "default_text_paint")]
        color: Paint,
    },
    Progress {
        #[serde(default = "default_progress_value")]
        value: Expression,
        #[serde(default)]
        direction: ProgressDirection,
        #[serde(default = "default_accent_paint")]
        fill: Paint,
        #[serde(default = "default_track_paint")]
        track: Paint,
        #[serde(default)]
        corner_radius: Expression,
        #[serde(default)]
        segments: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        segments_expression: Option<Expression>,
        #[serde(default = "default_segment_gap")]
        segment_gap: Expression,
    },
}

impl SceneObject {
    pub fn object(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            render: default_render(),
            visibility: default_visibility(),
            parent: None,
            placement: Placement::default(),
            anchor: ObjectAnchor::default(),
            x: 0.0.into(),
            y: 0.0.into(),
            width: default_layer_width(),
            height: default_layer_height(),
            rotation: 0.0.into(),
            background: LayerBackground::default(),
            border: None,
            mouse_events: None,
            corner_radius: 0.0.into(),
            layout: ChildLayout::Freeform,
            align: ChildAlignment::Start,
            gap: 0.0.into(),
            content: SceneContent::None,
            children: Vec::new(),
        }
    }

    pub fn root(
        id: impl Into<String>,
        name: impl Into<String>,
        width: Expression,
        height: Expression,
        placement: Placement,
    ) -> Self {
        let mut object = Self::object(id, name);
        object.width = width;
        object.height = height;
        object.placement = placement;
        object
    }
}

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
    ShowContextMenu(Option<String>),
}

/// Parse the deliberately small, line-or-semicolon separated action language.
/// Supported forms include `show_dashboard()`, `toggle_dashboard()`,
/// `show_context_menu()`, `show_context_menu("menu-id")`, `set(self.render, false)`,
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
            _ => return Err(format!("Unknown action: {name}")),
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildLayout {
    #[default]
    Freeform,
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildAlignment {
    #[default]
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectHorizontalAnchor {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectVerticalAnchor {
    #[default]
    Top,
    Center,
    Bottom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectAnchor {
    #[serde(default)]
    pub horizontal: ObjectHorizontalAnchor,
    #[serde(default)]
    pub vertical: ObjectVerticalAnchor,
}

impl ObjectAnchor {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    Contain,
    Cover,
    #[default]
    Stretch,
    Original,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayerBackground {
    #[default]
    None,
    Colour {
        colour: Paint,
    },
    Gradient {
        start: Paint,
        end: Paint,
        #[serde(default)]
        angle: Expression,
    },
    Image {
        path: String,
        #[serde(default)]
        fit: ImageFit,
    },
}

impl LayerBackground {
    fn from_legacy_paint(paint: Paint) -> Self {
        if paint.color.trim().eq_ignore_ascii_case("#00000000") {
            Self::None
        } else {
            Self::Colour { colour: paint }
        }
    }

    fn canvas_paint(&self) -> Paint {
        match self {
            Self::Colour { colour } => colour.clone(),
            Self::None | Self::Gradient { .. } | Self::Image { .. } => Paint::default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LayerBackgroundWire {
    Current(LayerBackgroundCurrent),
    Legacy(Paint),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LayerBackgroundCurrent {
    None,
    Colour {
        colour: Paint,
    },
    Gradient {
        start: Paint,
        end: Paint,
        #[serde(default)]
        angle: Expression,
    },
    Image {
        path: String,
        #[serde(default)]
        fit: ImageFit,
    },
}

impl<'de> Deserialize<'de> for LayerBackground {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match LayerBackgroundWire::deserialize(deserializer)? {
            LayerBackgroundWire::Current(LayerBackgroundCurrent::None) => Self::None,
            LayerBackgroundWire::Current(LayerBackgroundCurrent::Colour { colour }) => {
                Self::Colour { colour }
            }
            LayerBackgroundWire::Current(LayerBackgroundCurrent::Gradient {
                start,
                end,
                angle,
            }) => Self::Gradient { start, end, angle },
            LayerBackgroundWire::Current(LayerBackgroundCurrent::Image { path, fit }) => {
                Self::Image { path, fit }
            }
            LayerBackgroundWire::Legacy(paint) => Self::from_legacy_paint(paint),
        })
    }
}

#[derive(Deserialize)]
struct SceneObjectWire {
    id: String,
    name: String,
    #[serde(default = "default_render")]
    render: Expression,
    #[serde(default = "default_visibility")]
    visibility: Expression,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    placement: Placement,
    #[serde(default)]
    anchor: ObjectAnchor,
    #[serde(default)]
    x: Expression,
    #[serde(default)]
    y: Expression,
    #[serde(default = "default_layer_width")]
    width: Expression,
    #[serde(default = "default_layer_height")]
    height: Expression,
    #[serde(default)]
    rotation: Expression,
    #[serde(default)]
    background: LayerBackground,
    #[serde(default)]
    border: Option<Stroke>,
    #[serde(default)]
    mouse_events: Option<MouseEvents>,
    #[serde(default)]
    corner_radius: Expression,
    #[serde(default)]
    layout: ChildLayout,
    #[serde(default)]
    align: ChildAlignment,
    #[serde(default)]
    gap: Expression,
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default)]
    children: Vec<SceneObject>,
}

#[derive(Deserialize)]
struct LegacyImageContent {
    path: String,
    #[serde(default)]
    fit: ImageFit,
}

#[derive(Deserialize)]
struct LegacyShapeContent {
    #[serde(default = "default_accent_paint")]
    fill: Paint,
    #[serde(default)]
    stroke: Option<Stroke>,
    #[serde(default)]
    corner_radius: Expression,
}

impl<'de> Deserialize<'de> for SceneObject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut wire = SceneObjectWire::deserialize(deserializer)?;
        let content_type = wire
            .content
            .as_ref()
            .and_then(|content| content.get("type"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let content = match (content_type.as_deref(), wire.content.take()) {
            (Some("image"), Some(value)) => {
                let legacy: LegacyImageContent =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                wire.background = LayerBackground::Image {
                    path: legacy.path,
                    fit: legacy.fit,
                };
                SceneContent::None
            }
            (Some("shape"), Some(value)) => {
                let legacy: LegacyShapeContent =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                wire.background = LayerBackground::from_legacy_paint(legacy.fill);
                if wire.border.is_none() {
                    wire.border = legacy.stroke;
                }
                if wire.corner_radius.is_zero() {
                    wire.corner_radius = legacy.corner_radius;
                }
                SceneContent::None
            }
            (_, Some(serde_json::Value::Null) | None) => SceneContent::None,
            (_, Some(value)) => serde_json::from_value(value).map_err(serde::de::Error::custom)?,
        };
        Ok(Self {
            id: wire.id,
            name: wire.name,
            render: wire.render,
            visibility: wire.visibility,
            parent: wire.parent,
            placement: wire.placement,
            anchor: wire.anchor,
            x: wire.x,
            y: wire.y,
            width: wire.width,
            height: wire.height,
            rotation: wire.rotation,
            background: wire.background,
            border: wire.border,
            mouse_events: wire.mouse_events,
            corner_radius: wire.corner_radius,
            layout: wire.layout,
            align: wire.align,
            gap: wire.gap,
            content,
            children: wire.children,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressDirection {
    #[default]
    LeftToRight,
    RightToLeft,
    BottomToTop,
    TopToBottom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontWeight {
    Light,
    #[default]
    Regular,
    Medium,
    Semibold,
    Bold,
}

impl FontWeight {
    pub fn gdi_weight(self) -> i32 {
        match self {
            Self::Light => 300,
            Self::Regular => 400,
            Self::Medium => 500,
            Self::Semibold => 600,
            Self::Bold => 700,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontRendering {
    #[default]
    Antialiased,
    ClearType,
    Aliased,
}

impl FontRendering {
    fn gdi_quality(self) -> u32 {
        match self {
            Self::Antialiased => ANTIALIASED_QUALITY.0 as u32,
            Self::ClearType => CLEARTYPE_QUALITY.0 as u32,
            Self::Aliased => NONANTIALIASED_QUALITY.0 as u32,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stroke {
    #[serde(default = "default_text_paint")]
    pub color: Paint,
    #[serde(default = "default_stroke_width")]
    pub width: Expression,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Paint {
    #[serde(default = "default_paint_color")]
    pub color: String,
    #[serde(default = "default_opacity")]
    pub opacity: Expression,
}

impl Default for Paint {
    fn default() -> Self {
        Self {
            color: "#00000000".to_string(),
            opacity: Expression::from(1.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Expression(pub String);

impl Default for Expression {
    fn default() -> Self {
        Self("0".to_string())
    }
}

impl Expression {
    fn is_zero(&self) -> bool {
        self.0.trim().parse::<f64>().is_ok_and(|value| value == 0.0)
    }
}

impl From<f64> for Expression {
    fn from(value: f64) -> Self {
        Self(format_number(value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Clone, Debug)]
pub struct ResolvedObject<'a> {
    pub source: &'a SceneObject,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub parent_width: f64,
    pub parent_height: f64,
    pub opacity: f64,
    pub rotation: f64,
    pub clip: Vec<ClipRegion>,
}

#[derive(Clone, Copy, Debug)]
pub struct ClipRegion {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    rotation: f64,
}

#[derive(Clone, Debug)]
pub struct RenderedTheme {
    pub width: u32,
    pub height: u32,
    /// Top-down, premultiplied BGRA pixels, represented as 0xAARRGGBB.
    pub pixels: Vec<u32>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DataContext {
    values: HashMap<String, f64>,
    strings: HashMap<String, String>,
}

/// Runtime environment used by theme expressions and layout. Provider state is
/// deliberately independent of polling availability so a temporary provider
/// error never causes the widget to jump or resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeRuntime {
    providers: ProviderSet,
    pub poll_ok: bool,
    pub has_error: bool,
    pub language: LanguageId,
    host_width: u32,
    host_height: u32,
}

impl Default for ThemeRuntime {
    fn default() -> Self {
        Self {
            providers: ProviderSet::default(),
            poll_ok: true,
            has_error: false,
            language: LanguageId::English,
            host_width: default_canvas_width(),
            host_height: default_canvas_height(),
        }
    }
}

impl ThemeRuntime {
    #[cfg(test)]
    pub fn new(claude_enabled: bool, codex_enabled: bool, antigravity_enabled: bool) -> Self {
        let providers = ProviderSet::from_enabled(
            [
                (ProviderId::Claude, claude_enabled),
                (ProviderId::Codex, codex_enabled),
                (ProviderId::Antigravity, antigravity_enabled),
            ]
            .into_iter()
            .filter_map(|(provider, enabled)| enabled.then_some(provider)),
        );
        Self::from_providers(providers)
    }

    pub fn from_providers(providers: ProviderSet) -> Self {
        Self {
            providers,
            poll_ok: true,
            has_error: false,
            language: LanguageId::English,
            host_width: default_canvas_width(),
            host_height: default_canvas_height(),
        }
    }

    pub fn with_poll_state(mut self, poll_ok: bool, has_error: bool) -> Self {
        self.poll_ok = poll_ok;
        self.has_error = has_error;
        self
    }

    pub fn with_language(mut self, language: LanguageId) -> Self {
        self.language = language;
        self
    }

    /// Supply the selected native host's 96-DPI logical dimensions. Theme
    /// expressions consume these as `host.width` and `host.height`.
    pub fn with_host_dimensions(mut self, width: u32, height: u32) -> Self {
        self.host_width = width.max(1);
        self.host_height = height.max(1);
        self
    }

    pub fn provider_count(self) -> usize {
        self.providers.len().max(1)
    }

    pub fn provider_enabled(self, provider: ProviderId) -> bool {
        self.providers.contains(provider)
    }
}

impl DataContext {
    pub fn from_usage(data: Option<&AppUsageData>, canvas: &Canvas) -> Self {
        Self::from_usage_with_runtime(data, canvas, ThemeRuntime::default())
    }

    pub fn from_usage_with_runtime(
        data: Option<&AppUsageData>,
        canvas: &Canvas,
        runtime: ThemeRuntime,
    ) -> Self {
        let mut context = Self::default();
        context.insert("canvas.width", canvas.width as f64);
        context.insert("canvas.height", canvas.height as f64);
        context.insert("parent.width", canvas.width as f64);
        context.insert("parent.height", canvas.height as f64);
        context.insert("host.width", runtime.host_width as f64);
        context.insert("host.height", runtime.host_height as f64);
        context.insert("pi", std::f64::consts::PI);
        context.insert("e", std::f64::consts::E);
        context.insert("true", 1.0);
        context.insert("false", 0.0);
        context.insert_string("app.version", env!("CARGO_PKG_VERSION"));
        let mut version_parts = env!("CARGO_PKG_VERSION")
            .split(['.', '-', '+'])
            .filter_map(|part| part.parse::<f64>().ok());
        context.insert("app.version.major", version_parts.next().unwrap_or(0.0));
        context.insert("app.version.minor", version_parts.next().unwrap_or(0.0));
        context.insert("app.version.patch", version_parts.next().unwrap_or(0.0));
        context.insert("system.dark", crate::theme::is_dark_mode() as u8 as f64);
        context.insert("data.poll_ok", runtime.poll_ok as u8 as f64);
        context.insert("data.has_error", runtime.has_error as u8 as f64);
        context.insert(
            "data.loading",
            (!runtime.poll_ok && !runtime.has_error) as u8 as f64,
        );
        let strings = runtime.language.strings();
        context.insert_string("i18n.session_window", strings.session_window);
        context.insert_string("i18n.weekly_window", strings.weekly_window);
        context.insert_string("i18n.cursor_auto_window", strings.cursor_auto_window);
        context.insert_string("i18n.cursor_api_window", strings.cursor_api_window);
        context.insert_string("i18n.now", strings.now);
        context.insert_string("i18n.day_suffix", strings.day_suffix);
        context.insert_string("i18n.hour_suffix", strings.hour_suffix);
        context.insert_string("i18n.minute_suffix", strings.minute_suffix);
        context.insert_string("i18n.second_suffix", strings.second_suffix);
        context.insert("providers.count", runtime.provider_count() as f64);
        for descriptor in PROVIDER_DESCRIPTORS {
            context.insert(
                &format!("providers.{}.enabled", descriptor.key),
                runtime.provider_enabled(descriptor.id) as u8 as f64,
            );
        }
        if let Some(data) = data {
            for descriptor in PROVIDER_DESCRIPTORS {
                context.insert_provider(descriptor.key, data.get(descriptor.id));
            }
            let active = ProviderId::ALL
                .into_iter()
                .find_map(|provider| data.get(provider));
            context.insert_provider("active", active);
        } else {
            for descriptor in PROVIDER_DESCRIPTORS {
                context.insert_provider(descriptor.key, None);
            }
            context.insert_provider("active", None);
        }
        context
    }

    fn insert_provider(&mut self, name: &str, usage: Option<&crate::models::UsageData>) {
        let weekly_label = usage
            .and_then(|usage| usage.weekly_label.as_deref())
            .or_else(|| self.get_string("i18n.weekly_window"))
            .unwrap_or("7d")
            .to_string();
        self.insert_string(&format!("{name}.weekly.label"), weekly_label);
        let (session, weekly) = usage
            .map(|usage| (usage.session.percentage, usage.weekly.percentage))
            .unwrap_or((0.0, 0.0));
        self.insert(&format!("{name}.session.percentage"), session);
        self.insert(&format!("{name}.session.remaining"), 100.0 - session);
        self.insert(&format!("{name}.weekly.percentage"), weekly);
        self.insert(&format!("{name}.weekly.remaining"), 100.0 - weekly);
        self.insert(&format!("{name}.available"), usage.is_some() as u8 as f64);
        // Credits are absent for most accounts, so `credits.available` is what
        // a theme should gate the overlay on rather than `available`.
        let credits = usage.and_then(|usage| usage.credits.as_ref());
        let credits_percentage = credits.map(|credits| credits.percentage).unwrap_or(0.0);
        self.insert(&format!("{name}.credits.percentage"), credits_percentage);
        self.insert(
            &format!("{name}.credits.remaining"),
            100.0 - credits_percentage,
        );
        // Currency, unlike the percentages either side of it.
        self.insert(
            &format!("{name}.credits.balance"),
            credits.map(|credits| credits.remaining).unwrap_or(0.0),
        );
        self.insert(
            &format!("{name}.credits.total"),
            credits.map(|credits| credits.total).unwrap_or(0.0),
        );
        self.insert(
            &format!("{name}.credits.available"),
            credits.is_some() as u8 as f64,
        );
        // The single figure a badge should show: whatever is closest to its
        // limit. A provider can switch a window off entirely -- Codex has its
        // five-hour window disabled -- so binding a badge to one window alone
        // leaves it reporting 0% while another allowance is spent.
        self.insert(
            &format!("{name}.headline.percentage"),
            match credits {
                Some(credits) => credits.percentage,
                None => session.max(weekly),
            },
        );
        let reset_value = |reset: Option<std::time::SystemTime>| {
            let unix = reset
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|value| value.as_secs_f64())
                .unwrap_or(0.0);
            let seconds = reset
                .and_then(|value| value.duration_since(std::time::SystemTime::now()).ok())
                .map(|value| value.as_secs_f64())
                .unwrap_or(0.0);
            (unix, seconds)
        };
        let (session_unix, session_seconds) =
            reset_value(usage.and_then(|value| value.session.resets_at));
        let (weekly_unix, weekly_seconds) =
            reset_value(usage.and_then(|value| value.weekly.resets_at));
        for (window, unix, seconds) in [
            ("session", session_unix, session_seconds),
            ("weekly", weekly_unix, weekly_seconds),
        ] {
            self.insert(&format!("{name}.{window}.reset.unix"), unix);
            self.insert(&format!("{name}.{window}.reset.seconds"), seconds);
            self.insert(&format!("{name}.{window}.reset.minutes"), seconds / 60.0);
            self.insert(&format!("{name}.{window}.reset.hours"), seconds / 3600.0);
            self.insert(&format!("{name}.{window}.reset.days"), seconds / 86400.0);
        }
    }

    pub fn insert(&mut self, name: &str, value: f64) {
        self.values.insert(name.to_ascii_lowercase(), value);
    }

    pub fn get(&self, name: &str) -> Option<f64> {
        self.values.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn insert_string(&mut self, name: &str, value: impl Into<String>) {
        self.strings.insert(name.to_ascii_lowercase(), value.into());
    }

    pub fn get_string(&self, name: &str) -> Option<&str> {
        self.strings
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn with_object(mut self, object: &ResolvedObject<'_>) -> Self {
        self.insert("object.x", object.x);
        self.insert("object.y", object.y);
        self.insert("object.width", object.width);
        self.insert("object.height", object.height);
        self.insert("parent.width", object.parent_width);
        self.insert("parent.height", object.parent_height);
        self
    }
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

fn mouse_action_object_context(
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
    let (width, height) = resolve_surface_size(theme, surface_index, data, runtime);
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
                    resolve_surface_size(&effective, target_surface_index, data, runtime);
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

impl ThemeDocument {
    pub fn is_builtin(&self) -> bool {
        is_builtin_theme_id(&self.id)
    }

    pub fn is_builtin_classic(&self) -> bool {
        self.id == CLASSIC_THEME_ID
    }

    /// Identifies the short-lived first Studio approximation so it can be
    /// upgraded without replacing themes users have renamed or repurposed.
    pub fn is_obsolete_studio_starter(&self) -> bool {
        (self.id == "classic-segments" && self.name == "Classic Segments")
            || (self.id == "midnight-glass" && self.name == "Midnight Glass")
            || (self.id == "classic-usage-widget" && self.name == "Classic Usage Widget")
    }

    pub fn starter() -> Self {
        let mut theme: Self = serde_json::from_str(BUILTIN_THEME_SOURCES[0].1)
            .expect("built-in Classic theme must be valid JSON");
        theme.prepare_runtime();
        theme
    }

    /// Create the writable one-time upgrade target for pre-theme installs.
    /// Only the primary taskbar surface inherits legacy placement and
    /// visibility; notification-area roots remain registered with Explorer.
    pub fn migrated_from_legacy(placement: Option<(usize, i32)>, widget_visible: bool) -> Self {
        let mut theme = Self::starter();
        theme.id = "migrated-theme".into();
        theme.name = "Migrated Theme".into();
        if let Some(surface) = theme.surfaces.first_mut() {
            surface.render = (if widget_visible { 1.0 } else { 0.0 }).into();
            if let Some((display, offset_x)) = placement {
                surface.placement.reference.display = display;
                surface.placement.offset_x = offset_x;
                surface.placement.offset_x_expression = None;
            }
        }
        theme
    }

    pub fn prepare_runtime(&mut self) {
        // The empty-surface path supports documents assembled through the
        // runtime adapter fields before their root surface is materialized.
        if self.surfaces.is_empty() {
            let width = self
                .canvas
                .width_expression
                .clone()
                .unwrap_or_else(|| (self.canvas.width as f64).into());
            let height = self
                .canvas
                .height_expression
                .clone()
                .unwrap_or_else(|| (self.canvas.height as f64).into());
            let mut root = SceneObject::root(
                "main",
                "Main surface",
                width,
                height,
                self.placement.clone(),
            );
            root.background = LayerBackground::from_legacy_paint(self.canvas.background.clone());
            root.children = std::mem::take(&mut self.children);
            self.surfaces.push(root);
        }
        for surface in &mut self.surfaces {
            surface.placement.nest = surface
                .placement
                .nest
                .resolve(surface.placement.reference.region);
        }
        if let Some(surface) = self.surfaces.first() {
            let (width, height) =
                resolve_object_size(surface, None, ThemeRuntime::default(), &mut Vec::new());
            self.canvas = Canvas {
                width,
                width_expression: Some(surface.width.clone()),
                height,
                height_expression: Some(surface.height.clone()),
                background: surface.background.canvas_paint(),
            };
            self.placement = surface.placement.clone();
            self.children = surface.children.clone();
        }
    }

    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.schema_version != THEME_SCHEMA_VERSION {
            errors.push(format!(
                "Theme schema {} is not supported; expected {}",
                self.schema_version, THEME_SCHEMA_VERSION
            ));
        }
        if self.surfaces.is_empty() {
            errors.push("A theme needs at least one root object".into());
            return errors;
        }
        let mut root_ids = std::collections::HashSet::new();
        for (surface_index, root) in self.surfaces.iter().enumerate() {
            if root.parent.is_some() {
                errors.push(format!(
                    "{}.parent: root objects cannot have a parent",
                    root.name
                ));
            }
            if root.id.trim().is_empty() || !root_ids.insert(root.id.to_ascii_lowercase()) {
                errors.push(format!("Root object '{}' needs a unique id", root.name));
            }
            if !root.anchor.is_default() || !root.x.is_zero() || !root.y.is_zero() {
                errors.push(format!(
                    "{}: root objects use screen placement rather than parent anchors or X/Y offsets",
                    root.name
                ));
            }
            let (width, height) =
                resolve_object_size(root, None, ThemeRuntime::default(), &mut errors);
            let canvas = Canvas {
                width,
                width_expression: Some(root.width.clone()),
                height,
                height_expression: Some(root.height.clone()),
                background: root.background.canvas_paint(),
            };
            let context = DataContext::from_usage(None, &canvas);
            validate_scene_object(&mut errors, &context, root);
            validate_scene_mouse_events(&mut errors, &context, self, surface_index, root);

            let mut ids = std::collections::HashSet::new();
            ids.insert(root.id.to_ascii_lowercase());
            for object in &root.children {
                if object.id.trim().is_empty() {
                    errors.push(format!("{}: every object needs an id", root.name));
                } else if !ids.insert(object.id.to_ascii_lowercase()) {
                    errors.push(format!("{}: duplicate object id: {}", root.name, object.id));
                }
                if !object.children.is_empty() {
                    errors.push(format!(
                        "{}.children: descendants must be stored in their root's ordered child list",
                        object.name
                    ));
                }
                if !object.placement.is_default() {
                    errors.push(format!(
                        "{}.placement: screen placement is only available on root objects",
                        object.name
                    ));
                }
                validate_scene_object(&mut errors, &context, object);
                validate_scene_mouse_events(&mut errors, &context, self, surface_index, object);
            }
            let by_id: HashMap<&str, &SceneObject> = std::iter::once(root)
                .chain(root.children.iter())
                .map(|object| (object.id.as_str(), object))
                .collect();
            for object in &root.children {
                let mut cursor = object.parent.as_deref().or(Some(root.id.as_str()));
                let mut visited = std::collections::HashSet::new();
                while let Some(parent_id) = cursor {
                    if !visited.insert(parent_id) {
                        errors.push(format!(
                            "{}.parent: hierarchy contains a cycle",
                            object.name
                        ));
                        break;
                    }
                    let Some(parent) = by_id.get(parent_id).copied() else {
                        errors.push(format!(
                            "{}.parent: '{}' does not exist in root '{}'",
                            object.name, parent_id, root.name
                        ));
                        break;
                    };
                    cursor = parent.parent.as_deref();
                }
            }
        }
        errors
    }
}

fn validate_scene_mouse_events(
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

fn validate_scene_object(errors: &mut Vec<String>, context: &DataContext, object: &SceneObject) {
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
            context,
            &format!("{}.{}", object.name, property),
            expression,
        );
    }
    match &object.background {
        LayerBackground::None => {}
        LayerBackground::Colour { colour } => validate_paint(
            errors,
            context,
            &format!("{}.background.colour", object.name),
            colour,
        ),
        LayerBackground::Gradient { start, end, angle } => {
            validate_paint(
                errors,
                context,
                &format!("{}.background.gradient.start", object.name),
                start,
            );
            validate_paint(
                errors,
                context,
                &format!("{}.background.gradient.end", object.name),
                end,
            );
            validate_expression(
                errors,
                context,
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
            context,
            &format!("{}.border", object.name),
            &border.color,
        );
        validate_expression(
            errors,
            context,
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
                context,
                &format!("{}.font_size", object.name),
                font_size,
            );
            validate_expression(
                errors,
                context,
                &format!("{}.contrast", object.name),
                contrast,
            );
            validate_paint(errors, context, &format!("{}.color", object.name), color);
            for error in validate_template(template, context) {
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
                    context,
                    &format!("{}.{}", object.name, name),
                    expression,
                );
            }
            if let Some(expression) = segments_expression {
                validate_expression(
                    errors,
                    context,
                    &format!("{}.segments", object.name),
                    expression,
                );
            }
            validate_paint(errors, context, &format!("{}.fill", object.name), fill);
            validate_paint(errors, context, &format!("{}.track", object.name), track);
        }
    }
}

fn validate_expression(
    errors: &mut Vec<String>,
    context: &DataContext,
    label: &str,
    expression: &Expression,
) {
    if let Err(error) = evaluate(&expression.0, context) {
        errors.push(format!("{label}: {error}"));
    }
}

fn validate_paint(errors: &mut Vec<String>, context: &DataContext, label: &str, paint: &Paint) {
    if parse_color(&paint.color).is_none() {
        errors.push(format!(
            "{label}: '{}' is not #RRGGBB or #AARRGGBB",
            paint.color
        ));
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
        let (expression, format) = token.rsplit_once(':').unwrap_or((token, "0.##"));
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
        if let Err(error) = evaluate(expression, context) {
            errors.push(error);
        }
        remaining = &remaining[end + 1..];
    }
    errors
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            width: default_canvas_width(),
            width_expression: None,
            height: default_canvas_height(),
            height_expression: None,
            background: Paint::default(),
        }
    }
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            reference: ReferenceTarget::default(),
            nest: SurfaceNest::Taskbar,
            horizontal: HorizontalAnchor::Left,
            vertical: VerticalAnchor::Center,
            surface_horizontal: Some(HorizontalAnchor::Right),
            surface_vertical: Some(VerticalAnchor::Center),
            offset_x: -12,
            offset_x_expression: None,
            offset_y: 0,
            offset_y_expression: None,
        }
    }
}

fn legacy_surface_nest() -> SurfaceNest {
    SurfaceNest::Auto
}

impl Paint {
    pub fn new(color: &str) -> Self {
        Self {
            color: color.to_string(),
            opacity: 1.0.into(),
        }
    }
    pub fn resolve(&self, context: &DataContext) -> Rgba {
        let mut rgba = parse_color(&self.color).unwrap_or(Rgba {
            r: 255,
            g: 0,
            b: 255,
            a: 255,
        });
        let opacity = evaluate(&self.opacity.0, context)
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        rgba.a = ((rgba.a as f64) * opacity).round() as u8;
        rgba
    }
}

mod theme_storage;
pub use theme_storage::*;

mod theme_rendering;
pub use theme_rendering::*;

mod theme_expression;
pub use theme_expression::*;
fn schema_version() -> u32 {
    THEME_SCHEMA_VERSION
}
fn default_render() -> Expression {
    1.0.into()
}
fn default_visibility() -> Expression {
    100.0.into()
}
fn default_canvas_width() -> u32 {
    292
}
fn default_canvas_height() -> u32 {
    104
}
fn default_layer_width() -> Expression {
    100.0.into()
}
fn default_layer_height() -> Expression {
    32.0.into()
}
fn default_opacity() -> Expression {
    1.0.into()
}
fn default_font_size() -> Expression {
    13.0.into()
}
fn default_font_contrast() -> Expression {
    1.4.into()
}
fn default_stroke_width() -> Expression {
    1.0.into()
}
fn default_segment_gap() -> Expression {
    2.0.into()
}
fn default_progress_value() -> Expression {
    Expression("claude.session.percentage".into())
}
fn default_font_family() -> String {
    "Segoe UI Variable Text".into()
}
fn default_paint_color() -> String {
    "#00000000".into()
}
fn default_accent_paint() -> Paint {
    Paint::new("#D97757")
}
fn default_track_paint() -> Paint {
    Paint::new("#FFFFFF24")
}
fn default_text_paint() -> Paint {
    Paint::new("#FFFFFFFF")
}
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}
fn safe_file_stem(id: &str) -> String {
    let value: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if value.is_empty() {
        "custom-theme".into()
    } else {
        value
    }
}

#[cfg(test)]
mod tests;
