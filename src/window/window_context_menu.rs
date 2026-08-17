use super::*;

pub(super) fn show_context_menu_document(
    hwnd: HWND,
    reference: Option<&str>,
    origin: Option<(usize, String)>,
) {
    let document = match context_menu::resolve_context_menu(reference) {
        Ok(document) => document,
        Err(error) => {
            diagnose::log(format!("context menu load failed: {error}"));
            context_menu::classic_context_menu()
        }
    };
    let language = lock_state()
        .as_ref()
        .map(|state| state.language)
        .unwrap_or_else(localization::detect_system_language);
    let data_context = context_menu_data_context(origin.as_ref());
    let mut actions = Vec::new();
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        append_context_menu_items(
            menu,
            &document.items,
            language,
            &data_context,
            origin.as_ref(),
            &mut actions,
        );
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(hwnd);
        let selected = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD,
            point.x,
            point.y,
            0,
            hwnd,
            None,
        )
        .0 as usize;
        let _ = DestroyMenu(menu);
        if selected >= 1_000 {
            if let Some(action) = actions.get(selected - 1_000).cloned() {
                execute_context_menu_action(hwnd, action, origin);
            }
        }
    }
}

pub(super) fn context_menu_data_context(origin: Option<&(usize, String)>) -> DataContext {
    let state = lock_state();
    let Some(state) = state.as_ref() else {
        return DataContext::from_usage(None, &Canvas::default());
    };
    let runtime = theme_runtime_from_state(state);
    let mut canvas = Canvas::default();
    if let Some(theme) = effective_theme_from_state(state) {
        let surface_index = origin.map_or(0, |(surface_index, _)| *surface_index);
        if theme.surfaces.get(surface_index).is_some() {
            let (width, height) = theme_engine::resolve_surface_size(
                &theme,
                surface_index,
                state.data.as_ref(),
                runtime,
            );
            canvas.width = width;
            canvas.height = height;
        }
    }
    DataContext::from_usage_with_runtime(state.data.as_ref(), &canvas, runtime)
}

unsafe fn append_context_menu_items(
    menu: HMENU,
    items: &[ContextMenuItem],
    language: LanguageId,
    context: &DataContext,
    origin: Option<&(usize, String)>,
    actions: &mut Vec<ContextMenuAction>,
) {
    for item in items {
        match &item.kind {
            ContextMenuItemKind::Separator => {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            }
            ContextMenuItemKind::Text => {
                let label = native_interop::wide_str(&context_menu::rendered_label(
                    language,
                    &item.label,
                    context,
                ));
                let _ = AppendMenuW(menu, MF_GRAYED, 0, PCWSTR::from_raw(label.as_ptr()));
            }
            ContextMenuItemKind::Submenu { items } => {
                let Ok(submenu) = CreatePopupMenu() else {
                    continue;
                };
                append_context_menu_items(submenu, items, language, context, origin, actions);
                let label = native_interop::wide_str(&context_menu::rendered_label(
                    language,
                    &item.label,
                    context,
                ));
                let _ = AppendMenuW(
                    menu,
                    MF_POPUP,
                    submenu.0 as usize,
                    PCWSTR::from_raw(label.as_ptr()),
                );
            }
            ContextMenuItemKind::Action { action } => {
                let id = 1_000 + actions.len();
                let label = native_interop::wide_str(&context_menu::rendered_label(
                    language,
                    &item.label,
                    context,
                ));
                let flags = context_menu_action_flags(action, origin);
                let _ = AppendMenuW(menu, flags, id, PCWSTR::from_raw(label.as_ptr()));
                actions.push(action.clone());
            }
        }
    }
}

pub(super) fn context_menu_action_flags(
    action: &ContextMenuAction,
    origin: Option<&(usize, String)>,
) -> MENU_ITEM_FLAGS {
    let state = lock_state();
    let Some(state) = state.as_ref() else {
        return MENU_ITEM_FLAGS(0);
    };
    let checked = match action {
        ContextMenuAction::SetUpdateFrequency { seconds } => {
            state.poll_interval_ms == seconds.saturating_mul(1_000)
        }
        ContextMenuAction::ToggleProvider { provider } => state.providers.contains(*provider),
        ContextMenuAction::ToggleStartup => is_startup_enabled(),
        ContextMenuAction::ToggleWidget => state
            .active_theme
            .as_ref()
            .and_then(|theme| {
                let effective = theme_engine::apply_mouse_action_overrides(
                    theme,
                    &state.mouse_action_overrides,
                );
                context_menu_widget_origin(&effective).map(|(surface_index, _)| {
                    theme_engine::surface_should_render(
                        &effective,
                        surface_index,
                        state.data.as_ref(),
                        theme_runtime_from_state(state),
                    )
                })
            })
            .unwrap_or(false),
        ContextMenuAction::SetLanguage { language } => {
            if language.eq_ignore_ascii_case("system") {
                state.language_override.is_none()
            } else {
                state
                    .language_override
                    .is_some_and(|current| current.code().eq_ignore_ascii_case(language))
            }
        }
        ContextMenuAction::ToggleLayerRender { target } => state
            .active_theme
            .as_ref()
            .and_then(|theme| {
                let effective = theme_engine::apply_mouse_action_overrides(
                    theme,
                    &state.mouse_action_overrides,
                );
                effective
                    .surfaces
                    .iter()
                    .position(|surface| surface.id.eq_ignore_ascii_case(target))
                    .map(|surface_index| {
                        theme_engine::surface_should_render(
                            &effective,
                            surface_index,
                            state.data.as_ref(),
                            theme_runtime_from_state(state),
                        )
                    })
            })
            .unwrap_or(false),
        _ => false,
    };
    let disabled = matches!(
        action,
        ContextMenuAction::CheckForUpdates
            if matches!(state.update_status, UpdateStatus::Checking | UpdateStatus::Applying)
    ) || matches!(
        action,
        ContextMenuAction::LayerActions { .. } | ContextMenuAction::ToggleLayerRender { .. }
    ) && origin.is_none()
        && state.active_theme.is_none();
    match (checked, disabled) {
        (true, true) => MF_CHECKED | MF_GRAYED,
        (true, false) => MF_CHECKED,
        (false, true) => MF_GRAYED,
        (false, false) => MENU_ITEM_FLAGS(0),
    }
}

pub(super) fn context_menu_action_origin(
    origin: Option<(usize, String)>,
) -> Option<(usize, String)> {
    if origin.is_some() {
        return origin;
    }
    lock_state().as_ref().and_then(|state| {
        state
            .active_theme
            .as_ref()
            .and_then(|theme| theme.surfaces.first())
            .map(|surface| (0, surface.id.clone()))
    })
}

pub(super) fn context_menu_widget_origin(theme: &ThemeDocument) -> Option<(usize, String)> {
    theme
        .surfaces
        .iter()
        .enumerate()
        .find(|(_, surface)| surface.placement.nest != SurfaceNest::TrayIcon)
        .map(|(index, surface)| (index, surface.id.clone()))
}

pub(super) fn execute_context_menu_action(
    hwnd: HWND,
    action: ContextMenuAction,
    origin: Option<(usize, String)>,
) {
    let static_command = match &action {
        ContextMenuAction::OpenDashboard => Some(IDM_DASHBOARD),
        ContextMenuAction::Refresh => Some(1),
        ContextMenuAction::SetUpdateFrequency { seconds } => match *seconds {
            POLL_1_MIN_SECONDS => Some(IDM_FREQ_1MIN),
            POLL_5_MIN_SECONDS => Some(IDM_FREQ_5MIN),
            POLL_15_MIN_SECONDS => Some(IDM_FREQ_15MIN),
            POLL_1_HOUR_SECONDS => Some(IDM_FREQ_1HOUR),
            _ => None,
        },
        ContextMenuAction::ToggleProvider { provider } => {
            Some(provider.descriptor().native_menu_command_id)
        }
        ContextMenuAction::ToggleStartup => Some(IDM_START_WITH_WINDOWS),
        ContextMenuAction::SetLanguage { language } => {
            if language.eq_ignore_ascii_case("system") {
                Some(IDM_LANG_SYSTEM)
            } else {
                LanguageId::from_code(language).map(|language| match language {
                    LanguageId::English => IDM_LANG_ENGLISH,
                    LanguageId::Dutch => IDM_LANG_DUTCH,
                    LanguageId::Spanish => IDM_LANG_SPANISH,
                    LanguageId::French => IDM_LANG_FRENCH,
                    LanguageId::German => IDM_LANG_GERMAN,
                    LanguageId::Japanese => IDM_LANG_JAPANESE,
                    LanguageId::Korean => IDM_LANG_KOREAN,
                    LanguageId::TraditionalChinese => IDM_LANG_TRADITIONAL_CHINESE,
                    LanguageId::SimplifiedChinese => IDM_LANG_SIMPLIFIED_CHINESE,
                    LanguageId::Russian => IDM_LANG_RUSSIAN,
                    LanguageId::PortugueseBrazil => IDM_LANG_PORTUGUESE_BRAZIL,
                    LanguageId::Turkish => IDM_LANG_TURKISH,
                })
            }
        }
        ContextMenuAction::CheckForUpdates => Some(IDM_VERSION_ACTION),
        ContextMenuAction::Exit => Some(2),
        ContextMenuAction::ToggleWidget
        | ContextMenuAction::LegacyResetPosition
        | ContextMenuAction::ToggleLayerRender { .. }
        | ContextMenuAction::LayerActions { .. }
        | ContextMenuAction::OpenUrl { .. } => None,
    };
    if let Some(command) = static_command {
        unsafe {
            let _ = PostMessageW(hwnd, WM_COMMAND, WPARAM(command as usize), LPARAM(0));
        }
        return;
    }
    match action {
        ContextMenuAction::ToggleWidget => {
            let target = lock_state()
                .as_ref()
                .and_then(|state| state.active_theme.as_ref())
                .and_then(context_menu_widget_origin);
            if let Some((surface_index, root_id)) = target {
                let _ =
                    execute_mouse_action_source(surface_index, &root_id, "toggle(self, render)");
            }
        }
        ContextMenuAction::ToggleLayerRender { target } => {
            let Some((surface_index, self_id)) = context_menu_action_origin(origin) else {
                return;
            };
            let target = target.replace('\\', "\\\\").replace('"', "\\\"");
            let source = format!("toggle(\"{target}\", render)");
            let _ = execute_mouse_action_source(surface_index, &self_id, &source);
        }
        ContextMenuAction::LayerActions { actions } => {
            let Some((surface_index, self_id)) = context_menu_action_origin(origin) else {
                return;
            };
            let _ = execute_mouse_action_source(surface_index, &self_id, &actions);
        }
        ContextMenuAction::OpenUrl { url } if context_menu::supported_url(&url) => unsafe {
            let operation = native_interop::wide_str("open");
            let url = native_interop::wide_str(url.trim());
            let result = ShellExecuteW(
                hwnd,
                PCWSTR::from_raw(operation.as_ptr()),
                PCWSTR::from_raw(url.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
            if result.0 as isize <= 32 {
                diagnose::log("context menu URL could not be opened");
            }
        },
        _ => {}
    }
}
