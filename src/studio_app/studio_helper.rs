//! Catalogues and wiring for the unified helper.
//!
//! Expressions, text templates, mouse actions, and context-menu actions all
//! open the same helper. This module turns the data each one supports into
//! browsable entries, draws the entry-specific details, and applies the result
//! to the field that opened the helper.

use super::*;
use crate::ui::components::helper::{
    option_row, show_helper, Caret, HelperAction, HelperEntry, HelperInsertion, HelperScope,
    HelperState, HelperStatus, HelperView, InsertMode,
};

const USAGE: &str = "Usage";
const RESETS: &str = "Resets";
const DATE_AND_TIME: &str = "Date and time";
const LAYOUT: &str = "Layout";
const GENERAL: &str = "General";
const FUNCTIONS: &str = "Functions";
const OPERATORS: &str = "Operators";
const LABELS: &str = "Labels";
const DASHBOARD: &str = "Dashboard";
const LINKS_AND_MENUS: &str = "Links and menus";
const LAYERS: &str = "Layers";
const APP: &str = "App";
const SETTINGS: &str = "Settings";

/// Expressions and text templates read the same values, so both helpers
/// offer the same categories.
const VALUE_CATEGORIES: &[&str] = &[
    USAGE,
    RESETS,
    DATE_AND_TIME,
    LAYOUT,
    GENERAL,
    LABELS,
    FUNCTIONS,
    OPERATORS,
];

/// Actions come first, then the values their `set`, `increase` and `decrease`
/// values can use.
const MOUSE_ACTION_CATEGORIES: &[&str] = &[
    DASHBOARD,
    LINKS_AND_MENUS,
    LAYERS,
    USAGE,
    RESETS,
    DATE_AND_TIME,
    LAYOUT,
    GENERAL,
    LABELS,
    FUNCTIONS,
    OPERATORS,
];

const MENU_ACTION_CATEGORIES: &[&str] = &[
    APP,
    SETTINGS,
    LAYERS,
    LINKS_AND_MENUS,
    USAGE,
    RESETS,
    DATE_AND_TIME,
    LAYOUT,
    GENERAL,
    LABELS,
    FUNCTIONS,
    OPERATORS,
];

/// `(name, signature, insertion, description)`
const EXPRESSION_FUNCTIONS: &[(&str, &str, &str, &str)] = &[
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
];

/// `(operator, description)`
const EXPRESSION_OPERATORS: &[(&str, &str)] = &[
    ("&&", "And"),
    ("||", "Or"),
    ("!", "Not"),
    ("==", "Equal"),
    ("!=", "Not equal"),
    (">", "Greater than"),
    ("<", "Less than"),
    (">=", "Greater or equal"),
    ("<=", "Less or equal"),
    ("+", "Add"),
    ("-", "Subtract"),
    ("*", "Multiply"),
    ("/", "Divide"),
    ("%", "Remainder"),
    ("()", "Grouping"),
];

/// An action the helper can build: `(id, category, label, code, token)`.
type ActionSpec = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

const MOUSE_ACTIONS: &[ActionSpec] = &[
    (
        "show_dashboard",
        DASHBOARD,
        "Show dashboard",
        "show_dashboard()",
        "show_dashboard",
    ),
    (
        "toggle_dashboard",
        DASHBOARD,
        "Toggle dashboard",
        "toggle_dashboard()",
        "toggle_dashboard",
    ),
    (
        "open_url",
        LINKS_AND_MENUS,
        "Open URL",
        "open_url(\"https://…\")",
        "open_url",
    ),
    (
        "show_context_menu",
        LINKS_AND_MENUS,
        "Show context menu",
        "show_context_menu(\"menu\")",
        "show_context_menu",
    ),
    (
        "set",
        LAYERS,
        "Set property",
        "set(layer, property, value)",
        "set",
    ),
    (
        "toggle",
        LAYERS,
        "Toggle Render",
        "toggle(layer, render)",
        "toggle",
    ),
    (
        "reset",
        LAYERS,
        "Reset property",
        "reset(layer, property)",
        "reset",
    ),
    (
        "increase",
        LAYERS,
        "Increase value",
        "increase(layer, property, amount)",
        "increase",
    ),
    (
        "decrease",
        LAYERS,
        "Decrease value",
        "decrease(layer, property, amount)",
        "decrease",
    ),
];

const MENU_ACTIONS: &[ActionSpec] = &[
    (
        "open_dashboard",
        APP,
        "Open Dashboard",
        "open_dashboard()",
        "open_dashboard",
    ),
    ("refresh", APP, "Refresh", "refresh()", "refresh"),
    (
        "toggle_widget",
        APP,
        "Show widget",
        "toggle_widget()",
        "toggle_widget",
    ),
    (
        "toggle_startup",
        APP,
        "Toggle Start with Windows",
        "toggle_startup()",
        "toggle_startup",
    ),
    (
        "check_for_updates",
        APP,
        "Check for updates",
        "check_for_updates()",
        "check_for_updates",
    ),
    ("exit", APP, "Exit", "exit()", "exit"),
    (
        "set_update_frequency",
        SETTINGS,
        "Set update frequency",
        "set_update_frequency(seconds)",
        "set_update_frequency",
    ),
    (
        "toggle_provider",
        SETTINGS,
        "Toggle provider",
        "toggle_provider(provider)",
        "toggle_provider",
    ),
    (
        "set_language",
        SETTINGS,
        "Set language",
        "set_language(\"code\")",
        "set_language",
    ),
    (
        "toggle_layer_render",
        LAYERS,
        "Toggle layer Render",
        "toggle_layer_render(\"layer\")",
        "toggle_layer_render",
    ),
    (
        "layer:set",
        LAYERS,
        "Set layer property",
        "layer_actions(\"set(…)\")",
        "set",
    ),
    (
        "layer:reset",
        LAYERS,
        "Reset layer property",
        "layer_actions(\"reset(…)\")",
        "reset",
    ),
    (
        "layer:increase",
        LAYERS,
        "Increase layer value",
        "layer_actions(\"increase(…)\")",
        "increase",
    ),
    (
        "layer:decrease",
        LAYERS,
        "Decrease layer value",
        "layer_actions(\"decrease(…)\")",
        "decrease",
    ),
    (
        "open_url",
        LINKS_AND_MENUS,
        "Open URL",
        "open_url(\"https://…\")",
        "open_url",
    ),
];

const UPDATE_FREQUENCIES: [(u32, &str); 4] = [
    (POLL_1_MIN_SECONDS, "Every minute"),
    (POLL_5_MIN_SECONDS, "Every 5 minutes"),
    (POLL_15_MIN_SECONDS, "Every 15 minutes"),
    (POLL_1_HOUR_SECONDS, "Every hour"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ExpressionHelperTarget {
    Theme(Selection),
    ContextMenu(Vec<usize>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TextTemplateHelperTarget {
    Theme(Selection),
    ContextMenu(Vec<usize>),
}

/// The field a helper session edits.
pub(super) enum HelperTarget {
    Expression {
        target: ExpressionHelperTarget,
        field: ExpressionField,
    },
    Text(TextTemplateHelperTarget),
    MouseAction {
        selection: Selection,
        field: MouseEventField,
    },
    MenuAction {
        path: Vec<usize>,
    },
}

impl HelperTarget {
    /// Whether the helper edits the context menu rather than the theme.
    pub(super) fn is_context_menu(&self) -> bool {
        matches!(
            self,
            Self::Expression {
                target: ExpressionHelperTarget::ContextMenu(_),
                ..
            } | Self::Text(TextTemplateHelperTarget::ContextMenu(_))
                | Self::MenuAction { .. }
        )
    }
}

pub(super) struct HelperSession {
    pub(super) target: HelperTarget,
    pub(super) editor: HelperState,
    forms: HelperForms,
    catalogue: Option<HelperCatalogue>,
}

struct HelperCatalogue {
    context: DataContext,
    language: LanguageId,
    syntax: ValueSyntax,
    entries: Vec<HelperEntry>,
    scopes: Vec<HelperScope>,
}

/// Values chosen in the details pane, kept while the user browses entries.
pub(super) struct HelperForms {
    /// Format chosen for a text value, keyed by the value's expression.
    format: Option<(String, TextTemplateFormat)>,
    /// Layer id, or `self` for the layer that owns the action.
    layer: String,
    property: MouseActionProperty,
    value: String,
    url: String,
    context_menu_reference: String,
    context_menus: Option<Vec<context_menu::ContextMenuDescriptor>>,
    frequency: u32,
    provider: ContextMenuProvider,
    language_code: String,
}

impl Default for HelperForms {
    fn default() -> Self {
        Self {
            format: None,
            layer: "self".into(),
            property: MouseActionProperty::Render,
            value: "false".into(),
            url: "https://".into(),
            context_menu_reference: context_menu::CLASSIC_CONTEXT_MENU_ID.into(),
            context_menus: None,
            frequency: POLL_15_MIN_SECONDS,
            provider: ContextMenuProvider::Claude,
            language_code: "system".into(),
        }
    }
}

impl HelperSession {
    fn new(target: HelperTarget, draft: String) -> Self {
        Self {
            target,
            editor: HelperState::new(draft),
            forms: HelperForms::default(),
            catalogue: None,
        }
    }

    fn refresh_catalogue(
        &mut self,
        context: &DataContext,
        language: LanguageId,
        syntax: ValueSyntax,
    ) {
        if let Some(catalogue) = self.catalogue.as_mut().filter(|catalogue| {
            catalogue.language == language
                && catalogue.syntax == syntax
                && catalogue.context.same_catalogue_data(context)
        }) {
            // Keep milliseconds and the local/UTC clock live without rebuilding
            // every provider/account entry and formatting every template.
            for entry in &mut catalogue.entries {
                if entry.category == DATE_AND_TIME
                    && catalogue.context.get(&entry.id).map(f64::to_bits)
                        != context.get(&entry.id).map(f64::to_bits)
                {
                    entry.value = entry_value(context, syntax, &entry.id);
                    if let Some(value) = context.get(&entry.id) {
                        catalogue.context.insert(&entry.id, value);
                    }
                }
            }
            return;
        }
        let actions = match self.target {
            HelperTarget::MouseAction { .. } => MOUSE_ACTIONS,
            HelperTarget::MenuAction { .. } => MENU_ACTIONS,
            _ => &[],
        };
        let mut entries = action_entries(actions, language);
        entries.extend(value_entries(context, language, syntax));
        self.catalogue = Some(HelperCatalogue {
            context: context.clone(),
            language,
            syntax,
            entries,
            scopes: helper_scopes(language),
        });
    }

    pub(super) fn expression(selection: Selection, field: ExpressionField, draft: String) -> Self {
        Self::new(
            HelperTarget::Expression {
                target: ExpressionHelperTarget::Theme(selection),
                field,
            },
            draft,
        )
    }

    pub(super) fn context_menu_expression(path: Vec<usize>, draft: String) -> Self {
        Self::new(
            HelperTarget::Expression {
                target: ExpressionHelperTarget::ContextMenu(path),
                field: ExpressionField::Render,
            },
            draft,
        )
    }

    pub(super) fn text(target: TextTemplateHelperTarget, draft: String) -> Self {
        Self::new(HelperTarget::Text(target), draft)
    }

    pub(super) fn mouse_action(
        selection: Selection,
        field: MouseEventField,
        draft: String,
    ) -> Self {
        Self::new(HelperTarget::MouseAction { selection, field }, draft)
    }

    pub(super) fn menu_action(path: Vec<usize>, action: &ContextMenuAction) -> Self {
        let mut session = Self::new(
            HelperTarget::MenuAction { path },
            context_menu_action_script(action),
        );
        let forms = &mut session.forms;
        match action {
            ContextMenuAction::SetUpdateFrequency { seconds } => forms.frequency = *seconds,
            ContextMenuAction::ToggleProvider { provider } => forms.provider = *provider,
            ContextMenuAction::SetLanguage { language } => forms.language_code = language.clone(),
            ContextMenuAction::ToggleLayerRender { target } => forms.layer = target.clone(),
            ContextMenuAction::OpenUrl { url } => forms.url = url.clone(),
            _ => {}
        }
        session
    }
}

/// How the field being edited refers to a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ValueSyntax {
    /// Expressions use the value's name, such as `claude.session.percentage`.
    Expression,
    /// Text templates wrap an expression in a `{…}` placeholder with a format.
    Template,
    /// Actions take an expression as the value of `set`, `increase` and
    /// `decrease`.
    Action,
}

/// Fields shared by provider windows and reported quotas, as
/// `(suffix, label)`. The empty suffix is the window itself, which only a text
/// template can show, as a usage summary.
const WINDOW_FIELDS: &[(&str, &str)] = &[
    ("", "Summary"),
    (".label", "Label"),
    (".percentage", "Used"),
    (".remaining", "Remaining"),
    (".display", "Shown"),
    (".available", "Available"),
    (".is_active", "Active limit"),
    (".balance", "Balance"),
    (".total", "Total"),
    (".reset.seconds", "Seconds until reset"),
    (".reset.minutes", "Minutes until reset"),
    (".reset.hours", "Hours until reset"),
    (".reset.days", "Days until reset"),
    (".reset.unix", "Reset date and time"),
    (".key", "Key"),
    (".kind", "Kind"),
    (".model_id", "Model id"),
    (".scope", "Scope"),
];

/// Provider and account status fields, as `(suffix, label)`.
const OWNER_FIELDS: &[(&str, &str)] = &[
    (".available", "Provider available"),
    (".stale", "Showing earlier figures"),
    (".has_error", "Has an error"),
    (".selected", "Selected account"),
    (".name", "Account name"),
    (".account.name", "Account name"),
    (".limits.count", "Reported limit count"),
];

fn window_label(provider: &str, window: &str) -> &'static str {
    match (provider, window) {
        ("cursor", "session") => "Auto",
        ("cursor", "weekly") => "API",
        ("opencode", "weekly") => "Long window",
        (_, "session") => "Session",
        (_, "five_hour") => "5-hour",
        (_, "weekly") => "Weekly",
        (_, "monthly") => "Monthly",
        (_, "credits") => "Credits",
        _ => "Closest to limit",
    }
}

pub(super) fn helper_scopes(language: LanguageId) -> Vec<HelperScope> {
    std::iter::once(HelperScope {
        key: "active",
        label: language.text("Active provider").to_string(),
        mark: Some(LucideIcon::Zap.unicode()),
    })
    .chain(PROVIDER_DESCRIPTORS.iter().map(|descriptor| HelperScope {
        key: descriptor.key,
        label: language.text(descriptor.display_name).to_string(),
        mark: Some(provider_mark_glyph(descriptor.id)),
    }))
    .collect()
}

/// What kind of value `name` holds, which decides the formats a text template
/// can apply to it.
fn value_kind(name: &str, context: &DataContext) -> TextTemplateValueKind {
    if let Some(value) = text_template_value(name) {
        return value.kind;
    }
    if context.get(name).is_none() {
        return if context.get_string(name).is_some() {
            TextTemplateValueKind::Text
        } else {
            TextTemplateValueKind::UsageSummary
        };
    }
    if name == "time.now.unix" || name.ends_with(".reset.unix") {
        TextTemplateValueKind::Timestamp
    } else if name.ends_with(".reset.seconds") {
        TextTemplateValueKind::Duration
    } else if name.ends_with(".percentage") || name.ends_with(".remaining") {
        TextTemplateValueKind::Percentage
    } else if name.ends_with(".display") {
        TextTemplateValueKind::DisplayPercentage
    } else {
        TextTemplateValueKind::Number
    }
}

/// The format the helper suggests first. Timestamps default to a two-letter
/// weekday elsewhere, which says little about the value in a list.
fn preferred_text_format(kind: TextTemplateValueKind) -> TextTemplateFormat {
    match kind {
        TextTemplateValueKind::Timestamp => TextTemplateFormat::DateTimeShort,
        kind => default_text_template_format(kind),
    }
}

fn exists(context: &DataContext, name: &str) -> bool {
    context.get(name).is_some() || context.get_string(name).is_some()
}

fn entry_value(context: &DataContext, syntax: ValueSyntax, name: &str) -> Option<String> {
    match syntax {
        ValueSyntax::Expression | ValueSyntax::Action => context
            .get(name)
            .map(format_number_for_ui)
            .or_else(|| context.get_string(name).map(str::to_string)),
        ValueSyntax::Template => {
            let format = preferred_text_format(value_kind(name, context));
            Some(theme_engine::format_template(
                &text_template_token(name, format),
                context,
            ))
        }
    }
}

struct EntryList<'a> {
    context: &'a DataContext,
    language: LanguageId,
    syntax: ValueSyntax,
    entries: Vec<HelperEntry>,
}

impl EntryList<'_> {
    fn value(
        &mut self,
        category: &'static str,
        scope: Option<&'static str>,
        group: &str,
        label: &'static str,
        name: &str,
    ) {
        let label = self.language.text(label).to_string();
        self.labelled(category, scope, group, label, name);
    }

    /// Adds a value, showing it the way the field being edited would.
    fn labelled(
        &mut self,
        category: &'static str,
        scope: Option<&'static str>,
        group: &str,
        label: String,
        name: &str,
    ) {
        let value = entry_value(self.context, self.syntax, name);
        self.entries.push(HelperEntry {
            id: name.into(),
            category,
            scope,
            group: group.into(),
            label,
            code: name.into(),
            token: name.into(),
            value,
        });
    }

    /// Adds each field of a usage window or quota that the data provides.
    fn window(&mut self, scope: &'static str, group: &str, base: &str, summary: bool) {
        for (suffix, label) in WINDOW_FIELDS {
            let name = format!("{base}{suffix}");
            let listed = if suffix.is_empty() {
                summary && self.syntax == ValueSyntax::Template
            } else {
                exists(self.context, &name)
            };
            if listed {
                let category = if suffix.starts_with(".reset.") {
                    RESETS
                } else {
                    USAGE
                };
                self.value(category, Some(scope), group, label, &name);
            }
        }
    }
}

/// Everything expressions and text templates can read, grouped for browsing.
/// Both use the same values; only how an entry is inserted differs.
pub(super) fn value_entries(
    context: &DataContext,
    language: LanguageId,
    syntax: ValueSyntax,
) -> Vec<HelperEntry> {
    let mut list = EntryList {
        context,
        language,
        syntax,
        entries: Vec::new(),
    };

    // `(prefix, provider, title)` for the active provider, each provider, and
    // each named account.
    let mut owners: Vec<(String, &'static str, String)> = std::iter::once((
        "active".to_string(),
        "active",
        language.text("Active provider").to_string(),
    ))
    .chain(PROVIDER_DESCRIPTORS.iter().map(|descriptor| {
        (
            descriptor.key.to_string(),
            descriptor.key,
            language.text(descriptor.display_name).to_string(),
        )
    }))
    .collect();
    for prefix in context.account_prefixes() {
        let Some((provider, id)) = prefix
            .strip_prefix("accounts.")
            .and_then(|rest| rest.split_once('.'))
        else {
            continue;
        };
        let Some(descriptor) = PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.key == provider)
        else {
            continue;
        };
        let name = context
            .get_string(&format!("{prefix}.name"))
            .filter(|name| !name.is_empty())
            .unwrap_or(id);
        owners.push((
            prefix.to_string(),
            descriptor.key,
            format!("{} · {name}", language.text(descriptor.display_name)),
        ));
    }

    let limit_bases: Vec<&str> = context
        .limit_variables()
        .into_iter()
        .filter_map(|name| name.strip_suffix(".available"))
        .filter(|base| is_limit_base(base))
        .collect();
    for (prefix, provider, title) in &owners {
        for (suffix, label) in OWNER_FIELDS {
            let name = format!("{prefix}{suffix}");
            if exists(context, &name) {
                list.value(USAGE, Some(provider), title, label, &name);
            }
        }
        let codex = *provider == "codex" || *provider == "active";
        for window in [
            "session",
            "five_hour",
            "weekly",
            "monthly",
            "credits",
            "headline",
        ] {
            // Only Codex reports a five-hour window separately from its session.
            if window == "five_hour" && !codex {
                continue;
            }
            let group = format!(
                "{title} · {}",
                language.text(window_label(provider, window))
            );
            list.window(
                provider,
                &group,
                &format!("{prefix}.{window}"),
                window != "headline",
            );
        }
        for base in limit_bases.iter().filter(|base| {
            base.strip_prefix(prefix.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
        }) {
            let name = limit_display_name(base, context);
            let group = if base.ends_with(".scoped") {
                format!("{title} · {}", language.text("Active scoped limit"))
            } else if base.contains(".model.") {
                format!("{title} · {name} ({})", language.text("model"))
            } else {
                format!("{title} · {name}")
            };
            list.window(provider, &group, base, true);
        }
    }

    let now = language.text("Now");
    list.value(
        DATE_AND_TIME,
        None,
        now,
        "Current date and time",
        "time.now.unix",
    );
    list.value(
        DATE_AND_TIME,
        None,
        now,
        "Unix time (milliseconds)",
        "time.now.milliseconds",
    );
    for (zone, group) in [("local", "Local time"), ("utc", "UTC time")] {
        for (part, label) in [
            ("year", "Year"),
            ("month", "Month"),
            ("day", "Day"),
            ("weekday", "Weekday"),
            ("hour", "Hour"),
            ("minute", "Minute"),
            ("second", "Second"),
        ] {
            list.value(
                DATE_AND_TIME,
                None,
                language.text(group),
                label,
                &format!("time.{zone}.{part}"),
            );
        }
    }

    let layout = language.text("Layout");
    for (name, label) in [
        ("canvas.width", "Canvas width"),
        ("canvas.height", "Canvas height"),
        ("parent.width", "Parent width"),
        ("parent.height", "Parent height"),
        ("host.width", "Host width"),
        ("host.height", "Host height"),
        ("this.gap", "Gap"),
    ] {
        if exists(context, name) {
            list.value(LAYOUT, None, layout, label, name);
        }
    }

    let constants = language.text("Constants");
    for (name, label) in [
        ("true", "True"),
        ("false", "False"),
        ("pi", "Pi"),
        ("e", "Euler's number"),
    ] {
        list.value(GENERAL, None, constants, label, name);
    }
    let providers = language.text("Providers");
    list.value(
        GENERAL,
        None,
        providers,
        "Enabled provider count",
        "providers.count",
    );
    for descriptor in PROVIDER_DESCRIPTORS {
        list.labelled(
            GENERAL,
            Some(descriptor.key),
            providers,
            format!(
                "{} {}",
                language.text(descriptor.display_name),
                language.text("enabled")
            ),
            &format!("providers.{}.enabled", descriptor.key),
        );
    }
    let status = language.text("Status");
    for (name, label) in [
        ("display.countdown", "Counting down"),
        ("system.dark", "Dark mode"),
        ("data.loading", "Loading"),
        ("data.poll_ok", "Last update succeeded"),
        ("data.has_error", "Last update failed"),
    ] {
        list.value(GENERAL, None, status, label, name);
    }
    let application = language.text("Application");
    for (name, label) in [
        ("app.version", "App version"),
        ("app.version.major", "App version major"),
        ("app.version.minor", "App version minor"),
        ("app.version.patch", "App version patch"),
    ] {
        list.value(GENERAL, None, application, label, name);
    }

    let labels = language.text("Labels");
    for (name, label) in [
        ("i18n.session_window", "Session window label"),
        ("i18n.weekly_window", "Weekly window label"),
        ("i18n.cursor_auto_window", "Cursor Auto window label"),
        ("i18n.cursor_api_window", "Cursor API window label"),
        ("i18n.now", "Now label"),
        ("i18n.day_suffix", "Day suffix"),
        ("i18n.hour_suffix", "Hour suffix"),
        ("i18n.minute_suffix", "Minute suffix"),
        ("i18n.second_suffix", "Second suffix"),
        ("i18n.locale", "Language code"),
    ] {
        list.value(LABELS, None, labels, label, name);
    }

    let mut entries = list.entries;
    let functions = language.text("Functions");
    entries.extend(
        EXPRESSION_FUNCTIONS
            .iter()
            .map(|(name, signature, _, detail)| HelperEntry {
                id: format!("fn:{name}"),
                category: FUNCTIONS,
                scope: None,
                group: functions.into(),
                label: language.text(detail).into(),
                code: (*signature).into(),
                token: (*name).into(),
                value: None,
            }),
    );
    let operators = language.text("Operators");
    entries.extend(
        EXPRESSION_OPERATORS
            .iter()
            .map(|(operator, detail)| HelperEntry {
                id: format!("op:{operator}"),
                category: OPERATORS,
                scope: None,
                group: operators.into(),
                label: language.text(detail).into(),
                code: (*operator).into(),
                token: String::new(),
                value: None,
            }),
    );
    entries
}

fn action_entries(actions: &[ActionSpec], language: LanguageId) -> Vec<HelperEntry> {
    actions
        .iter()
        .map(|(id, category, label, code, token)| HelperEntry {
            id: (*id).into(),
            category,
            scope: None,
            group: language.text(category).into(),
            label: language.text(label).into(),
            code: (*code).into(),
            token: (*token).into(),
            value: None,
        })
        .collect()
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(muted())).wrap());
}

fn form_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text).small().color(muted()));
}

fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn property_label(language: LanguageId, property: MouseActionProperty) -> &'static str {
    match property {
        MouseActionProperty::Render => language.text("Render"),
        MouseActionProperty::Visibility => language.text("Visibility"),
        MouseActionProperty::X => language.text("X"),
        MouseActionProperty::Y => language.text("Y"),
        MouseActionProperty::Width => language.text("Width"),
        MouseActionProperty::Height => language.text("Height"),
        MouseActionProperty::Rotation => language.text("Rotation"),
    }
}

fn category_note(category: &str) -> Option<&'static str> {
    match category {
        USAGE => Some("Usage percentages run from 0 to 100 and update with each refresh."),
        RESETS => Some("Time remaining until the usage window resets, updated live."),
        DATE_AND_TIME => Some("Read from the system clock each time the theme updates."),
        LAYOUT => Some("Sizes in pixels for the canvas, the parent layer, and the host window."),
        LABELS => Some("Text in the language chosen for the app."),
        _ => None,
    }
}

/// Explains a value and builds its insertion: the bare expression for an
/// expression or an action value, or a formatted `{…}` placeholder for a text
/// template. Inside an existing placeholder a template takes the bare
/// expression too.
#[allow(clippy::too_many_arguments)]
pub(super) fn value_details(
    ui: &mut egui::Ui,
    entry: &HelperEntry,
    syntax: ValueSyntax,
    caret: Caret<'_>,
    forms: &mut HelperForms,
    context: &DataContext,
    language: LanguageId,
) -> Result<HelperInsertion, String> {
    let action_value = match syntax {
        ValueSyntax::Action => theme_engine::action_value_at(caret.draft, caret.byte()),
        _ => None,
    };
    let bare = match syntax {
        ValueSyntax::Expression => true,
        ValueSyntax::Template => caret.in_placeholder(),
        ValueSyntax::Action => action_value.is_some(),
    };
    let misplaced = || -> String {
        language
            .text(match syntax {
                ValueSyntax::Action => {
                    "Place the cursor in the value of a set, increase or decrease action to use this."
                }
                _ => "Place the cursor inside a {…} value to use an operator.",
            })
            .into()
    };
    let caret_note = |ui: &mut egui::Ui| {
        match syntax {
        ValueSyntax::Template if bare => note(
            ui,
            language.text(
                "The cursor is inside a {…} value, so this adds to its expression and keeps its format.",
            ),
        ),
        ValueSyntax::Action if entry.category == LAYOUT => note(
            ui,
            language.text("In an action value, this and parent mean the layer being changed."),
        ),
        _ => {}
    }
    };
    let insertion = |expression: &str| {
        let text = match action_value {
            Some(theme_engine::ActionValue::Quoted) => {
                expression.replace('\\', "\\\\").replace('"', "\\\"")
            }
            _ => expression.to_string(),
        };
        HelperInsertion::new(text, InsertMode::Spaced)
    };
    let (expression, kind) = if let Some((_, _, insertion, _)) = entry
        .id
        .strip_prefix("fn:")
        .and_then(|name| EXPRESSION_FUNCTIONS.iter().find(|item| item.0 == name))
    {
        note(
            ui,
            language.text("Replace each argument with a value from the list or a number."),
        );
        ((*insertion).to_string(), TextTemplateValueKind::Number)
    } else if let Some(operator) = entry.id.strip_prefix("op:") {
        note(
            ui,
            language.text("Operators combine values into comparisons and calculations."),
        );
        caret_note(ui);
        return if bare {
            Ok(insertion(operator))
        } else {
            Err(misplaced())
        };
    } else {
        if let Some(text) = category_note(entry.category) {
            note(ui, language.text(text));
        }
        (entry.code.clone(), value_kind(&entry.code, context))
    };

    caret_note(ui);
    if syntax != ValueSyntax::Template {
        let hint = match kind {
            TextTemplateValueKind::Timestamp => Some("A Unix time in seconds."),
            TextTemplateValueKind::Duration => Some("A number of seconds."),
            TextTemplateValueKind::Text => {
                Some("Text. Compare it with == or != and a quoted value.")
            }
            _ => None,
        };
        if let Some(text) = hint {
            note(ui, language.text(text));
        }
    }
    if bare {
        return Ok(insertion(&expression));
    }
    if syntax == ValueSyntax::Action {
        return Err(misplaced());
    }

    let formats = text_template_formats(kind);
    let mut format = match &forms.format {
        Some((id, format)) if *id == entry.id && formats.contains(format) => *format,
        _ => preferred_text_format(kind),
    };
    form_label(ui, language.text("Format"));
    for candidate in formats {
        let sample =
            theme_engine::format_template(&text_template_token(&expression, *candidate), context);
        if option_row(
            ui,
            format == *candidate,
            text_template_format_label(language, *candidate),
            &sample,
        )
        .clicked()
        {
            format = *candidate;
        }
    }
    forms.format = Some((entry.id.clone(), format));
    Ok(HelperInsertion::new(
        text_template_token(&expression, format),
        InsertMode::Inline,
    ))
}

fn url_form(
    ui: &mut egui::Ui,
    forms: &mut HelperForms,
    language: LanguageId,
) -> Result<String, String> {
    note(ui, language.text("Opens a link in the default browser."));
    form_label(ui, language.text("URL"));
    ui.add(
        singleline_text_edit(&mut forms.url)
            .desired_width(ui.available_width())
            .hint_text("https://example.com/usage"),
    );
    if context_menu::supported_url(&forms.url) {
        Ok(forms.url.clone())
    } else {
        Err(language
            .text("Only http and https links are allowed.")
            .into())
    }
}

fn layer_label(
    layer: &str,
    targets: &[(String, String)],
    self_name: Option<&str>,
    language: LanguageId,
) -> String {
    if layer == "self" {
        return format!(
            "{} ({})",
            language.text("Self"),
            self_name.unwrap_or_default()
        );
    }
    targets.iter().find(|(id, _)| id == layer).map_or_else(
        || layer.to_string(),
        |(id, name)| format!("{name}  ·  {id}"),
    )
}

fn layer_picker(
    ui: &mut egui::Ui,
    forms: &mut HelperForms,
    targets: &[(String, String)],
    self_name: Option<&str>,
    language: LanguageId,
) {
    if self_name.is_none() && forms.layer == "self" {
        if let Some((id, _)) = targets.first() {
            forms.layer = id.clone();
        }
    }
    form_label(ui, language.text("Layer"));
    Dropdown::from_id_salt("helper-layer")
        .width(ui.available_width())
        .selected_text(layer_label(&forms.layer, targets, self_name, language))
        .show_ui(ui, |ui| {
            if let Some(name) = self_name {
                dropdown_selectable_value(
                    ui,
                    &mut forms.layer,
                    "self".to_string(),
                    format!("{} ({name})", language.text("Self")),
                );
            }
            for (id, name) in targets {
                dropdown_selectable_value(
                    ui,
                    &mut forms.layer,
                    id.clone(),
                    format!("{name}  ·  {id}"),
                );
            }
        });
}

/// Draws the form for a layer action verb and returns the action it builds.
fn layer_action_form(
    ui: &mut egui::Ui,
    verb: &str,
    forms: &mut HelperForms,
    targets: &[(String, String)],
    self_name: Option<&str>,
    language: LanguageId,
) -> Result<String, String> {
    note(
        ui,
        language.text(match verb {
            "set" => "Changes a layer property until it is reset. The value is an expression, so it keeps following the values it uses.",
            "toggle" => "Shows the layer when hidden and hides it when shown.",
            "reset" => "Reset removes the runtime override and restores the saved expression.",
            "increase" => "Adds an amount to a numeric property.",
            _ => "Subtracts an amount from a numeric property.",
        }),
    );
    layer_picker(ui, forms, targets, self_name, language);
    let numeric = matches!(verb, "increase" | "decrease");
    if verb == "toggle" {
        forms.property = MouseActionProperty::Render;
    } else {
        if numeric && forms.property == MouseActionProperty::Render {
            forms.property = MouseActionProperty::X;
            forms.value = "10".into();
        }
        form_label(ui, language.text("Property"));
        Dropdown::from_id_salt("helper-property")
            .width(ui.available_width())
            .selected_text(property_label(language, forms.property))
            .show_ui(ui, |ui| {
                for property in MouseActionProperty::ALL
                    .into_iter()
                    .filter(|property| !numeric || *property != MouseActionProperty::Render)
                {
                    dropdown_selectable_value(
                        ui,
                        &mut forms.property,
                        property,
                        property_label(language, property),
                    );
                }
            });
    }
    let needs_value = matches!(verb, "set" | "increase" | "decrease");
    if needs_value {
        form_label(
            ui,
            language.text(if numeric {
                "Amount"
            } else {
                "Value expression"
            }),
        );
        ui.add(
            singleline_text_edit(&mut forms.value)
                .desired_width(ui.available_width())
                .hint_text(language.text("e.g. false, 120, parent.width / 2")),
        );
        note(
            ui,
            language.text(
                "To use a live value, insert the action, click in its value and pick from the list.",
            ),
        );
    }
    let target = if forms.layer == "self" {
        "self".to_string()
    } else {
        quoted(&forms.layer)
    };
    let property = forms.property.name();
    let value = forms.value.trim();
    if needs_value && value.is_empty() {
        return Err(language.text("Enter a value").into());
    }
    Ok(match verb {
        "toggle" | "reset" => format!("{verb}({target}, {property})"),
        _ => format!("{verb}({target}, {property}, {value})"),
    })
}

fn mouse_action_details(
    ui: &mut egui::Ui,
    entry: &HelperEntry,
    forms: &mut HelperForms,
    targets: &[(String, String)],
    self_name: &str,
    language: LanguageId,
) -> Result<HelperInsertion, String> {
    let script = match entry.id.as_str() {
        "show_dashboard" => {
            note(ui, language.text("Opens the dashboard window."));
            "show_dashboard()".to_string()
        }
        "toggle_dashboard" => {
            note(
                ui,
                language.text("Opens the dashboard, or closes it when it is already open."),
            );
            "toggle_dashboard()".to_string()
        }
        "open_url" => format!("open_url({})", quoted(&url_form(ui, forms, language)?)),
        "show_context_menu" => {
            note(ui, language.text("Shows a context menu at the pointer."));
            form_label(ui, language.text("Context menu"));
            let menus = forms
                .context_menus
                .get_or_insert_with(|| context_menu::list_context_menus().unwrap_or_default());
            Dropdown::from_id_salt("helper-context-menu")
                .width(ui.available_width())
                .selected_text(
                    menus
                        .iter()
                        .find(|menu| menu.id == forms.context_menu_reference)
                        .map_or(forms.context_menu_reference.as_str(), |menu| {
                            menu.name.as_str()
                        }),
                )
                .show_ui(ui, |ui| {
                    for menu in menus.iter() {
                        dropdown_selectable_value(
                            ui,
                            &mut forms.context_menu_reference,
                            menu.id.clone(),
                            &menu.name,
                        );
                    }
                });
            format!(
                "show_context_menu({})",
                quoted(&forms.context_menu_reference)
            )
        }
        verb => layer_action_form(ui, verb, forms, targets, Some(self_name), language)?,
    };
    ui.add_space(8.0);
    note(
        ui,
        language.text("Actions run from top to bottom in one update."),
    );
    Ok(HelperInsertion::new(script, InsertMode::Line))
}

fn menu_action_details(
    ui: &mut egui::Ui,
    entry: &HelperEntry,
    forms: &mut HelperForms,
    targets: &[(String, String)],
    language: LanguageId,
) -> Result<HelperInsertion, String> {
    let action = match entry.id.as_str() {
        "open_dashboard" => ContextMenuAction::OpenDashboard,
        "refresh" => ContextMenuAction::Refresh,
        "toggle_widget" => ContextMenuAction::ToggleWidget,
        "toggle_startup" => ContextMenuAction::ToggleStartup,
        "check_for_updates" => ContextMenuAction::CheckForUpdates,
        "exit" => ContextMenuAction::Exit,
        "set_update_frequency" => {
            form_label(ui, language.text("Update frequency"));
            Dropdown::from_id_salt("helper-frequency")
                .width(ui.available_width())
                .selected_text(interval_name(
                    language,
                    forms.frequency.saturating_mul(1_000),
                ))
                .show_ui(ui, |ui| {
                    for (seconds, label) in UPDATE_FREQUENCIES {
                        dropdown_selectable_value(
                            ui,
                            &mut forms.frequency,
                            seconds,
                            language.text(label),
                        );
                    }
                });
            ContextMenuAction::SetUpdateFrequency {
                seconds: forms.frequency,
            }
        }
        "toggle_provider" => {
            form_label(ui, language.text("Provider"));
            Dropdown::from_id_salt("helper-provider")
                .width(ui.available_width())
                .selected_text(language.text(forms.provider.descriptor().display_name))
                .show_ui(ui, |ui| {
                    for descriptor in PROVIDER_DESCRIPTORS {
                        dropdown_selectable_value(
                            ui,
                            &mut forms.provider,
                            descriptor.id,
                            language.text(descriptor.display_name),
                        );
                    }
                });
            ContextMenuAction::ToggleProvider {
                provider: forms.provider,
            }
        }
        "set_language" => {
            form_label(ui, language.text("Language"));
            Dropdown::from_id_salt("helper-language")
                .width(ui.available_width())
                .selected_text(language_name(language, &forms.language_code))
                .show_ui(ui, |ui| {
                    for (code, name) in languages(language) {
                        dropdown_selectable_value(
                            ui,
                            &mut forms.language_code,
                            code.to_string(),
                            name,
                        );
                    }
                });
            ContextMenuAction::SetLanguage {
                language: forms.language_code.clone(),
            }
        }
        "toggle_layer_render" => {
            note(
                ui,
                language.text("Shows the layer when hidden and hides it when shown."),
            );
            layer_picker(ui, forms, targets, None, language);
            ContextMenuAction::ToggleLayerRender {
                target: forms.layer.clone(),
            }
        }
        "open_url" => ContextMenuAction::OpenUrl {
            url: url_form(ui, forms, language)?,
        },
        id => {
            let verb = id.strip_prefix("layer:").unwrap_or(id);
            ContextMenuAction::LayerActions {
                actions: layer_action_form(ui, verb, forms, targets, None, language)?,
            }
        }
    };
    Ok(HelperInsertion::new(
        context_menu_action_script(&action),
        InsertMode::Replace,
    ))
}

impl StudioApp {
    pub(super) fn helper_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut session) = self.helper.take() else {
            return;
        };
        let action = match session.target {
            HelperTarget::Expression { .. } | HelperTarget::Text(_) => {
                self.value_helper_view(ui, &mut session)
            }
            HelperTarget::MouseAction { .. } => self.mouse_action_helper_view(ui, &mut session),
            HelperTarget::MenuAction { .. } => self.menu_action_helper_view(ui, &mut session),
        };
        match action {
            HelperAction::Continue => self.helper = Some(session),
            HelperAction::Close => {}
            HelperAction::Apply => self.apply_helper(session),
        }
    }

    /// Closes the open helper if it edits the theme (`false`) or the context
    /// menu (`true`), leaving a helper for the other document open.
    pub(super) fn close_helper(&mut self, context_menu: bool) {
        if self
            .helper
            .as_ref()
            .is_some_and(|helper| helper.target.is_context_menu() == context_menu)
        {
            self.helper = None;
        }
    }

    fn menu_data_context(&self) -> DataContext {
        DataContext::from_usage_with_runtime(
            self.usage.as_ref(),
            &Canvas::default(),
            self.selected_theme_runtime(),
        )
    }

    fn layer_targets(&self) -> Vec<(String, String)> {
        self.theme
            .surfaces
            .iter()
            .flat_map(|root| {
                std::iter::once((root.id.clone(), root.name.clone())).chain(
                    root.children
                        .iter()
                        .map(|object| (object.id.clone(), object.name.clone())),
                )
            })
            .collect()
    }

    /// The expression and text helpers: the same values, inserted as a bare
    /// expression or as a formatted template placeholder.
    fn value_helper_view(&self, ui: &mut egui::Ui, session: &mut HelperSession) -> HelperAction {
        let language = self.language();
        let (syntax, field, context) = match &session.target {
            HelperTarget::Expression { target, field } => (
                ValueSyntax::Expression,
                Some(*field),
                match target {
                    ExpressionHelperTarget::ContextMenu(_) => self.menu_data_context(),
                    ExpressionHelperTarget::Theme(selection) => self.expression_context(*selection),
                },
            ),
            HelperTarget::Text(TextTemplateHelperTarget::Theme(selection)) => (
                ValueSyntax::Template,
                None,
                self.expression_context(*selection),
            ),
            HelperTarget::Text(TextTemplateHelperTarget::ContextMenu(_)) => {
                (ValueSyntax::Template, None, self.menu_data_context())
            }
            _ => return HelperAction::Close,
        };
        session.refresh_catalogue(&context, language, syntax);
        let HelperSession {
            editor,
            forms,
            catalogue,
            ..
        } = session;
        let catalogue = catalogue.as_ref().expect("catalogue was refreshed");
        let preview = |draft: &str| theme_engine::format_template(draft, &context);
        let view = match syntax {
            ValueSyntax::Expression | ValueSyntax::Action => HelperView {
                kind_icon: LucideIcon::Braces,
                kind: language.text("Expression"),
                description: language.text(
                    "Build and validate an expression using the values supported by the theme engine.",
                ),
                hint: language.text("Enter an expression..."),
                code_editor: true,
                editor_height: 96.0,
                categories: VALUE_CATEGORIES,
                scopes: &catalogue.scopes,
                entries: &catalogue.entries,
            },
            ValueSyntax::Template => HelperView {
                kind_icon: LucideIcon::Type,
                kind: language.text("Text"),
                description: language.text(
                    "Type words directly and insert live values at the cursor. Anything inside {…} is an expression. Type {{ for a literal brace.",
                ),
                hint: language.text("Type text here, then insert values below..."),
                code_editor: false,
                editor_height: 64.0,
                categories: VALUE_CATEGORIES,
                scopes: &catalogue.scopes,
                entries: &catalogue.entries,
            },
        };
        show_helper(
            ui,
            editor,
            view,
            language,
            |draft| match (syntax, field) {
                (ValueSyntax::Expression, Some(field)) => {
                    match theme_engine::evaluate(draft, &context) {
                        Ok(value) if value.is_finite() => HelperStatus::Valid {
                            message: language.text("Valid expression").into(),
                            result: Some(format_expression_result(field, value, language)),
                        },
                        Ok(_) => HelperStatus::Invalid(
                            language.text("Expression result is not finite").into(),
                        ),
                        Err(error) => HelperStatus::Invalid(error),
                    }
                }
                _ => {
                    let errors = theme_engine::validate_template(draft, &context);
                    if errors.is_empty() {
                        HelperStatus::Valid {
                            message: language.text("Template is valid").into(),
                            result: None,
                        }
                    } else {
                        HelperStatus::Invalid(errors.join(" · "))
                    }
                }
            },
            (syntax == ValueSyntax::Template).then_some(&preview as &dyn Fn(&str) -> String),
            |ui, entry, caret| value_details(ui, entry, syntax, caret, forms, &context, language),
        )
    }

    fn mouse_action_helper_view(
        &self,
        ui: &mut egui::Ui,
        session: &mut HelperSession,
    ) -> HelperAction {
        let language = self.language();
        let HelperTarget::MouseAction { selection, .. } = session.target else {
            return HelperAction::Close;
        };
        let surface_index = match selection {
            Selection::Surface(surface) | Selection::Object(surface, _) => surface,
        };
        let Some(surface) = self.theme.surfaces.get(surface_index) else {
            return HelperAction::Close;
        };
        let (self_id, self_name) = match selection {
            Selection::Object(_, object) => surface.children.get(object),
            Selection::Surface(_) => None,
        }
        .map_or_else(
            || (surface.id.clone(), surface.name.clone()),
            |object| (object.id.clone(), object.name.clone()),
        );
        let targets = self
            .layer_targets()
            .into_iter()
            .filter(|(id, _)| !id.eq_ignore_ascii_case(&self_id))
            .collect::<Vec<_>>();
        let context = self.expression_context(selection);
        session.refresh_catalogue(&context, language, ValueSyntax::Action);
        let HelperSession {
            editor,
            forms,
            catalogue,
            ..
        } = session;
        let catalogue = catalogue.as_ref().expect("catalogue was refreshed");
        show_helper(
            ui,
            editor,
            HelperView {
                kind_icon: LucideIcon::MousePointerClick,
                kind: language.text("Mouse action"),
                description: language.text(
                    "Build safe mouse actions that affect layers at runtime. The value of set, increase and decrease can use any value or function.",
                ),
                hint: language.text("Enter actions..."),
                code_editor: true,
                editor_height: 96.0,
                categories: MOUSE_ACTION_CATEGORIES,
                scopes: &catalogue.scopes,
                entries: &catalogue.entries,
            },
            language,
            |draft| {
                let errors = theme_engine::validate_mouse_action_script(
                    draft,
                    &self.theme,
                    surface_index,
                    &self_id,
                    &context,
                );
                if !errors.is_empty() {
                    return HelperStatus::Invalid(errors.join("\n"));
                }
                match theme_engine::parse_mouse_actions(draft) {
                    Ok(actions) => HelperStatus::Valid {
                        message: language.text("Valid actions").into(),
                        result: Some(format!(
                            "{} {}",
                            actions.len(),
                            language.text(if actions.len() == 1 {
                                "action"
                            } else {
                                "actions"
                            })
                        )),
                    },
                    Err(error) => HelperStatus::Invalid(error),
                }
            },
            None,
            |ui, entry, caret| {
                if MOUSE_ACTIONS.iter().any(|action| action.0 == entry.id) {
                    mouse_action_details(ui, entry, forms, &targets, &self_name, language)
                } else {
                    let syntax = ValueSyntax::Action;
                    value_details(ui, entry, syntax, caret, forms, &context, language)
                }
            },
        )
    }

    fn menu_action_helper_view(
        &self,
        ui: &mut egui::Ui,
        session: &mut HelperSession,
    ) -> HelperAction {
        let language = self.language();
        let targets = self.layer_targets();
        let context = self.menu_data_context();
        session.refresh_catalogue(&context, language, ValueSyntax::Action);
        let HelperSession {
            editor,
            forms,
            catalogue,
            ..
        } = session;
        let catalogue = catalogue.as_ref().expect("catalogue was refreshed");
        show_helper(
            ui,
            editor,
            HelperView {
                kind_icon: LucideIcon::SquareMenu,
                kind: language.text("Menu action"),
                description: language.text(
                    "Choose one action for this context menu item. The value of a layer set, increase or decrease can use any value or function.",
                ),
                hint: language.text("Enter an action..."),
                code_editor: true,
                editor_height: 40.0,
                categories: MENU_ACTION_CATEGORIES,
                scopes: &catalogue.scopes,
                entries: &catalogue.entries,
            },
            language,
            |draft| match parse_context_menu_action_script(draft) {
                Ok(_) => HelperStatus::Valid {
                    message: language.text("Valid menu action").into(),
                    result: None,
                },
                Err(error) => HelperStatus::Invalid(error),
            },
            None,
            |ui, entry, caret| {
                if MENU_ACTIONS.iter().any(|action| action.0 == entry.id) {
                    menu_action_details(ui, entry, forms, &targets, language)
                } else {
                    let syntax = ValueSyntax::Action;
                    value_details(ui, entry, syntax, caret, forms, &context, language)
                }
            },
        )
    }

    fn apply_helper(&mut self, session: HelperSession) {
        let draft = session.editor.draft;
        match session.target {
            HelperTarget::Expression {
                target: ExpressionHelperTarget::ContextMenu(path),
                ..
            } => {
                if self.context_menu.is_builtin() {
                    return;
                }
                if let Some(item) = context_menu_item_mut(&mut self.context_menu.items, &path) {
                    item.render = Expression(draft);
                    self.context_menu_selection = Some(path);
                    self.context_menu_dirty = true;
                }
            }
            HelperTarget::Expression {
                target: ExpressionHelperTarget::Theme(selection),
                field,
            } => {
                let expression = Expression(draft);
                let applied = match selection {
                    Selection::Surface(surface_index) => {
                        self.theme.surfaces.get_mut(surface_index).is_some_and(
                            |surface| match field {
                                ExpressionField::Render => {
                                    surface.render = expression;
                                    true
                                }
                                ExpressionField::Visibility => {
                                    surface.visibility = expression;
                                    true
                                }
                                ExpressionField::ObjectWidth => {
                                    surface.width = expression;
                                    true
                                }
                                ExpressionField::ObjectHeight => {
                                    surface.height = expression;
                                    true
                                }
                                ExpressionField::PlacementOffsetX => {
                                    surface.placement.offset_x_expression = Some(expression);
                                    true
                                }
                                ExpressionField::PlacementOffsetY => {
                                    surface.placement.offset_y_expression = Some(expression);
                                    true
                                }
                                field => set_object_expression(surface, field, expression),
                            },
                        )
                    }
                    Selection::Object(surface_index, object_index) => self
                        .theme
                        .surfaces
                        .get_mut(surface_index)
                        .and_then(|surface| surface.children.get_mut(object_index))
                        .is_some_and(|object| set_object_expression(object, field, expression)),
                };
                if applied {
                    self.selection = selection;
                    self.changed();
                }
            }
            HelperTarget::Text(TextTemplateHelperTarget::Theme(selection)) => {
                let applied = match selection {
                    Selection::Surface(surface_index) => self
                        .theme
                        .surfaces
                        .get_mut(surface_index)
                        .is_some_and(|surface| set_text_template(&mut surface.content, draft)),
                    Selection::Object(surface_index, object_index) => self
                        .theme
                        .surfaces
                        .get_mut(surface_index)
                        .and_then(|surface| surface.children.get_mut(object_index))
                        .is_some_and(|object| set_text_template(&mut object.content, draft)),
                };
                if applied {
                    self.selection = selection;
                    self.changed();
                }
            }
            HelperTarget::Text(TextTemplateHelperTarget::ContextMenu(path)) => {
                if let Some(item) = context_menu_item_mut(&mut self.context_menu.items, &path) {
                    item.label = draft;
                    self.context_menu_selection = Some(path);
                    self.context_menu_dirty = true;
                }
            }
            HelperTarget::MouseAction { selection, field } => {
                let applied = match selection {
                    Selection::Surface(index) => self.theme.surfaces.get_mut(index),
                    Selection::Object(surface, object) => self
                        .theme
                        .surfaces
                        .get_mut(surface)
                        .and_then(|surface| surface.children.get_mut(object)),
                }
                .is_some_and(|object| {
                    let events = object.mouse_events.get_or_insert_with(MouseEvents::default);
                    *events.handler_mut(field.kind()) = draft;
                    true
                });
                if applied {
                    self.selection = selection;
                    self.changed();
                }
            }
            HelperTarget::MenuAction { path } => match parse_context_menu_action_script(&draft) {
                Ok(action) => {
                    if let Some(item) = context_menu_item_mut(&mut self.context_menu.items, &path) {
                        item.kind = ContextMenuItemKind::Action { action };
                        self.context_menu_selection = Some(path);
                        self.context_menu_dirty = true;
                    }
                }
                Err(error) => self.theme_error = Some(error),
            },
        }
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    fn assert_fresh(
        session: &HelperSession,
        context: &DataContext,
        language: LanguageId,
        syntax: ValueSyntax,
    ) {
        let cached = &session.catalogue.as_ref().unwrap().entries;
        let fresh = value_entries(context, language, syntax);
        assert_eq!(cached.len(), fresh.len());
        for (cached, fresh) in cached.iter().zip(&fresh) {
            assert_eq!(
                (
                    &cached.id,
                    &cached.label,
                    &cached.group,
                    &cached.code,
                    &cached.value,
                    cached.category,
                    cached.scope
                ),
                (
                    &fresh.id,
                    &fresh.label,
                    &fresh.group,
                    &fresh.code,
                    &fresh.value,
                    fresh.category,
                    fresh.scope
                ),
            );
        }
    }

    #[test]
    fn helper_catalogue_reuses_entries_and_keeps_clock_values_live() {
        for syntax in [
            ValueSyntax::Expression,
            ValueSyntax::Template,
            ValueSyntax::Action,
        ] {
            let language = LanguageId::English;
            let mut data = DataContext::from_usage(None, &Canvas::default());
            let mut session = HelperSession::context_menu_expression(vec![], String::new());
            session.refresh_catalogue(&data, language, syntax);
            let allocation = session.catalogue.as_ref().unwrap().entries.as_ptr();
            session.refresh_catalogue(&data, language, syntax);
            assert_eq!(
                session.catalogue.as_ref().unwrap().entries.as_ptr(),
                allocation
            );
            for (name, value) in [
                ("time.now.unix", 1_800_000_000.25),
                ("time.now.milliseconds", 1_800_000_000_250.0),
                ("time.local.second", 45.0),
                ("time.utc.minute", 20.0),
            ] {
                data.insert(name, value);
            }
            session.refresh_catalogue(&data, language, syntax);
            assert_eq!(
                session.catalogue.as_ref().unwrap().entries.as_ptr(),
                allocation
            );
            assert_fresh(&session, &data, language, syntax);
        }
    }

    #[test]
    fn helper_catalogue_invalidates_for_data_language_and_syntax() {
        let mut data = DataContext::from_usage(None, &Canvas::default());
        let original = data.clone();
        let mut session = HelperSession::context_menu_expression(vec![], String::new());
        let language = LanguageId::English;
        let syntax = ValueSyntax::Template;
        session.refresh_catalogue(&data, language, syntax);
        for (name, value) in [
            ("active.session.percentage", 37.0),
            ("active.session.reset.seconds", 120.0),
            ("canvas.width", 640.0),
            ("claude.limits.new_quota.available", 1.0),
            ("claude.limits.new_quota.percentage", 42.0),
        ] {
            let allocation = session.catalogue.as_ref().unwrap().entries.as_ptr();
            data.insert(name, value);
            session.refresh_catalogue(&data, language, syntax);
            assert_ne!(
                session.catalogue.as_ref().unwrap().entries.as_ptr(),
                allocation
            );
            assert_fresh(&session, &data, language, syntax);
        }
        data.insert_string("claude.limits.new_quota.label", "New quota");
        data.insert_string("accounts.claude.work.name", "Work account");
        session.refresh_catalogue(&data, language, syntax);
        assert_fresh(&session, &data, language, syntax);
        // Removed accounts/quotas must disappear as well.
        session.refresh_catalogue(&original, language, syntax);
        assert_fresh(&session, &original, language, syntax);
        session.refresh_catalogue(&original, LanguageId::from_code("de").unwrap(), syntax);
        assert_fresh(
            &session,
            &original,
            LanguageId::from_code("de").unwrap(),
            syntax,
        );
        session.refresh_catalogue(
            &original,
            LanguageId::from_code("de").unwrap(),
            ValueSyntax::Expression,
        );
        assert_fresh(
            &session,
            &original,
            LanguageId::from_code("de").unwrap(),
            ValueSyntax::Expression,
        );
    }
}
