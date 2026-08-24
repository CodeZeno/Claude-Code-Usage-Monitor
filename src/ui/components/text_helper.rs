use eframe::egui;
use lucide_icons::Icon as LucideIcon;

use crate::localization::LanguageId;
use crate::ui::components::card::card;
use crate::ui::components::icon::{icon_only_button, icon_text, labeled_icon_button};
use crate::ui::theme::{danger, helper_border, helper_surface, muted, success};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextTemplateFormat {
    Automatic,
    WholeNumber,
    OneDecimal,
    TwoDecimals,
    Percentage,
    ShortDuration,
    DetailedDuration,
    UsageLine,
    UsageBadge,
    WeekdayTwo,
    WeekdayShort,
    WeekdayLong,
    Day,
    DayTwo,
    Month,
    MonthTwo,
    MonthShort,
    MonthLong,
    YearTwo,
    Year,
    DateShort,
    DateLong,
    TimeShort,
    TimeSeconds,
    Time24,
    Time24Seconds,
    Time12,
    Time12Seconds,
    DateTimeShort,
    DateTimeLong,
    IsoDate,
    IsoTime,
    IsoDateTime,
    PlainText,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextTemplateValueKind {
    Number,
    Percentage,
    Duration,
    Timestamp,
    UsageSummary,
    Text,
}

pub(crate) struct TextHelperState {
    pub(crate) draft: String,
    original_draft: String,
    pub(crate) value_filter: String,
    pub(crate) selected_value: &'static str,
    pub(crate) selected_format: TextTemplateFormat,
}

impl TextHelperState {
    pub(crate) fn new(draft: String) -> Self {
        Self {
            original_draft: draft.clone(),
            draft,
            value_filter: String::new(),
            selected_value: "active.session.percentage",
            selected_format: TextTemplateFormat::Percentage,
        }
    }

    pub(crate) fn has_unsaved_changes(&self) -> bool {
        self.draft != self.original_draft
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextHelperAction {
    Continue,
    Close,
    Apply,
}

pub(crate) fn show_text_helper(
    ui: &mut egui::Ui,
    state: &mut TextHelperState,
    language: LanguageId,
    validate: impl Fn(&str) -> Vec<String>,
    build_preview: impl Fn(&str) -> String,
    render_reference_panels: impl FnOnce(&mut egui::Ui, &mut TextHelperState, f32),
) -> TextHelperAction {
    let mut action = TextHelperAction::Continue;
    let width = ui.available_width();
    let height = ui.available_height();
    let can_apply = validate(&state.draft).is_empty();

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
                        egui::RichText::new(language.text("Text helper"))
                            .size(20.0)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(
                            language.text("Build text from regular words and correctly formatted provider values."),
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
                        .on_hover_text(language.text("Discard text changes"))
                    } else {
                        ui.add(icon_only_button(LucideIcon::X))
                        .on_hover_text(language.text("Close text helper"))
                    };
                    if close_response.clicked() {
                        action = TextHelperAction::Close;
                    }
                    if ui
                        .add_enabled(
                            can_apply,
                            labeled_icon_button(LucideIcon::Save, language.text("Apply")),
                        )
                        .clicked()
                    {
                        action = TextHelperAction::Apply;
                    }
                });
            });
            ui.add_space(10.0);
            ui.add_sized(
                [ui.available_width(), 116.0],
                egui::TextEdit::multiline(&mut state.draft)
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::same(10))
                    .hint_text(language.text("Type text here, then insert provider values below...")),
            );

            let validation = validate(&state.draft);
            let preview = build_preview(&state.draft);
            ui.add_space(8.0);
            card(
                ui,
                ui.available_width(),
                76.0,
                language.text("Live preview"),
                egui::Margin::symmetric(11, 9),
                |ui| {
                    if preview.is_empty() {
                        ui.label(
                            egui::RichText::new(language.text("Preview is empty"))
                                .italics()
                                .color(muted()),
                        );
                    } else {
                        ui.label(egui::RichText::new(&preview).size(18.0).strong());
                    }
                },
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if validation.is_empty() {
                    ui.label(
                        icon_text(LucideIcon::CheckCircle, 15.0).color(success()),
                    );
                    ui.colored_label(success(), language.text("Template is valid"));
                } else {
                    ui.label(
                        icon_text(LucideIcon::AlertCircle, 15.0).color(danger()),
                    );
                    ui.colored_label(danger(), validation.join(" · "));
                }
            });
            ui.add_space(10.0);

            let panel_height = ui.available_height().max(150.0);
            render_reference_panels(ui, state, panel_height);
        });

    action
}
