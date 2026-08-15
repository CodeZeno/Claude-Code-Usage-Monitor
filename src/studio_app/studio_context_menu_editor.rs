use super::*;

pub(super) fn nav(ui: &mut egui::Ui, current: &mut Page, page: Page, title: &str) {
    let selected = *current == page;
    if crate::ui::components::navigation::navigation_item(ui, selected, title).clicked() {
        *current = page;
    }
}

#[allow(dead_code)]
pub(super) fn context_menu_label_editor(
    ui: &mut egui::Ui,
    label: &mut String,
    context: &DataContext,
    read_only: bool,
    language: LanguageId,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= ui
            .add_enabled(
                !read_only,
                singleline_text_edit(label).desired_width((ui.available_width() - 98.0).max(120.0)),
            )
            .changed();
        ui.add_enabled_ui(!read_only, |ui| {
            ui.menu_button(language.text("ƒx Values"), |ui| {
                ui.set_min_width(380.0);
                ui.label(
                    egui::RichText::new(language.text("Insert a live value into this label"))
                        .small()
                        .color(muted()),
                );
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("context-menu-label-values")
                    .max_height(420.0)
                    .show(ui, |ui| {
                        let mut last_group = "";
                        for value in TEXT_TEMPLATE_VALUES.iter().copied() {
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
                            let format = default_text_template_format(value.kind);
                            let token = text_template_token(value.expression, format);
                            let sample = text_template_value_sample(value, format, context);
                            if ui
                                .button(format!("{}  —  {}", language.text(value.label), sample))
                                .on_hover_text(&token)
                                .clicked()
                            {
                                append_expression_token(label, &token);
                                changed = true;
                                ui.close();
                            }
                        }
                    });
            });
        });
    });
    let preview = theme_engine::format_template(label, context);
    ui.label(
        egui::RichText::new(format!("{}: {preview}", language.text("Preview")))
            .small()
            .color(muted()),
    );
    changed
}

pub(super) fn flatten_context_menu_items(
    items: &[ContextMenuItem],
) -> Vec<(Vec<usize>, usize, String, &'static str)> {
    fn visit(
        items: &[ContextMenuItem],
        parent: &[usize],
        depth: usize,
        rows: &mut Vec<(Vec<usize>, usize, String, &'static str)>,
    ) {
        for (index, item) in items.iter().enumerate() {
            let mut path = parent.to_vec();
            path.push(index);
            let kind = match &item.kind {
                ContextMenuItemKind::Action { .. } => "action",
                ContextMenuItemKind::Text => "text",
                ContextMenuItemKind::Separator => "separator",
                ContextMenuItemKind::Submenu { .. } => "submenu",
            };
            rows.push((path.clone(), depth, item.label.clone(), kind));
            if let ContextMenuItemKind::Submenu { items } = &item.kind {
                visit(items, &path, depth + 1, rows);
            }
        }
    }
    let mut rows = Vec::new();
    visit(items, &[], 0, &mut rows);
    rows
}

pub(super) fn context_menu_item<'a>(
    items: &'a [ContextMenuItem],
    path: &[usize],
) -> Option<&'a ContextMenuItem> {
    let (index, remaining) = path.split_first()?;
    let item = items.get(*index)?;
    if remaining.is_empty() {
        Some(item)
    } else if let ContextMenuItemKind::Submenu { items } = &item.kind {
        context_menu_item(items, remaining)
    } else {
        None
    }
}

pub(super) fn context_menu_item_mut<'a>(
    items: &'a mut [ContextMenuItem],
    path: &[usize],
) -> Option<&'a mut ContextMenuItem> {
    let (index, remaining) = path.split_first()?;
    let item = items.get_mut(*index)?;
    if remaining.is_empty() {
        Some(item)
    } else if let ContextMenuItemKind::Submenu { items } = &mut item.kind {
        context_menu_item_mut(items, remaining)
    } else {
        None
    }
}

pub(super) fn context_menu_items_mut<'a>(
    items: &'a mut Vec<ContextMenuItem>,
    parent_path: &[usize],
) -> Option<&'a mut Vec<ContextMenuItem>> {
    if parent_path.is_empty() {
        return Some(items);
    }
    let item = context_menu_item_mut(items, parent_path)?;
    if let ContextMenuItemKind::Submenu { items } = &mut item.kind {
        Some(items)
    } else {
        None
    }
}

pub(super) fn add_context_menu_item(
    items: &mut Vec<ContextMenuItem>,
    selection: Option<&[usize]>,
    item: ContextMenuItem,
) -> Vec<usize> {
    if let Some(path) = selection {
        if let Some(ContextMenuItem {
            kind: ContextMenuItemKind::Submenu { items: children },
            ..
        }) = context_menu_item_mut(items, path)
        {
            let mut child_path = path.to_vec();
            child_path.push(children.len());
            children.push(item);
            return child_path;
        }
    }
    let path = vec![items.len()];
    items.push(item);
    path
}

pub(super) fn remove_context_menu_item(
    items: &mut Vec<ContextMenuItem>,
    path: &[usize],
) -> Option<ContextMenuItem> {
    let (&index, parent_path) = path.split_last()?;
    let siblings = context_menu_items_mut(items, parent_path)?;
    (index < siblings.len()).then(|| siblings.remove(index))
}

pub(super) fn find_context_menu_item_path(
    items: &[ContextMenuItem],
    id: &str,
) -> Option<Vec<usize>> {
    fn visit(items: &[ContextMenuItem], id: &str, parent: &[usize]) -> Option<Vec<usize>> {
        for (index, item) in items.iter().enumerate() {
            let mut path = parent.to_vec();
            path.push(index);
            if item.id == id {
                return Some(path);
            }
            if let ContextMenuItemKind::Submenu { items } = &item.kind {
                if let Some(found) = visit(items, id, &path) {
                    return Some(found);
                }
            }
        }
        None
    }
    visit(items, id, &[])
}

pub(super) fn move_context_menu_item_to(
    items: &mut Vec<ContextMenuItem>,
    source: &[usize],
    target: &ContextMenuDropTarget,
    selection: &mut Option<Vec<usize>>,
) -> bool {
    let target_path = match target {
        ContextMenuDropTarget::Before(path)
        | ContextMenuDropTarget::After(path)
        | ContextMenuDropTarget::Into(path) => path,
    };
    if source == target_path || target_path.starts_with(source) {
        return false;
    }
    let Some(target_id) = context_menu_item(items, target_path).map(|item| item.id.clone()) else {
        return false;
    };
    let Some(moving) = remove_context_menu_item(items, source) else {
        return false;
    };
    let moving_id = moving.id.clone();
    let Some(adjusted_target) = find_context_menu_item_path(items, &target_id) else {
        return false;
    };
    let new_path = match target {
        ContextMenuDropTarget::Into(_) => {
            let Some(ContextMenuItemKind::Submenu { items: children }) =
                context_menu_item_mut(items, &adjusted_target).map(|item| &mut item.kind)
            else {
                return false;
            };
            let mut path = adjusted_target;
            path.push(children.len());
            children.push(moving);
            path
        }
        ContextMenuDropTarget::Before(_) | ContextMenuDropTarget::After(_) => {
            let (&target_index, parent_path) = adjusted_target.split_last().unwrap();
            let Some(siblings) = context_menu_items_mut(items, parent_path) else {
                return false;
            };
            let insert_at = if matches!(target, ContextMenuDropTarget::After(_)) {
                target_index + 1
            } else {
                target_index
            }
            .min(siblings.len());
            siblings.insert(insert_at, moving);
            let mut path = parent_path.to_vec();
            path.push(insert_at);
            path
        }
    };
    *selection = Some(new_path);
    find_context_menu_item_path(items, &moving_id).is_some()
}

pub(super) fn duplicate_context_menu_item(
    items: &mut Vec<ContextMenuItem>,
    source: &[usize],
) -> Option<Vec<usize>> {
    let mut duplicate = context_menu_item(items, source)?.clone();
    let mut existing = flatten_context_menu_items(items)
        .into_iter()
        .filter_map(|(path, _, _, _)| context_menu_item(items, &path).map(|item| item.id.clone()))
        .collect::<std::collections::HashSet<_>>();
    fn remap(item: &mut ContextMenuItem, existing: &mut std::collections::HashSet<String>) {
        let base = format!("{}-copy", item.id);
        let id = (1..)
            .map(|suffix| {
                if suffix == 1 {
                    base.clone()
                } else {
                    format!("{base}-{suffix}")
                }
            })
            .find(|candidate| !existing.contains(candidate))
            .unwrap_or_else(|| format!("{base}-new"));
        existing.insert(id.clone());
        item.id = id;
        if let ContextMenuItemKind::Submenu { items } = &mut item.kind {
            for child in items {
                remap(child, existing);
            }
        }
    }
    remap(&mut duplicate, &mut existing);
    let (&index, parent) = source.split_last()?;
    let siblings = context_menu_items_mut(items, parent)?;
    let insert_at = (index + 1).min(siblings.len());
    siblings.insert(insert_at, duplicate);
    let mut path = parent.to_vec();
    path.push(insert_at);
    Some(path)
}

pub(super) fn next_context_menu_item_id(items: &[ContextMenuItem], prefix: &str) -> String {
    let existing = flatten_context_menu_items(items)
        .into_iter()
        .filter_map(|(path, _, _, _)| context_menu_item(items, &path).map(|item| item.id.clone()))
        .collect::<std::collections::HashSet<_>>();
    (1..)
        .map(|suffix| format!("{prefix}-{suffix}"))
        .find(|candidate| !existing.contains(candidate))
        .unwrap_or_else(|| format!("{prefix}-new"))
}

pub(super) fn context_menu_action_script(action: &ContextMenuAction) -> String {
    let string_arg = |value: &str| serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into());
    match action {
        ContextMenuAction::OpenDashboard => "open_dashboard()".into(),
        ContextMenuAction::Refresh => "refresh()".into(),
        ContextMenuAction::SetUpdateFrequency { seconds } => {
            format!("set_update_frequency({seconds})")
        }
        ContextMenuAction::ToggleProvider { provider } => {
            format!("toggle_provider({})", provider.descriptor().key)
        }
        ContextMenuAction::ToggleStartup => "toggle_startup()".into(),
        ContextMenuAction::ToggleWidget => "toggle_widget()".into(),
        ContextMenuAction::LegacyResetPosition => String::new(),
        ContextMenuAction::SetLanguage { language } => {
            format!("set_language({})", string_arg(language))
        }
        ContextMenuAction::CheckForUpdates => "check_for_updates()".into(),
        ContextMenuAction::ToggleLayerRender { target } => {
            format!("toggle_layer_render({})", string_arg(target))
        }
        ContextMenuAction::LayerActions { actions } => {
            format!("layer_actions({})", string_arg(actions))
        }
        ContextMenuAction::OpenUrl { url } => format!("open_url({})", string_arg(url)),
        ContextMenuAction::Exit => "exit()".into(),
    }
}

pub(super) fn parse_context_menu_action_script(script: &str) -> Result<ContextMenuAction, String> {
    let script = script.trim();
    if script.contains('\n') || script.contains('\r') {
        return Err("A context menu item accepts exactly one action".into());
    }
    for (name, action) in [
        ("open_dashboard", ContextMenuAction::OpenDashboard),
        ("refresh", ContextMenuAction::Refresh),
        ("toggle_startup", ContextMenuAction::ToggleStartup),
        ("toggle_widget", ContextMenuAction::ToggleWidget),
        ("check_for_updates", ContextMenuAction::CheckForUpdates),
        ("exit", ContextMenuAction::Exit),
    ] {
        if script == format!("{name}()") {
            return Ok(action);
        }
    }
    let call_arg = |name: &str| {
        script
            .strip_prefix(name)
            .and_then(|value| value.strip_prefix('('))
            .and_then(|value| value.strip_suffix(')'))
            .map(str::trim)
    };
    let string_arg = |name: &str| -> Result<String, String> {
        let value = call_arg(name).ok_or_else(|| format!("Expected {name}(value)"))?;
        let value = if value.starts_with('"') {
            serde_json::from_str(value).map_err(|_| format!("{name} needs a valid quoted value"))?
        } else {
            value.to_string()
        };
        if value.trim().is_empty() {
            Err(format!("{name} needs a value"))
        } else {
            Ok(value)
        }
    };
    if let Some(value) = call_arg("set_update_frequency") {
        let seconds = value
            .parse::<u32>()
            .map_err(|_| "Update frequency must be seconds".to_string())?;
        let seconds = match seconds {
            POLL_1_MIN | POLL_5_MIN | POLL_15_MIN | POLL_1_HOUR => seconds / 1_000,
            seconds => seconds,
        };
        if !matches!(
            seconds,
            POLL_1_MIN_SECONDS | POLL_5_MIN_SECONDS | POLL_15_MIN_SECONDS | POLL_1_HOUR_SECONDS
        ) {
            return Err("Choose a supported update frequency".into());
        }
        return Ok(ContextMenuAction::SetUpdateFrequency { seconds });
    }
    if let Some(value) = call_arg("toggle_provider") {
        let key = value.trim_matches('"').to_ascii_lowercase();
        let provider = ContextMenuProvider::from_key(&key)
            .ok_or_else(|| format!("Unknown provider: {key}"))?;
        return Ok(ContextMenuAction::ToggleProvider { provider });
    }
    if call_arg("set_language").is_some() {
        return Ok(ContextMenuAction::SetLanguage {
            language: string_arg("set_language")?,
        });
    }
    if call_arg("toggle_layer_render").is_some() {
        return Ok(ContextMenuAction::ToggleLayerRender {
            target: string_arg("toggle_layer_render")?,
        });
    }
    if call_arg("layer_actions").is_some() {
        let actions = string_arg("layer_actions")?;
        theme_engine::parse_mouse_actions(&actions)
            .map_err(|error| format!("Invalid layer action: {error}"))?;
        return Ok(ContextMenuAction::LayerActions { actions });
    }
    if call_arg("open_url").is_some() {
        let url = string_arg("open_url")?;
        if !context_menu::supported_url(&url) {
            return Err("URL must start with http:// or https://".into());
        }
        return Ok(ContextMenuAction::OpenUrl { url });
    }
    Err("Choose an action from the helper, or enter a supported action".into())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn context_menu_action_reference_panels(
    ui: &mut egui::Ui,
    height: f32,
    layer_targets: &[(String, String)],
    target: &mut String,
    property: &mut MouseActionProperty,
    value: &mut String,
    draft: &mut String,
    language: LanguageId,
) {
    let gap = ui.spacing().item_spacing.x;
    let panel_width = ((ui.available_width() - gap * 2.0) / 3.0).max(1.0);
    ui.horizontal(|ui| {
        expression_reference_card(ui, panel_width, height, language.text("Actions"), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("context-menu-helper-actions")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for kind in ContextMenuActionKind::ALL {
                        if ui.button(language.text(kind.label())).clicked() {
                            *draft = context_menu_action_script(&kind.default_action());
                        }
                    }
                    ui.separator();
                    ui.label(egui::RichText::new(language.text("Layer actions")).small().strong());
                    let mut set_layer_action = |source: String| {
                        *draft = context_menu_action_script(&ContextMenuAction::LayerActions {
                            actions: source,
                        });
                    };
                    if ui.button(language.text("Set property")).clicked() {
                        set_layer_action(format!(
                            "set({target}, {}, {})",
                            property.name(),
                            value.trim()
                        ));
                    }
                    if ui
                        .add_enabled(
                            *property == MouseActionProperty::Render,
                            egui::Button::new(language.text("Toggle property")),
                        )
                        .clicked()
                    {
                        set_layer_action(format!("toggle({target}, {})", property.name()));
                    }
                    if ui.button(language.text("Reset property")).clicked() {
                        set_layer_action(format!("reset({target}, {})", property.name()));
                    }
                    let numeric = *property != MouseActionProperty::Render;
                    if ui
                        .add_enabled(numeric, egui::Button::new(language.text("Increase value")))
                        .clicked()
                    {
                        set_layer_action(format!(
                            "increase({target}, {}, {})",
                            property.name(),
                            value.trim()
                        ));
                    }
                    if ui
                        .add_enabled(numeric, egui::Button::new(language.text("Decrease value")))
                        .clicked()
                    {
                        set_layer_action(format!(
                            "decrease({target}, {}, {})",
                            property.name(),
                            value.trim()
                        ));
                    }
                });
        });
        expression_reference_card(ui, panel_width, height, language.text("Options"), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("context-menu-helper-options")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(language.text("Update frequency")).small().strong());
                    for (seconds, label) in [
                        (POLL_1_MIN_SECONDS, "Every minute"),
                        (POLL_5_MIN_SECONDS, "Every 5 minutes"),
                        (POLL_15_MIN_SECONDS, "Every 15 minutes"),
                        (POLL_1_HOUR_SECONDS, "Every hour"),
                    ] {
                        if ui.button(language.text(label)).clicked() {
                            *draft = format!("set_update_frequency({seconds})");
                        }
                    }
                    ui.separator();
                    ui.label(egui::RichText::new(language.text("Provider")).small().strong());
                    for descriptor in PROVIDER_DESCRIPTORS {
                        if ui.button(language.text(descriptor.display_name)).clicked() {
                            *draft = format!("toggle_provider({})", descriptor.key);
                        }
                    }
                    ui.separator();
                    ui.label(
                        egui::RichText::new(language.text("Edit quoted values in the action field for URLs, languages, and layer-action scripts."))
                            .small()
                            .color(muted()),
                    );
                });
        });
        expression_reference_card(ui, panel_width, height, language.text("Layers"), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("context-menu-helper-layers")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if ui
                        .selectable_label(target == "self", language.text("Self"))
                        .clicked()
                    {
                        *target = "self".into();
                    }
                    for (id, name) in layer_targets {
                        let token = format!(
                            "\"{}\"",
                            id.replace('\\', "\\\\").replace('"', "\\\"")
                        );
                        if ui
                            .selectable_label(*target == token, format!("{name}  ·  {id}"))
                            .clicked()
                        {
                            *target = token;
                        }
                    }
                    ui.separator();
                    ui.label(egui::RichText::new(language.text("Properties")).small().strong());
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
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(language.text("Value expression")).small().color(muted()));
                    ui.add(singleline_text_edit(value).desired_width(ui.available_width()));
                });
        });
    });
}

#[allow(dead_code)]
pub(super) fn context_menu_action_editor(
    ui: &mut egui::Ui,
    action: &mut ContextMenuAction,
    read_only: bool,
    language: LanguageId,
) -> bool {
    let mut changed = false;
    match action {
        ContextMenuAction::SetUpdateFrequency { seconds } => {
            labeled(ui, language.text("Update frequency"), |ui| {
                Dropdown::from_id_salt("menu-frequency")
                    .width(inspector_control_width(ui))
                    .selected_text(interval_name(language, seconds.saturating_mul(1_000)))
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            (POLL_1_MIN_SECONDS, "Every minute"),
                            (POLL_5_MIN_SECONDS, "Every 5 minutes"),
                            (POLL_15_MIN_SECONDS, "Every 15 minutes"),
                            (POLL_1_HOUR_SECONDS, "Every hour"),
                        ] {
                            changed |=
                                dropdown_selectable_value(ui, seconds, value, language.text(label))
                                    .changed();
                        }
                    });
            });
        }
        ContextMenuAction::ToggleProvider { provider } => {
            let selected = language.text(provider.descriptor().display_name);
            labeled(ui, language.text("Provider"), |ui| {
                Dropdown::from_id_salt("menu-provider")
                    .width(inspector_control_width(ui))
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for descriptor in PROVIDER_DESCRIPTORS {
                            changed |= dropdown_selectable_value(
                                ui,
                                provider,
                                descriptor.id,
                                language.text(descriptor.display_name),
                            )
                            .changed();
                        }
                    });
            });
        }
        ContextMenuAction::SetLanguage { language: code } => {
            labeled(ui, language.text("Language"), |ui| {
                Dropdown::from_id_salt("menu-language")
                    .width(inspector_control_width(ui))
                    .selected_text(language_name(language, code))
                    .show_ui(ui, |ui| {
                        for (candidate, name) in languages(language) {
                            changed |= dropdown_selectable_value(ui, code, candidate.into(), name)
                                .changed();
                        }
                    });
            });
        }
        ContextMenuAction::ToggleLayerRender { target } => {
            labeled(ui, language.text("Target layer id"), |ui| {
                changed |= ui
                    .add_enabled(
                        !read_only,
                        singleline_text_edit(target)
                            .desired_width(inspector_control_width(ui))
                            .hint_text("main"),
                    )
                    .changed();
            });
        }
        ContextMenuAction::LayerActions { actions } => {
            ui.label(language.text("Layer actions"));
            changed |= ui
                .add_enabled(
                    !read_only,
                    egui::TextEdit::multiline(actions)
                        .code_editor()
                        .desired_width(ui.available_width())
                        .desired_rows(6)
                        .hint_text("toggle(self, render)"),
                )
                .changed();
            ui.label(
                egui::RichText::new(
                    language.text("Uses the same safe action language as layer mouse events."),
                )
                .small()
                .color(muted()),
            );
        }
        ContextMenuAction::OpenUrl { url } => {
            labeled(ui, language.text("URL"), |ui| {
                changed |= ui
                    .add_enabled(
                        !read_only,
                        singleline_text_edit(url)
                            .desired_width(inspector_control_width(ui))
                            .hint_text("https://example.com"),
                    )
                    .changed();
            });
            ui.label(
                egui::RichText::new(language.text("Only http and https links are allowed."))
                    .small()
                    .color(muted()),
            );
        }
        _ => {}
    }
    changed
}
