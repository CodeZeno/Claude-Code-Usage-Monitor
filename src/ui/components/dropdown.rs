use eframe::egui;

use crate::ui::theme::{accent, selected_menu_fill};
use crate::ui::tokens::{CONTROL_CORNER_RADIUS, CONTROL_HEIGHT, DROPDOWN_CORNER_RADIUS};

const MAX_VISIBLE_OPTIONS: usize = 6;

/// A styled dropdown whose popup visually joins its trigger button.
pub(crate) struct Dropdown {
    id_salt: egui::IdSalt,
    selected_text: egui::WidgetText,
    width: Option<f32>,
}

impl Dropdown {
    pub(crate) fn from_id_salt(id_salt: impl egui::AsIdSalt) -> Self {
        Self {
            id_salt: egui::IdSalt::new(id_salt),
            selected_text: egui::WidgetText::default(),
            width: None,
        }
    }

    pub(crate) fn selected_text(mut self, selected_text: impl Into<egui::WidgetText>) -> Self {
        self.selected_text = selected_text.into();
        self
    }

    pub(crate) fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub(crate) fn show_ui<R>(
        self,
        ui: &mut egui::Ui,
        menu_contents: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<Option<R>> {
        let Self {
            id_salt,
            selected_text,
            width,
        } = self;
        let button_id = ui.make_persistent_id(id_salt);
        let button_width = width.unwrap_or_else(|| ui.spacing().combo_width);
        let (_, button_rect) = ui.allocate_space(egui::vec2(button_width, CONTROL_HEIGHT));
        let button_response = ui.interact(button_rect, button_id, egui::Sense::click());
        let popup_id = button_id.with("popup");
        let popup_width = button_rect.width();
        let popup = egui::Popup::menu(&button_response)
            .id(popup_id)
            .width(popup_width)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick);
        let opens_above = popup_opens_above(&popup);
        let (button_corner_radius, menu_corner_radius) = dropdown_corner_radii(opens_above);
        let is_open = egui::Popup::is_id_open(ui.ctx(), popup_id);
        let will_be_open = is_open != button_response.clicked();
        let visuals = if will_be_open {
            &ui.visuals().widgets.open
        } else {
            ui.style().interact(&button_response)
        };
        let button_fill = visuals.weak_bg_fill;
        ui.painter().rect(
            button_rect.expand(visuals.expansion),
            if will_be_open {
                button_corner_radius
            } else {
                egui::CornerRadius::same(CONTROL_CORNER_RADIUS)
            },
            button_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );

        let icon_size = ui.spacing().icon_width;
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(
                button_rect.right() - ui.spacing().button_padding.x - icon_size * 0.5,
                button_rect.center().y,
            ),
            egui::vec2(icon_size * 0.7, icon_size * 0.45),
        );
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                icon_rect.left_top(),
                icon_rect.right_top(),
                icon_rect.center_bottom(),
            ],
            visuals.fg_stroke.color,
            egui::Stroke::NONE,
        ));

        let text_inset = ui.spacing().button_padding.x;
        let text_right = icon_rect.left() - ui.spacing().icon_spacing;
        let text_width = (text_right - button_rect.left() - text_inset).max(1.0);
        let current_text = selected_text.text().to_owned();
        let natural_text_width = ui
            .painter()
            .layout_no_wrap(
                current_text.clone(),
                egui::TextStyle::Button.resolve(ui.style()),
                visuals.text_color(),
            )
            .size()
            .x;
        let galley = selected_text.into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            text_width,
            egui::TextStyle::Button,
        );
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(button_rect.left() + text_inset, button_rect.top()),
            egui::pos2(text_right, button_rect.bottom()),
        );
        let text_rect = egui::Align2::LEFT_CENTER.align_size_within_rect(galley.size(), text_rect);
        ui.painter()
            .galley(text_rect.min, galley, visuals.text_color());
        button_response.widget_info(|| {
            let mut info = egui::WidgetInfo::new(egui::WidgetType::ComboBox);
            info.enabled = ui.is_enabled();
            info.current_text_value = Some(current_text.clone());
            info
        });
        if natural_text_width > text_width {
            button_response.clone().on_hover_text(current_text);
        }

        let popup_frame = egui::Frame::popup(ui.style())
            .corner_radius(menu_corner_radius)
            .shadow(dropdown_popup_shadow(
                ui.visuals().popup_shadow,
                opens_above,
            ));
        let popup_content_width = (popup_width - popup_frame.total_margin().sum().x).max(1.0);
        let shown = popup.frame(popup_frame).show(|ui| {
            // Calculate the content width from the trigger and frame margins.
            // `available_width` can be only a few pixels during the popup's
            // first sizing pass, while using the outer width here would make
            // the frame wider than its trigger.
            ui.set_min_width(popup_content_width);
            egui::ScrollArea::vertical()
                .max_height(dropdown_scroll_height(ui))
                .show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    menu_contents(ui)
                })
                .inner
        });
        if let Some(shown) = &shown {
            paint_dropdown_join(
                ui,
                popup_id,
                shown.response.rect,
                button_rect,
                opens_above,
                button_fill,
            );
        }

        egui::InnerResponse {
            inner: shown.map(|shown| shown.inner),
            response: button_response,
        }
    }
}

pub(crate) fn dropdown_scroll_height(ui: &egui::Ui) -> f32 {
    dropdown_scroll_height_for_spacing(ui.spacing().item_spacing.y)
}

fn dropdown_scroll_height_for_spacing(row_spacing: f32) -> f32 {
    CONTROL_HEIGHT * MAX_VISIBLE_OPTIONS as f32
        + row_spacing * MAX_VISIBLE_OPTIONS.saturating_sub(1) as f32
}

fn popup_opens_above(popup: &egui::Popup<'_>) -> bool {
    if let (Some(anchor_rect), Some(popup_rect)) = (popup.get_anchor_rect(), popup.get_popup_rect())
    {
        popup_rect.center().y < anchor_rect.center().y
    } else {
        matches!(
            popup.get_best_align(),
            egui::RectAlign::TOP_START | egui::RectAlign::TOP | egui::RectAlign::TOP_END
        )
    }
}

fn dropdown_corner_radii(opens_above: bool) -> (egui::CornerRadius, egui::CornerRadius) {
    if opens_above {
        (
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: CONTROL_CORNER_RADIUS,
                se: CONTROL_CORNER_RADIUS,
            },
            egui::CornerRadius {
                nw: DROPDOWN_CORNER_RADIUS,
                ne: DROPDOWN_CORNER_RADIUS,
                sw: 0,
                se: 0,
            },
        )
    } else {
        (
            egui::CornerRadius {
                nw: CONTROL_CORNER_RADIUS,
                ne: CONTROL_CORNER_RADIUS,
                sw: 0,
                se: 0,
            },
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: DROPDOWN_CORNER_RADIUS,
                se: DROPDOWN_CORNER_RADIUS,
            },
        )
    }
}

fn dropdown_popup_shadow(
    mut shadow: egui::epaint::Shadow,
    opens_above: bool,
) -> egui::epaint::Shadow {
    let joint_clearance = shadow
        .spread
        .saturating_add(shadow.blur.div_ceil(2))
        .min(i8::MAX as u8) as i8;
    shadow.offset[1] = if opens_above {
        -joint_clearance
    } else {
        joint_clearance
    };
    shadow
}

fn paint_dropdown_join(
    ui: &egui::Ui,
    popup_id: egui::Id,
    popup_rect: egui::Rect,
    button_rect: egui::Rect,
    opens_above: bool,
    button_fill: egui::Color32,
) {
    let left = popup_rect.left().max(button_rect.left()) + 1.0;
    let right = popup_rect.right().min(button_rect.right()) - 1.0;
    if right <= left {
        return;
    }

    let joint_y = if opens_above {
        button_rect.top()
    } else {
        button_rect.bottom()
    };
    let popup_side = if opens_above {
        egui::Rect::from_min_max(egui::pos2(left, joint_y - 2.0), egui::pos2(right, joint_y))
    } else {
        egui::Rect::from_min_max(egui::pos2(left, joint_y), egui::pos2(right, joint_y + 2.0))
    };
    let button_side = if opens_above {
        egui::Rect::from_min_max(egui::pos2(left, joint_y), egui::pos2(right, joint_y + 2.0))
    } else {
        egui::Rect::from_min_max(egui::pos2(left, joint_y - 2.0), egui::pos2(right, joint_y))
    };
    // A tooltip-order layer sits above both the parent control and popup Area.
    // Repainting each side with its own fill removes their stacked strokes at
    // the shared edge without flattening the outer border or either surface.
    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(egui::Order::Foreground, popup_id));
    painter.rect_filled(popup_side, 0.0, ui.visuals().window_fill());
    painter.rect_filled(button_side, 0.0, button_fill);
}

pub(crate) fn dropdown_selectable_label(
    ui: &mut egui::Ui,
    selected: bool,
    label: impl Into<String>,
) -> egui::Response {
    let label = label.into();
    let width = ui.available_width().max(1.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, CONTROL_HEIGHT), egui::Sense::click());
    let visuals = ui.style().interact_selectable(&response, selected);
    let fill = if selected {
        selected_menu_fill()
    } else if response.hovered() || response.highlighted() || response.has_focus() {
        visuals.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    let text_inset = 12.0;
    let text_width = (rect.width() - text_inset * 2.0).max(1.0);
    let natural_text_width = ui
        .painter()
        .layout_no_wrap(
            label.clone(),
            egui::TextStyle::Button.resolve(ui.style()),
            visuals.text_color(),
        )
        .size()
        .x;
    let galley = egui::WidgetText::from(label.clone()).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        text_width,
        egui::TextStyle::Button,
    );
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + text_inset, rect.top()),
        egui::pos2(rect.right() - text_inset, rect.bottom()),
    );
    let text_rect = egui::Align2::LEFT_CENTER.align_size_within_rect(galley.size(), text_rect);
    ui.painter().galley(
        text_rect.min,
        galley,
        if selected {
            egui::Color32::WHITE
        } else {
            visuals.text_color()
        },
    );
    if selected {
        let marker_clip = egui::Rect::from_min_max(
            response.rect.min,
            egui::pos2(response.rect.left() + 6.0, response.rect.bottom()),
        );
        ui.painter()
            .with_clip_rect(marker_clip)
            .rect_filled(response.rect, 4.0, accent());
    }
    if natural_text_width > text_width {
        response.on_hover_text(label)
    } else {
        response
    }
}

pub(crate) fn dropdown_selectable_value<T: PartialEq>(
    ui: &mut egui::Ui,
    current: &mut T,
    value: T,
    label: impl Into<String>,
) -> egui::Response {
    let selected = *current == value;
    let mut response = dropdown_selectable_label(ui, selected, label);
    if response.clicked() && !selected {
        *current = value;
        response.mark_changed();
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_dropdowns_have_joined_corner_radii() {
        let (button_below, menu_below) = dropdown_corner_radii(false);
        assert_eq!(
            button_below,
            egui::CornerRadius {
                nw: CONTROL_CORNER_RADIUS,
                ne: CONTROL_CORNER_RADIUS,
                sw: 0,
                se: 0,
            }
        );
        assert_eq!(
            menu_below,
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: DROPDOWN_CORNER_RADIUS,
                se: DROPDOWN_CORNER_RADIUS,
            }
        );

        let (button_above, menu_above) = dropdown_corner_radii(true);
        assert_eq!(
            button_above,
            egui::CornerRadius {
                nw: 0,
                ne: 0,
                sw: CONTROL_CORNER_RADIUS,
                se: CONTROL_CORNER_RADIUS,
            }
        );
        assert_eq!(
            menu_above,
            egui::CornerRadius {
                nw: DROPDOWN_CORNER_RADIUS,
                ne: DROPDOWN_CORNER_RADIUS,
                sw: 0,
                se: 0,
            }
        );
    }

    #[test]
    fn dropdown_shadow_stays_clear_of_the_joined_edge() {
        let shadow = egui::epaint::Shadow {
            offset: [0, 0],
            blur: 12,
            spread: 2,
            color: egui::Color32::BLACK,
        };

        assert_eq!(dropdown_popup_shadow(shadow, false).margin().top, 0.0);
        assert_eq!(dropdown_popup_shadow(shadow, true).margin().bottom, 0.0);
    }

    #[test]
    fn dropdowns_show_six_complete_rows_before_scrolling() {
        assert_eq!(dropdown_scroll_height_for_spacing(8.0), 220.0);
    }
}
