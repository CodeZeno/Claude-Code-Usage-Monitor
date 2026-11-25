//! Builds `docs/theme-reference.md` from the theme language tables in
//! `theme_engine::theme_language`, evaluating every example against the
//! sample usage. `theme_reference_is_current` fails when the checked-in file
//! differs; `--theme-reference` prints it.

use std::fmt::Write as _;

use super::{parse_usage, runtime_for_surface, DEFAULT_NOW_UNIX, SAMPLE_USAGE};
use crate::providers::PROVIDER_DESCRIPTORS;
use crate::theme_engine::{
    bundled_theme, describe_number, describe_result, format_template, is_usage_value,
    split_usage_owner, split_usage_window, value_group, DataContext, FieldKind,
    FixedRenderEnvironment, MouseActionProperty, ThemeField, DOCUMENT_FIELDS, EXPRESSION_FUNCTIONS,
    EXPRESSION_LITERALS, EXPRESSION_OPERATORS, MOUSE_ACTIONS, OBJECT_FIELDS, PAINT_FIELDS,
    PROVIDER_FIELDS, PROVIDER_WINDOWS, STACKED_ROWS_THEME_ID, TEMPLATE_FORMATS, THEME_REFERENCE,
    USAGE_OWNERS, VALUE_GROUPS,
};

/// The values every example is evaluated with: the sample usage, drawn on
/// Stacked Rows' widget surface in dark mode at `DEFAULT_NOW_UNIX`.
pub fn sample_context() -> DataContext {
    let usage = parse_usage(SAMPLE_USAGE).expect("the sample usage fixture parses");
    let theme = bundled_theme(STACKED_ROWS_THEME_ID)
        .expect("Stacked Rows is built in")
        .expect("Stacked Rows parses");
    let environment = FixedRenderEnvironment {
        now_unix: DEFAULT_NOW_UNIX,
        utc_offset_minutes: 0,
        dark: true,
        asset_root: None,
    };
    crate::theme_engine::with_fixed_render_environment(environment, || {
        let runtime = runtime_for_surface(&theme, 0, &usage);
        DataContext::from_usage_with_runtime(Some(&usage), &theme.canvas, runtime)
    })
}

pub fn theme_reference() -> String {
    let context = sample_context();
    let environment = FixedRenderEnvironment {
        now_unix: DEFAULT_NOW_UNIX,
        utc_offset_minutes: 0,
        dark: true,
        asset_root: None,
    };
    crate::theme_engine::with_fixed_render_environment(environment, || write_reference(&context))
}

fn write_reference(context: &DataContext) -> String {
    let mut out = String::new();
    let o = &mut out;
    line(o, "# Theme reference");
    line(o, "");
    line(
        o,
        "<!-- Generated from src/theme_engine/theme_language.rs by src/theme_render/reference.rs. \
         Do not edit; run `CCUM_UPDATE_SNAPSHOTS=1 cargo test --locked theme_reference` instead. -->",
    );
    line(o, "");
    line(
        o,
        "Theme files are JSON (see the [built-in themes](../src/themes/)). Most fields are \
         **expressions**, not fixed numbers: `\"width\": \"canvas.width - 8\"` sizes a layer \
         from its surface, and `\"render\": \"claude.session.percentage > 80\"` shows it only \
         near the limit. Text layers use **templates** with `{expression}` tokens, and mouse \
         events run **actions**. The [Fields](#fields) section says what each field takes.",
    );
    line(o, "");
    line(
        o,
        "Results below are computed with `tests/fixtures/theme-usage.json` on Stacked Rows' \
         `main` surface, in dark mode, at 2027-01-15 08:00 UTC: the same inputs \
         `--render-theme` uses. Check a theme with `claude-code-usage-monitor --render-theme \
         <theme.json> --out <folder>`; it prints unknown fields and failing expressions.",
    );
    line(o, "");

    line(o, "## Expressions");
    line(o, "");
    line(
        o,
        "Expressions combine values, numbers and text with operators and functions. A \
         condition is true when it is not 0.",
    );
    line(o, "");
    line(o, "| Literal | Meaning | Example | Result |");
    line(o, "| --- | --- | --- | --- |");
    for literal in EXPRESSION_LITERALS {
        let result = example_result(literal.example, context);
        line(
            o,
            &format!(
                "| {} | {} | `{}` | {} |",
                literal.symbol, literal.description, literal.example, result
            ),
        );
    }
    line(o, "");
    line(o, "### Operators");
    line(o, "");
    line(o, "| Operator | Meaning | Example | Result |");
    line(o, "| --- | --- | --- | --- |");
    for operator in EXPRESSION_OPERATORS {
        line(
            o,
            &format!(
                "| `{}` | {} | `{}` | {} |",
                cell(operator.symbol),
                operator.description,
                cell(operator.example),
                example_result(operator.example, context)
            ),
        );
    }
    line(o, "");
    line(o, "### Functions");
    line(o, "");
    line(
        o,
        "`get(owner, field)` reads `owner.field`; `get(self, width)` reads the current layer.",
    );
    line(o, "");
    line(o, "| Function | Meaning | Example | Result |");
    line(o, "| --- | --- | --- | --- |");
    for function in EXPRESSION_FUNCTIONS {
        line(
            o,
            &format!(
                "| `{}` | {} | `{}` | {} |",
                function.signature,
                function.description,
                cell(function.example),
                example_result(function.example, context)
            ),
        );
    }
    line(o, "");

    line(o, "## Templates");
    line(o, "");
    line(
        o,
        "A text layer's `template` is literal text with `{expression}` or \
         `{expression:format}` tokens, such as `\"{claude.session.percentage:0}% used\"`. \
         `{{` writes a literal `{`. A token that cannot be evaluated shows `--`.",
    );
    line(o, "");
    line(o, "## Formats");
    line(o, "");
    line(o, "| Format | Meaning | Example | Result |");
    line(o, "| --- | --- | --- | --- |");
    for format in TEMPLATE_FORMATS {
        let code = std::iter::once(format.code)
            .chain(format.aliases.iter().copied())
            .map(|code| format!("`{code}`"))
            .collect::<Vec<_>>()
            .join(", ");
        line(
            o,
            &format!(
                "| {code} | {} | `{}` | {} |",
                format.description,
                cell(format.example),
                text_result(&format_template(format.example, context))
            ),
        );
    }
    line(o, "");

    let properties = MouseActionProperty::ALL
        .iter()
        .map(|property| format!("`{}`", property.name()))
        .collect::<Vec<_>>()
        .join(", ");
    line(o, "## Mouse actions");
    line(o, "");
    line(
        o,
        &format!(
            "`mouse_events` fields hold scripts of one or more actions separated by `;` or new lines. A layer is `self` or its quoted id; layer properties are {properties}. `set`, `increase` and `decrease` take an expression."
        ),
    );
    line(o, "");
    line(o, "| Action | Meaning | Example |");
    line(o, "| --- | --- | --- |");
    for action in MOUSE_ACTIONS {
        line(
            o,
            &format!(
                "| `{}` | {} | `{}` |",
                action.signature, action.label, action.example
            ),
        );
    }
    line(o, "");

    line(o, "## Fields");
    line(o, "");
    line(
        o,
        "`placement` and `tooltip` apply to surfaces; `parent`, `anchor`, `x` and `y` to \
         layers. Other object fields apply to both. `background(gradient).angle` is the \
         `angle` key of a `background` whose `type` is `gradient`. Fields not listed here \
         are ignored, so `--render-theme` reports them.",
    );
    line(o, "");
    field_table(o, "Theme", DOCUMENT_FIELDS);
    field_table(o, "Surfaces and layers", OBJECT_FIELDS);
    field_table(o, "Paints", PAINT_FIELDS);

    line(o, "## Values");
    line(o, "");
    line(
        o,
        "Names an expression can read. Text values work in templates and comparisons.",
    );
    line(o, "");
    line(o, "| Name | Meaning | Sample values |");
    line(o, "| --- | --- | --- |");
    let names: Vec<&str> = context
        .names()
        .into_iter()
        .filter(|name| !is_usage_value(name))
        .collect();
    for group in VALUE_GROUPS {
        let samples = names
            .iter()
            .filter(|name| value_group(name).is_some_and(|found| found.pattern == group.pattern))
            .map(|name| format!("`{name}` = {}", value_text(name, context)))
            .collect::<Vec<_>>()
            .join("<br>");
        let samples = if samples.is_empty() {
            "(inside layers)".to_string()
        } else {
            samples
        };
        let pattern = if group.pattern.ends_with('.') {
            format!("`{}*`", group.pattern)
        } else {
            format!("`{}`", group.pattern)
        };
        line(
            o,
            &format!("| {pattern} | {} | {samples} |", group.description),
        );
    }
    line(o, "");

    line(o, "### Usage values");
    line(o, "");
    line(
        o,
        "Usage values are `<owner>.<window>.<field>`, such as `claude.session.percentage`, or \
         `<owner>.<field>` for the provider itself, such as `claude.available`. Missing \
         values make an expression fail, so check `available` before using a window a \
         provider may not report.",
    );
    line(o, "");
    line(o, "| Owner | Meaning |");
    line(o, "| --- | --- |");
    for descriptor in PROVIDER_DESCRIPTORS {
        line(
            o,
            &format!("| `{}` | {} |", descriptor.key, descriptor.display_name),
        );
    }
    for (owner, description) in USAGE_OWNERS {
        line(o, &format!("| `{owner}` | {description} |"));
    }
    line(o, "");
    let usage_names: Vec<&str> = context
        .names()
        .into_iter()
        .filter(|name| is_usage_value(name) && !name.starts_with("accounts."))
        .collect();
    let mut all_owners: Vec<&str> = usage_names
        .iter()
        .filter_map(|name| split_usage_owner(name))
        .map(|(owner, _)| owner)
        .collect();
    all_owners.sort_unstable();
    all_owners.dedup();
    line(o, "| Window | Meaning | In the sample |");
    line(o, "| --- | --- | --- |");
    for window in PROVIDER_WINDOWS {
        let mut owners: Vec<&str> = usage_names
            .iter()
            .filter_map(|name| split_usage_owner(name))
            .filter(|(_, rest)| split_usage_window(rest).0 == window.pattern)
            .map(|(owner, _)| owner)
            .collect();
        owners.sort_unstable();
        owners.dedup();
        let name = if window.pattern.is_empty() {
            "(none)".to_string()
        } else {
            format!("`{}`", window.pattern)
        };
        line(
            o,
            &format!(
                "| {name} | {} | {} |",
                window.description,
                if owners.len() == all_owners.len() {
                    "every owner".to_string()
                } else {
                    owners
                        .iter()
                        .map(|owner| format!("`{owner}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
        );
    }
    line(o, "");
    line(o, "| Field | Meaning | Example | Sample value |");
    line(o, "| --- | --- | --- | --- |");
    for field in PROVIDER_FIELDS {
        let example = [
            "claude.session.",
            "claude.",
            "claude.account.",
            "claude.credits.",
            "claude.limits.",
        ]
        .iter()
        .map(|prefix| format!("{prefix}{}", field.pattern))
        .chain(
            usage_names
                .iter()
                .filter(|name| {
                    split_usage_owner(name)
                        .is_some_and(|(_, rest)| split_usage_window(rest).1 == field.pattern)
                })
                .map(|name| (*name).to_string()),
        )
        .find(|name| context.get(name).is_some() || context.get_string(name).is_some());
        let (example, value) = example.map_or_else(
            || {
                (
                    format!("`claude.{}`", field.pattern),
                    "(not in the sample)".into(),
                )
            },
            |name| {
                let value = value_text(&name, context);
                (format!("`{name}`"), value)
            },
        );
        line(
            o,
            &format!(
                "| `{}` | {} | {example} | {value} |",
                field.pattern, field.description
            ),
        );
    }
    out
}

fn field_table(o: &mut String, title: &str, fields: &[ThemeField]) {
    line(o, &format!("### {title}"));
    line(o, "");
    line(o, "| Field | Takes | Meaning |");
    line(o, "| --- | --- | --- |");
    for field in fields {
        let kind = match field.kind {
            FieldKind::Choice(values) => values
                .iter()
                .map(|value| format!("`{value}`"))
                .collect::<Vec<_>>()
                .join(", "),
            kind => kind.label().to_string(),
        };
        line(
            o,
            &format!("| `{}` | {kind} | {} |", field.path, field.description),
        );
    }
    line(o, "");
}

fn example_result(expression: &str, context: &DataContext) -> String {
    match describe_result(expression, context) {
        Ok(result) => format!("`{}`", cell(&result)),
        Err(error) => panic!("{THEME_REFERENCE} example `{expression}` fails: {error}"),
    }
}

fn text_result(text: &str) -> String {
    assert!(
        !text.contains("--"),
        "{THEME_REFERENCE} format example renders as {text}"
    );
    format!("`{}`", cell(text))
}

fn value_text(name: &str, context: &DataContext) -> String {
    // The version changes with every release; the reference must not.
    if name.starts_with("app.version") {
        return if name == "app.version" {
            "`\"X.Y.Z\"`".into()
        } else {
            "`…`".into()
        };
    }
    if let Some(text) = context.get_string(name) {
        return format!("`\"{}\"`", cell(text));
    }
    context
        .get(name)
        .map(|value| format!("`{}`", describe_number(value)))
        .unwrap_or_default()
}

/// Escapes a table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

fn line(out: &mut String, text: &str) {
    let _ = writeln!(out, "{text}");
}
