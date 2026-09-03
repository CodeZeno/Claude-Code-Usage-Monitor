use super::*;

#[derive(Clone, Copy)]
pub(super) struct TextTemplateValue {
    pub(super) group: &'static str,
    pub(super) label: &'static str,
    pub(super) expression: &'static str,
    pub(super) kind: TextTemplateValueKind,
}

pub(super) const TEXT_TEMPLATE_VALUES: &[TextTemplateValue] = &[
    TextTemplateValue {
        group: "Date and time",
        label: "Current date and time",
        expression: "time.now.unix",
        kind: TextTemplateValueKind::Timestamp,
    },
    TextTemplateValue {
        group: "Application",
        label: "App version",
        expression: "app.version",
        kind: TextTemplateValueKind::Text,
    },
    TextTemplateValue {
        group: "Application",
        label: "App version major",
        expression: "app.version.major",
        kind: TextTemplateValueKind::Number,
    },
    TextTemplateValue {
        group: "Application",
        label: "App version minor",
        expression: "app.version.minor",
        kind: TextTemplateValueKind::Number,
    },
    TextTemplateValue {
        group: "Application",
        label: "App version patch",
        expression: "app.version.patch",
        kind: TextTemplateValueKind::Number,
    },
    TextTemplateValue {
        group: "General",
        label: "Enabled provider count",
        expression: "providers.count",
        kind: TextTemplateValueKind::Number,
    },
    TextTemplateValue {
        group: "Active provider",
        label: "Session summary",
        expression: "active.session",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Active provider",
        label: "Session used",
        expression: "active.session.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Active provider",
        label: "Session remaining",
        expression: "active.session.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Active provider",
        label: "Session reset",
        expression: "active.session.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Active provider",
        label: "Session reset date and time",
        expression: "active.session.reset.unix",
        kind: TextTemplateValueKind::Timestamp,
    },
    TextTemplateValue {
        group: "Active provider",
        label: "Weekly summary",
        expression: "active.weekly",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Active provider",
        label: "Weekly used",
        expression: "active.weekly.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Active provider",
        label: "Weekly remaining",
        expression: "active.weekly.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Active provider",
        label: "Weekly reset",
        expression: "active.weekly.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Active provider",
        label: "Weekly reset date and time",
        expression: "active.weekly.reset.unix",
        kind: TextTemplateValueKind::Timestamp,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Session summary",
        expression: "claude.session",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Session used",
        expression: "claude.session.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Session remaining",
        expression: "claude.session.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Session reset",
        expression: "claude.session.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Weekly summary",
        expression: "claude.weekly",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Weekly used",
        expression: "claude.weekly.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Weekly remaining",
        expression: "claude.weekly.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Weekly reset",
        expression: "claude.weekly.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Fable weekly summary",
        expression: "claude.fable",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Fable weekly used",
        expression: "claude.fable.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Fable weekly remaining",
        expression: "claude.fable.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Fable weekly reset",
        expression: "claude.fable.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Fable weekly reset date and time",
        expression: "claude.fable.reset.unix",
        kind: TextTemplateValueKind::Timestamp,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Session summary",
        expression: "codex.session",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Session used",
        expression: "codex.session.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Session remaining",
        expression: "codex.session.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Session reset",
        expression: "codex.session.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Five-hour summary (exact)",
        expression: "codex.five_hour",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Five-hour used (exact)",
        expression: "codex.five_hour.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Five-hour remaining (exact)",
        expression: "codex.five_hour.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Five-hour reset (exact)",
        expression: "codex.five_hour.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Five-hour reset date and time (exact)",
        expression: "codex.five_hour.reset.unix",
        kind: TextTemplateValueKind::Timestamp,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Weekly summary",
        expression: "codex.weekly",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Weekly used",
        expression: "codex.weekly.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Weekly remaining",
        expression: "codex.weekly.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Weekly reset",
        expression: "codex.weekly.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Codex",
        label: "Weekly reset date and time",
        expression: "codex.weekly.reset.unix",
        kind: TextTemplateValueKind::Timestamp,
    },
    TextTemplateValue {
        group: "Antigravity",
        label: "Session summary",
        expression: "antigravity.session",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Antigravity",
        label: "Session used",
        expression: "antigravity.session.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Antigravity",
        label: "Session remaining",
        expression: "antigravity.session.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Antigravity",
        label: "Session reset",
        expression: "antigravity.session.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Antigravity",
        label: "Weekly summary",
        expression: "antigravity.weekly",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Antigravity",
        label: "Weekly used",
        expression: "antigravity.weekly.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Antigravity",
        label: "Weekly remaining",
        expression: "antigravity.weekly.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Antigravity",
        label: "Weekly reset",
        expression: "antigravity.weekly.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "OpenCode",
        label: "Session summary",
        expression: "opencode.session",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "OpenCode",
        label: "Session used",
        expression: "opencode.session.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "OpenCode",
        label: "Session remaining",
        expression: "opencode.session.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "OpenCode",
        label: "Session reset",
        expression: "opencode.session.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "OpenCode",
        label: "Long-window label",
        expression: "opencode.weekly.label",
        kind: TextTemplateValueKind::Text,
    },
    TextTemplateValue {
        group: "OpenCode",
        label: "Long-window summary",
        expression: "opencode.weekly",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "OpenCode",
        label: "Long-window used",
        expression: "opencode.weekly.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "OpenCode",
        label: "Long-window remaining",
        expression: "opencode.weekly.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "OpenCode",
        label: "Long-window reset",
        expression: "opencode.weekly.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Cursor",
        label: "Auto summary",
        expression: "cursor.session",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Cursor",
        label: "Auto used",
        expression: "cursor.session.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Cursor",
        label: "Auto remaining",
        expression: "cursor.session.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Cursor",
        label: "Auto reset",
        expression: "cursor.session.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Cursor",
        label: "API summary",
        expression: "cursor.weekly",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Cursor",
        label: "API used",
        expression: "cursor.weekly.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Cursor",
        label: "API remaining",
        expression: "cursor.weekly.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Cursor",
        label: "API reset",
        expression: "cursor.weekly.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Labels",
        label: "Session window label",
        expression: "i18n.session_window",
        kind: TextTemplateValueKind::Text,
    },
    TextTemplateValue {
        group: "Labels",
        label: "Weekly window label",
        expression: "i18n.weekly_window",
        kind: TextTemplateValueKind::Text,
    },
    TextTemplateValue {
        group: "Labels",
        label: "Now label",
        expression: "i18n.now",
        kind: TextTemplateValueKind::Text,
    },
];

pub(super) fn text_template_value(expression: &str) -> Option<TextTemplateValue> {
    TEXT_TEMPLATE_VALUES
        .iter()
        .copied()
        .find(|value| value.expression == expression)
}

pub(super) fn text_template_formats(kind: TextTemplateValueKind) -> &'static [TextTemplateFormat] {
    use TextTemplateFormat as Format;
    match kind {
        TextTemplateValueKind::Number => &[
            Format::Automatic,
            Format::WholeNumber,
            Format::OneDecimal,
            Format::TwoDecimals,
        ],
        TextTemplateValueKind::Percentage => &[
            Format::Percentage,
            Format::WholeNumber,
            Format::OneDecimal,
            Format::TwoDecimals,
            Format::Automatic,
        ],
        TextTemplateValueKind::Duration => &[
            Format::ShortDuration,
            Format::DetailedDuration,
            Format::WholeNumber,
        ],
        TextTemplateValueKind::Timestamp => &[
            Format::WeekdayTwo,
            Format::WeekdayShort,
            Format::WeekdayLong,
            Format::Day,
            Format::DayTwo,
            Format::Month,
            Format::MonthTwo,
            Format::MonthShort,
            Format::MonthLong,
            Format::YearTwo,
            Format::Year,
            Format::DateShort,
            Format::DateLong,
            Format::TimeShort,
            Format::TimeSeconds,
            Format::Time24,
            Format::Time24Seconds,
            Format::Time12,
            Format::Time12Seconds,
            Format::DateTimeShort,
            Format::DateTimeLong,
            Format::IsoDate,
            Format::IsoTime,
            Format::IsoDateTime,
            Format::WholeNumber,
        ],
        TextTemplateValueKind::UsageSummary => &[Format::UsageLine, Format::UsageBadge],
        TextTemplateValueKind::Text => &[Format::PlainText],
    }
}

pub(super) fn default_text_template_format(kind: TextTemplateValueKind) -> TextTemplateFormat {
    text_template_formats(kind)[0]
}

pub(super) fn text_template_format_label(
    language: LanguageId,
    format: TextTemplateFormat,
) -> &'static str {
    match format {
        TextTemplateFormat::Automatic => language.text("Automatic number"),
        TextTemplateFormat::WholeNumber => language.text("Whole number"),
        TextTemplateFormat::OneDecimal => language.text("One decimal"),
        TextTemplateFormat::TwoDecimals => language.text("Two decimals"),
        TextTemplateFormat::Percentage => language.text("Percentage"),
        TextTemplateFormat::ShortDuration => language.text("Short duration"),
        TextTemplateFormat::DetailedDuration => language.text("Detailed duration"),
        TextTemplateFormat::UsageLine => language.text("Usage and reset"),
        TextTemplateFormat::UsageBadge => language.text("Usage only"),
        TextTemplateFormat::WeekdayTwo => language.text("Weekday (2 letters)"),
        TextTemplateFormat::WeekdayShort => language.text("Weekday (short)"),
        TextTemplateFormat::WeekdayLong => language.text("Weekday (full)"),
        TextTemplateFormat::Day => language.text("Day"),
        TextTemplateFormat::DayTwo => language.text("Day (2 digits)"),
        TextTemplateFormat::Month => language.text("Month"),
        TextTemplateFormat::MonthTwo => language.text("Month (2 digits)"),
        TextTemplateFormat::MonthShort => language.text("Month (short)"),
        TextTemplateFormat::MonthLong => language.text("Month (full)"),
        TextTemplateFormat::YearTwo => language.text("Year (2 digits)"),
        TextTemplateFormat::Year => language.text("Year"),
        TextTemplateFormat::DateShort => language.text("Short date"),
        TextTemplateFormat::DateLong => language.text("Long date"),
        TextTemplateFormat::TimeShort => language.text("Time"),
        TextTemplateFormat::TimeSeconds => language.text("Time with seconds"),
        TextTemplateFormat::Time24 => language.text("24-hour time"),
        TextTemplateFormat::Time24Seconds => language.text("24-hour time with seconds"),
        TextTemplateFormat::Time12 => language.text("12-hour time"),
        TextTemplateFormat::Time12Seconds => language.text("12-hour time with seconds"),
        TextTemplateFormat::DateTimeShort => language.text("Short date and time"),
        TextTemplateFormat::DateTimeLong => language.text("Long date and time"),
        TextTemplateFormat::IsoDate => language.text("ISO date"),
        TextTemplateFormat::IsoTime => language.text("ISO time"),
        TextTemplateFormat::IsoDateTime => language.text("ISO date and time"),
        TextTemplateFormat::PlainText => language.text("Plain text"),
    }
}

pub(super) fn text_template_format_code(format: TextTemplateFormat) -> Option<&'static str> {
    match format {
        TextTemplateFormat::Automatic => Some("0.##"),
        TextTemplateFormat::WholeNumber => Some("0"),
        TextTemplateFormat::OneDecimal => Some("0.0"),
        TextTemplateFormat::TwoDecimals => Some("0.00"),
        TextTemplateFormat::Percentage => Some("percent"),
        TextTemplateFormat::ShortDuration => Some("duration_short"),
        TextTemplateFormat::DetailedDuration => Some("duration"),
        TextTemplateFormat::UsageLine => Some("usage_line"),
        TextTemplateFormat::UsageBadge => Some("usage_badge"),
        TextTemplateFormat::WeekdayTwo => Some("weekday_2"),
        TextTemplateFormat::WeekdayShort => Some("weekday_short"),
        TextTemplateFormat::WeekdayLong => Some("weekday_long"),
        TextTemplateFormat::Day => Some("day"),
        TextTemplateFormat::DayTwo => Some("day_2"),
        TextTemplateFormat::Month => Some("month"),
        TextTemplateFormat::MonthTwo => Some("month_2"),
        TextTemplateFormat::MonthShort => Some("month_short"),
        TextTemplateFormat::MonthLong => Some("month_long"),
        TextTemplateFormat::YearTwo => Some("year_2"),
        TextTemplateFormat::Year => Some("year"),
        TextTemplateFormat::DateShort => Some("date_short"),
        TextTemplateFormat::DateLong => Some("date_long"),
        TextTemplateFormat::TimeShort => Some("time_short"),
        TextTemplateFormat::TimeSeconds => Some("time_seconds"),
        TextTemplateFormat::Time24 => Some("time_24"),
        TextTemplateFormat::Time24Seconds => Some("time_24_seconds"),
        TextTemplateFormat::Time12 => Some("time_12"),
        TextTemplateFormat::Time12Seconds => Some("time_12_seconds"),
        TextTemplateFormat::DateTimeShort => Some("datetime_short"),
        TextTemplateFormat::DateTimeLong => Some("datetime_long"),
        TextTemplateFormat::IsoDate => Some("iso_date"),
        TextTemplateFormat::IsoTime => Some("iso_time"),
        TextTemplateFormat::IsoDateTime => Some("iso_datetime"),
        TextTemplateFormat::PlainText => None,
    }
}

pub(super) fn text_template_token(expression: &str, format: TextTemplateFormat) -> String {
    text_template_format_code(format).map_or_else(
        || format!("{{{expression}}}"),
        |format| format!("{{{expression}:{format}}}"),
    )
}

pub(super) fn set_text_template(content: &mut SceneContent, template: String) -> bool {
    match content {
        SceneContent::Text {
            template: current, ..
        } => {
            *current = template;
            true
        }
        _ => false,
    }
}

pub(super) fn format_number_for_ui(value: f64) -> String {
    if value.fract().abs() < 0.000_001 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

pub(super) fn append_expression_token(draft: &mut String, token: &str) {
    let needs_space = !draft.is_empty()
        && !draft.ends_with(char::is_whitespace)
        && !draft.ends_with('(')
        && !token.starts_with(')')
        && !token.starts_with(',');
    if needs_space {
        draft.push(' ');
    }
    draft.push_str(token);
}

pub(super) fn text_template_value_sample(
    value: TextTemplateValue,
    format: TextTemplateFormat,
    context: &DataContext,
) -> String {
    theme_engine::format_template(&text_template_token(value.expression, format), context)
}

pub(super) fn text_template_values_panel(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    context: &DataContext,
    filter: &mut String,
    selected_value: &mut &'static str,
    selected_format: &mut TextTemplateFormat,
    language: LanguageId,
) {
    expression_reference_card(ui, size.x, size.y, language.text("Provider values"), |ui| {
        ui.add(
            singleline_text_edit(filter)
                .desired_width(ui.available_width())
                .hint_text(language.text("Search values...")),
        );
        ui.add_space(4.0);
        let needle = filter.trim().to_ascii_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("text-template-values")
            .auto_shrink([false, false])
            .content_margin(egui::Margin {
                right: 12,
                ..egui::Margin::ZERO
            })
            .max_height((size.y - 72.0).max(80.0))
            .show(ui, |ui| {
                let mut last_group = "";
                for value in TEXT_TEMPLATE_VALUES.iter().copied().filter(|value| {
                    needle.is_empty()
                        || value.label.to_ascii_lowercase().contains(&needle)
                        || language.text(value.label).to_lowercase().contains(&needle)
                        || value.group.to_ascii_lowercase().contains(&needle)
                        || language.text(value.group).to_lowercase().contains(&needle)
                        || value.expression.to_ascii_lowercase().contains(&needle)
                }) {
                    if value.group != last_group {
                        if !last_group.is_empty() {
                            ui.add_space(6.0);
                        }
                        ui.label(
                            egui::RichText::new(language.text(value.group))
                                .small()
                                .strong()
                                .color(muted()),
                        );
                        last_group = value.group;
                    }
                    ui.horizontal(|ui| {
                        let is_selected = *selected_value == value.expression;
                        if ui
                            .add(
                                egui::Button::selectable(is_selected, language.text(value.label))
                                    .frame(false),
                            )
                            .on_hover_text(value.expression)
                            .clicked()
                        {
                            *selected_value = value.expression;
                            *selected_format = default_text_template_format(value.kind);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let sample = text_template_value_sample(
                                value,
                                default_text_template_format(value.kind),
                                context,
                            );
                            ui.label(egui::RichText::new(sample).color(muted()));
                        });
                    });
                }
            });
    });
}

pub(super) fn text_template_formats_panel(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    context: &DataContext,
    selected_value: &str,
    selected_format: &mut TextTemplateFormat,
    draft: &mut String,
    language: LanguageId,
) {
    expression_reference_card(ui, size.x, size.y, language.text("Format"), |ui| {
        let value = text_template_value(selected_value).unwrap_or(TEXT_TEMPLATE_VALUES[0]);
        ui.label(egui::RichText::new(language.text(value.label)).strong());
        ui.label(
            egui::RichText::new(value.expression)
                .small()
                .family(egui::FontFamily::Monospace)
                .color(muted()),
        );
        ui.add_space(8.0);
        for format in text_template_formats(value.kind) {
            let sample = text_template_value_sample(value, *format, context);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::selectable(
                            *selected_format == *format,
                            text_template_format_label(language, *format),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    *selected_format = *format;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(sample).color(muted()));
                });
            });
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        let token = text_template_token(value.expression, *selected_format);
        ui.label(
            egui::RichText::new(&token)
                .small()
                .color(muted())
                .monospace(),
        );
        ui.add_space(6.0);
        if ui
            .add_sized(
                [ui.available_width(), CONTROL_HEIGHT],
                lucide_labeled_button(LucideIcon::Code, language.text("Insert value")),
            )
            .clicked()
        {
            draft.push_str(&token);
        }
    });
}

pub(super) fn text_template_guide_panel(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    language: LanguageId,
) {
    expression_reference_card(ui, width, height, language.text("Guide"), |ui| {
        ui.label(language.text("Type ordinary words directly in the editor."));
        ui.add_space(8.0);
        ui.label(language.text("Select a provider value, choose its format, then insert it."));
        ui.add_space(8.0);
        ui.label(language.text("Values are inserted at the end of the current text and can be moved or edited afterwards."));
        ui.add_space(8.0);
        ui.label(language.text("To show a literal opening brace, type:"));
        ui.label(egui::RichText::new("{{").monospace().color(muted()));
        ui.add_space(8.0);
        ui.label(language.text("Advanced expressions are supported inside a value token."));
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn action_reference_panels(
    ui: &mut egui::Ui,
    height: f32,
    targets: &[(String, String)],
    self_id: &str,
    target: &mut String,
    property: &mut MouseActionProperty,
    value: &mut String,
    context_menus: &[context_menu::ContextMenuDescriptor],
    context_menu_reference: &mut String,
    draft: &mut String,
    language: LanguageId,
) {
    let gap = ui.spacing().item_spacing.x;
    let panel_width = ((ui.available_width() - gap * 2.0) / 3.0).max(1.0);
    ui.horizontal(|ui| {
        expression_reference_card(ui, panel_width, height, language.text("Actions"), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("action-helper-actions")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if ui.button(language.text("Show dashboard")).clicked() {
                        append_action(draft, "show_dashboard()");
                    }
                    if ui.button(language.text("Toggle dashboard")).clicked() {
                        append_action(draft, "toggle_dashboard()");
                    }
                    ui.label(
                        egui::RichText::new(language.text("Context menu"))
                            .small()
                            .color(muted()),
                    );
                    Dropdown::from_id_salt("action-helper-context-menu")
                        .width(ui.available_width())
                        .selected_text(
                            context_menus
                                .iter()
                                .find(|menu| menu.id == *context_menu_reference)
                                .map(|menu| menu.name.as_str())
                                .unwrap_or(context_menu_reference.as_str()),
                        )
                        .show_ui(ui, |ui| {
                            for menu in context_menus {
                                dropdown_selectable_value(
                                    ui,
                                    context_menu_reference,
                                    menu.id.clone(),
                                    &menu.name,
                                );
                            }
                        });
                    if ui.button(language.text("Show context menu")).clicked() {
                        let reference = context_menu_reference
                            .replace('\\', "\\\\")
                            .replace('"', "\\\"");
                        append_action(draft, &format!("show_context_menu(\"{reference}\")"));
                    }
                    ui.separator();
                    if ui.button(language.text("Set property")).clicked() {
                        append_action(
                            draft,
                            &format!("set({target}, {}, {})", property.name(), value.trim()),
                        );
                    }
                    if ui
                        .add_enabled(
                            *property == MouseActionProperty::Render,
                            egui::Button::new(language.text("Toggle property")),
                        )
                        .on_disabled_hover_text(
                            language.text("Toggle currently supports Render only"),
                        )
                        .clicked()
                    {
                        append_action(draft, &format!("toggle({target}, {})", property.name()));
                    }
                    if ui.button(language.text("Reset property")).clicked() {
                        append_action(draft, &format!("reset({target}, {})", property.name()));
                    }
                    let numeric_property = *property != MouseActionProperty::Render;
                    if ui
                        .add_enabled(
                            numeric_property,
                            egui::Button::new(language.text("Increase value")),
                        )
                        .on_disabled_hover_text(language.text("Choose a numeric property"))
                        .clicked()
                    {
                        append_action(
                            draft,
                            &format!("increase({target}, {}, {})", property.name(), value.trim()),
                        );
                    }
                    if ui
                        .add_enabled(
                            numeric_property,
                            egui::Button::new(language.text("Decrease value")),
                        )
                        .on_disabled_hover_text(language.text("Choose a numeric property"))
                        .clicked()
                    {
                        append_action(
                            draft,
                            &format!("decrease({target}, {}, {})", property.name(), value.trim()),
                        );
                    }
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(
                            language.text("Actions run from top to bottom in one update."),
                        )
                        .small()
                        .color(muted()),
                    );
                });
        });
        expression_reference_card(ui, panel_width, height, language.text("Layers"), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("action-helper-layers")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (id, name) in targets {
                        let token = if id.eq_ignore_ascii_case(self_id) {
                            "self".to_string()
                        } else {
                            format!("\"{}\"", id.replace('\\', "\\\\").replace('"', "\\\""))
                        };
                        let label = if token == "self" {
                            format!("{} ({})", language.text("Self"), name)
                        } else {
                            format!("{name}  ·  {id}")
                        };
                        if ui.selectable_label(*target == token, label).clicked() {
                            *target = token;
                        }
                    }
                });
        });
        expression_reference_card(ui, panel_width, height, language.text("Properties"), |ui| {
            for candidate in MouseActionProperty::ALL {
                let label = match candidate {
                    MouseActionProperty::Render => language.text("Render"),
                    MouseActionProperty::Visibility => language.text("Visibility"),
                    MouseActionProperty::X => language.text("X"),
                    MouseActionProperty::Y => language.text("Y"),
                    MouseActionProperty::Width => language.text("Width"),
                    MouseActionProperty::Height => language.text("Height"),
                    MouseActionProperty::Rotation => language.text("Rotation"),
                };
                if ui.selectable_label(*property == candidate, label).clicked() {
                    *property = candidate;
                }
            }
            ui.separator();
            ui.label(
                egui::RichText::new(language.text("Value expression"))
                    .small()
                    .color(muted()),
            );
            ui.add(
                singleline_text_edit(value)
                    .desired_width(ui.available_width())
                    .hint_text(language.text("e.g. false, 120, parent.width / 2")),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    language.text(
                        "Reset removes the runtime override and restores the saved expression.",
                    ),
                )
                .small()
                .color(muted()),
            );
        });
    });
}

pub(super) fn append_action(draft: &mut String, action: &str) {
    if !draft.trim().is_empty() && !draft.ends_with('\n') {
        draft.push('\n');
    }
    draft.push_str(action);
}

pub(super) fn expression_variables_panel(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    context: &DataContext,
    filter: &mut String,
    draft: &mut String,
    language: LanguageId,
) {
    expression_reference_card(ui, width, height, language.text("Variables"), |ui| {
        ui.add(
            singleline_text_edit(filter)
                .desired_width(ui.available_width())
                .hint_text(language.text("Search variables...")),
        );
        ui.add_space(4.0);
        let needle = filter.trim().to_ascii_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("expression-variables")
            .auto_shrink([false, false])
            .content_margin(egui::Margin {
                right: 12,
                ..egui::Margin::ZERO
            })
            .max_height((height - 72.0).max(80.0))
            .show(ui, |ui| {
                let basic = ["true", "false", "pi", "e"];
                expression_variable_group(
                    ui,
                    language.text("Constants"),
                    &basic,
                    &needle,
                    context,
                    draft,
                    language,
                );
                let layout = [
                    "canvas.width",
                    "canvas.height",
                    "parent.width",
                    "parent.height",
                    "host.width",
                    "host.height",
                ];
                expression_variable_group(
                    ui,
                    language.text("Layout"),
                    &layout,
                    &needle,
                    context,
                    draft,
                    language,
                );
                let date_time = [
                    "time.now.unix",
                    "time.now.milliseconds",
                    "time.local.year",
                    "time.local.month",
                    "time.local.day",
                    "time.local.weekday",
                    "time.local.hour",
                    "time.local.minute",
                    "time.local.second",
                    "time.utc.year",
                    "time.utc.month",
                    "time.utc.day",
                    "time.utc.weekday",
                    "time.utc.hour",
                    "time.utc.minute",
                    "time.utc.second",
                ];
                expression_variable_group(
                    ui,
                    language.text("Date and time"),
                    &date_time,
                    &needle,
                    context,
                    draft,
                    language,
                );
                let mut providers = vec!["providers.count".to_string()];
                providers.extend(
                    PROVIDER_DESCRIPTORS
                        .iter()
                        .map(|descriptor| format!("providers.{}.enabled", descriptor.key)),
                );
                let providers: Vec<&str> = providers.iter().map(String::as_str).collect();
                expression_variable_group(
                    ui,
                    language.text("Providers"),
                    &providers,
                    &needle,
                    context,
                    draft,
                    language,
                );
                let application = [
                    "app.version.major",
                    "app.version.minor",
                    "app.version.patch",
                ];
                expression_variable_group(
                    ui,
                    language.text("Application"),
                    &application,
                    &needle,
                    context,
                    draft,
                    language,
                );
                for (title, provider) in std::iter::once(("Active provider", "active")).chain(
                    PROVIDER_DESCRIPTORS
                        .iter()
                        .map(|descriptor| (descriptor.display_name, descriptor.key)),
                ) {
                    let mut names = vec![
                        format!("{provider}.available"),
                        format!("{provider}.five_hour.available"),
                    ];
                    let windows = match provider {
                        "active" => &["session", "five_hour", "weekly", "fable"][..],
                        "codex" => &["session", "five_hour", "weekly"][..],
                        "claude" => &["session", "weekly", "fable"][..],
                        _ => &["session", "weekly"][..],
                    };
                    if matches!(provider, "active" | "claude") {
                        names.push(format!("{provider}.fable.available"));
                    }
                    for window in windows {
                        for metric in ["percentage", "remaining"] {
                            names.push(format!("{provider}.{window}.{metric}"));
                        }
                        for unit in ["unix", "seconds", "minutes", "hours", "days"] {
                            names.push(format!("{provider}.{window}.reset.{unit}"));
                        }
                    }
                    let names: Vec<&str> = names.iter().map(String::as_str).collect();
                    expression_variable_group(
                        ui,
                        language.text(title),
                        &names,
                        &needle,
                        context,
                        draft,
                        language,
                    );
                }
            });
    });
}

pub(super) fn expression_variable_group(
    ui: &mut egui::Ui,
    title: &str,
    names: &[&str],
    needle: &str,
    context: &DataContext,
    draft: &mut String,
    language: LanguageId,
) {
    let matches: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| needle.is_empty() || name.to_ascii_lowercase().contains(needle))
        .collect();
    if matches.is_empty() {
        return;
    }
    ui.label(egui::RichText::new(title).small().strong().color(muted()));
    for name in matches {
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::selectable(false, name).frame(false))
                .on_hover_text(language.text("Insert variable"))
                .clicked()
            {
                append_expression_token(draft, name);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let value = context
                    .get(name)
                    .map(format_number_for_ui)
                    .unwrap_or_else(|| "—".into());
                ui.label(egui::RichText::new(value).color(muted()));
            });
        });
    }
    ui.add_space(6.0);
}

pub(super) fn expression_functions_panel(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    filter: &mut String,
    draft: &mut String,
    language: LanguageId,
) {
    expression_reference_card(ui, width, height, language.text("Functions"), |ui| {
        ui.add(
            singleline_text_edit(filter)
                .desired_width(ui.available_width())
                .hint_text(language.text("Search functions...")),
        );
        ui.add_space(4.0);
        let needle = filter.trim().to_ascii_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("expression-functions")
            .auto_shrink([false, false])
            .max_height((height - 72.0).max(80.0))
            .show(ui, |ui| {
                for (name, signature, insertion, detail) in [
                    (
                        "get",
                        "get(this, property)",
                        "get(this, gap)",
                        "Value expression",
                    ),
                    ("min", "min(a, b)", "min(0, 0)", "Smaller value"),
                    ("max", "max(a, b)", "max(0, 0)", "Larger value"),
                    (
                        "clamp",
                        "clamp(value, min, max)",
                        "clamp(0, 0, 100)",
                        "Constrain a value",
                    ),
                    ("round", "round(value)", "round(0)", "Nearest integer"),
                    ("floor", "floor(value)", "floor(0)", "Round down"),
                    ("ceil", "ceil(value)", "ceil(0)", "Round up"),
                    ("abs", "abs(value)", "abs(0)", "Absolute value"),
                    ("sqrt", "sqrt(value)", "sqrt(0)", "Square root"),
                    ("pow", "pow(base, power)", "pow(0, 2)", "Exponent"),
                    (
                        "if",
                        "if(condition, yes, no)",
                        "if(true, 1, 0)",
                        "Conditional value",
                    ),
                    (
                        "lerp",
                        "lerp(start, end, amount)",
                        "lerp(0, 100, 0.5)",
                        "Linear interpolation",
                    ),
                ] {
                    if !needle.is_empty()
                        && !name.contains(&needle)
                        && !detail.to_ascii_lowercase().contains(&needle)
                    {
                        continue;
                    }
                    if ui
                        .add(
                            egui::Button::selectable(false, signature)
                                .frame(false)
                                .wrap(),
                        )
                        .on_hover_text(language.text(detail))
                        .clicked()
                    {
                        append_expression_token(draft, insertion);
                    }
                }
            });
    });
}

pub(super) fn expression_operators_panel(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    draft: &mut String,
    language: LanguageId,
) {
    expression_reference_card(ui, width, height, language.text("Operators"), |ui| {
        egui::ScrollArea::vertical()
            .id_salt("expression-operators")
            .auto_shrink([false, false])
            .max_height((height - 38.0).max(80.0))
            .show(ui, |ui| {
                for (operator, insertion, detail) in [
                    ("&&", "&&", "And"),
                    ("||", "||", "Or"),
                    ("!", "!", "Not"),
                    ("==", "==", "Equal"),
                    ("!=", "!=", "Not equal"),
                    (">", ">", "Greater than"),
                    ("<", "<", "Less than"),
                    (">=", ">=", "Greater or equal"),
                    ("<=", "<=", "Less or equal"),
                    ("+", "+", "Add"),
                    ("-", "-", "Subtract"),
                    ("*", "*", "Multiply"),
                    ("/", "/", "Divide"),
                    ("%", "%", "Remainder"),
                    ("( )", "()", "Grouping"),
                ] {
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized(
                                [44.0, CONTROL_HEIGHT],
                                egui::Button::new(
                                    egui::RichText::new(operator)
                                        .family(egui::FontFamily::Monospace),
                                ),
                            )
                            .on_hover_text(language.text("Insert operator"))
                            .clicked()
                        {
                            append_expression_token(draft, insertion);
                        }
                        ui.label(
                            egui::RichText::new(language.text(detail))
                                .small()
                                .color(muted()),
                        );
                    });
                }
            });
    });
}
