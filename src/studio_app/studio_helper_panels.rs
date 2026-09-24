use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TextTemplateFormat {
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
pub(super) enum TextTemplateValueKind {
    Number,
    Percentage,
    DisplayPercentage,
    Duration,
    Timestamp,
    UsageSummary,
    Text,
}

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
        group: "General",
        label: "Counting down",
        expression: "display.countdown",
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
        label: "Session shown",
        expression: "active.session.display",
        kind: TextTemplateValueKind::DisplayPercentage,
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
        label: "Weekly shown",
        expression: "active.weekly.display",
        kind: TextTemplateValueKind::DisplayPercentage,
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
        label: "Session shown",
        expression: "claude.session.display",
        kind: TextTemplateValueKind::DisplayPercentage,
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
        label: "Weekly shown",
        expression: "claude.weekly.display",
        kind: TextTemplateValueKind::DisplayPercentage,
    },
    TextTemplateValue {
        group: "Claude Code",
        label: "Weekly reset",
        expression: "claude.weekly.reset.seconds",
        kind: TextTemplateValueKind::Duration,
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
        label: "Session shown",
        expression: "codex.session.display",
        kind: TextTemplateValueKind::DisplayPercentage,
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
        label: "Five-hour shown (exact)",
        expression: "codex.five_hour.display",
        kind: TextTemplateValueKind::DisplayPercentage,
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
        label: "Weekly shown",
        expression: "codex.weekly.display",
        kind: TextTemplateValueKind::DisplayPercentage,
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
        label: "Session shown",
        expression: "antigravity.session.display",
        kind: TextTemplateValueKind::DisplayPercentage,
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
        label: "Weekly shown",
        expression: "antigravity.weekly.display",
        kind: TextTemplateValueKind::DisplayPercentage,
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
        label: "Session shown",
        expression: "opencode.session.display",
        kind: TextTemplateValueKind::DisplayPercentage,
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
        label: "Long-window shown",
        expression: "opencode.weekly.display",
        kind: TextTemplateValueKind::DisplayPercentage,
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
        label: "Auto shown",
        expression: "cursor.session.display",
        kind: TextTemplateValueKind::DisplayPercentage,
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
        label: "API shown",
        expression: "cursor.weekly.display",
        kind: TextTemplateValueKind::DisplayPercentage,
    },
    TextTemplateValue {
        group: "Cursor",
        label: "API reset",
        expression: "cursor.weekly.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Grok",
        label: "Pool summary",
        expression: "grok.weekly",
        kind: TextTemplateValueKind::UsageSummary,
    },
    TextTemplateValue {
        group: "Grok",
        label: "Pool used",
        expression: "grok.weekly.percentage",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Grok",
        label: "Pool remaining",
        expression: "grok.weekly.remaining",
        kind: TextTemplateValueKind::Percentage,
    },
    TextTemplateValue {
        group: "Grok",
        label: "Pool shown",
        expression: "grok.weekly.display",
        kind: TextTemplateValueKind::DisplayPercentage,
    },
    TextTemplateValue {
        group: "Grok",
        label: "Pool reset",
        expression: "grok.weekly.reset.seconds",
        kind: TextTemplateValueKind::Duration,
    },
    TextTemplateValue {
        group: "Grok",
        label: "Pool period label",
        expression: "grok.weekly.label",
        kind: TextTemplateValueKind::Text,
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

pub(super) struct TextTemplateChoice {
    pub(super) group: &'static str,
    pub(super) label: String,
    pub(super) expression: String,
    pub(super) kind: TextTemplateValueKind,
}

pub(super) fn limit_provider(expression: &str) -> Option<(&'static str, &'static str)> {
    let path = expression.strip_prefix("accounts.").unwrap_or(expression);
    let (provider, _) = path.split_once('.')?;
    if provider == "active" {
        return Some(("Active provider", "active"));
    }
    PROVIDER_DESCRIPTORS
        .iter()
        .find(|item| item.key == provider)
        .map(|item| (item.display_name, item.key))
}

// Owned names let the picker follow the API's quota names without leaking
// strings or limiting selection to the built-in static catalogue.
pub(super) fn text_template_choice(
    expression: &str,
    context: &DataContext,
    language: LanguageId,
) -> Option<TextTemplateChoice> {
    if let Some(value) = text_template_value(expression) {
        return Some(TextTemplateChoice {
            group: value.group,
            label: language.text(value.label).into(),
            expression: expression.into(),
            kind: value.kind,
        });
    }
    let (group, _) = limit_provider(expression)?;
    let (base, label, kind) = [
        (".percentage", "Used", TextTemplateValueKind::Percentage),
        (".remaining", "Remaining", TextTemplateValueKind::Percentage),
        (
            ".display",
            "Shown",
            TextTemplateValueKind::DisplayPercentage,
        ),
        (".label", "Label", TextTemplateValueKind::Text),
        (".available", "Available", TextTemplateValueKind::Number),
        (".is_active", "Active limit", TextTemplateValueKind::Number),
        (
            ".reset.seconds",
            "Reset countdown",
            TextTemplateValueKind::Duration,
        ),
        (
            ".reset.unix",
            "Reset date and time",
            TextTemplateValueKind::Timestamp,
        ),
    ]
    .into_iter()
    .find_map(|(suffix, label, kind)| {
        expression
            .strip_suffix(suffix)
            .map(|base| (base, label, kind))
    })
    .unwrap_or((expression, "Summary", TextTemplateValueKind::UsageSummary));
    if !is_limit_base(base) || context.get(&format!("{base}.available")).is_none() {
        return None;
    }
    let mut name = limit_display_name(base, context);
    if let Some(account) = base.strip_prefix("accounts.").and_then(|path| {
        let (provider, rest) = path.split_once('.')?;
        let (id, _) = rest.split_once('.')?;
        Some((format!("accounts.{provider}.{id}.name"), id))
    }) {
        let account_name = context
            .get_string(&account.0)
            .filter(|name| !name.is_empty())
            .unwrap_or(account.1);
        name = format!("{account_name}: {name}");
    }
    Some(TextTemplateChoice {
        group,
        label: format!("{name} — {}", language.text(label)),
        expression: expression.into(),
        kind,
    })
}

/// Whether `base` names a reported quota, such as `claude.limits.<key>`.
pub(super) fn is_limit_base(base: &str) -> bool {
    base.contains(".limits.") || base.contains(".model.") || base.ends_with(".scoped")
}

/// The quota's reported label, or its key when the API sent none.
pub(super) fn limit_display_name(base: &str, context: &DataContext) -> String {
    let name = context
        .get_string(&format!("{base}.label"))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| base.rsplit('.').next().unwrap_or(base).replace('_', " "));
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

pub(super) fn text_template_choices(
    context: &DataContext,
    language: LanguageId,
) -> Vec<TextTemplateChoice> {
    let mut result = Vec::new();
    for (index, value) in TEXT_TEMPLATE_VALUES.iter().enumerate() {
        result.push(text_template_choice(value.expression, context, language).unwrap());
        if TEXT_TEMPLATE_VALUES
            .get(index + 1)
            .is_some_and(|next| next.group == value.group)
        {
            continue;
        }
        // List each reported quota once using its canonical binding. Model
        // aliases remain available in the expression editor.
        for base in context
            .limit_variables()
            .into_iter()
            .filter(|name| name.contains(".limits."))
            .filter_map(|name| name.strip_suffix(".available"))
            .filter(|base| limit_provider(base).is_some_and(|(group, _)| group == value.group))
        {
            for suffix in [
                "",
                ".label",
                ".percentage",
                ".remaining",
                ".display",
                ".reset.seconds",
                ".reset.unix",
                ".available",
                ".is_active",
            ] {
                if let Some(choice) =
                    text_template_choice(&format!("{base}{suffix}"), context, language)
                {
                    result.push(choice);
                }
            }
        }
    }
    result
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
        TextTemplateValueKind::DisplayPercentage => &[
            Format::Percentage,
            Format::WholeNumber,
            Format::OneDecimal,
            Format::TwoDecimals,
            Format::Automatic,
            Format::UsageLine,
            Format::UsageBadge,
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
    value: &TextTemplateChoice,
    format: TextTemplateFormat,
    context: &DataContext,
) -> String {
    theme_engine::format_template(&text_template_token(&value.expression, format), context)
}
