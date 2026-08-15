use eframe::egui;

use crate::ui::theme::{accent, splitter_hover_surface, splitter_idle};

pub(crate) fn vertical_splitter(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
) -> egui::Response {
    let response = ui.interact(rect, ui.make_persistent_id(id), egui::Sense::drag());
    let response = response.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    let active = response.hovered() || response.dragged();
    if active {
        ui.painter()
            .rect_filled(rect, 2.0, splitter_hover_surface());
    }
    ui.painter().line_segment(
        [
            egui::pos2(rect.center().x, rect.top() + 8.0),
            egui::pos2(rect.center().x, rect.bottom() - 8.0),
        ],
        egui::Stroke::new(
            if active { 2.0 } else { 1.0 },
            if active { accent() } else { splitter_idle() },
        ),
    );
    response
}
