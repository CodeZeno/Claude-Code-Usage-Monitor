use std::ops::RangeInclusive;

use eframe::egui;

use crate::ui::tokens::CONTROL_HEIGHT;

/// A fixed-height numeric field backed by egui's drag-to-edit control.
pub(crate) struct NumberField<'a> {
    widget: egui::DragValue<'a>,
}

impl<'a> NumberField<'a> {
    pub(crate) fn new<Num: egui::emath::Numeric>(value: &'a mut Num) -> Self {
        Self {
            widget: egui::DragValue::new(value),
        }
    }

    pub(crate) fn range<Num: egui::emath::Numeric>(mut self, range: RangeInclusive<Num>) -> Self {
        self.widget = self.widget.range(range);
        self
    }

    pub(crate) fn speed(mut self, speed: impl Into<f64>) -> Self {
        self.widget = self.widget.speed(speed);
        self
    }

    pub(crate) fn suffix(mut self, suffix: &'a str) -> Self {
        self.widget = self.widget.suffix(suffix);
        self
    }

    /// Shows the field in a row whose height cannot change when keyboard
    /// editing replaces the drag button with a text field.
    pub(crate) fn show(self, ui: &mut egui::Ui, width: f32) -> egui::Response {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, CONTROL_HEIGHT), egui::Sense::hover());
        response.union(self.show_at(ui, rect))
    }

    pub(crate) fn show_at(self, ui: &mut egui::Ui, rect: egui::Rect) -> egui::Response {
        ui.place(rect, self.widget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_field_reserves_one_fixed_height_row() {
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let mut value = 12.0;
            let top = ui.cursor().top();
            NumberField::new(&mut value).show(ui, 120.0);
            let consumed_height = ui.cursor().top() - top - ui.spacing().item_spacing.y;

            assert_eq!(consumed_height, CONTROL_HEIGHT);
        });
        output.textures_delta.clear();
    }
}
