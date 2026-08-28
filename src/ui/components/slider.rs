use eframe::egui;

use crate::ui::components::number_field::NumberField;
use crate::ui::tokens::{CONTROL_HEIGHT, PERCENTAGE_VALUE_WIDTH};

/// A percentage slider paired with a numeric percentage field.
///
/// `available_width` is shared between the slider and value field so the
/// component fills whichever settings or inspector row contains it.
pub(crate) fn percentage_slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    available_width: f32,
) -> egui::Response {
    let gap = ui.spacing().item_spacing.x;
    let slider_width = (available_width - PERCENTAGE_VALUE_WIDTH - gap).max(1.0);
    let (rect, mut response) = ui.allocate_exact_size(
        egui::vec2(available_width, CONTROL_HEIGHT),
        egui::Sense::hover(),
    );
    let slider_rect = egui::Rect::from_min_size(rect.min, egui::vec2(slider_width, CONTROL_HEIGHT));
    let value_rect = egui::Rect::from_min_size(
        egui::pos2(slider_rect.right() + gap, rect.top()),
        egui::vec2(PERCENTAGE_VALUE_WIDTH, CONTROL_HEIGHT),
    );

    // `Slider` otherwise caps itself at the global slider width. `place` keeps
    // both controls inside the row allocated above, so DragValue switching to
    // keyboard-edit mode cannot change the height of the surrounding layout.
    let previous_slider_width = ui.spacing().slider_width;
    ui.spacing_mut().slider_width = slider_width;
    let slider_response = ui.place(
        slider_rect,
        egui::Slider::new(value, 0.0..=100.0)
            .integer()
            .show_value(false),
    );
    ui.spacing_mut().slider_width = previous_slider_width;
    let value_response = NumberField::new(value)
        .range(0.0..=100.0)
        .speed(1.0)
        .suffix("%")
        .show_at(ui, value_rect);

    if slider_response.changed() || value_response.changed() {
        response.mark_changed();
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentage_slider_reserves_one_fixed_height_row() {
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let mut value = 50.0;
            let response = percentage_slider(ui, &mut value, 240.0);

            assert_eq!(response.rect.height(), CONTROL_HEIGHT);
        });
        output.textures_delta.clear();
    }
}
