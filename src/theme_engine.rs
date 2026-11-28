//! Versioned custom theme documents, expression evaluation, and live data binding.
//!
//! The renderer deliberately consumes this module's resolved scene rather than the
//! persisted structs directly. That keeps malformed user input recoverable and
//! makes theme files portable across machines and future renderer revisions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::*;

use crate::localization::LanguageId;
use crate::models::AppUsageData;
use crate::providers::{ProviderId, ProviderSet, PROVIDER_DESCRIPTORS};

pub const THEME_SCHEMA_VERSION: u32 = 1;
pub const CLASSIC_THEME_ID: &str = "classic-usage-widget";
pub const COMPACT_FLUENT_QUAD_THEME_ID: &str = "compact-fluent-quad";
pub const CLASSIC_VERTICAL_THEME_ID: &str = "classic-vertical";
pub const STACKED_ROWS_THEME_ID: &str = "stacked-rows";
pub const MINECRAFT_THEME_ID: &str = "theme-minecraft";
pub const MINECRAFT_CLAUDE_THEME_ID: &str = "theme-minecraft-claude";

const BUILTIN_THEME_SOURCES: &[(&str, &str)] = &[
    (
        CLASSIC_THEME_ID,
        include_str!(concat!(env!("OUT_DIR"), "/classic-usage-widget.json")),
    ),
    (
        COMPACT_FLUENT_QUAD_THEME_ID,
        include_str!(concat!(env!("OUT_DIR"), "/compact-fluent-quad.json")),
    ),
    (
        CLASSIC_VERTICAL_THEME_ID,
        include_str!(concat!(env!("OUT_DIR"), "/classic-vertical.json")),
    ),
    (
        STACKED_ROWS_THEME_ID,
        include_str!(concat!(env!("OUT_DIR"), "/stacked-rows.json")),
    ),
];

/// Bundled starting points are copied into the managed library only when they
/// are missing. Their ids are deliberately excluded from `is_builtin_theme_id`
/// so users can edit, rename, export, or delete them in Theme Studio.
///
/// Entries are (theme id, theme JSON, install marker). Each theme has its own
/// marker so one added in a later release still reaches existing installs,
/// while a bundled theme the user deleted stays deleted.
const BUNDLED_EDITABLE_THEME_SOURCES: &[(&str, &str, &str)] = &[
    (
        MINECRAFT_THEME_ID,
        include_str!(concat!(env!("OUT_DIR"), "/minecraft-codex.json")),
        ".minecraft-theme-installed",
    ),
    (
        MINECRAFT_CLAUDE_THEME_ID,
        include_str!(concat!(env!("OUT_DIR"), "/minecraft-claude.json")),
        ".minecraft-claude-theme-installed",
    ),
];

const BUNDLED_THEME_ASSETS: &[(&str, &[u8])] = &[
    (
        "minecraft-empty.png",
        include_bytes!("themes/assets/minecraft-empty.png"),
    ),
    (
        "minecraft-full.png",
        include_bytes!("themes/assets/minecraft-full.png"),
    ),
    (
        "minecraft-claude-hearts-empty.png",
        include_bytes!("themes/assets/minecraft-claude-hearts-empty.png"),
    ),
    (
        "minecraft-claude-hearts-full.png",
        include_bytes!("themes/assets/minecraft-claude-hearts-full.png"),
    ),
    (
        "minecraft-claude-hunger-empty.png",
        include_bytes!("themes/assets/minecraft-claude-hunger-empty.png"),
    ),
    (
        "minecraft-claude-hunger-full.png",
        include_bytes!("themes/assets/minecraft-claude-hunger-full.png"),
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
    /// Native hover popup for a root surface. Omitted by older/custom themes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<SurfaceTooltip>,
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
    /// Runtime-only layout host retained during undocking; never edits the theme.
    #[serde(skip)]
    pub host_dimensions: Option<(u32, u32)>,
    /// Runtime-only protection for drag anchoring; authored placements stay exact.
    #[serde(skip)]
    pub clamp_taskbar_drag: bool,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceTooltip {
    UsageSummary,
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
    /// A provider's logo from the brand marks the dashboard draws, tinted
    /// with `color` and fitted, centred, into the object. Themes need no image
    /// assets of their own to show a provider.
    ProviderMark {
        /// A provider key, as used by bindings: `claude`, `codex`, ...
        #[serde(default = "default_provider_mark")]
        provider: String,
        #[serde(default = "default_text_paint")]
        color: Paint,
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
            tooltip: None,
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
        use serde::de::Error as _;
        // A background with a `type` is the current format, and its errors are
        // reported. Only an untyped paint is the legacy format; trying it after
        // a failed typed background would hide the error behind a default paint.
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("type").is_none() {
            return serde_json::from_value(value)
                .map(Self::from_legacy_paint)
                .map_err(D::Error::custom);
        }
        Ok(
            match serde_json::from_value(value).map_err(D::Error::custom)? {
                LayerBackgroundCurrent::None => Self::None,
                LayerBackgroundCurrent::Colour { colour } => Self::Colour { colour },
                LayerBackgroundCurrent::Gradient { start, end, angle } => {
                    Self::Gradient { start, end, angle }
                }
                LayerBackgroundCurrent::Image { path, fit } => Self::Image { path, fit },
            },
        )
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
    tooltip: Option<SurfaceTooltip>,
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
            tooltip: wire.tooltip,
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
    /// The colour in both appearances, or only in dark mode when
    /// `light_color` is set.
    #[serde(default = "default_paint_color")]
    pub color: String,
    /// An optional replacement used while Windows is in light mode, so one
    /// object can serve both appearances instead of a pair of layers gated on
    /// `system.dark`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light_color: Option<String>,
    #[serde(default = "default_opacity")]
    pub opacity: Expression,
}

impl Default for Paint {
    fn default() -> Self {
        Self {
            color: "#00000000".to_string(),
            light_color: None,
            opacity: Expression::from(1.0),
        }
    }
}

/// A formula stored as text, such as `"canvas.width - 8"` or
/// `"claude.session.percentage > 80"`. Most theme fields are expressions; the
/// values, operators and functions they can use are in `theme_language.rs` and
/// the generated `docs/theme-reference.md`.
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
    pub gap: f64,
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

impl ThemeDocument {
    /// How often a live clock can visibly change. Minute-only clocks avoid the
    /// much more expensive second-by-second surface rendering path.
    pub fn current_time_refresh_interval(&self) -> Option<Duration> {
        current_time_refresh_interval_for(self)
    }

    pub fn surface_current_time_refresh_interval(&self, surface_index: usize) -> Option<Duration> {
        current_time_refresh_interval_for(self.surfaces.get(surface_index)?)
    }

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
            for (field, expression) in [
                ("offset_x_expression", &root.placement.offset_x_expression),
                ("offset_y_expression", &root.placement.offset_y_expression),
            ] {
                if let Some(expression) = expression {
                    let label = format!("{}.placement.{field}", root.name);
                    validate_expression(&mut errors, &context, &label, expression);
                }
            }
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

fn current_time_refresh_interval_for(value: &impl Serialize) -> Option<Duration> {
    let source = serde_json::to_string(value).ok()?.to_ascii_lowercase();
    let uses_clock =
        source.contains("time.now") || source.contains("time.local") || source.contains("time.utc");
    if !uses_clock {
        return None;
    }

    let needs_seconds = source.contains("time.now.milliseconds")
        || source.contains("time.local.second")
        || source.contains("time.utc.second")
        || unix_clock_requires_second_refresh(&source);
    Some(Duration::from_secs(if needs_seconds { 1 } else { 60 }))
}

fn unix_clock_requires_second_refresh(source: &str) -> bool {
    let mut remaining = source;
    while let Some(index) = remaining.find("time.now.unix") {
        let after_clock = &remaining[index + "time.now.unix".len()..];
        let Some(token_end) = after_clock.find('}') else {
            return true;
        };
        let token_tail = &after_clock[..token_end];
        let Some((_, format)) = token_tail.rsplit_once(':') else {
            return true;
        };
        if !is_minute_or_slower_clock_format(format) {
            return true;
        }
        remaining = &after_clock[token_end + 1..];
    }
    false
}

fn is_minute_or_slower_clock_format(format: &str) -> bool {
    let format = format.trim().strip_prefix("utc_").unwrap_or(format.trim());
    matches!(
        format,
        "weekday_2"
            | "weekday_short"
            | "weekday_long"
            | "day"
            | "day_2"
            | "month"
            | "month_2"
            | "month_short"
            | "month_long"
            | "year_2"
            | "year"
            | "date"
            | "date_short"
            | "date_long"
            | "time"
            | "time_short"
            | "time_24"
            | "time_12"
            | "am_pm"
            | "datetime_short"
            | "iso_date"
    )
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
            host_dimensions: None,
            clamp_taskbar_drag: false,
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
            light_color: None,
            opacity: 1.0.into(),
        }
    }

    /// The colour string for an appearance: the light-mode colour when one is
    /// set and Windows is light, otherwise the shared colour.
    pub fn color_for(&self, dark: bool) -> &str {
        match &self.light_color {
            Some(light) if !dark => light,
            _ => &self.color,
        }
    }

    pub fn resolve(&self, context: &DataContext) -> Rgba {
        // Contexts without the system binding (tests, previews) count as dark,
        // which keeps the shared colour.
        let dark = context.get("system.dark").is_none_or(|dark| dark != 0.0);
        let mut rgba = parse_color(self.color_for(dark)).unwrap_or(Rgba {
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
mod theme_data_context;
mod theme_mouse_actions;
mod theme_validation;
pub use theme_data_context::*;
pub use theme_mouse_actions::*;
pub use theme_validation::*;

mod theme_datetime;
mod theme_language;
pub use theme_language::*;
mod theme_limits;
mod theme_render_environment;
use theme_datetime::*;
pub use theme_render_environment::{
    current_fixed_render_environment, with_fixed_render_environment, FixedRenderEnvironment,
};
use theme_render_environment::{render_dark_mode, render_now, render_now_unix};
fn schema_version() -> u32 {
    THEME_SCHEMA_VERSION
}
pub(crate) fn default_render() -> Expression {
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
fn default_provider_mark() -> String {
    "claude".into()
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
