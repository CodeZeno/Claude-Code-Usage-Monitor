use eframe::egui;

use crate::ui::theme::{accent, menu_hover, menu_text, selected_menu_fill};

pub(crate) fn navigation_item(ui: &mut egui::Ui, selected: bool, title: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::click());
    let fill = if selected {
        selected_menu_fill()
    } else if response.hovered() {
        menu_hover()
    } else {
        egui::Color32::TRANSPARENT
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    if selected {
        let marker_clip =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 6.0, rect.bottom()));
        ui.painter()
            .with_clip_rect(marker_clip)
            .rect_filled(rect, 4.0, accent());
    }
    ui.painter().text(
        egui::pos2(rect.left() + 18.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(16.0),
        menu_text(),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
