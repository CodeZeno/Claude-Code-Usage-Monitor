use super::*;

#[derive(Clone)]
pub(super) struct NativeContextMenuAppearance {
    pub(super) background: egui::Color32,
    pub(super) text: egui::Color32,
    pub(super) disabled_text: egui::Color32,
    pub(super) highlight: egui::Color32,
    pub(super) highlight_text: egui::Color32,
    pub(super) border: egui::Color32,
    pub(super) font: egui::FontId,
    pub(super) row_height: f32,
    pub(super) separator_height: f32,
    pub(super) arrow_width: f32,
    pub(super) left_gutter: f32,
    pub(super) right_gutter: f32,
    pub(super) menu_unit: f32,
    pub(super) frame_inset: f32,
}

impl NativeContextMenuAppearance {
    pub(super) fn detect(pixels_per_point: f32) -> Self {
        let pixels_per_point = pixels_per_point.max(1.0);
        let dpi = (pixels_per_point * 96.0).round().clamp(96.0, 768.0) as u32;
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        unsafe {
            let _ = SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                metrics.cbSize,
                Some(std::ptr::from_mut(&mut metrics).cast()),
                0,
                dpi,
            );
        }
        let points = |pixels: i32| pixels.unsigned_abs() as f32 / pixels_per_point;
        let font_size = points(metrics.lfMenuFont.lfHeight).clamp(9.0, 24.0);
        let menu_height = points(metrics.iMenuHeight);
        let check_width =
            unsafe { GetSystemMetricsForDpi(SM_CXMENUCHECK, dpi) } as f32 / pixels_per_point;
        let check_height =
            unsafe { GetSystemMetricsForDpi(SM_CYMENUCHECK, dpi) } as f32 / pixels_per_point;
        let edge_x = unsafe { GetSystemMetricsForDpi(SM_CXEDGE, dpi) } as f32 / pixels_per_point;
        let edge_y = unsafe { GetSystemMetricsForDpi(SM_CYEDGE, dpi) } as f32 / pixels_per_point;
        let menu_unit = points(metrics.iMenuWidth).max(15.0);
        let check_width = check_width.max(13.0);
        let themed = native_menu_theme_colors();
        Self {
            background: themed
                .as_ref()
                .map_or_else(|| win32_color(COLOR_MENU), |colors| colors.background),
            text: themed
                .as_ref()
                .map_or_else(|| win32_color(COLOR_MENUTEXT), |colors| colors.text),
            disabled_text: themed.as_ref().map_or_else(
                || win32_color(COLOR_GRAYTEXT),
                |colors| colors.disabled_text,
            ),
            highlight: themed
                .as_ref()
                .map_or_else(|| win32_color(COLOR_HIGHLIGHT), |colors| colors.highlight),
            highlight_text: themed.as_ref().map_or_else(
                || win32_color(COLOR_HIGHLIGHTTEXT),
                |colors| colors.highlight_text,
            ),
            border: themed
                .as_ref()
                .map_or_else(|| win32_color(COLOR_WINDOWFRAME), |colors| colors.border),
            font: egui::FontId::new(font_size, egui::FontFamily::Name("native-menu".into())),
            row_height: (menu_height + 3.0)
                .max(font_size + 9.0)
                .max(check_height + 6.0),
            separator_height: (menu_height * 0.55 - 1.0).clamp(6.0, 11.0),
            arrow_width: check_width,
            left_gutter: menu_unit + check_width + 1.0,
            right_gutter: menu_unit * 2.0 + check_width - 1.0,
            menu_unit,
            frame_inset: edge_x.max(edge_y).max(2.0),
        }
    }
}

pub(super) struct NativeMenuThemeColors {
    pub(super) background: egui::Color32,
    pub(super) text: egui::Color32,
    pub(super) disabled_text: egui::Color32,
    pub(super) highlight: egui::Color32,
    pub(super) highlight_text: egui::Color32,
    pub(super) border: egui::Color32,
}

pub(super) fn native_menu_theme_colors() -> Option<NativeMenuThemeColors> {
    unsafe {
        let class = native_interop::wide_str("Menu");
        let theme = OpenThemeData(None, PCWSTR::from_raw(class.as_ptr()));
        if theme.is_invalid() {
            return None;
        }
        let color = |part: i32, state: i32, property| {
            GetThemeColor(theme, part, state, property)
                .ok()
                .map(|color| colorref_to_egui(color.0))
        };
        let colors = NativeMenuThemeColors {
            background: color(MENU_POPUPBACKGROUND.0, 0, TMT_FILLCOLOR)
                .unwrap_or_else(|| win32_color(COLOR_MENU)),
            text: color(MENU_POPUPITEM.0, MPI_NORMAL.0, TMT_TEXTCOLOR)
                .unwrap_or_else(|| win32_color(COLOR_MENUTEXT)),
            disabled_text: color(MENU_POPUPITEM.0, MPI_DISABLED.0, TMT_TEXTCOLOR)
                .unwrap_or_else(|| win32_color(COLOR_GRAYTEXT)),
            highlight: color(MENU_POPUPITEM.0, MPI_HOT.0, TMT_FILLCOLOR)
                .unwrap_or_else(|| win32_color(COLOR_HIGHLIGHT)),
            highlight_text: color(MENU_POPUPITEM.0, MPI_HOT.0, TMT_TEXTCOLOR)
                .unwrap_or_else(|| win32_color(COLOR_HIGHLIGHTTEXT)),
            border: color(MENU_POPUPBORDERS.0, 0, TMT_BORDERCOLOR)
                .unwrap_or_else(|| win32_color(COLOR_WINDOWFRAME)),
        };
        let _ = CloseThemeData(theme);
        Some(normalize_native_menu_theme_colors(colors))
    }
}

pub(super) fn normalize_native_menu_theme_colors(
    mut colors: NativeMenuThemeColors,
) -> NativeMenuThemeColors {
    // On Windows 11, the Menu theme API still reports the older #f0f0f0 / #646464
    // palette even though TrackPopupMenu is drawn with the Fluent popup surface.
    // Match the pixels produced by the native renderer for that legacy result.
    if colors.background == egui::Color32::from_rgb(240, 240, 240)
        && colors.border == egui::Color32::from_rgb(100, 100, 100)
        && colors.text.r() < 64
    {
        colors.background = egui::Color32::from_rgb(249, 249, 249);
        colors.text = egui::Color32::from_rgb(31, 31, 31);
        colors.disabled_text = egui::Color32::from_rgb(157, 157, 157);
        colors.highlight = egui::Color32::from_rgb(240, 240, 240);
        colors.highlight_text = colors.text;
        colors.border = egui::Color32::from_rgb(229, 229, 229);
    }
    colors
}

pub(super) fn win32_color(index: windows::Win32::Graphics::Gdi::SYS_COLOR_INDEX) -> egui::Color32 {
    colorref_to_egui(unsafe { GetSysColor(index) })
}

pub(super) fn colorref_to_egui(color: u32) -> egui::Color32 {
    egui::Color32::from_rgb(
        (color & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        ((color >> 16) & 0xff) as u8,
    )
}

pub(super) fn apply_native_context_menu_style(
    ui: &mut egui::Ui,
    appearance: &NativeContextMenuAppearance,
) {
    let mut style = ui.style().as_ref().clone();
    style
        .text_styles
        .insert(egui::TextStyle::Button, appearance.font.clone());
    style
        .text_styles
        .insert(egui::TextStyle::Body, appearance.font.clone());
    style.spacing.item_spacing = egui::Vec2::ZERO;
    style.spacing.button_padding = egui::vec2(appearance.frame_inset + 2.0, 0.0);
    style.spacing.interact_size.y = appearance.row_height;
    style.spacing.menu_margin = egui::Margin::same(appearance.frame_inset.round() as i8);
    style.visuals.dark_mode = appearance.background.r() < 128;
    style.visuals.window_fill = appearance.background;
    style.visuals.window_stroke = egui::Stroke::new(1.0, appearance.border);
    style.visuals.menu_corner_radius =
        if appearance.background == egui::Color32::from_rgb(249, 249, 249) {
            egui::CornerRadius::same(4)
        } else {
            egui::CornerRadius::ZERO
        };
    style.visuals.popup_shadow = egui::epaint::Shadow {
        offset: [2, 2],
        blur: 5,
        spread: 0,
        color: egui::Color32::from_black_alpha(90),
    };
    style.visuals.disabled_alpha = 1.0;
    style.visuals.button_frame = false;
    for visuals in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.noninteractive,
    ] {
        visuals.bg_fill = appearance.background;
        visuals.weak_bg_fill = appearance.background;
        visuals.fg_stroke = egui::Stroke::new(1.0, appearance.text);
        visuals.bg_stroke = egui::Stroke::NONE;
        visuals.corner_radius = egui::CornerRadius::ZERO;
    }
    style.visuals.widgets.noninteractive.fg_stroke =
        egui::Stroke::new(1.0, appearance.disabled_text);
    for visuals in [
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        visuals.bg_fill = appearance.highlight;
        visuals.weak_bg_fill = appearance.highlight;
        visuals.fg_stroke = egui::Stroke::new(1.0, appearance.highlight_text);
        visuals.bg_stroke = egui::Stroke::NONE;
        visuals.corner_radius = egui::CornerRadius::ZERO;
    }
    ui.set_style(style);
}

pub(super) fn native_context_menu_width(
    ui: &egui::Ui,
    items: &[ContextMenuItem],
    language: LanguageId,
    context: &DataContext,
    appearance: &NativeContextMenuAppearance,
) -> f32 {
    let label_width = items
        .iter()
        .filter(|item| !matches!(&item.kind, ContextMenuItemKind::Separator))
        .map(|item| context_menu::rendered_label(language, &item.label, context))
        .map(|label| {
            ui.ctx().fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(label, appearance.font.clone(), appearance.text)
                    .size()
                    .x
            })
        })
        .fold(0.0, f32::max);
    (label_width + appearance.left_gutter + appearance.right_gutter + appearance.frame_inset * 2.0)
        .max(96.0)
}

pub(super) struct ContextMenuPreviewState<'a> {
    pub(super) language: LanguageId,
    pub(super) context: &'a DataContext,
    pub(super) settings: &'a SettingsFile,
    pub(super) startup_enabled: bool,
    pub(super) theme: &'a ThemeDocument,
    pub(super) usage: Option<&'a AppUsageData>,
    pub(super) runtime: ThemeRuntime,
    pub(super) appearance: &'a NativeContextMenuAppearance,
    pub(super) preview_bounds: &'a std::cell::Cell<egui::Rect>,
    pub(super) open_submenu_state_ids: &'a std::cell::RefCell<Vec<egui::Id>>,
}

pub(super) fn preview_context_menu_items(
    ui: &mut egui::Ui,
    items: &[ContextMenuItem],
    state: &mut ContextMenuPreviewState<'_>,
) {
    // Popup UIs are created outside the parent menu UI, so explicitly carry the
    // native styling and exact width into every submenu level.
    apply_native_context_menu_style(ui, state.appearance);
    ui.set_width(native_context_menu_width(
        ui,
        items,
        state.language,
        state.context,
        state.appearance,
    ));
    for item in items {
        match &item.kind {
            ContextMenuItemKind::Separator => {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), state.appearance.separator_height),
                    egui::Sense::hover(),
                );
                let y = rect.center().y.round() - 1.0;
                ui.painter().line_segment(
                    [
                        egui::pos2(rect.left() + state.appearance.frame_inset + 4.0, y),
                        egui::pos2(rect.right() - state.appearance.frame_inset - 4.0, y),
                    ],
                    egui::Stroke::new(1.0, native_menu_separator_color(state.appearance)),
                );
            }
            ContextMenuItemKind::Text => {
                let label =
                    context_menu::rendered_label(state.language, &item.label, state.context);
                let response = ui.add_enabled(
                    false,
                    native_context_menu_button(state.appearance, ui.available_width()),
                );
                paint_native_context_menu_row(
                    ui,
                    &response,
                    &label,
                    false,
                    false,
                    true,
                    state.appearance,
                );
            }
            ContextMenuItemKind::Action { action } => {
                let label =
                    context_menu::rendered_label(state.language, &item.label, state.context);
                let checked = context_menu_preview_action_checked(
                    action,
                    state.settings,
                    state.startup_enabled,
                    state.theme,
                    state.usage,
                    state.runtime,
                );
                let response = ui.add(native_context_menu_button(
                    state.appearance,
                    ui.available_width(),
                ));
                paint_native_context_menu_row(
                    ui,
                    &response,
                    &label,
                    checked,
                    false,
                    false,
                    state.appearance,
                );
            }
            ContextMenuItemKind::Submenu { items } => {
                let label =
                    context_menu::rendered_label(state.language, &item.label, state.context);
                let button = native_context_menu_button(state.appearance, ui.available_width());
                // The root preview is manually framed, so egui does not consider
                // it an active menu. This is still a submenu and should open to
                // the side, just like the real Win32 popup menu.
                let preview_bounds = state.preview_bounds;
                let open_submenu_state_ids = state.open_submenu_state_ids;
                let response = preview_context_menu_submenu(
                    ui,
                    button,
                    preview_bounds,
                    open_submenu_state_ids,
                    |ui| {
                        preview_context_menu_items(ui, items, state);
                    },
                )
                .0;
                paint_native_context_menu_row(
                    ui,
                    &response,
                    &label,
                    false,
                    true,
                    false,
                    state.appearance,
                );
            }
        }
    }
}

pub(super) fn preview_context_menu_submenu<R>(
    ui: &mut egui::Ui,
    button: egui::Button<'static>,
    preview_bounds: &std::cell::Cell<egui::Rect>,
    open_submenu_state_ids: &std::cell::RefCell<Vec<egui::Id>>,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> (egui::Response, Option<egui::InnerResponse<R>>) {
    use egui::containers::menu::{find_menu_root, MenuConfig, SubMenu};

    let widget_id = ui.next_auto_id();
    let submenu_id = SubMenu::id_from_widget_id(widget_id);
    let preview_state_id = find_menu_root(ui)
        .id
        .with("context-menu-preview-open-submenu");
    if !open_submenu_state_ids.borrow().contains(&preview_state_id) {
        open_submenu_state_ids.borrow_mut().push(preview_state_id);
    }
    let mut open = ui
        .ctx()
        .data(|data| data.get_temp::<egui::Id>(preview_state_id) == Some(submenu_id));
    let inactive = ui.style().visuals.widgets.inactive;
    if open {
        ui.style_mut().visuals.widgets.inactive = ui.style().visuals.widgets.open;
    }
    let response = ui.add(button);
    ui.style_mut().visuals.widgets.inactive = inactive;

    let button_rect = response.rect.expand2(ui.style().spacing.item_spacing / 2.0);
    let hovered = ui
        .ctx()
        .pointer_hover_pos()
        .is_some_and(|position| button_rect.contains(position));
    if ui.is_enabled() && (hovered || response.clicked()) {
        ui.ctx()
            .data_mut(|data| data.insert_temp(preview_state_id, submenu_id));
        open = true;
    }
    let config = MenuConfig::find(ui);
    let frame = egui::Frame::menu(ui.style());
    let mut popup_anchor = response.clone();
    popup_anchor.interact_rect = popup_anchor
        .interact_rect
        .expand2(egui::vec2(0.0, frame.total_margin().sum().y / 2.0));
    let popup = egui::Popup::from_response(&popup_anchor)
        .id(submenu_id)
        .open(open)
        .align(egui::RectAlign::RIGHT_START)
        .layout(egui::Layout::top_down_justified(egui::Align::Min))
        .gap(-CONTEXT_MENU_SUBMENU_OVERLAP)
        .style(config.style.clone())
        .frame(frame)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .info(
            egui::UiStackInfo::new(egui::UiKind::Menu)
                .with_tag_value(MenuConfig::MENU_CONFIG_TAG, config),
        )
        .show(content);

    if let Some(popup) = &popup {
        preview_bounds.set(preview_bounds.get().union(popup.response.rect));
    }

    if popup
        .as_ref()
        .is_some_and(|popup| popup.response.should_close())
    {
        ui.ctx()
            .data_mut(|data| data.remove::<egui::Id>(preview_state_id));
    }

    (response, popup)
}

pub(super) fn collapse_context_menu_preview_on_outside_click(
    context: &egui::Context,
    preview_bounds: egui::Rect,
    open_submenu_state_ids: &[egui::Id],
) {
    let clicked_outside = context.input(|input| {
        input.pointer.any_click()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|position| !preview_bounds.contains(position))
    });
    if clicked_outside {
        context.data_mut(|data| {
            for state_id in open_submenu_state_ids {
                data.remove::<egui::Id>(*state_id);
            }
        });
        context.request_repaint();
    }
}

pub(super) fn native_context_menu_button(
    appearance: &NativeContextMenuAppearance,
    width: f32,
) -> egui::Button<'static> {
    egui::Button::new("")
        .frame(true)
        .frame_when_inactive(true)
        .corner_radius(egui::CornerRadius::ZERO)
        .min_size(egui::vec2(width, appearance.row_height))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_native_context_menu_row(
    ui: &egui::Ui,
    response: &egui::Response,
    label: &str,
    checked: bool,
    submenu: bool,
    disabled: bool,
    appearance: &NativeContextMenuAppearance,
) {
    let highlighted = response.hovered() && !disabled;
    let background = if highlighted {
        appearance.highlight
    } else {
        appearance.background
    };
    let text_color = if disabled {
        appearance.disabled_text
    } else if highlighted {
        appearance.highlight_text
    } else {
        appearance.text
    };
    ui.painter()
        .rect_filled(response.rect, egui::CornerRadius::ZERO, background);
    ui.painter().text(
        egui::pos2(
            response.rect.left() + appearance.left_gutter,
            response.rect.center().y - 1.0,
        ),
        egui::Align2::LEFT_CENTER,
        label,
        appearance.font.clone(),
        text_color,
    );
    if checked {
        let center = egui::pos2(
            response.rect.left() + appearance.left_gutter * 0.40,
            response.rect.center().y,
        );
        let stroke = egui::Stroke::new(1.0, text_color);
        ui.painter().line_segment(
            [
                egui::pos2(center.x - 3.5, center.y),
                egui::pos2(center.x - 1.0, center.y + 2.0),
            ],
            stroke,
        );
        ui.painter().line_segment(
            [
                egui::pos2(center.x - 1.0, center.y + 2.0),
                egui::pos2(center.x + 4.0, center.y - 2.5),
            ],
            stroke,
        );
    }
    if submenu {
        let center = egui::pos2(
            response.rect.right() - (appearance.menu_unit - 4.0).max(6.0),
            response.rect.center().y + 0.5,
        );
        let half_width = (appearance.arrow_width * 0.12).clamp(1.5, 2.0);
        let half_height = (appearance.row_height * 0.14).clamp(3.0, 3.5);
        let stroke = egui::Stroke::new(1.0, text_color);
        ui.painter().line_segment(
            [
                egui::pos2(center.x - half_width, center.y - half_height),
                egui::pos2(center.x + half_width, center.y),
            ],
            stroke,
        );
        ui.painter().line_segment(
            [
                egui::pos2(center.x + half_width, center.y),
                egui::pos2(center.x - half_width, center.y + half_height),
            ],
            stroke,
        );
    }
}

pub(super) fn native_menu_separator_color(
    appearance: &NativeContextMenuAppearance,
) -> egui::Color32 {
    if appearance.background == egui::Color32::from_rgb(249, 249, 249) {
        return egui::Color32::from_rgb(215, 215, 215);
    }
    let blend = |left: u8, right: u8| ((u16::from(left) + u16::from(right)) / 2) as u8;
    egui::Color32::from_rgb(
        blend(appearance.background.r(), appearance.border.r()),
        blend(appearance.background.g(), appearance.border.g()),
        blend(appearance.background.b(), appearance.border.b()),
    )
}

pub(super) fn context_menu_preview_action_checked(
    action: &ContextMenuAction,
    settings: &SettingsFile,
    startup_enabled: bool,
    theme: &ThemeDocument,
    usage: Option<&AppUsageData>,
    runtime: ThemeRuntime,
) -> bool {
    match action {
        ContextMenuAction::SetUpdateFrequency { seconds } => {
            settings.poll_interval_ms == seconds.saturating_mul(1_000)
        }
        ContextMenuAction::ToggleProvider { provider } => settings.provider_enabled(*provider),
        ContextMenuAction::ToggleStartup => startup_enabled,
        ContextMenuAction::ToggleWidget => theme
            .surfaces
            .iter()
            .position(|surface| surface.placement.nest != SurfaceNest::TrayIcon)
            .is_some_and(|surface_index| {
                theme_engine::surface_should_render(theme, surface_index, usage, runtime)
            }),
        ContextMenuAction::SetLanguage { language } => settings.language.as_deref().map_or_else(
            || language.eq_ignore_ascii_case("system"),
            |current| current.eq_ignore_ascii_case(language),
        ),
        ContextMenuAction::ToggleLayerRender { target } => theme
            .surfaces
            .iter()
            .position(|surface| surface.id.eq_ignore_ascii_case(target))
            .is_some_and(|surface_index| {
                theme_engine::surface_should_render(theme, surface_index, usage, runtime)
            }),
        _ => false,
    }
}
