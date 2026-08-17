/// Standard height for interactive controls.
pub(crate) const CONTROL_HEIGHT: f32 = 30.0;

/// Corner radius used by buttons, fields, and other controls.
pub(crate) const CONTROL_CORNER_RADIUS: u8 = 5;

/// Corner radius used by dropdown menus.
pub(crate) const DROPDOWN_CORNER_RADIUS: u8 = 6;

/// Width of the numeric field displayed beside a percentage slider.
pub(crate) const PERCENTAGE_VALUE_WIDTH: f32 = 56.0;

/// Width of the standard toggle, including its state label.
pub(crate) const TOGGLE_WIDTH: f32 = 122.0;

pub(crate) const ANCHOR_CELL_SIZE: f32 = 9.0;
pub(crate) const ANCHOR_CELL_GAP: f32 = 4.0;

pub(crate) const INSPECTOR_LABEL_WIDTH: f32 = 104.0;
pub(crate) const INSPECTOR_RIGHT_GUTTER: f32 = 18.0;
pub(crate) const INSPECTOR_CONTROL_MAX_WIDTH: f32 = 180.0;

pub(crate) const CANVAS_ZOOM_LEVELS: &[f32] = &[0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
pub(crate) const DEFAULT_MENU_WIDTH: f32 = 150.0;
pub(crate) const DEFAULT_SCENE_WIDTH: f32 = 240.0;
pub(crate) const DEFAULT_CANVAS_WIDTH: f32 = 330.0;
pub(crate) const DEFAULT_INSPECTOR_WIDTH: f32 = 324.0;

const DASHBOARD_FRAME_HORIZONTAL_MARGIN: f32 = 20.0;
const SHELL_DIVIDER_AND_GAPS: f32 = 20.0;
const STUDIO_SPLITTER_WIDTHS: f32 = 16.0;

pub(crate) const DEFAULT_DASHBOARD_WIDTH: f32 = DASHBOARD_FRAME_HORIZONTAL_MARGIN
    + DEFAULT_MENU_WIDTH
    + SHELL_DIVIDER_AND_GAPS
    + DEFAULT_SCENE_WIDTH
    + DEFAULT_CANVAS_WIDTH
    + DEFAULT_INSPECTOR_WIDTH
    + STUDIO_SPLITTER_WIDTHS;
pub(crate) const DEFAULT_DASHBOARD_HEIGHT: f32 = 600.0;
