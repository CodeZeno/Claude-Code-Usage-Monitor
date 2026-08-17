//! DPI-aware settings dashboard and visual theme studio, hosted in a separate process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use lucide_icons::Icon as LucideIcon;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
};
use windows::Win32::Graphics::Gdi::{
    GetSysColor, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_MENU, COLOR_MENUTEXT,
    COLOR_WINDOWFRAME,
};
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_FILEMUSTEXIST, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
    OPENFILENAMEW,
};
use windows::Win32::UI::Controls::{
    CloseThemeData, GetThemeColor, OpenThemeData, MENU_POPUPBACKGROUND, MENU_POPUPBORDERS,
    MENU_POPUPITEM, MPI_DISABLED, MPI_HOT, MPI_NORMAL, TMT_BORDERCOLOR, TMT_FILLCOLOR,
    TMT_TEXTCOLOR,
};
use windows::Win32::UI::HiDpi::{GetSystemMetricsForDpi, SystemParametersInfoForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, SendMessageW, ICON_BIG, ICON_SMALL, NONCLIENTMETRICSW, SM_CXEDGE, SM_CXMENUCHECK,
    SM_CYEDGE, SM_CYMENUCHECK, SPI_GETNONCLIENTMETRICS, WM_SETICON,
};

use crate::app_settings::{
    self, SettingsFile, POLL_15_MIN, POLL_15_MIN_SECONDS, POLL_1_HOUR, POLL_1_HOUR_SECONDS,
    POLL_1_MIN, POLL_1_MIN_SECONDS, POLL_5_MIN, POLL_5_MIN_SECONDS,
};
use crate::context_menu::{
    self, ContextMenuAction, ContextMenuDocument, ContextMenuItem, ContextMenuItemKind,
    ContextMenuProvider,
};
use crate::font_catalog::installed_font_families;
use crate::localization::{self, LanguageId};
use crate::models::AppUsageData;
use crate::native_interop::{self, WM_APP_REFRESH_NOW, WM_APP_SETTINGS_UPDATED};
use crate::providers::PROVIDER_DESCRIPTORS;
use crate::theme_engine::{
    self, Canvas, ChildAlignment, ChildLayout, DataContext, Expression, FontRendering, FontWeight,
    HorizontalAnchor, ImageFit, LayerBackground, MouseActionOverrideKey, MouseActionProperty,
    MouseEventKind, MouseEvents, ObjectAnchor, ObjectHorizontalAnchor, ObjectVerticalAnchor, Paint,
    Placement, ProgressDirection, ReferenceRegion, ReferenceTarget, SceneContent, SceneObject,
    SurfaceNest, TextAlign, ThemeDocument, ThemeRuntime, VerticalAnchor,
};
use crate::theme_package;
use crate::ui::components::action_helper::show_action_helper;
use crate::ui::components::anchor_point::{AnchorPoint, AnchorPointPicker};
use crate::ui::components::card::reference_card as expression_reference_card;
use crate::ui::components::dropdown::{
    dropdown_selectable_label, dropdown_selectable_value, Dropdown,
};
use crate::ui::components::expression_helper::{
    show_expression_helper, ExpressionHelperAction,
    ExpressionHelperState as ExpressionHelperEditorState,
};
use crate::ui::components::helper_field::helper_preview_field;
use crate::ui::components::icon::{
    icon_only_button as lucide_icon_button, labeled_icon_button as lucide_labeled_button,
    leading_icon_button,
};
use crate::ui::components::layout::{
    available_control_width as inspector_control_width, inspector_row as labeled, setting_row,
    setting_separator, settings_scroll_area, settings_section as section, studio_region,
};
use crate::ui::components::number_field::NumberField;
use crate::ui::components::searchable_dropdown::searchable_dropdown;
use crate::ui::components::slider::percentage_slider;
use crate::ui::components::splitter::vertical_splitter as workspace_splitter;
use crate::ui::components::text_field::{
    name_editor as inspector_name_editor,
    name_editor_with_prefix as inspector_prefixed_name_editor, singleline as singleline_text_edit,
};
use crate::ui::components::text_helper::{
    show_text_helper, TextHelperAction, TextHelperState as TextHelperEditorState,
    TextTemplateFormat, TextTemplateValueKind,
};
use crate::ui::components::toggle::Toggle;
use crate::ui::components::tree_row::{
    paint_background as paint_scene_row_background, selected_style as scene_row_style,
};
use crate::ui::theme::{accent, configure_style, menu_surface, muted};
use crate::ui::tokens::{
    CANVAS_ZOOM_LEVELS, CONTROL_HEIGHT, DEFAULT_DASHBOARD_HEIGHT, DEFAULT_DASHBOARD_WIDTH,
    DEFAULT_INSPECTOR_WIDTH, DEFAULT_MENU_WIDTH, DEFAULT_SCENE_WIDTH,
};

const DEFAULT_LIVE_APPLY: bool = false;
const CONTEXT_MENU_SUBMENU_OVERLAP: f32 = 2.0;

#[derive(Clone, Copy)]
enum ExpressionControlKind {
    Boolean,
    Percentage,
}

pub fn handle_cli_mode(args: &[String]) -> bool {
    if !args.iter().any(|argument| argument == "--studio") {
        return false;
    }
    let owner = args
        .iter()
        .position(|value| value == "--owner")
        .and_then(|index| args.get(index + 1))
        .and_then(|value| value.parse::<isize>().ok())
        .unwrap_or(0);
    let owner_hwnd = HWND(owner as *mut _);
    let _dashboard_instance = match crate::dashboard::claim_instance() {
        Ok(Some(instance)) => instance,
        Ok(None) => return true,
        Err(error) => {
            crate::dashboard::report_launch_failure(owner_hwnd, &error);
            return true;
        }
    };
    let initial_page = if args.iter().any(|argument| argument == "--theme-studio") {
        Page::Studio
    } else {
        Page::Settings
    };
    let settings = app_settings::load_settings();
    let dashboard_width = settings.dashboard_width.unwrap_or(DEFAULT_DASHBOARD_WIDTH);
    let dashboard_height = settings
        .dashboard_height
        .unwrap_or(DEFAULT_DASHBOARD_HEIGHT);
    let dashboard_icon = eframe::icon_data::from_png_bytes(include_bytes!("icons/16x16.png"))
        .expect("src/icons/16x16.png must be a valid PNG app icon");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Usage Monitor")
            .with_inner_size([dashboard_width, dashboard_height])
            .with_icon(dashboard_icon),
        renderer: eframe::Renderer::Glow,
        centered: true,
        ..Default::default()
    };
    if let Err(error) = eframe::run_native(
        "ClaudeCodeUsageMonitor.Studio",
        options,
        Box::new(move |context| Ok(Box::new(StudioApp::new(context, owner, initial_page)))),
    ) {
        let settings = app_settings::load_settings();
        let language = localization::resolve_language(
            settings.language.as_deref().and_then(LanguageId::from_code),
        );
        crate::dashboard::report_launch_failure(
            owner_hwnd,
            &format!(
                "{}: {error}",
                language.text("The dashboard could not initialize")
            ),
        );
    }
    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Settings,
    Studio,
    ContextMenus,
    Assets,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextMenuActionKind {
    OpenDashboard,
    Refresh,
    SetUpdateFrequency,
    ToggleProvider,
    ToggleStartup,
    ToggleWidget,
    SetLanguage,
    CheckForUpdates,
    ToggleLayerRender,
    LayerActions,
    OpenUrl,
    Exit,
}

impl ContextMenuActionKind {
    const ALL: [Self; 12] = [
        Self::OpenDashboard,
        Self::Refresh,
        Self::SetUpdateFrequency,
        Self::ToggleProvider,
        Self::ToggleStartup,
        Self::ToggleWidget,
        Self::SetLanguage,
        Self::CheckForUpdates,
        Self::ToggleLayerRender,
        Self::LayerActions,
        Self::OpenUrl,
        Self::Exit,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::OpenDashboard => "Open Dashboard",
            Self::Refresh => "Refresh",
            Self::SetUpdateFrequency => "Set update frequency",
            Self::ToggleProvider => "Toggle provider",
            Self::ToggleStartup => "Toggle Start with Windows",
            Self::ToggleWidget => "Show widget",
            Self::SetLanguage => "Set language",
            Self::CheckForUpdates => "Check for updates",
            Self::ToggleLayerRender => "Toggle layer Render",
            Self::LayerActions => "Run layer actions",
            Self::OpenUrl => "Open URL",
            Self::Exit => "Exit",
        }
    }

    fn default_action(self) -> ContextMenuAction {
        match self {
            Self::OpenDashboard => ContextMenuAction::OpenDashboard,
            Self::Refresh => ContextMenuAction::Refresh,
            Self::SetUpdateFrequency => ContextMenuAction::SetUpdateFrequency {
                seconds: POLL_15_MIN_SECONDS,
            },
            Self::ToggleProvider => ContextMenuAction::ToggleProvider {
                provider: ContextMenuProvider::Claude,
            },
            Self::ToggleStartup => ContextMenuAction::ToggleStartup,
            Self::ToggleWidget => ContextMenuAction::ToggleWidget,
            Self::SetLanguage => ContextMenuAction::SetLanguage {
                language: "system".into(),
            },
            Self::CheckForUpdates => ContextMenuAction::CheckForUpdates,
            Self::ToggleLayerRender => ContextMenuAction::ToggleLayerRender {
                target: "main".into(),
            },
            Self::LayerActions => ContextMenuAction::LayerActions {
                actions: "toggle(self, render)".into(),
            },
            Self::OpenUrl => ContextMenuAction::OpenUrl {
                url: "https://".into(),
            },
            Self::Exit => ContextMenuAction::Exit,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Selection {
    Surface(usize),
    Object(usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SceneDropTarget {
    RootAt(usize),
    Into(Selection),
    Before(Selection),
    After(Selection),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ContextMenuDropTarget {
    Before(Vec<usize>),
    After(Vec<usize>),
    Into(Vec<usize>),
}

struct SceneRowResponses {
    item: egui::Response,
    expand_button: egui::Response,
    drag_handle: egui::Response,
    name_changed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpressionField {
    Render,
    Visibility,
    ObjectWidth,
    ObjectHeight,
    PlacementOffsetX,
    PlacementOffsetY,
    ObjectX,
    ObjectY,
    ObjectRotation,
    ObjectCornerRadius,
    ObjectBorderWidth,
    ChildGap,
    BackgroundGradientAngle,
    TextFontSize,
    TextFontContrast,
    ProgressValue,
    ProgressCornerRadius,
    ProgressSegments,
    ProgressSegmentGap,
}

#[derive(Clone, Copy)]
enum LayerInspectorRequest {
    Expression(ExpressionField),
    TextTemplate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseEventField {
    Click,
    DoubleClick,
    RightClick,
    MouseEnter,
    MouseLeave,
}

impl MouseEventField {
    fn kind(self) -> MouseEventKind {
        match self {
            Self::Click => MouseEventKind::Click,
            Self::DoubleClick => MouseEventKind::DoubleClick,
            Self::RightClick => MouseEventKind::RightClick,
            Self::MouseEnter => MouseEventKind::MouseEnter,
            Self::MouseLeave => MouseEventKind::MouseLeave,
        }
    }
}

struct ExpressionHelperState {
    selection: Selection,
    field: ExpressionField,
    editor: ExpressionHelperEditorState,
}

impl ExpressionHelperState {
    fn new(selection: Selection, field: ExpressionField, draft: String) -> Self {
        Self {
            selection,
            field,
            editor: ExpressionHelperEditorState::new(draft),
        }
    }
}

struct ActionHelperState {
    selection: Selection,
    field: MouseEventField,
    editor: ExpressionHelperEditorState,
    target: String,
    property: MouseActionProperty,
    value: String,
    context_menu_reference: String,
}

impl ActionHelperState {
    fn new(selection: Selection, field: MouseEventField, draft: String) -> Self {
        Self {
            selection,
            field,
            editor: ExpressionHelperEditorState::new(draft),
            target: "self".into(),
            property: MouseActionProperty::Render,
            value: "false".into(),
            context_menu_reference: context_menu::CLASSIC_CONTEXT_MENU_ID.into(),
        }
    }
}

struct TextTemplateHelperState {
    target: TextTemplateHelperTarget,
    editor: TextHelperEditorState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TextTemplateHelperTarget {
    Theme(Selection),
    ContextMenu(Vec<usize>),
}

impl TextTemplateHelperState {
    fn for_theme(selection: Selection, draft: String) -> Self {
        Self {
            target: TextTemplateHelperTarget::Theme(selection),
            editor: TextHelperEditorState::new(draft),
        }
    }

    fn for_context_menu(path: Vec<usize>, draft: String) -> Self {
        Self {
            target: TextTemplateHelperTarget::ContextMenu(path),
            editor: TextHelperEditorState::new(draft),
        }
    }
}

struct ContextMenuActionHelperState {
    path: Vec<usize>,
    editor: ExpressionHelperEditorState,
    target: String,
    property: MouseActionProperty,
    value: String,
}

impl ContextMenuActionHelperState {
    fn new(path: Vec<usize>, action: &ContextMenuAction) -> Self {
        Self {
            path,
            editor: ExpressionHelperEditorState::new(context_menu_action_script(action)),
            target: "self".into(),
            property: MouseActionProperty::Render,
            value: "false".into(),
        }
    }
}

struct ThemeDeletionConfirmation {
    path: PathBuf,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingUnsavedAction {
    Close,
    ActivateTheme(PathBuf),
    NewTheme,
}

struct AssetPickerState {
    target: Selection,
    selected_path: Option<String>,
    filter: String,
}

struct AssetDeletionConfirmation {
    asset: theme_engine::ManagedAsset,
}

struct StudioApp {
    owner: isize,
    page: Page,
    settings: SettingsFile,
    startup_enabled: bool,
    theme: ThemeDocument,
    theme_path: Option<PathBuf>,
    selection: Selection,
    preview: Option<egui::TextureHandle>,
    preview_dirty: bool,
    usage: Option<AppUsageData>,
    usage_poll_ok: bool,
    usage_has_error: bool,
    last_cache_read: Instant,
    dirty: bool,
    live_apply: bool,
    zoom: f32,
    preview_pan: egui::Vec2,
    undo_stack: Vec<ThemeDocument>,
    redo_stack: Vec<ThemeDocument>,
    history_snapshot: ThemeDocument,
    scene_width: f32,
    inspector_width: f32,
    hovered_scene_item: Option<Selection>,
    expression_helper: Option<ExpressionHelperState>,
    action_helper: Option<ActionHelperState>,
    text_template_helper: Option<TextTemplateHelperState>,
    preview_mouse_overrides: HashMap<MouseActionOverrideKey, Expression>,
    preview_hover_target: Option<(usize, String)>,
    preview_pending_click: Option<(Instant, usize, String)>,
    asset_picker: Option<AssetPickerState>,
    asset_thumbnails: HashMap<String, egui::TextureHandle>,
    asset_page_filter: String,
    asset_page_selected: Option<String>,
    asset_error: Option<String>,
    settings_error: Option<String>,
    theme_error: Option<String>,
    pending_unsaved_action: Option<PendingUnsavedAction>,
    asset_delete_confirmation: Option<AssetDeletionConfirmation>,
    new_theme_name: Option<String>,
    duplicate_theme_name: Option<String>,
    delete_theme_confirmation: Option<ThemeDeletionConfirmation>,
    context_menu: ContextMenuDocument,
    context_menu_path: Option<PathBuf>,
    context_menu_dirty: bool,
    context_menu_selection: Option<Vec<usize>>,
    context_menu_action_helper: Option<ContextMenuActionHelperState>,
    delete_context_menu_confirmation: Option<(PathBuf, String)>,
}

mod studio_assets;
mod studio_context_menus;
mod studio_core;
mod studio_settings;
mod studio_theme_workspace;

fn scene_subtree_ids(objects: &[SceneObject], root_id: &str) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::from([root_id.to_string()]);
    loop {
        let before = ids.len();
        for object in objects {
            if object
                .parent
                .as_ref()
                .is_some_and(|parent| ids.contains(parent))
            {
                ids.insert(object.id.clone());
            }
        }
        if ids.len() == before {
            return ids;
        }
    }
}

fn take_scene_subtree(objects: &mut Vec<SceneObject>, root_id: &str) -> Vec<SceneObject> {
    let ids = scene_subtree_ids(objects, root_id);
    let mut moving = Vec::new();
    let mut remaining = Vec::new();
    for object in std::mem::take(objects) {
        if ids.contains(&object.id) {
            moving.push(object);
        } else {
            remaining.push(object);
        }
    }
    *objects = remaining;
    moving
}

fn scene_subtree_end(objects: &[SceneObject], root_id: &str) -> usize {
    let ids = scene_subtree_ids(objects, root_id);
    objects
        .iter()
        .enumerate()
        .filter(|(_, object)| ids.contains(&object.id))
        .map(|(index, _)| index + 1)
        .max()
        .unwrap_or(objects.len())
}

fn remap_scene_ids(
    objects: &mut [SceneObject],
    reserved: &std::collections::HashSet<String>,
    remap_every_id: bool,
) {
    let mut used = reserved.clone();
    let mut replacements = std::collections::HashMap::new();
    for object in objects.iter() {
        let old_key = object.id.to_ascii_lowercase();
        if remap_every_id || used.contains(&old_key) {
            let mut replacement = unique_id("object");
            while used.contains(&replacement.to_ascii_lowercase()) {
                replacement = unique_id("object");
            }
            used.insert(replacement.to_ascii_lowercase());
            replacements.insert(object.id.clone(), replacement);
        } else {
            used.insert(old_key);
        }
    }
    for object in objects {
        if let Some(replacement) = replacements.get(&object.id) {
            object.id = replacement.clone();
        }
        if let Some(parent) = object.parent.as_mut() {
            if let Some(replacement) = replacements.get(parent) {
                *parent = replacement.clone();
            }
        }
        if let Some(events) = &mut object.mouse_events {
            for event in [
                MouseEventKind::Click,
                MouseEventKind::DoubleClick,
                MouseEventKind::RightClick,
                MouseEventKind::MouseEnter,
                MouseEventKind::MouseLeave,
            ] {
                let handler = events.handler_mut(event);
                for (old, new) in &replacements {
                    let old = format!("\"{}\"", old.replace('\\', "\\\\").replace('"', "\\\""));
                    let new = format!("\"{}\"", new.replace('\\', "\\\\").replace('"', "\\\""));
                    *handler = handler.replace(&old, &new);
                }
            }
        }
    }
}

fn reserved_scene_ids(surface: &SceneObject) -> std::collections::HashSet<String> {
    std::iter::once(surface)
        .chain(surface.children.iter())
        .map(|object| object.id.to_ascii_lowercase())
        .collect()
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_dropped_files(ui.ctx());
        if let Some(size) = ui
            .ctx()
            .input(|input| input.viewport().inner_rect.map(|rect| rect.size()))
        {
            self.settings.dashboard_width = Some(size.x);
            self.settings.dashboard_height = Some(size.y);
        }
        let close_requested = ui.ctx().input(|input| input.viewport().close_requested());
        if close_requested && (self.dirty || self.pending_unsaved_action.is_some()) {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.pending_unsaved_action.is_none() {
                self.pending_unsaved_action = Some(PendingUnsavedAction::Close);
            }
        }
        self.refresh_usage_cache();
        if self.page == Page::Studio && !ui.ctx().egui_wants_keyboard_input() {
            let undo = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Z);
            let redo = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Y);
            if ui.ctx().input_mut(|input| input.consume_shortcut(&undo)) {
                self.undo_theme();
            } else if ui.ctx().input_mut(|input| input.consume_shortcut(&redo)) {
                self.redo_theme();
            }
        }
        egui::Frame::new()
            .fill(menu_surface())
            .inner_margin(egui::Margin {
                left: 10,
                right: 10,
                top: 0,
                bottom: 10,
            })
            .show(ui, |ui| self.shell(ui));
        self.unsaved_changes_dialog(ui.ctx());
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Reload first so a monitor-process settings update made while the
        // dashboard was open is not overwritten by this final size save.
        let mut settings = app_settings::load_settings();
        settings.dashboard_width = self.settings.dashboard_width;
        settings.dashboard_height = self.settings.dashboard_height;
        if let Err(error) = app_settings::save_settings(&settings) {
            crate::diagnose::log(format!("dashboard size save failed: {error}"));
        }
    }
}

fn style_native_titlebar(context: &eframe::CreationContext<'_>) {
    let Ok(window_handle) = context.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = window_handle.as_raw() else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    let (large_icon, small_icon) = crate::tray_icon::load_app_icons();
    // COLORREF uses 0x00BBGGRR. Keep the caption integrated with the app while
    // retaining the subtle outline that distinguishes ordinary Windows apps.
    let surface_color = 0x0020_2020u32;
    let border_color = 0x0054_5454u32;
    let text_color = 0x00F0_F0F0u32;
    unsafe {
        if !large_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_BIG as usize),
                LPARAM(large_icon.0 as isize),
            );
        }
        if !small_icon.is_invalid() {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                WPARAM(ICON_SMALL as usize),
                LPARAM(small_icon.0 as isize),
            );
        }
        for (attribute, color) in [
            (DWMWA_CAPTION_COLOR, surface_color),
            (DWMWA_BORDER_COLOR, border_color),
            (DWMWA_TEXT_COLOR, text_color),
        ] {
            let _ = DwmSetWindowAttribute(
                hwnd,
                attribute,
                std::ptr::from_ref(&color).cast(),
                std::mem::size_of_val(&color) as u32,
            );
        }
    }
}

fn context_menu_tree_row_contents(
    ui: &mut egui::Ui,
    item: &mut ContextMenuItem,
    path: &[usize],
    submenu_open: Option<bool>,
    editable_name: bool,
    draggable: bool,
    language: LanguageId,
) -> SceneRowResponses {
    ui.horizontal(|ui| {
        let icon = match &item.kind {
            ContextMenuItemKind::Action { .. } => LucideIcon::MousePointerClick,
            ContextMenuItemKind::Text => LucideIcon::Type,
            ContextMenuItemKind::Separator => LucideIcon::Minus,
            ContextMenuItemKind::Submenu { .. } => {
                if submenu_open.unwrap_or(false) {
                    LucideIcon::ChevronDown
                } else {
                    LucideIcon::ChevronRight
                }
            }
        };
        let (rect, expand_button) =
            ui.allocate_exact_size(egui::vec2(27.0, CONTROL_HEIGHT), egui::Sense::click());
        crate::ui::components::icon::paint_centered_icon(
            ui,
            rect,
            icon,
            16.0,
            ui.style().interact(&expand_button).text_color(),
        );
        let expand_button = expand_button
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if submenu_open.is_some() {
                language.text("Expand or collapse")
            } else {
                language.text("Select object")
            });
        let row_width = ui.available_width();
        let (name, name_changed, drag_handle) = ui
            .allocate_ui_with_layout(
                egui::vec2(row_width, CONTROL_HEIGHT),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    let drag_handle =
                        crate::ui::components::tree_row::drag_handle(ui, draggable, language);
                    if draggable {
                        drag_handle.dnd_set_drag_payload(path.to_vec());
                    }
                    let width = ui.available_width().max(1.0);
                    let edit_id = ui.make_persistent_id(("context-menu-layer-name", &item.id));
                    let mut separator_name = language.text("Separator").to_string();
                    let name = if matches!(&item.kind, ContextMenuItemKind::Separator) {
                        &mut separator_name
                    } else {
                        &mut item.label
                    };
                    let (response, changed) = crate::ui::components::text_field::inline_rename(
                        ui,
                        name,
                        edit_id,
                        width,
                        editable_name,
                        language,
                    );
                    (response, changed, drag_handle)
                },
            )
            .inner;
        SceneRowResponses {
            item: expand_button.clone() | name.clone() | drag_handle.clone(),
            expand_button,
            drag_handle,
            name_changed,
        }
    })
    .inner
}

fn context_menu_drop_from_response(
    ui: &mut egui::Ui,
    response: &egui::Response,
    target: &[usize],
    can_contain: bool,
) -> Option<(Vec<usize>, ContextMenuDropTarget)> {
    let source = response.dnd_hover_payload::<Vec<usize>>()?;
    if source.as_slice() == target || target.starts_with(source.as_slice()) {
        return None;
    }
    let pointer_y = ui.ctx().pointer_interact_pos()?.y;
    let relative_y = ((pointer_y - response.rect.top()) / response.rect.height()).clamp(0.0, 1.0);
    let drop_target = if can_contain && (0.25..=0.75).contains(&relative_y) {
        ContextMenuDropTarget::Into(target.to_vec())
    } else if relative_y < 0.5 {
        ContextMenuDropTarget::Before(target.to_vec())
    } else {
        ContextMenuDropTarget::After(target.to_vec())
    };
    match &drop_target {
        ContextMenuDropTarget::Into(_) => {
            ui.painter().rect_stroke(
                response.rect.shrink(1.0),
                4.0,
                egui::Stroke::new(2.0, accent()),
                egui::StrokeKind::Inside,
            );
        }
        ContextMenuDropTarget::Before(_) | ContextMenuDropTarget::After(_) => {
            let y = if matches!(&drop_target, ContextMenuDropTarget::Before(_)) {
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
    }
    response
        .dnd_release_payload::<Vec<usize>>()
        .map(|source| ((*source).clone(), drop_target))
}

mod studio_native_menu;
use studio_native_menu::*;
mod studio_scene_helpers;
use studio_scene_helpers::*;
mod studio_helper_panels;
use studio_helper_panels::*;
mod studio_context_menu_editor;
use studio_context_menu_editor::*;
mod studio_inspectors;
use studio_inspectors::*;
mod studio_utilities;
use studio_utilities::*;
#[cfg(test)]
mod tests;
