use eframe::egui;
use lucide_icons::Icon as LucideIcon;

use crate::ui::components::expression_button::expression_button;
use crate::ui::components::icon::paint_centered_icon;
use crate::ui::theme::muted;
use crate::ui::tokens::{CONTROL_CORNER_RADIUS, CONTROL_HEIGHT};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HelperFieldAction {
    pub(crate) open: bool,
    pub(crate) remove: bool,
}

pub(crate) fn helper_preview_field(
    ui: &mut egui::Ui,
    id: egui::Id,
    preview: &str,
    available_width: f32,
    has_helper_value: bool,
    helper_name: &str,
    horizontal_align: egui::Align,
) -> HelperFieldAction {
    let gap = ui.spacing().item_spacing.x;
    let field_width = (available_width - CONTROL_HEIGHT - gap).max(40.0);
    let (_, field_rect) = ui.allocate_space(egui::vec2(field_width, CONTROL_HEIGHT));
    let open_response = ui
        .interact(field_rect, id.with("open_helper"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!("Open {helper_name}"));
    let visuals = ui.style().interact(&open_response);
    ui.painter().rect_filled(
        field_rect.expand(visuals.expansion),
        egui::CornerRadius::same(CONTROL_CORNER_RADIUS),
        visuals.weak_bg_fill,
    );

    let delete_inset = 2.0;
    let delete_size = (field_rect.height() - delete_inset * 2.0).max(0.0);
    let delete_rect = egui::Rect::from_min_size(
        egui::pos2(
            field_rect.right() - delete_inset - delete_size,
            field_rect.top() + delete_inset,
        ),
        egui::vec2(delete_size, delete_size),
    );
    let show_delete =
        has_helper_value && (open_response.hovered() || ui.rect_contains_pointer(delete_rect));
    let text = if preview.is_empty() {
        "No value"
    } else {
        preview
    };
    let text_color = if preview.is_empty() {
        muted()
    } else {
        visuals.text_color()
    };
    let horizontal_padding = 8.0;
    let right_padding = if has_helper_value && horizontal_align == egui::Align::Min {
        delete_size + delete_inset * 2.0
    } else {
        horizontal_padding
    };
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(field_rect.left() + horizontal_padding, field_rect.top()),
        egui::pos2(field_rect.right() - right_padding, field_rect.bottom()),
    );
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        text_rect.width().max(1.0),
        egui::TextStyle::Body,
    );
    let text_anchor = match horizontal_align {
        egui::Align::Min => egui::Align2::LEFT_CENTER,
        egui::Align::Center => egui::Align2::CENTER_CENTER,
        egui::Align::Max => egui::Align2::RIGHT_CENTER,
    };
    let text_position = text_anchor.align_size_within_rect(galley.size(), text_rect);
    ui.painter().galley(text_position.min, galley, text_color);

    let remove = if show_delete {
        let delete_response = ui
            .interact(
                delete_rect,
                id.with("remove_expression"),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Remove value");
        let icon_color = if delete_response.hovered() {
            ui.visuals().widgets.hovered.fg_stroke.color
        } else {
            ui.visuals().widgets.inactive.fg_stroke.color
        };
        paint_centered_icon(ui, delete_rect, LucideIcon::X, 16.0, icon_color);
        delete_response.clicked()
    } else {
        false
    };
    let button_open = expression_button(ui, has_helper_value)
        .on_hover_text(format!("Open {helper_name}"))
        .clicked();
    HelperFieldAction {
        open: (open_response.clicked() && !remove) || button_open,
        remove,
    }
}
