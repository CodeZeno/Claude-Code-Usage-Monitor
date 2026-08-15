use eframe::egui;

use crate::ui::theme::{muted, section_border, section_surface, setting_separator_color};
use crate::ui::tokens::{
    CONTROL_HEIGHT, INSPECTOR_CONTROL_MAX_WIDTH, INSPECTOR_LABEL_WIDTH, INSPECTOR_RIGHT_GUTTER,
};

pub(crate) fn studio_region(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 7,
            right: 7,
            top: 7,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width((width - 14.0).max(1.0));
            ui.set_min_height((height - 7.0).max(1.0));
            body(ui);
        });
}

pub(crate) fn settings_scroll_area<R>(
    ui: &mut egui::Ui,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 0))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .content_margin(egui::Margin::symmetric(16, 0))
                .show(ui, body)
                .inner
        })
        .inner
}

pub(crate) fn settings_section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(title).size(25.0).strong());
    ui.add_space(10.0);
    egui::Frame::new()
        .fill(section_surface())
        .stroke(egui::Stroke::new(1.0, section_border()))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(20, 8))
        .show(ui, body);
    ui.add_space(18.0);
}

pub(crate) fn setting_row(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    let row_width = ui.available_width();
    let (row_rect, _) = ui.allocate_exact_size(egui::vec2(row_width, 62.0), egui::Sense::hover());
    let control_width = row_width.min(360.0);
    let control_rect = egui::Rect::from_min_max(
        egui::pos2(row_rect.right() - control_width, row_rect.top()),
        row_rect.max,
    );
    let label_rect = egui::Rect::from_min_max(
        row_rect.min,
        egui::pos2(
            (control_rect.left() - 16.0).max(row_rect.left()),
            row_rect.bottom(),
        ),
    );
    let mut label_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(label_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    label_ui.set_clip_rect(label_rect.intersect(ui.clip_rect()));
    label_ui.add_space(8.0);
    label_ui.label(egui::RichText::new(title).size(16.0).strong());
    label_ui.label(egui::RichText::new(detail).size(14.0).color(muted()));

    let mut control_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(control_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    control_ui.set_clip_rect(control_rect.intersect(ui.clip_rect()));
    control(&mut control_ui);
}

pub(crate) fn setting_separator(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0, setting_separator_color()),
    );
}

pub(crate) fn inspector_row(ui: &mut egui::Ui, label: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(INSPECTOR_LABEL_WIDTH, CONTROL_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(egui::RichText::new(label).color(muted()));
            },
        );
        let available_width = ui.available_width().max(1.0);
        let control_width = control_width(available_width - INSPECTOR_RIGHT_GUTTER);
        ui.allocate_ui_with_layout(
            egui::vec2(available_width, CONTROL_HEIGHT),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.add_space(INSPECTOR_RIGHT_GUTTER);
                ui.allocate_ui_with_layout(
                    egui::vec2(control_width, CONTROL_HEIGHT),
                    egui::Layout::left_to_right(egui::Align::Center),
                    body,
                );
            },
        );
    });
}

pub(crate) fn available_control_width(ui: &egui::Ui) -> f32 {
    control_width(ui.available_width())
}

pub(crate) fn control_width(available_width: f32) -> f32 {
    available_width.clamp(1.0, INSPECTOR_CONTROL_MAX_WIDTH)
}
