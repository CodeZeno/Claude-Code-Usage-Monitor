//! Reusable, versioned native context-menu documents edited by Theme Studio.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

use crate::providers::ProviderId;

pub const CONTEXT_MENU_SCHEMA_VERSION: u32 = 1;
pub const CLASSIC_CONTEXT_MENU_ID: &str = "classic-v1";
pub const DASHBOARD_V2_CONTEXT_MENU_ID: &str = "dashboard-v2";
pub const LEGACY_CLASSIC_CONTEXT_MENU_ID: &str = "classic-v1-4-9";
pub const LEGACY_DASHBOARD_V2_CONTEXT_MENU_ID: &str = "dashboard-and-exit";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuDocument {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub items: Vec<ContextMenuItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextMenuItem {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(flatten)]
    pub kind: ContextMenuItemKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextMenuItemKind {
    Action {
        action: ContextMenuAction,
    },
    /// A non-clickable informational row. Its label is evaluated as a theme
    /// text template whenever the menu opens.
    Text,
    Separator,
    Submenu {
        items: Vec<ContextMenuItem>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextMenuAction {
    OpenDashboard,
    Refresh,
    SetUpdateFrequency {
        #[serde(
            alias = "milliseconds",
            deserialize_with = "deserialize_update_frequency_seconds"
        )]
        seconds: u32,
    },
    ToggleProvider {
        provider: ContextMenuProvider,
    },
    ToggleStartup,
    ToggleWidget,
    /// Accepted only so menus saved by older versions can be loaded and
    /// cleaned up. New menus cannot create or execute this legacy action.
    #[serde(rename = "reset_position")]
    LegacyResetPosition,
    SetLanguage {
        language: String,
    },
    CheckForUpdates,
    ToggleLayerRender {
        target: String,
    },
    LayerActions {
        actions: String,
    },
    OpenUrl {
        url: String,
    },
    Exit,
}

fn deserialize_update_frequency_seconds<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = u32::deserialize(deserializer)?;
    Ok(match value {
        crate::app_settings::POLL_1_MIN
        | crate::app_settings::POLL_5_MIN
        | crate::app_settings::POLL_15_MIN
        | crate::app_settings::POLL_1_HOUR => value / 1_000,
        _ => value,
    })
}

pub type ContextMenuProvider = ProviderId;

#[derive(Clone, Debug)]
pub struct ContextMenuDescriptor {
    pub path: PathBuf,
    pub id: String,
    pub name: String,
    pub built_in: bool,
}

impl ContextMenuItem {
    pub fn action(id: &str, label: &str, action: ContextMenuAction) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ContextMenuItemKind::Action { action },
        }
    }

    pub fn separator(id: &str) -> Self {
        Self {
            id: id.into(),
            label: String::new(),
            kind: ContextMenuItemKind::Separator,
        }
    }

    pub fn text(id: &str, label: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ContextMenuItemKind::Text,
        }
    }

    pub fn submenu(id: &str, label: &str, items: Vec<Self>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ContextMenuItemKind::Submenu { items },
        }
    }
}

impl ContextMenuDocument {
    pub fn blank(name: &str) -> Self {
        let id = unique_document_id(name);
        Self {
            schema_version: CONTEXT_MENU_SCHEMA_VERSION,
            id,
            name: name.into(),
            items: vec![
                ContextMenuItem::action(
                    "open-dashboard",
                    "Open Dashboard",
                    ContextMenuAction::OpenDashboard,
                ),
                ContextMenuItem::separator("separator-1"),
                ContextMenuItem::action("exit", "Exit", ContextMenuAction::Exit),
            ],
        }
    }

    pub fn is_builtin(&self) -> bool {
        self.id.eq_ignore_ascii_case(CLASSIC_CONTEXT_MENU_ID)
            || self.id.eq_ignore_ascii_case(DASHBOARD_V2_CONTEXT_MENU_ID)
            || self.id.eq_ignore_ascii_case(LEGACY_CLASSIC_CONTEXT_MENU_ID)
            || self
                .id
                .eq_ignore_ascii_case(LEGACY_DASHBOARD_V2_CONTEXT_MENU_ID)
    }

    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.schema_version != CONTEXT_MENU_SCHEMA_VERSION {
            errors.push(format!(
                "Context menu schema {} is not supported; expected {}",
                self.schema_version, CONTEXT_MENU_SCHEMA_VERSION
            ));
        }
        if self.id.trim().is_empty() {
            errors.push("Context menu id cannot be empty".into());
        }
        if self.name.trim().is_empty() {
            errors.push("Context menu name cannot be empty".into());
        }
        if self.items.is_empty() {
            errors.push("A context menu needs at least one item".into());
        }
        let mut ids = std::collections::HashSet::new();
        validate_items(&self.items, 0, &mut ids, &mut errors);
        errors
    }
}

fn validate_items(
    items: &[ContextMenuItem],
    depth: usize,
    ids: &mut std::collections::HashSet<String>,
    errors: &mut Vec<String>,
) {
    if depth > 5 {
        errors.push("Context menus support at most six nested levels".into());
        return;
    }
    for item in items {
        if item.id.trim().is_empty() || !ids.insert(item.id.to_ascii_lowercase()) {
            errors.push(format!("Menu item '{}' needs a unique id", item.label));
        }
        match &item.kind {
            ContextMenuItemKind::Separator => {}
            ContextMenuItemKind::Text => validate_item_label(item, "Menu text", errors),
            ContextMenuItemKind::Submenu { items } => {
                validate_item_label(item, "Submenu", errors);
                if items.is_empty() {
                    errors.push(format!("Submenu '{}' needs at least one item", item.label));
                }
                validate_items(items, depth + 1, ids, errors);
            }
            ContextMenuItemKind::Action { action } => {
                validate_item_label(item, "Menu action", errors);
                match action {
                    ContextMenuAction::SetUpdateFrequency { seconds }
                        if !matches!(
                            *seconds,
                            crate::app_settings::POLL_1_MIN_SECONDS
                                | crate::app_settings::POLL_5_MIN_SECONDS
                                | crate::app_settings::POLL_15_MIN_SECONDS
                                | crate::app_settings::POLL_1_HOUR_SECONDS
                        ) =>
                    {
                        errors.push(format!(
                            "{}.seconds must be a supported update frequency",
                            item.id
                        ));
                    }
                    ContextMenuAction::SetLanguage { language } if language.trim().is_empty() => {
                        errors.push(format!("{}.language cannot be empty", item.id));
                    }
                    ContextMenuAction::ToggleLayerRender { target } if target.trim().is_empty() => {
                        errors.push(format!("{}.target cannot be empty", item.id));
                    }
                    ContextMenuAction::LayerActions { actions } => {
                        if actions.trim().is_empty() {
                            errors.push(format!("{}.actions cannot be empty", item.id));
                        } else if let Err(error) = crate::theme_engine::parse_mouse_actions(actions)
                        {
                            errors.push(format!("{}.actions: {error}", item.id));
                        }
                    }
                    ContextMenuAction::OpenUrl { url } if !supported_url(url) => errors.push(
                        format!("{}.url must start with http:// or https://", item.id),
                    ),
                    _ => {}
                }
            }
        }
    }
}

fn validate_item_label(item: &ContextMenuItem, kind: &str, errors: &mut Vec<String>) {
    if item.label.trim().is_empty() {
        errors.push(format!("{kind} '{}' needs a label", item.id));
        return;
    }
    let context =
        crate::theme_engine::DataContext::from_usage(None, &crate::theme_engine::Canvas::default());
    for error in crate::theme_engine::validate_template(&item.label, &context) {
        errors.push(format!("{}.label: {error}", item.id));
    }
}

pub fn supported_url(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    url.starts_with("https://") || url.starts_with("http://")
}

pub fn rendered_label(
    language: crate::localization::LanguageId,
    label: &str,
    context: &crate::theme_engine::DataContext,
) -> String {
    let translated = match label {
        "Settings" => language.text("Settings"),
        "Update frequency" => language.text("Update frequency"),
        "Start with Windows" => language.text("Start with Windows"),
        "Language" => language.text("Language"),
        "System default" => language.text("System default"),
        "Refresh" => language.text("Refresh"),
        "Exit" => language.text("Exit"),
        "Claude Code" => language.text("Claude Code"),
        "Codex" => language.text("Codex"),
        "Antigravity" => language.text("Antigravity"),
        "OpenCode" => language.text("OpenCode"),
        "Open Dashboard" => language.text("Open Dashboard"),
        "Every minute" => language.text("Every minute"),
        "Every 5 minutes" => language.text("Every 5 minutes"),
        "Every 15 minutes" => language.text("Every 15 minutes"),
        "Every hour" => language.text("Every hour"),
        "Providers" => language.text("Providers"),
        "Check for updates" => language.text("Check for updates"),
        "Show widget" => language.text("Show widget"),
        _ => label,
    };
    crate::theme_engine::format_template(translated, context)
}

pub fn classic_context_menu() -> ContextMenuDocument {
    use ContextMenuAction as Action;
    use ContextMenuProvider as Provider;

    let frequency = ContextMenuItem::submenu(
        "update-frequency",
        "Update frequency",
        vec![
            ContextMenuItem::action(
                "frequency-1-minute",
                "Every minute",
                Action::SetUpdateFrequency {
                    seconds: crate::app_settings::POLL_1_MIN_SECONDS,
                },
            ),
            ContextMenuItem::action(
                "frequency-5-minutes",
                "Every 5 minutes",
                Action::SetUpdateFrequency {
                    seconds: crate::app_settings::POLL_5_MIN_SECONDS,
                },
            ),
            ContextMenuItem::action(
                "frequency-15-minutes",
                "Every 15 minutes",
                Action::SetUpdateFrequency {
                    seconds: crate::app_settings::POLL_15_MIN_SECONDS,
                },
            ),
            ContextMenuItem::action(
                "frequency-1-hour",
                "Every hour",
                Action::SetUpdateFrequency {
                    seconds: crate::app_settings::POLL_1_HOUR_SECONDS,
                },
            ),
        ],
    );
    let providers = ContextMenuItem::submenu(
        "providers",
        "Providers",
        vec![
            ContextMenuItem::action(
                "provider-claude",
                "Claude Code",
                Action::ToggleProvider {
                    provider: Provider::Claude,
                },
            ),
            ContextMenuItem::action(
                "provider-codex",
                "Codex",
                Action::ToggleProvider {
                    provider: Provider::Codex,
                },
            ),
            ContextMenuItem::action(
                "provider-antigravity",
                "Antigravity",
                Action::ToggleProvider {
                    provider: Provider::Antigravity,
                },
            ),
            ContextMenuItem::action(
                "provider-opencode",
                "OpenCode",
                Action::ToggleProvider {
                    provider: Provider::OpenCode,
                },
            ),
        ],
    );
    let languages = std::iter::once(ContextMenuItem::action(
        "language-system",
        "System default",
        Action::SetLanguage {
            language: "system".into(),
        },
    ))
    .chain(
        crate::localization::LanguageId::ALL
            .into_iter()
            .map(|language| {
                ContextMenuItem::action(
                    &format!("language-{}", language.code().to_ascii_lowercase()),
                    language.native_name(),
                    Action::SetLanguage {
                        language: language.code().into(),
                    },
                )
            }),
    )
    .collect();
    let settings = ContextMenuItem::submenu(
        "settings",
        "Settings",
        vec![
            ContextMenuItem::action(
                "start-with-windows",
                "Start with Windows",
                Action::ToggleStartup,
            ),
            ContextMenuItem::submenu("language", "Language", languages),
            ContextMenuItem::separator("settings-separator"),
            ContextMenuItem::action(
                "check-updates",
                "Check for updates",
                Action::CheckForUpdates,
            ),
        ],
    );

    ContextMenuDocument {
        schema_version: CONTEXT_MENU_SCHEMA_VERSION,
        id: CLASSIC_CONTEXT_MENU_ID.into(),
        name: "Classic v1".into(),
        items: vec![
            ContextMenuItem::action("refresh", "Refresh", Action::Refresh),
            frequency,
            providers,
            settings,
            ContextMenuItem::action("toggle-widget", "Show widget", Action::ToggleWidget),
            ContextMenuItem::action("open-dashboard", "Open Dashboard", Action::OpenDashboard),
            ContextMenuItem::separator("root-separator"),
            ContextMenuItem::action("exit", "Exit", Action::Exit),
        ],
    }
}

pub fn dashboard_v2_context_menu() -> ContextMenuDocument {
    ContextMenuDocument {
        schema_version: CONTEXT_MENU_SCHEMA_VERSION,
        id: DASHBOARD_V2_CONTEXT_MENU_ID.into(),
        name: "Dashboard v2".into(),
        items: vec![
            ContextMenuItem::action(
                "open-dashboard",
                "Open Dashboard",
                ContextMenuAction::OpenDashboard,
            ),
            ContextMenuItem::separator("dashboard-separator"),
            ContextMenuItem::action("exit", "Exit", ContextMenuAction::Exit),
        ],
    }
}

pub fn context_menus_directory() -> PathBuf {
    crate::app_settings::app_data_directory().join("context-menus")
}

pub fn ensure_builtin_context_menus() -> Result<PathBuf, String> {
    let directory = context_menus_directory();
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut classic_path = None;
    for document in [classic_context_menu(), dashboard_v2_context_menu()] {
        let errors = document.validate();
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        let path = directory.join(format!("{}.json", document.id));
        let canonical = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
        if std::fs::read(&path).ok().as_deref() != Some(canonical.as_slice()) {
            crate::app_settings::write_json_atomic(&path, &document)?;
        }
        if document.id == CLASSIC_CONTEXT_MENU_ID {
            classic_path = Some(path);
        }
    }
    for legacy_id in [
        LEGACY_CLASSIC_CONTEXT_MENU_ID,
        LEGACY_DASHBOARD_V2_CONTEXT_MENU_ID,
    ] {
        let legacy_path = directory.join(format!("{legacy_id}.json"));
        match std::fs::remove_file(legacy_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    classic_path.ok_or_else(|| "The Classic context menu could not be created".into())
}

pub fn load_context_menu(path: &Path) -> Result<ContextMenuDocument, String> {
    let source = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut document: ContextMenuDocument =
        serde_json::from_str(&source).map_err(|error| error.to_string())?;
    remove_legacy_context_menu_actions(&mut document.items);
    let errors = document.validate();
    if errors.is_empty() {
        Ok(document)
    } else {
        Err(errors.join("\n"))
    }
}

pub fn list_context_menus() -> Result<Vec<ContextMenuDescriptor>, String> {
    let built_in_path = ensure_builtin_context_menus()?;
    let mut menus = std::fs::read_dir(context_menus_directory())
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .filter_map(|path| {
            let menu = load_context_menu(&path).ok()?;
            Some(ContextMenuDescriptor {
                built_in: path == built_in_path || menu.is_builtin(),
                path,
                id: menu.id,
                name: menu.name,
            })
        })
        .collect::<Vec<_>>();
    menus.sort_by_key(|menu| (!menu.built_in, menu.name.to_ascii_lowercase()));
    Ok(menus)
}

pub fn resolve_context_menu(reference: Option<&str>) -> Result<ContextMenuDocument, String> {
    let reference = reference.unwrap_or(CLASSIC_CONTEXT_MENU_ID).trim();
    let reference = canonical_context_menu_reference(reference);
    let menus = list_context_menus()?;
    let mut matches = menus
        .iter()
        .filter(|menu| menu.id.eq_ignore_ascii_case(reference));
    if let Some(menu) = matches.next() {
        return load_context_menu(&menu.path);
    }
    let matches = menus
        .iter()
        .filter(|menu| menu.name.eq_ignore_ascii_case(reference))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [menu] => load_context_menu(&menu.path),
        [] => Err(format!("Context menu '{reference}' was not found")),
        _ => Err(format!(
            "Context menu name '{reference}' is ambiguous; use its id"
        )),
    }
}

fn canonical_context_menu_reference(reference: &str) -> &str {
    match reference {
        reference if reference.eq_ignore_ascii_case(LEGACY_CLASSIC_CONTEXT_MENU_ID) => {
            CLASSIC_CONTEXT_MENU_ID
        }
        reference if reference.eq_ignore_ascii_case(LEGACY_DASHBOARD_V2_CONTEXT_MENU_ID) => {
            DASHBOARD_V2_CONTEXT_MENU_ID
        }
        reference => reference,
    }
}

pub fn save_context_menu(document: &ContextMenuDocument) -> Result<PathBuf, String> {
    let mut document = document.clone();
    document.schema_version = CONTEXT_MENU_SCHEMA_VERSION;
    remove_legacy_context_menu_actions(&mut document.items);
    if document.is_builtin() {
        return Err(format!(
            "{} is read-only; duplicate it to make changes",
            document.name
        ));
    }
    let errors = document.validate();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let directory = context_menus_directory();
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("{}.json", safe_file_stem(&document.id)));
    crate::app_settings::write_json_atomic(&path, &document)?;
    Ok(path)
}

fn remove_legacy_context_menu_actions(items: &mut Vec<ContextMenuItem>) {
    for item in items.iter_mut() {
        if let ContextMenuItemKind::Submenu { items } = &mut item.kind {
            remove_legacy_context_menu_actions(items);
        }
    }
    items.retain(|item| match &item.kind {
        ContextMenuItemKind::Action {
            action: ContextMenuAction::LegacyResetPosition,
        } => false,
        ContextMenuItemKind::Submenu { items } => !items.is_empty(),
        _ => true,
    });
}

pub fn delete_context_menu(path: &Path) -> Result<(), String> {
    let directory =
        std::fs::canonicalize(context_menus_directory()).map_err(|error| error.to_string())?;
    let path = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    if path.parent() != Some(directory.as_path()) {
        return Err("Context menu path is outside the managed library".into());
    }
    let document = load_context_menu(&path)?;
    if document.is_builtin() {
        return Err("Built-in context menus cannot be deleted".into());
    }
    std::fs::remove_file(path).map_err(|error| error.to_string())
}

pub fn unique_document_id(name: &str) -> String {
    let base = safe_file_stem(name);
    let base = if base.is_empty() {
        "context-menu"
    } else {
        &base
    };
    let existing = list_context_menus().unwrap_or_default();
    if !existing
        .iter()
        .any(|menu| menu.id.eq_ignore_ascii_case(base))
    {
        return base.into();
    }
    (2..)
        .map(|suffix| format!("{base}-{suffix}"))
        .find(|candidate| {
            !existing
                .iter()
                .any(|menu| menu.id.eq_ignore_ascii_case(candidate))
        })
        .unwrap_or_else(|| format!("{base}-copy"))
}

fn safe_file_stem(value: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_lowercase());
            separator = false;
        } else if !separator && !result.is_empty() {
            result.push('-');
            separator = true;
        }
    }
    result.trim_matches('-').to_string()
}

fn schema_version() -> u32 {
    CONTEXT_MENU_SCHEMA_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_menu_is_valid_and_contains_the_v1_controls() {
        let menu = classic_context_menu();
        assert_eq!(menu.id, CLASSIC_CONTEXT_MENU_ID);
        assert_eq!(menu.name, "Classic v1");
        assert!(!serde_json::to_string(&menu)
            .unwrap()
            .contains("description"));
        assert!(menu.validate().is_empty());
        assert!(menu.items.iter().any(|item| item.id == "refresh"));
        assert!(menu.items.iter().any(|item| item.id == "update-frequency"));
        assert!(serde_json::to_string(&menu)
            .unwrap()
            .contains("provider-opencode"));
        assert!(menu.items.iter().any(|item| {
            item.id == "toggle-widget"
                && matches!(
                    &item.kind,
                    ContextMenuItemKind::Action {
                        action: ContextMenuAction::ToggleWidget
                    }
                )
        }));
        assert!(!serde_json::to_string(&menu)
            .unwrap()
            .contains("reset_position"));
        let mut legacy = menu.clone();
        legacy.id = LEGACY_CLASSIC_CONTEXT_MENU_ID.into();
        assert!(legacy.is_builtin());
    }

    #[test]
    fn dashboard_menu_is_builtin_and_separates_its_actions() {
        let menu = dashboard_v2_context_menu();
        assert_eq!(menu.id, DASHBOARD_V2_CONTEXT_MENU_ID);
        assert_eq!(menu.name, "Dashboard v2");
        assert!(menu.is_builtin());
        let mut legacy = menu.clone();
        legacy.id = LEGACY_DASHBOARD_V2_CONTEXT_MENU_ID.into();
        assert!(legacy.is_builtin());
        assert_eq!(
            canonical_context_menu_reference(LEGACY_DASHBOARD_V2_CONTEXT_MENU_ID),
            DASHBOARD_V2_CONTEXT_MENU_ID
        );
        assert!(menu.validate().is_empty());
        assert_eq!(
            menu.items,
            vec![
                ContextMenuItem::action(
                    "open-dashboard",
                    "Open Dashboard",
                    ContextMenuAction::OpenDashboard,
                ),
                ContextMenuItem::separator("dashboard-separator"),
                ContextMenuItem::action("exit", "Exit", ContextMenuAction::Exit),
            ]
        );
    }

    #[test]
    fn legacy_frequency_milliseconds_are_normalized_to_seconds() {
        let action: ContextMenuAction =
            serde_json::from_str(r#"{"type":"set_update_frequency","milliseconds":300000}"#)
                .unwrap();
        assert_eq!(
            action,
            ContextMenuAction::SetUpdateFrequency { seconds: 300 }
        );
        let serialized = serde_json::to_string(&action).unwrap();
        assert!(serialized.contains(r#""seconds":300"#));
        assert!(!serialized.contains("milliseconds"));
    }

    #[test]
    fn legacy_reset_position_actions_are_removed() {
        let mut items = vec![
            ContextMenuItem::action(
                "reset-position",
                "Reset position",
                ContextMenuAction::LegacyResetPosition,
            ),
            ContextMenuItem::submenu(
                "legacy-settings",
                "Settings",
                vec![ContextMenuItem::action(
                    "nested-reset-position",
                    "Reset position",
                    ContextMenuAction::LegacyResetPosition,
                )],
            ),
            ContextMenuItem::action("exit", "Exit", ContextMenuAction::Exit),
        ];
        remove_legacy_context_menu_actions(&mut items);
        assert_eq!(
            items,
            vec![ContextMenuItem::action(
                "exit",
                "Exit",
                ContextMenuAction::Exit
            )]
        );
    }

    #[test]
    fn urls_are_limited_to_user_facing_protocols() {
        assert!(supported_url("https://example.com"));
        assert!(supported_url("http://example.com"));
        assert!(!supported_url("mailto:test@example.com"));
        assert!(!supported_url("file:///C:/secret.txt"));
        assert!(!supported_url("powershell:whoami"));
    }

    #[test]
    fn duplicate_item_ids_are_rejected_across_submenus() {
        let mut menu = ContextMenuDocument::blank("Test");
        menu.items.push(ContextMenuItem::submenu(
            "submenu",
            "Submenu",
            vec![ContextMenuItem::action(
                "exit",
                "Duplicate",
                ContextMenuAction::Exit,
            )],
        ));
        assert!(menu
            .validate()
            .iter()
            .any(|error| error.contains("unique id")));
    }

    #[test]
    fn informational_text_accepts_usage_and_version_templates() {
        let mut menu = ContextMenuDocument::blank("Test");
        menu.items.insert(
            0,
            ContextMenuItem::text("usage", "v{app.version} - {claude.session:usage_line}"),
        );
        assert!(menu.validate().is_empty());
    }
}
