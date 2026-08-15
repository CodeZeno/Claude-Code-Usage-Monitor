use std::ops::RangeInclusive;

use eframe::egui;

use crate::localization::LanguageId;
use crate::ui::tokens::CONTROL_HEIGHT;

pub(crate) fn zoom_control(
    ui: &mut egui::Ui,
    zoom: &mut f32,
    range: RangeInclusive<f32>,
    reset_value: f32,
    language: LanguageId,
) -> egui::Response {
    let slider = ui
        .add_sized(
            [120.0, CONTROL_HEIGHT],
            egui::Slider::new(zoom, range).show_value(false),
        )
        .on_hover_text(language.text("Use the mouse wheel over the canvas to zoom"));
    let reset = ui
        .add_sized(
            [54.0, CONTROL_HEIGHT],
            egui::Button::new(format!("{:.0}%", *zoom * 100.0)),
        )
        .on_hover_text(language.text("Reset zoom to 100%"));
    if reset.clicked() && *zoom != reset_value {
        *zoom = reset_value;
    }
    slider.union(reset)
}
