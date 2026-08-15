use eframe::egui;
use lucide_icons::Icon as LucideIcon;

use crate::localization::LanguageId;
use crate::ui::components::icon::paint_centered_icon;
use crate::ui::tokens::CONTROL_HEIGHT;

pub(crate) fn paint_background(ui: &mut egui::Ui, selected: bool) {
    let rect = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(ui.available_width(), CONTROL_HEIGHT),
    );
    let fill = if selected || ui.rect_contains_pointer(rect) {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 5.0, fill);
    }
}

pub(crate) fn selected_style<R>(
    ui: &mut egui::Ui,
    selected: bool,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.scope(|ui| {
        if selected {
            let foreground = egui::Color32::WHITE;
            let visuals = ui.visuals_mut();
            visuals.override_text_color = Some(foreground);
            visuals.widgets.inactive.fg_stroke.color = foreground;
            visuals.widgets.hovered.fg_stroke.color = foreground;
            visuals.widgets.active.fg_stroke.color = foreground;
        }
        body(ui)
    })
    .inner
}

pub(crate) fn drag_handle(
    ui: &mut egui::Ui,
    editable: bool,
    language: LanguageId,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(26.0, CONTROL_HEIGHT),
        if editable {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::click()
        },
    );
    let response = if editable {
        response
            .on_hover_cursor(egui::CursorIcon::Grab)
            .on_hover_text(language.text("Drag to reorder or change the parent"))
    } else {
        response.on_hover_text(language.text("This item is read-only"))
    };
    paint_centered_icon(
        ui,
        rect,
        LucideIcon::GripVertical,
        16.0,
        ui.style().interact(&response).text_color(),
    );
    response
}
