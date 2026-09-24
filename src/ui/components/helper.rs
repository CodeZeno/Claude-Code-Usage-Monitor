//! The single helper used to edit expressions, text templates, and actions.
//!
//! Every helper shares one layout: an editor for the draft, a status line, and
//! a searchable browser. The browser narrows a flat list of entries with
//! category and provider chips, and a details pane explains the selected entry
//! and inserts it at the editor's caret.

use eframe::egui;
use lucide_icons::Icon as LucideIcon;

use crate::localization::LanguageId;
use crate::ui::components::icon::{icon_only_button, icon_text, labeled_icon_button};
use crate::ui::components::text_field::singleline;
use crate::ui::theme::{
    accent, danger, helper_border, helper_card_surface, helper_surface, menu_hover, menu_text,
    muted, selected_menu_fill, success,
};
use crate::ui::tokens::CONTROL_HEIGHT;

/// Category chip that shows only entries already used by the draft.
pub(crate) const IN_USE_CATEGORY: &str = "In use";

const ENTRY_ROW_HEIGHT: f32 = 52.0;
/// Space between an entry's title and its code.
const ENTRY_LINE_GAP: f32 = 5.0;
/// Where entry titles, codes and group headings start within a row, leaving
/// room for the selection marker and the in-use dot.
const ENTRY_TEXT_INSET: f32 = 28.0;
const IN_USE_DOT_X: f32 = 17.0;
const CHIP_HEIGHT: f32 = 26.0;
const SEARCH_WIDTH: f32 = 360.0;
const DETAILS_FOOTER_HEIGHT: f32 = 104.0;

pub(crate) struct HelperState {
    pub(crate) draft: String,
    original_draft: String,
    pub(crate) search: String,
    pub(crate) category: Option<&'static str>,
    pub(crate) scope: Option<&'static str>,
    pub(crate) selected: Option<String>,
    /// Character index of the editor caret, remembered while focus moves to
    /// the browser so insertions land where the user left off.
    cursor: Option<usize>,
    insert_requested: bool,
    focus_editor: bool,
    scroll_to_selected: bool,
}

impl HelperState {
    pub(crate) fn new(draft: String) -> Self {
        Self {
            original_draft: draft.clone(),
            draft,
            search: String::new(),
            category: None,
            scope: None,
            selected: None,
            cursor: None,
            insert_requested: false,
            focus_editor: false,
            scroll_to_selected: true,
        }
    }

    /// Places the caret at character index `cursor`, as if the user had.
    #[cfg(test)]
    pub(crate) fn set_cursor(&mut self, cursor: usize) {
        self.cursor = Some(cursor);
    }

    pub(crate) fn has_unsaved_changes(&self) -> bool {
        self.draft != self.original_draft
    }

    /// Inserts `insertion` at the remembered caret and moves the caret after it.
    pub(crate) fn insert(&mut self, insertion: &HelperInsertion) {
        let caret = self
            .cursor
            .unwrap_or_else(|| self.draft.chars().count())
            .min(self.draft.chars().count());
        let byte = char_to_byte(&self.draft, caret);
        let (start, text) = match insertion.mode {
            InsertMode::Replace => {
                self.draft = insertion.text.clone();
                self.cursor = Some(self.draft.chars().count());
                return;
            }
            InsertMode::Inline => (byte, insertion.text.clone()),
            InsertMode::Spaced => {
                let before = &self.draft[..byte];
                let after = &self.draft[byte..];
                let token = insertion.text.as_str();
                let mut text = String::new();
                if !before.is_empty()
                    && !before.ends_with(|c: char| c.is_whitespace() || matches!(c, '(' | '{'))
                    && !token.starts_with([')', ','])
                {
                    text.push(' ');
                }
                text.push_str(token);
                if !after.is_empty()
                    && !after.starts_with(|c: char| {
                        c.is_whitespace() || matches!(c, ')' | ',' | '}' | ':')
                    })
                {
                    text.push(' ');
                }
                (byte, text)
            }
            InsertMode::Line => {
                // Actions are one per line, so a new action goes after the
                // line holding the caret instead of splitting it.
                let end = self.draft[byte..]
                    .find('\n')
                    .map_or(self.draft.len(), |offset| byte + offset);
                let mut text = String::new();
                if !self.draft[..end].trim().is_empty() {
                    text.push('\n');
                }
                text.push_str(&insertion.text);
                (end, text)
            }
        };
        self.draft.insert_str(start, &text);
        self.cursor = Some(self.draft[..start + text.len()].chars().count());
    }

    fn caret(&self) -> Caret<'_> {
        let index = self.cursor.unwrap_or_else(|| self.draft.chars().count());
        Caret::new(&self.draft, index)
    }
}

/// Where the editor caret sits, for entries whose insertion depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Caret<'a> {
    pub(crate) draft: &'a str,
    /// Character index of the caret in `draft`.
    pub(crate) index: usize,
}

impl<'a> Caret<'a> {
    pub(crate) fn new(draft: &'a str, index: usize) -> Self {
        Self {
            draft,
            index: index.min(draft.chars().count()),
        }
    }

    /// Byte offset of the caret in `draft`.
    pub(crate) fn byte(&self) -> usize {
        char_to_byte(self.draft, self.index)
    }

    /// Whether the caret is between the braces of a text template `{…}`
    /// placeholder.
    pub(crate) fn in_placeholder(&self) -> bool {
        in_placeholder(self.draft, self.index)
    }
}

/// Whether the character index `caret` falls inside a `{…}` placeholder.
/// `{{` outside a placeholder is a literal brace, as in the template engine.
fn in_placeholder(text: &str, caret: usize) -> bool {
    let byte = char_to_byte(text, caret);
    let mut open = false;
    let mut chars = text[..byte].chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '{' if !open && chars.peek() == Some(&'{') => {
                chars.next();
            }
            '{' => open = true,
            '}' => open = false,
            _ => {}
        }
    }
    open && text[byte..].contains('}')
}

fn char_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(byte, _)| byte)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelperAction {
    Continue,
    Close,
    Apply,
}

/// How an entry's text is added to the draft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InsertMode {
    /// Insert exactly at the caret (text templates).
    Inline,
    /// Insert at the caret, separated from neighbouring tokens by spaces.
    Spaced,
    /// Insert on a new line after the caret's line (action scripts).
    Line,
    /// Replace the whole draft (single-action fields).
    Replace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HelperInsertion {
    pub(crate) text: String,
    pub(crate) mode: InsertMode,
}

impl HelperInsertion {
    pub(crate) fn new(text: impl Into<String>, mode: InsertMode) -> Self {
        Self {
            text: text.into(),
            mode,
        }
    }
}

/// One item the user can browse and insert.
pub(crate) struct HelperEntry {
    /// Stable identity used for selection.
    pub(crate) id: String,
    /// Untranslated category key shown as a quick filter chip.
    pub(crate) category: &'static str,
    /// Provider key used by the provider chips, if the entry belongs to one.
    pub(crate) scope: Option<&'static str>,
    /// Heading the entry is listed under.
    pub(crate) group: String,
    pub(crate) label: String,
    /// Monospace text shown under the label.
    pub(crate) code: String,
    /// Identifier that marks the entry as used when it appears in the draft.
    pub(crate) token: String,
    /// Current value, shown to the right of the entry.
    pub(crate) value: Option<String>,
}

impl HelperEntry {
    fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || [&self.label, &self.code, &self.group]
                .iter()
                .any(|text| text.to_lowercase().contains(needle))
    }

    fn in_use(&self, draft: &str) -> bool {
        contains_token(draft, &self.token)
    }
}

/// Whether `token` appears in `text` as a whole identifier, so
/// `claude.session` does not match `claude.session.percentage`.
pub(crate) fn contains_token(text: &str, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let identifier = |c: char| c.is_alphanumeric() || c == '_' || c == '.';
    text.match_indices(token).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + token.len()..].chars().next();
        !before.is_some_and(identifier) && !after.is_some_and(identifier)
    })
}

pub(crate) enum HelperStatus {
    Valid {
        message: String,
        result: Option<String>,
    },
    Invalid(String),
}

impl HelperStatus {
    fn is_valid(&self) -> bool {
        matches!(self, Self::Valid { .. })
    }
}

pub(crate) struct HelperView<'a> {
    /// What Builder is editing, shown as a badge beside its name.
    pub(crate) kind_icon: LucideIcon,
    pub(crate) kind: &'a str,
    pub(crate) description: &'a str,
    pub(crate) hint: &'a str,
    /// Uses a monospace editor for code rather than prose.
    pub(crate) code_editor: bool,
    pub(crate) editor_height: f32,
    /// Category chip order; categories not listed follow in entry order.
    pub(crate) categories: &'a [&'static str],
    /// Provider chips, shown when matching entries exist.
    pub(crate) scopes: &'a [HelperScope],
    pub(crate) entries: &'a [HelperEntry],
}

/// A provider filter chip.
pub(crate) struct HelperScope {
    pub(crate) key: &'static str,
    pub(crate) label: String,
    /// Brand mark drawn before the label.
    pub(crate) mark: Option<char>,
}

/// Draws the helper and returns what the user asked to do with the draft.
///
/// `status` validates the draft and `preview` renders it for display; both run
/// after the editor so they always reflect this frame's text. `details` draws
/// entry-specific controls and returns the text to insert for that entry at
/// the given caret.
pub(crate) fn show_helper(
    ui: &mut egui::Ui,
    state: &mut HelperState,
    view: HelperView<'_>,
    language: LanguageId,
    status: impl Fn(&str) -> HelperStatus,
    preview: Option<&dyn Fn(&str) -> String>,
    details: impl FnOnce(&mut egui::Ui, &HelperEntry, Caret<'_>) -> Result<HelperInsertion, String>,
) -> HelperAction {
    let mut action = HelperAction::Continue;
    let width = ui.available_width();
    let height = ui.available_height();
    let editor_id = ui.make_persistent_id("helper-editor");

    egui::Frame::new()
        .fill(helper_surface())
        .stroke(egui::Stroke::new(1.0, helper_border()))
        .corner_radius(egui::CornerRadius::same(7))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width((width - 28.0).max(1.0));
            ui.set_min_height((height - 28.0).max(1.0));

            let can_apply = status(&state.draft).is_valid();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    let text_left = ui
                        .horizontal(|ui| {
                            let text_left = builder_title(ui, language.text("Builder"));
                            ui.add_space(4.0);
                            kind_badge(ui, view.kind_icon, view.kind);
                            text_left
                        })
                        .inner;
                    ui.horizontal(|ui| {
                        ui.add_space(text_left);
                        ui.label(egui::RichText::new(view.description).color(muted()));
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    let close = if state.has_unsaved_changes() {
                        ui.add(egui::Button::new((
                            language.text("Discard"),
                            icon_text(LucideIcon::X, 16.0),
                        )))
                        .on_hover_text(language.text("Discard changes"))
                    } else {
                        ui.add(icon_only_button(LucideIcon::X))
                            .on_hover_text(language.text("Close Builder"))
                    };
                    if close.clicked() {
                        action = HelperAction::Close;
                    }
                    if ui
                        .add_enabled(
                            can_apply,
                            labeled_icon_button(LucideIcon::Save, language.text("Apply")),
                        )
                        .on_disabled_hover_text(language.text("Fix the problem before applying"))
                        .clicked()
                    {
                        action = HelperAction::Apply;
                    }
                });
            });

            ui.add_space(10.0);
            let editor_size = egui::vec2(ui.available_width(), view.editor_height);
            let mut editor = egui::TextEdit::multiline(&mut state.draft)
                .id(editor_id)
                .desired_width(f32::INFINITY)
                .min_size(editor_size)
                .margin(egui::Margin::same(10))
                .hint_text(view.hint);
            if view.code_editor {
                editor = editor.code_editor();
            }
            let output = ui.allocate_ui(editor_size, |ui| editor.show(ui)).inner;
            if output.response.response.has_focus() {
                if let Some(range) = output.cursor_range {
                    state.cursor = Some(range.primary.index.0);
                }
            }

            let status = status(&state.draft);
            ui.add_space(8.0);
            status_row(
                ui,
                &status,
                preview.map(|preview| preview(&state.draft)),
                language,
            );

            ui.add_space(10.0);
            browser(ui, state, &view, language, details, editor_id);
        });

    action
}

fn status_row(
    ui: &mut egui::Ui,
    status: &HelperStatus,
    preview: Option<String>,
    language: LanguageId,
) {
    egui::Frame::new()
        .fill(helper_card_surface())
        .stroke(egui::Stroke::new(1.0, helper_border()))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                match status {
                    HelperStatus::Valid { message, result } => {
                        ui.label(icon_text(LucideIcon::CheckCircle, 15.0).color(success()));
                        ui.colored_label(success(), message);
                        if let Some(result) = result.as_ref().filter(|_| preview.is_none()) {
                            ui.separator();
                            ui.label(
                                egui::RichText::new(language.text("Current result")).color(muted()),
                            );
                            ui.label(egui::RichText::new(result).strong());
                        }
                    }
                    HelperStatus::Invalid(error) => {
                        ui.label(icon_text(LucideIcon::AlertCircle, 15.0).color(danger()));
                        ui.add(egui::Label::new(egui::RichText::new(error).color(danger())).wrap());
                    }
                }
                if let Some(preview) = preview {
                    ui.separator();
                    ui.label(egui::RichText::new(language.text("Preview")).color(muted()));
                    if preview.is_empty() {
                        ui.label(
                            egui::RichText::new(language.text("Preview is empty"))
                                .italics()
                                .color(muted()),
                        );
                    } else {
                        ui.add(
                            egui::Label::new(egui::RichText::new(preview).size(17.0).strong())
                                .truncate(),
                        );
                    }
                }
            });
        });
}

fn browser(
    ui: &mut egui::Ui,
    state: &mut HelperState,
    view: &HelperView<'_>,
    language: LanguageId,
    details: impl FnOnce(&mut egui::Ui, &HelperEntry, Caret<'_>) -> Result<HelperInsertion, String>,
    editor_id: egui::Id,
) {
    let needle = state.search.trim().to_lowercase();
    let searched: Vec<&HelperEntry> = view
        .entries
        .iter()
        .filter(|entry| entry.matches(&needle))
        .collect();

    // Category chips count entries that match the search, so an empty chip
    // tells the user where their search has no results.
    let mut categories: Vec<(&'static str, usize)> = Vec::new();
    for entry in &searched {
        match categories
            .iter_mut()
            .find(|(key, _)| *key == entry.category)
        {
            Some((_, count)) => *count += 1,
            None => categories.push((entry.category, 1)),
        }
    }
    if let Some(category) = state
        .category
        .filter(|category| *category != IN_USE_CATEGORY)
        .filter(|category| !categories.iter().any(|(key, _)| key == category))
    {
        // Keep the active chip visible when a search empties it.
        categories.push((category, 0));
    }
    categories.sort_by_key(|(key, _)| {
        view.categories
            .iter()
            .position(|category| category == key)
            .unwrap_or(usize::MAX)
    });
    let in_use = searched
        .iter()
        .filter(|entry| entry.in_use(&state.draft))
        .count();
    let in_category = |entry: &&&HelperEntry, category: Option<&str>| match category {
        None => true,
        Some(IN_USE_CATEGORY) => entry.in_use(&state.draft),
        Some(category) => entry.category == category,
    };
    let categorised: Vec<&HelperEntry> = searched
        .iter()
        .filter(|entry| in_category(entry, state.category))
        .copied()
        .collect();
    let scopes: Vec<(&HelperScope, usize)> = view
        .scopes
        .iter()
        .filter_map(|scope| {
            let count = categorised
                .iter()
                .filter(|entry| entry.scope == Some(scope.key))
                .count();
            (count > 0).then_some((scope, count))
        })
        .collect();
    if state
        .scope
        .is_some_and(|key| !scopes.iter().any(|(scope, _)| scope.key == key))
    {
        state.scope = None;
    }
    let visible: Vec<&HelperEntry> = categorised
        .into_iter()
        .filter(|entry| state.scope.is_none() || entry.scope == state.scope)
        .collect();

    let search_id = ui.make_persistent_id("helper-search");
    let mut insert_from_search = false;
    ui.horizontal(|ui| {
        ui.label(icon_text(LucideIcon::Search, 15.0).color(muted()));
        let search = singleline(&mut state.search)
            .id(search_id)
            .desired_width(SEARCH_WIDTH)
            .hint_text(language.text("Search everything..."))
            .show(ui)
            .response
            .response;
        // Single-line text edits surrender focus when Enter is pressed.
        if search.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            insert_from_search = true;
            state.insert_requested = true;
            search.request_focus();
        }
        if search.has_focus() {
            let (down, up) = ui.input(|input| {
                (
                    input.key_pressed(egui::Key::ArrowDown),
                    input.key_pressed(egui::Key::ArrowUp),
                )
            });
            let current = visible
                .iter()
                .position(|entry| Some(&entry.id) == state.selected.as_ref());
            let next = match (down, up, current) {
                (true, _, Some(index)) => Some((index + 1).min(visible.len().saturating_sub(1))),
                (true, _, None) => Some(0),
                (_, true, Some(index)) => Some(index.saturating_sub(1)),
                _ => None,
            };
            if let Some(entry) = next.and_then(|index| visible.get(index)) {
                state.selected = Some(entry.id.clone());
            }
            if next.is_some() {
                state.scroll_to_selected = true;
            }
        }
    });
    ui.add_space(8.0);

    let category_label = language.text("Category:");
    let provider_label = language.text("Provider:");
    // Both rows share a label column so their chips start at the same place.
    let label_width = [category_label, provider_label]
        .into_iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(label.to_string(), filter_label_font(), accent())
                .size()
                .x
        })
        .fold(0.0, f32::max)
        + 10.0;
    filter_row(ui, category_label, label_width, |ui| {
        if chip(
            ui,
            state.category.is_none(),
            None,
            language.text("All"),
            Some(searched.len()),
        )
        .clicked()
        {
            state.category = None;
        }
        if in_use > 0 || state.category == Some(IN_USE_CATEGORY) {
            let response = chip(
                ui,
                state.category == Some(IN_USE_CATEGORY),
                None,
                language.text(IN_USE_CATEGORY),
                Some(in_use),
            )
            .on_hover_text(language.text("Entries already used in the editor"));
            if response.clicked() {
                state.category = Some(IN_USE_CATEGORY);
            }
        }
        for (key, count) in &categories {
            if chip(
                ui,
                state.category == Some(*key),
                None,
                language.text(key),
                Some(*count),
            )
            .clicked()
            {
                state.category = Some(*key);
            }
        }
    });
    if !scopes.is_empty() {
        ui.add_space(4.0);
        filter_row(ui, provider_label, label_width, |ui| {
            if chip(ui, state.scope.is_none(), None, language.text("Any"), None).clicked() {
                state.scope = None;
            }
            for (scope, count) in &scopes {
                if chip(
                    ui,
                    state.scope == Some(scope.key),
                    scope.mark,
                    &scope.label,
                    Some(*count),
                )
                .clicked()
                {
                    state.scope = Some(scope.key);
                }
            }
        });
    }
    ui.add_space(10.0);

    if !visible
        .iter()
        .any(|entry| Some(&entry.id) == state.selected.as_ref())
    {
        // Prefer something the draft already uses so reopening a helper
        // shows the relevant entry straight away.
        state.selected = visible
            .iter()
            .find(|entry| entry.in_use(&state.draft))
            .or(visible.first())
            .map(|entry| entry.id.clone());
        state.scroll_to_selected = true;
    }

    let height = ui.available_height().max(220.0);
    let gap = ui.spacing().item_spacing.x;
    let list_width = ((ui.available_width() - gap) * 0.56).floor();
    let details_width = (ui.available_width() - gap - list_width).max(1.0);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(list_width, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| entry_list(ui, state, &visible, list_width, height, language),
        );
        ui.allocate_ui_with_layout(
            egui::vec2(details_width, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                let selected = visible
                    .iter()
                    .find(|entry| Some(&entry.id) == state.selected.as_ref())
                    .copied();
                details_pane(
                    ui,
                    state,
                    selected,
                    details_width,
                    height,
                    language,
                    details,
                );
            },
        );
    });

    // An Enter press with no visible entry must not insert later when the
    // search changes and results become available again.
    state.insert_requested = false;
    let focus_editor = std::mem::take(&mut state.focus_editor) && !insert_from_search;
    if let Some(cursor) = state.cursor {
        if let Some(mut text_state) = egui::text_edit::TextEditState::load(ui.ctx(), editor_id) {
            if focus_editor || !ui.memory(|memory| memory.has_focus(editor_id)) {
                let cursor = egui::text::CCursor::new(cursor);
                text_state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(cursor)));
                text_state.store(ui.ctx(), editor_id);
            }
        }
    }
    if focus_editor {
        // Restore the insertion caret before handing focus back for typing.
        ui.memory_mut(|memory| memory.request_focus(editor_id));
    }
}

fn entry_list(
    ui: &mut egui::Ui,
    state: &mut HelperState,
    visible: &[&HelperEntry],
    width: f32,
    height: f32,
    language: LanguageId,
) {
    egui::Frame::new()
        .fill(helper_card_surface())
        .stroke(egui::Stroke::new(1.0, helper_border()))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(4, 6))
        .show(ui, |ui| {
            ui.set_width(width - 8.0);
            ui.set_height(height - 12.0);
            if visible.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(language.text("Nothing matches these filters"))
                            .color(muted()),
                    );
                });
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("helper-entries")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let mut last_group: Option<&str> = None;
                    for entry in visible {
                        if last_group != Some(entry.group.as_str()) {
                            if last_group.is_some() {
                                ui.add_space(10.0);
                            }
                            group_heading(ui, &entry.group);
                            last_group = Some(&entry.group);
                        }
                        let selected = state.selected.as_ref() == Some(&entry.id);
                        let response = entry_row(ui, entry, selected, entry.in_use(&state.draft));
                        if selected && state.scroll_to_selected {
                            // The list's size is unknown until it has been laid
                            // out once, so keep asking until the row is shown.
                            if ui.clip_rect().contains_rect(response.rect) {
                                state.scroll_to_selected = false;
                            } else {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                        }
                        if response.clicked() {
                            state.selected = Some(entry.id.clone());
                        }
                        if response.double_clicked() {
                            state.selected = Some(entry.id.clone());
                            state.insert_requested = true;
                        }
                    }
                });
        });
}

fn entry_row(
    ui: &mut egui::Ui,
    entry: &HelperEntry,
    selected: bool,
    in_use: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ENTRY_ROW_HEIGHT),
        egui::Sense::click(),
    );
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    // Matches the navigation menu: a filled row with an accent marker.
    if selected {
        painter.rect_filled(rect, 4.0, selected_menu_fill());
        let marker_clip =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 6.0, rect.bottom()));
        painter
            .with_clip_rect(marker_clip.intersect(ui.clip_rect()))
            .rect_filled(rect, 4.0, accent());
    } else if response.hovered() {
        painter.rect_filled(rect, 4.0, menu_hover());
    }
    let text_color = menu_text();
    let left = rect.left() + ENTRY_TEXT_INSET;
    let value = entry.value.as_deref().filter(|value| !value.is_empty());
    let value_galley = value.map(|value| {
        painter.layout_job(truncated(
            value,
            egui::FontId::proportional(13.0),
            if selected { text_color } else { muted() },
            rect.width() * 0.32,
        ))
    });
    let value_width = value_galley
        .as_ref()
        .map_or(0.0, |galley| galley.size().x + 16.0);
    let text_width = (rect.width() - ENTRY_TEXT_INSET - 12.0 - value_width).max(20.0);
    let label = painter.layout_job(truncated(
        &entry.label,
        entry_title_font(),
        text_color,
        text_width,
    ));
    let code = painter.layout_job(truncated(
        &entry.code,
        egui::FontId::monospace(11.5),
        muted(),
        text_width,
    ));
    // Centre the title and code as a block, with a gap between them.
    let top = rect.center().y - (label.size().y + ENTRY_LINE_GAP + code.size().y) / 2.0;
    let code_top = top + label.size().y + ENTRY_LINE_GAP;
    painter.galley(egui::pos2(left, top), label, text_color);
    painter.galley(egui::pos2(left, code_top), code, muted());
    if let Some(galley) = value_galley {
        let pos = egui::pos2(
            rect.right() - 12.0 - galley.size().x,
            rect.center().y - galley.size().y / 2.0,
        );
        painter.galley(pos, galley, muted());
    }
    if in_use {
        painter.circle_filled(
            egui::pos2(rect.left() + IN_USE_DOT_X, rect.center().y),
            3.0,
            success(),
        );
    }
    response
}

fn entry_title_font() -> egui::FontId {
    egui::FontId::proportional(14.0)
}

fn filter_label_font() -> egui::FontId {
    entry_title_font()
}

/// A group heading in the entry list, styled like an entry title but in the
/// accent colour and underlined.
fn group_heading(ui: &mut egui::Ui, text: &str) {
    let width = ui.available_width();
    let mut job = truncated(
        text,
        entry_title_font(),
        accent(),
        width - ENTRY_TEXT_INSET - 12.0,
    );
    for section in &mut job.sections {
        section.format.underline = egui::Stroke::new(1.0, accent());
    }
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, galley.size().y + 8.0),
        egui::Sense::hover(),
    );
    ui.painter().galley(
        egui::pos2(rect.left() + ENTRY_TEXT_INSET, rect.top() + 2.0),
        galley,
        accent(),
    );
}

/// A row of filter chips introduced by a label in a fixed-width column.
fn filter_row(
    ui: &mut egui::Ui,
    label: &str,
    label_width: f32,
    add_chips: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal_top(|ui| {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(label_width, CHIP_HEIGHT), egui::Sense::hover());
        let label = ui
            .painter()
            .layout_no_wrap(label.to_string(), filter_label_font(), accent());
        let pos = egui::pos2(rect.left(), rect.center().y - visual_centre(&label, false));
        ui.painter().galley(pos, label, accent());
        ui.horizontal_wrapped(add_chips);
    });
}

/// Builder's icon and name, centred on the same line as the kind badge.
/// Returns how far the name is indented, so the description can line up.
fn builder_title(ui: &mut egui::Ui, title: &str) -> f32 {
    let icon = glyph_galley(ui, LucideIcon::Blocks.unicode(), 22.0, accent());
    let strong = ui.visuals().strong_text_color();
    let title =
        ui.painter()
            .layout_no_wrap(title.to_string(), egui::FontId::proportional(20.0), strong);
    let gap = 10.0;
    let text_left = icon.size().x + gap;
    let size = egui::vec2(
        text_left + title.size().x,
        icon.size().y.max(title.size().y),
    );
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let centre = rect.center().y;
    let painter = ui.painter();
    painter.galley(
        egui::pos2(rect.left(), centre - visual_centre(&icon, true)),
        icon,
        accent(),
    );
    painter.galley(
        egui::pos2(
            rect.left() + text_left,
            centre - visual_centre(&title, false),
        ),
        title,
        strong,
    );
    text_left
}

/// The pill beside Builder's name that says what is being edited. Painted so
/// the icon and label share one centre line.
fn kind_badge(ui: &mut egui::Ui, icon: LucideIcon, label: &str) {
    let text = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(13.0),
        accent(),
    );
    let icon = glyph_galley(ui, icon.unicode(), 14.0, accent());
    let padding = 10.0;
    let gap = 6.0;
    let size = egui::vec2(
        padding * 2.0 + icon.size().x + gap + text.size().x,
        CHIP_HEIGHT,
    );
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let radius = pill_radius(size.y);
    let painter = ui.painter();
    painter.rect_filled(rect, radius, helper_card_surface());
    painter.rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0, helper_border()),
        egui::StrokeKind::Inside,
    );
    let icon_left = rect.left() + padding;
    let text_left = icon_left + icon.size().x + gap;
    let centre = rect.center().y;
    painter.galley(
        egui::pos2(icon_left, centre - visual_centre(&icon, true)),
        icon,
        accent(),
    );
    painter.galley(
        egui::pos2(text_left, centre - visual_centre(&text, false)),
        text,
        accent(),
    );
}

/// A single glyph from the `lucide` family, which also holds the brand marks.
fn glyph_galley(
    ui: &egui::Ui,
    glyph: char,
    size: f32,
    color: egui::Color32,
) -> std::sync::Arc<egui::Galley> {
    ui.painter().layout_no_wrap(
        glyph.to_string(),
        egui::FontId::new(size, egui::FontFamily::Name("lucide".into())),
        color,
    )
}

/// The height, from the top of `galley`, that its first line looks centred
/// on. Icons centre on their ink. Text centres between the top of its tallest
/// glyph and the baseline, since descenders do not count towards how centred
/// a word looks. Font boxes centre neither, which is what pushes icons and
/// labels out of line when they share a row.
fn visual_centre(galley: &egui::Galley, icon: bool) -> f32 {
    let Some(row) = galley.rows.first() else {
        return galley.size().y / 2.0;
    };
    let mut top = f32::MAX;
    let mut bottom = f32::MIN;
    for glyph in &row.row.glyphs {
        if glyph.uv_rect.size.y <= 0.0 {
            continue;
        }
        let baseline = row.pos.y + glyph.pos.y;
        let ink_top = baseline + glyph.uv_rect.offset.y;
        top = top.min(ink_top);
        bottom = bottom.max(if icon {
            ink_top + glyph.uv_rect.size.y
        } else {
            baseline
        });
    }
    if top > bottom {
        galley.size().y / 2.0
    } else {
        (top + bottom) / 2.0
    }
}

/// Corner radius for a fully rounded pill. A radius of exactly half the height
/// makes the two rounded ends meet, and a translucent fill then draws a line
/// where they overlap.
fn pill_radius(height: f32) -> egui::CornerRadius {
    egui::CornerRadius::same((height / 2.0 - 1.0).max(0.0) as u8)
}

fn truncated(
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    job
}

fn details_pane(
    ui: &mut egui::Ui,
    state: &mut HelperState,
    entry: Option<&HelperEntry>,
    width: f32,
    height: f32,
    language: LanguageId,
    details: impl FnOnce(&mut egui::Ui, &HelperEntry, Caret<'_>) -> Result<HelperInsertion, String>,
) {
    egui::Frame::new()
        .fill(helper_card_surface())
        .stroke(egui::Stroke::new(1.0, helper_border()))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(width - 28.0);
            ui.set_height(height - 28.0);
            let Some(entry) = entry else {
                ui.label(
                    egui::RichText::new(language.text("Select an entry to see its details."))
                        .color(muted()),
                );
                return;
            };
            ui.label(egui::RichText::new(&entry.label).size(17.0).strong());
            ui.add(
                egui::Label::new(egui::RichText::new(&entry.code).monospace().color(muted()))
                    .wrap(),
            );
            if let Some(value) = entry.value.as_deref().filter(|value| !value.is_empty()) {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(language.text("Current value")).color(muted()));
                    ui.label(egui::RichText::new(value).strong());
                });
            }
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);
            let body_height = (ui.available_height() - DETAILS_FOOTER_HEIGHT).max(60.0);
            let caret = state.caret();
            let insertion = egui::ScrollArea::vertical()
                .id_salt(("helper-details", &entry.id))
                .auto_shrink([false, true])
                .max_height(body_height)
                .show(ui, |ui| details(ui, entry, caret))
                .inner;

            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                let can_insert = insertion.is_ok();
                let label = match insertion.as_ref().map(|insertion| insertion.mode) {
                    Ok(InsertMode::Replace) => language.text("Use this action"),
                    _ => language.text("Insert"),
                };
                let button = ui
                    .add_enabled(
                        can_insert,
                        labeled_icon_button(LucideIcon::CornerDownLeft, label)
                            .min_size(egui::vec2(ui.available_width(), CONTROL_HEIGHT)),
                    )
                    .on_hover_text(
                        language
                            .text("Double-click an entry or press Enter in search to insert it"),
                    );
                ui.add_space(6.0);
                match &insertion {
                    Ok(insertion) => {
                        egui::Frame::new()
                            .fill(helper_surface())
                            .corner_radius(egui::CornerRadius::same(4))
                            .inner_margin(egui::Margin::symmetric(8, 6))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&insertion.text)
                                            .monospace()
                                            .color(accent()),
                                    )
                                    .truncate(),
                                );
                            });
                    }
                    Err(reason) => {
                        ui.label(egui::RichText::new(reason).color(danger()));
                    }
                }
                ui.label(
                    egui::RichText::new(match insertion.as_ref().map(|insertion| insertion.mode) {
                        Ok(InsertMode::Replace) => language.text("Will replace the editor with"),
                        Ok(InsertMode::Line) => language.text("Will add a new line"),
                        _ => language.text("Will insert at the cursor"),
                    })
                    .small()
                    .color(muted()),
                );
                let requested = std::mem::take(&mut state.insert_requested);
                if let Ok(insertion) = &insertion {
                    if button.clicked() || requested {
                        state.insert(insertion);
                        state.focus_editor = true;
                    }
                }
            });
        });
}

/// A selectable row with a label and a muted sample on the right, used for
/// choices inside the details pane.
pub(crate) fn option_row(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    sample: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), CONTROL_HEIGHT),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 5, accent().gamma_multiply(0.16));
            painter.rect_stroke(
                rect,
                5,
                egui::Stroke::new(1.0, accent().gamma_multiply(0.6)),
                egui::StrokeKind::Inside,
            );
        } else if response.hovered() {
            painter.rect_filled(rect, 5, egui::Color32::from_white_alpha(8));
        }
        let text_color = if selected {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().text_color()
        };
        let sample = painter.layout_job(truncated(
            sample,
            egui::FontId::proportional(13.0),
            muted(),
            rect.width() * 0.5,
        ));
        let label_width = (rect.width() - 30.0 - sample.size().x).max(20.0);
        let label = painter.layout_job(truncated(
            label,
            egui::FontId::proportional(14.0),
            text_color,
            label_width,
        ));
        painter.galley(
            egui::pos2(rect.left() + 10.0, rect.center().y - label.size().y / 2.0),
            label,
            text_color,
        );
        painter.galley(
            egui::pos2(
                rect.right() - 10.0 - sample.size().x,
                rect.center().y - sample.size().y / 2.0,
            ),
            sample,
            muted(),
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A rounded filter pill with an optional brand mark and count.
pub(crate) fn chip(
    ui: &mut egui::Ui,
    selected: bool,
    mark: Option<char>,
    label: &str,
    count: Option<usize>,
) -> egui::Response {
    let font = egui::FontId::proportional(13.0);
    let text_color = if selected {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    let label = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), text_color);
    let count = count.map(|count| {
        ui.painter()
            .layout_no_wrap(count.to_string(), egui::FontId::proportional(11.5), muted())
    });
    let padding = 10.0;
    let mark = mark.map(|mark| glyph_galley(ui, mark, 14.0, text_color));
    let mark_width = mark.as_ref().map_or(0.0, |mark| mark.size().x + 6.0);
    let count_width = count.as_ref().map_or(0.0, |count| count.size().x + 6.0);
    let size = egui::vec2(
        mark_width + label.size().x + count_width + padding * 2.0,
        CHIP_HEIGHT,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = pill_radius(size.y);
        let (fill, stroke) = if selected {
            (accent().gamma_multiply(0.22), accent())
        } else if response.hovered() {
            (egui::Color32::from_white_alpha(10), muted())
        } else {
            (egui::Color32::TRANSPARENT, helper_border())
        };
        painter.rect_filled(rect, radius, fill);
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0, stroke),
            egui::StrokeKind::Inside,
        );
        let centre = rect.center().y;
        if let Some(mark) = mark {
            let pos = egui::pos2(rect.left() + padding, centre - visual_centre(&mark, true));
            painter.galley(pos, mark, text_color);
        }
        let label_pos = egui::pos2(
            rect.left() + padding + mark_width,
            centre - visual_centre(&label, false),
        );
        let label_width = label.size().x;
        let label_bottom = label_pos.y + label.size().y;
        painter.galley(label_pos, label, text_color);
        if let Some(count) = count {
            // Share the label's baseline rather than centring the smaller
            // text on its own.
            let pos = egui::pos2(
                label_pos.x + label_width + 6.0,
                label_bottom - count.size().y,
            );
            painter.galley(pos, count, muted());
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(draft: &str, cursor: usize, text: &str, mode: InsertMode) -> String {
        let mut state = HelperState::new(draft.into());
        state.cursor = Some(cursor);
        state.insert(&HelperInsertion::new(text, mode));
        state.draft
    }

    #[test]
    fn typing_continues_after_a_browser_insertion() {
        let context = egui::Context::default();
        crate::ui::theme::configure_style(&context, LanguageId::English);
        let mut state = HelperState::new("before  after".into());
        state.set_cursor(7);
        let entries = [HelperEntry {
            id: "value".into(),
            category: "General",
            scope: None,
            group: "General".into(),
            label: "Value".into(),
            code: "value".into(),
            token: "value".into(),
            value: None,
        }];
        for frame in 0..3 {
            state.insert_requested = frame == 1;
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 900.0),
                )),
                ..Default::default()
            };
            if frame == 2 {
                input.events.push(egui::Event::Text("!".into()));
            }
            let mut output = context.run_ui(input, |ui| {
                show_helper(
                    ui,
                    &mut state,
                    HelperView {
                        kind_icon: LucideIcon::Type,
                        kind: "Text",
                        description: "",
                        hint: "",
                        code_editor: false,
                        editor_height: 64.0,
                        categories: &[],
                        scopes: &[],
                        entries: &entries,
                    },
                    LanguageId::English,
                    |_| HelperStatus::Valid {
                        message: "Valid".into(),
                        result: None,
                    },
                    None,
                    |_, _, _| Ok(HelperInsertion::new("value", InsertMode::Inline)),
                );
            });
            output.textures_delta.clear();
        }
        assert_eq!(state.draft, "before value! after");
    }

    #[test]
    fn search_enter_retains_focus_and_does_not_queue_empty_results() {
        let context = egui::Context::default();
        crate::ui::theme::configure_style(&context, LanguageId::English);
        let mut state = HelperState::new("before  after".into());
        state.set_cursor(7);
        let entries = [HelperEntry {
            id: "value".into(),
            category: "General",
            scope: None,
            group: "General".into(),
            label: "Value".into(),
            code: "value".into(),
            token: "value".into(),
            value: None,
        }];
        for frame in 0..8 {
            if frame == 4 {
                state.search = "no matching entry".into();
            } else if frame == 7 {
                state.search.clear();
            }
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 900.0),
                )),
                ..Default::default()
            };
            if matches!(frame, 1..=4 | 6..=7) {
                input.events.push(egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: matches!(frame, 1 | 3 | 6),
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            if frame == 2 {
                input.events.push(egui::Event::Text("val".into()));
            }
            let mut output = context.run_ui(input, |ui| {
                let editor_id = ui.make_persistent_id("test-editor");
                let search_id = ui.make_persistent_id("helper-search");
                egui::TextEdit::multiline(&mut state.draft)
                    .id(editor_id)
                    .show(ui);
                browser(
                    ui,
                    &mut state,
                    &HelperView {
                        kind_icon: LucideIcon::Type,
                        kind: "Text",
                        description: "",
                        hint: "",
                        code_editor: false,
                        editor_height: 64.0,
                        categories: &[],
                        scopes: &[],
                        entries: &entries,
                    },
                    LanguageId::English,
                    |_, _, _| Ok(HelperInsertion::new("value", InsertMode::Inline)),
                    editor_id,
                );
                if frame == 0 {
                    ui.memory_mut(|memory| memory.request_focus(search_id));
                }
                assert!(
                    ui.memory(|memory| memory.has_focus(search_id)),
                    "search lost focus on frame {frame}; draft {:?}",
                    state.draft
                );
                if frame >= 1 {
                    let expected = if frame < 3 { 12 } else { 17 };
                    assert_eq!(state.cursor, Some(expected));
                    let editor = egui::text_edit::TextEditState::load(ui.ctx(), editor_id).unwrap();
                    assert_eq!(
                        editor.cursor.char_range().unwrap().primary.index.0,
                        expected
                    );
                }
            });
            output.textures_delta.clear();
            if frame == 2 {
                assert_eq!(state.search, "val");
                assert_eq!(state.draft, "before value after");
            }
            if frame >= 3 {
                assert_eq!(state.draft, "before valuevalue after");
            }
        }
    }

    #[test]
    fn insertions_respect_the_caret_and_mode() {
        assert_eq!(
            insert("Used  today", 5, "{a:percent}", InsertMode::Inline),
            "Used {a:percent} today"
        );
        assert_eq!(insert("a >", 3, "b", InsertMode::Spaced), "a > b");
        assert_eq!(insert("min()", 4, "a", InsertMode::Spaced), "min(a)");
        assert_eq!(insert("x + y", 2, "*", InsertMode::Spaced), "x * + y");
        assert_eq!(
            insert("show_dashboard()\nexit()", 3, "refresh()", InsertMode::Line),
            "show_dashboard()\nrefresh()\nexit()"
        );
        assert_eq!(insert("", 0, "refresh()", InsertMode::Line), "refresh()");
        assert_eq!(insert("old()", 2, "new()", InsertMode::Replace), "new()");
        assert_eq!(insert("é ", 2, "x", InsertMode::Inline), "é x");
        assert_eq!(insert("{}", 1, "a", InsertMode::Spaced), "{a}");
        assert_eq!(
            insert("{a:percent}", 2, "+ 1", InsertMode::Spaced),
            "{a + 1:percent}"
        );
    }

    #[test]
    fn placeholders_are_found_around_the_caret() {
        assert!(in_placeholder("Used {a} today", 6));
        assert!(in_placeholder("Used {a:percent} today", 8));
        assert!(!in_placeholder("Used {a} today", 3));
        assert!(!in_placeholder("Used {a} today", 9));
        assert!(!in_placeholder("{{a} today", 2), "{{ is a literal brace");
        assert!(
            !in_placeholder("Used {a", 7),
            "an unclosed brace is literal"
        );
        assert!(in_placeholder("é {a}", 3));
    }

    #[test]
    fn tokens_only_match_whole_identifiers() {
        let draft = "{claude.session.percentage:percent} and min(a, b)";
        assert!(contains_token(draft, "claude.session.percentage"));
        assert!(!contains_token(draft, "claude.session"));
        assert!(!contains_token(draft, "session.percentage"));
        assert!(contains_token(draft, "min"));
        assert!(!contains_token(draft, "mi"));
        assert!(!contains_token(draft, ""));
    }
}
