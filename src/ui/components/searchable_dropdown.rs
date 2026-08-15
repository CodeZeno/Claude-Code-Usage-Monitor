use eframe::egui;

use crate::ui::components::dropdown::{dropdown_scroll_height, dropdown_selectable_label};
use crate::ui::components::text_field::singleline;
use crate::ui::theme::muted;
use crate::ui::tokens::CONTROL_HEIGHT;

/// A searchable text field backed by a caller-provided list of options.
pub(crate) fn searchable_dropdown<T: AsRef<str>>(
    ui: &mut egui::Ui,
    id: egui::Id,
    selected_value: &mut String,
    options: &[T],
    width: f32,
    hint_text: &str,
    no_results_text: &str,
) {
    let response = ui.add_sized(
        [width, CONTROL_HEIGHT],
        singleline(selected_value)
            .desired_width(f32::INFINITY)
            .id(id)
            .hint_text(hint_text),
    );
    let filter = selected_value.trim().to_lowercase();
    let mut selected = None;
    let popup_width = response.rect.width();
    let popup_frame = egui::Frame::popup(ui.style());
    let popup_content_width = (popup_width - popup_frame.total_margin().sum().x).max(1.0);
    egui::Popup::from_response(&response)
        .id(id.with("suggestions"))
        .width(popup_width)
        .frame(popup_frame)
        .open(response.has_focus())
        .show(|ui| {
            ui.set_min_width(popup_content_width);
            egui::ScrollArea::vertical()
                .max_height(dropdown_scroll_height(ui))
                .show(ui, |ui| {
                    let mut found = false;
                    for option in options
                        .iter()
                        .map(AsRef::as_ref)
                        .filter(|option| option_matches(option, &filter))
                    {
                        found = true;
                        if dropdown_selectable_label(
                            ui,
                            option.eq_ignore_ascii_case(selected_value),
                            option,
                        )
                        .clicked()
                        {
                            selected = Some(option.to_owned());
                        }
                    }
                    if !found {
                        ui.label(egui::RichText::new(no_results_text).color(muted()));
                    }
                });
        });
    if let Some(selected) = selected {
        *selected_value = selected;
        ui.memory_mut(|memory| memory.surrender_focus(response.id));
    }
}

pub(crate) fn option_matches(option: &str, lowercase_filter: &str) -> bool {
    lowercase_filter.is_empty() || option.to_lowercase().contains(lowercase_filter)
}
