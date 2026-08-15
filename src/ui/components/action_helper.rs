use eframe::egui;
use lucide_icons::Icon as LucideIcon;

use crate::localization::LanguageId;
use crate::ui::components::expression_helper::{ExpressionHelperAction, ExpressionHelperState};
use crate::ui::components::icon::{icon_only_button, icon_text, labeled_icon_button};
use crate::ui::theme::{danger, helper_border, helper_surface, muted, success};

pub(crate) fn show_action_helper(
    ui: &mut egui::Ui,
    state: &mut ExpressionHelperState,
    language: LanguageId,
    detail: &'static str,
    validate: impl Fn(&str) -> Result<String, String>,
    render_reference_panels: impl FnOnce(&mut egui::Ui, &mut ExpressionHelperState, f32),
) -> ExpressionHelperAction {
    let mut action = ExpressionHelperAction::Continue;
    let width = ui.available_width();
    let height = ui.available_height();
    let can_apply = validate(&state.draft).is_ok();

    egui::Frame::new()
        .fill(helper_surface())
        .stroke(egui::Stroke::new(1.0, helper_border()))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width((width - 28.0).max(1.0));
            ui.set_min_height((height - 28.0).max(1.0));
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(language.text("Action helper"))
                            .size(20.0)
                            .strong(),
                    );
                    ui.label(egui::RichText::new(language.text(detail)).color(muted()));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let close = if state.has_unsaved_changes() {
                        ui.add(egui::Button::new((
                            language.text("Discard"),
                            icon_text(LucideIcon::X, 16.0),
                        )))
                        .on_hover_text(language.text("Discard action changes"))
                    } else {
                        ui.add(icon_only_button(LucideIcon::X))
                            .on_hover_text(language.text("Close action helper"))
                    };
                    if close.clicked() {
                        action = ExpressionHelperAction::Close;
                    }
                    if ui
                        .add_enabled(
                            can_apply,
                            labeled_icon_button(LucideIcon::Save, language.text("Apply")),
                        )
                        .clicked()
                    {
                        action = ExpressionHelperAction::Apply;
                    }
                });
            });
            ui.add_space(10.0);
            ui.add_sized(
                [ui.available_width(), 132.0],
                egui::TextEdit::multiline(&mut state.draft)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::same(10))
                    .hint_text(language.text("Enter actions...")),
            );
            let validation = validate(&state.draft);
            ui.add_space(6.0);
            ui.horizontal(|ui| match validation {
                Ok(summary) => {
                    ui.label(icon_text(LucideIcon::CheckCircle, 15.0).color(success()));
                    ui.colored_label(success(), language.text("Valid actions"));
                    if !summary.is_empty() {
                        ui.separator();
                        ui.label(summary);
                    }
                }
                Err(error) => {
                    ui.label(icon_text(LucideIcon::AlertCircle, 15.0).color(danger()));
                    ui.colored_label(danger(), error);
                }
            });
            ui.add_space(10.0);
            let panel_height = ui.available_height().max(180.0);
            render_reference_panels(ui, state, panel_height);
        });

    action
}
