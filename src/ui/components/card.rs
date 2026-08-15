use eframe::egui;

use crate::ui::theme::{helper_border, helper_card_surface};

pub(crate) fn reference_card(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    title: &str,
    body: impl FnOnce(&mut egui::Ui),
) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            card(ui, width, height, title, egui::Margin::same(10), body);
        },
    );
}

pub(crate) fn card(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    title: &str,
    margin: egui::Margin,
    body: impl FnOnce(&mut egui::Ui),
) {
    let horizontal_margin = f32::from(margin.left + margin.right);
    let vertical_margin = f32::from(margin.top + margin.bottom);
    egui::Frame::new()
        .fill(helper_card_surface())
        .stroke(egui::Stroke::new(1.0, helper_border()))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_width((width - horizontal_margin).max(1.0));
            ui.set_min_height((height - vertical_margin).max(1.0));
            ui.label(egui::RichText::new(title).strong());
            ui.add_space(4.0);
            body(ui);
        });
}
