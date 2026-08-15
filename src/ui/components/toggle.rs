use eframe::egui;

use crate::ui::theme::{accent, toggle_inactive, toggle_inactive_hover, toggle_knob, toggle_label};
use crate::ui::tokens::{CONTROL_HEIGHT, TOGGLE_WIDTH};

/// A compact boolean switch with caller-provided state labels.
pub(crate) struct Toggle<'a> {
    value: &'a mut bool,
    enabled_label: &'a str,
    disabled_label: &'a str,
}

impl<'a> Toggle<'a> {
    pub(crate) fn new(value: &'a mut bool) -> Self {
        Self {
            value,
            enabled_label: "On",
            disabled_label: "Off",
        }
    }

    pub(crate) fn labels(mut self, enabled: &'a str, disabled: &'a str) -> Self {
        self.enabled_label = enabled;
        self.disabled_label = disabled;
        self
    }

    pub(crate) fn show(self, ui: &mut egui::Ui) -> egui::Response {
        let (rect, mut response) = ui.allocate_exact_size(
            egui::vec2(TOGGLE_WIDTH, CONTROL_HEIGHT),
            egui::Sense::click(),
        );
        if response.clicked() {
            *self.value = !*self.value;
            response.mark_changed();
        }
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        let switch_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 20.0, rect.center().y),
            egui::vec2(40.0, 22.0),
        );
        let track = if *self.value {
            accent()
        } else if response.hovered() {
            toggle_inactive_hover()
        } else {
            toggle_inactive()
        };
        ui.painter().rect_filled(switch_rect, 11.0, track);
        let knob_x = if *self.value {
            switch_rect.right() - 11.0
        } else {
            switch_rect.left() + 11.0
        };
        ui.painter().circle_filled(
            egui::pos2(knob_x, switch_rect.center().y),
            8.0,
            toggle_knob(),
        );
        ui.painter().text(
            egui::pos2(switch_rect.left() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            if *self.value {
                self.enabled_label
            } else {
                self.disabled_label
            },
            egui::FontId::proportional(14.0),
            toggle_label(),
        );
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_reserves_one_fixed_height_row() {
        let context = egui::Context::default();
        let _ = context.run_ui(egui::RawInput::default(), |ui| {
            let mut value = false;
            let top = ui.cursor().top();
            Toggle::new(&mut value).labels("Yes", "No").show(ui);
            let consumed_height = ui.cursor().top() - top - ui.spacing().item_spacing.y;

            assert_eq!(consumed_height, CONTROL_HEIGHT);
        });
    }
}
