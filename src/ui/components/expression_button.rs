use eframe::egui;
use lucide_icons::Icon as LucideIcon;

use crate::ui::components::icon::icon_button;

/// The shared button used to open or activate an expression editor.
pub(crate) fn expression_button(ui: &mut egui::Ui, active: bool) -> egui::Response {
    icon_button(ui, LucideIcon::Code, active)
}
