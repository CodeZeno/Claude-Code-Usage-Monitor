use eframe::egui;
use lucide_icons::Icon as LucideIcon;

use crate::localization::LanguageId;
use crate::ui::components::icon::{icon_only_button, icon_text, labeled_icon_button};
use crate::ui::theme::{danger, helper_border, helper_surface, muted, success};

pub(crate) struct ExpressionHelperState {
    pub(crate) draft: String,
    original_draft: String,
    pub(crate) variable_filter: String,
    pub(crate) function_filter: String,
}

impl ExpressionHelperState {
    pub(crate) fn new(draft: String) -> Self {
        Self {
            original_draft: draft.clone(),
            draft,
            variable_filter: String::new(),
            function_filter: String::new(),
        }
    }

    pub(crate) fn has_unsaved_changes(&self) -> bool {
        self.draft != self.original_draft
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExpressionHelperAction {
    Continue,
    Close,
    Apply,
}

pub(crate) fn show_expression_helper(
    ui: &mut egui::Ui,
    state: &mut ExpressionHelperState,
    language: LanguageId,
    evaluate: impl Fn(&str) -> Result<String, String>,
    render_reference_panels: impl FnOnce(&mut egui::Ui, &mut ExpressionHelperState, f32),
) -> ExpressionHelperAction {
    let mut action = ExpressionHelperAction::Continue;
    let width = ui.available_width();
    let height = ui.available_height();
    let can_apply = evaluate(&state.draft).is_ok();

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
                        egui::RichText::new(language.text("Expression helper"))
                            .size(20.0)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(
                            language.text("Build and validate an expression using the values supported by the theme engine."),
                        )
                        .color(muted()),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let close_response = if state.has_unsaved_changes() {
                        ui.add(egui::Button::new((
                            language.text("Discard"),
                            icon_text(LucideIcon::X, 16.0),
                        )))
                        .on_hover_text(language.text("Discard expression changes"))
                    } else {
                        ui.add(icon_only_button(LucideIcon::X))
                        .on_hover_text(language.text("Close expression helper"))
                    };
                    if close_response.clicked() {
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
                    .hint_text(language.text("Enter an expression...")),
            );

            let validation = evaluate(&state.draft);
            ui.add_space(6.0);
            ui.horizontal(|ui| match &validation {
                Ok(result) => {
                    ui.label(
                        icon_text(LucideIcon::CheckCircle, 15.0).color(success()),
                    );
                    ui.colored_label(success(), language.text("Valid expression"));
                    ui.separator();
                    ui.label(
                        egui::RichText::new(language.text("Current result")).color(muted()),
                    );
                    ui.label(egui::RichText::new(result).strong());
                }
                Err(error) => {
                    ui.label(
                        icon_text(LucideIcon::AlertCircle, 15.0).color(danger()),
                    );
                    ui.colored_label(danger(), error);
                }
            });
            ui.add_space(10.0);

            let panel_height = ui.available_height().max(180.0);
            render_reference_panels(ui, state, panel_height);
        });

    action
}
