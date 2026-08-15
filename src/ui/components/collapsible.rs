use std::hash::Hash;

use eframe::egui;

pub(crate) fn inspector_section<R>(
    ui: &mut egui::Ui,
    id: impl Hash + std::fmt::Debug,
    title: &str,
    body: impl FnOnce(&mut egui::Ui) -> R,
) {
    egui::CollapsingHeader::new(title)
        .id_salt(id)
        .default_open(true)
        .show(ui, body);
}
