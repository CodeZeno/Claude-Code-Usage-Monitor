use super::*;

fn clock_refresh_delay(interval: Duration) -> Duration {
    let interval_ms = interval.as_millis().max(1);
    let elapsed_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    Duration::from_millis((interval_ms - elapsed_ms % interval_ms) as u64)
}

impl StudioApp {
    pub(super) fn language(&self) -> LanguageId {
        localization::resolve_language(
            self.settings
                .language
                .as_deref()
                .and_then(LanguageId::from_code),
        )
    }

    pub(super) fn theme_is_read_only(&self) -> bool {
        self.theme.is_builtin()
    }

    pub(super) fn theme_runtime(&self) -> ThemeRuntime {
        let language = self.language();
        ThemeRuntime::from_providers(self.settings.enabled_providers())
            .with_poll_state(self.usage_poll_ok, self.usage_has_error)
            .with_language(language)
            .with_countdown(self.settings.usage_countdown)
    }

    pub(super) fn theme_runtime_for_surface(&self, surface_index: usize) -> ThemeRuntime {
        crate::window::query_theme_runtime_for_surface(
            &self.theme,
            surface_index,
            self.theme_runtime(),
        )
    }

    pub(super) fn selected_theme_runtime(&self) -> ThemeRuntime {
        let surface_index = match self.selection {
            Selection::Surface(index) | Selection::Object(index, _) => index,
        };
        self.theme_runtime_for_surface(surface_index)
    }

    pub(super) fn new(
        context: &eframe::CreationContext<'_>,
        owner: isize,
        initial_page: Page,
    ) -> Self {
        crate::diagnose::log_lazy(|| format!("dashboard started owner={owner}"));
        let settings = app_settings::load_settings();
        let language = localization::resolve_language(
            settings.language.as_deref().and_then(LanguageId::from_code),
        );
        configure_style(&context.egui_ctx, language);
        style_native_titlebar(context);
        let classic_theme_path = theme_engine::ensure_starter_theme().ok();
        let configured_path = settings.active_theme_path.as_ref().map(PathBuf::from);
        let configured_theme = configured_path
            .as_deref()
            .and_then(|path| theme_engine::load_theme(path).ok())
            .filter(|theme| !theme.is_obsolete_studio_starter());
        let (theme_path, mut theme) = configured_theme
            .map(|theme| (configured_path, theme))
            .unwrap_or_else(|| {
                let path = classic_theme_path;
                let theme = path
                    .as_deref()
                    .and_then(|path| theme_engine::load_theme(path).ok())
                    .unwrap_or_else(ThemeDocument::starter);
                (path, theme)
            });
        theme.prepare_runtime();
        let history_snapshot = theme.clone();
        let context_menu_path = context_menu::ensure_builtin_context_menus().ok();
        let context_menu = context_menu_path
            .as_deref()
            .and_then(|path| context_menu::load_context_menu(path).ok())
            .unwrap_or_else(context_menu::classic_context_menu);
        let usage_cache = app_settings::load_usage_cache().map(|mut cache| {
            cache.data.select_accounts(&settings.accounts);
            cache
        });
        let usage_poll_ok = usage_cache
            .as_ref()
            .is_some_and(|cache| cache.poll_ok && !cache.data.is_empty());
        let usage_has_error = usage_cache.as_ref().is_some_and(|cache| {
            !cache.poll_ok
                || (cache.data.is_empty()
                    && cache
                        .data
                        .accounts
                        .iter()
                        .any(|account| account.error.is_some()))
        });
        let usage = usage_cache.map(|cache| cache.data);
        let next_preview_countdown_refresh = preview_countdown_refresh_delay(usage.as_ref())
            .and_then(|delay| Instant::now().checked_add(delay));
        let next_preview_clock_refresh = theme
            .current_time_refresh_interval()
            .and_then(|interval| Instant::now().checked_add(clock_refresh_delay(interval)));
        Self {
            owner,
            update_status: crate::dashboard::read_update_status(owner),
            diagnostics: studio_diagnostics::DiagnosticsView::new(),
            page: initial_page,
            synced_poll_interval_ms: settings.poll_interval_ms,
            poll_interval_editor_generation: 0,
            settings,
            startup_enabled: crate::window::is_startup_enabled(),
            theme,
            theme_path,
            selection: Selection::Surface(0),
            preview: None,
            preview_dirty: true,
            preview_renderer: PreviewRenderer::new(context.egui_ctx.clone()),
            preview_generation: 0,
            preview_render_key: None,
            usage,
            usage_poll_ok,
            usage_has_error,
            last_cache_read: Instant::now(),
            next_preview_countdown_refresh,
            next_preview_clock_refresh,
            dirty: false,
            live_apply: DEFAULT_LIVE_APPLY,
            zoom: 1.0,
            preview_pan: egui::Vec2::ZERO,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            history_snapshot,
            scene_width: DEFAULT_SCENE_WIDTH,
            inspector_width: DEFAULT_INSPECTOR_WIDTH,
            hovered_scene_item: None,
            expression_helper: None,
            action_helper: None,
            text_template_helper: None,
            preview_mouse_overrides: HashMap::new(),
            preview_hover_target: None,
            preview_pending_click: None,
            asset_picker: None,
            asset_thumbnails: HashMap::new(),
            asset_page_filter: String::new(),
            asset_page_selected: None,
            asset_error: None,
            settings_error: None,
            theme_error: None,
            pending_unsaved_action: None,
            asset_delete_confirmation: None,
            new_theme_name: None,
            duplicate_theme_name: None,
            delete_theme_confirmation: None,
            context_menu,
            context_menu_path,
            context_menu_dirty: false,
            context_menu_selection: None,
            context_menu_action_helper: None,
            delete_context_menu_confirmation: None,
        }
    }

    pub(super) fn notify_owner(&self) {
        if self.owner != 0 {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(self.owner as *mut _)),
                    WM_APP_SETTINGS_UPDATED,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    pub(super) fn request_refresh(&mut self) {
        crate::diagnose::log("Refresh now clicked in dashboard");
        match studio_diagnostics::send_owner_message(self.owner, WM_APP_REFRESH_NOW) {
            Ok(()) => {
                crate::diagnose::log("refresh request delivered to monitor message queue");
                self.settings_error = None;
            }
            Err(error) => {
                crate::diagnose::log(&error);
                self.settings_error = Some(error);
            }
        }
    }

    pub(super) fn sync_poll_interval(&mut self, persisted_interval: u32) {
        // A local frequency edit wins; other dashboard edits must preserve a
        // newer interval selected from the widget's context menu.
        if self.settings.poll_interval_ms == self.synced_poll_interval_ms
            && self.settings.poll_interval_ms != persisted_interval
        {
            self.settings.poll_interval_ms = persisted_interval;
            // Drop the numeric control's old text buffer so losing focus cannot
            // commit the previous custom value over the menu selection.
            self.poll_interval_editor_generation =
                self.poll_interval_editor_generation.wrapping_add(1);
        }
        self.synced_poll_interval_ms = persisted_interval;
    }

    pub(super) fn save_settings(&mut self) {
        self.sync_poll_interval(app_settings::load_settings().poll_interval_ms);
        match app_settings::save_settings(&self.settings) {
            Ok(()) => {
                self.synced_poll_interval_ms = self.settings.poll_interval_ms;
                self.settings_error = None;
                self.notify_owner();
            }
            Err(error) => {
                self.settings_error = Some(format!(
                    "{}: {error}",
                    self.language().text("Unable to save settings")
                ));
            }
        }
    }

    pub(super) fn save_theme(&mut self) -> bool {
        let language = self.language();
        if self.theme_is_read_only() {
            self.theme_error = Some(
                language
                    .text("Built-in themes are read-only; duplicate the theme first")
                    .into(),
            );
            return false;
        }
        self.theme.prepare_runtime();
        let path = match theme_engine::save_theme(&self.theme) {
            Ok(path) => path,
            Err(error) => {
                self.theme_error = Some(format!(
                    "{}: {error}",
                    language.text("Unable to save theme")
                ));
                return false;
            }
        };
        self.theme_path = Some(path.clone());
        self.settings.active_theme_path = Some(path.to_string_lossy().into_owned());
        self.settings.custom_theme_enabled = true;
        self.dirty = false;
        if let Err(error) = app_settings::save_settings(&self.settings) {
            self.theme_error = Some(format!(
                "{}: {error}",
                language.text("The theme was saved, but it could not be activated")
            ));
            return false;
        }
        self.theme_error = None;
        self.notify_owner();
        true
    }

    pub(super) fn activate_theme(&mut self, path: PathBuf) {
        let language = self.language();
        let theme = match theme_engine::load_theme(&path) {
            Ok(theme) => theme,
            Err(error) => {
                self.theme_error = Some(format!(
                    "{}: {error}",
                    language.text("Unable to open theme")
                ));
                return;
            }
        };
        self.activate_loaded_theme(theme, path);
    }

    pub(super) fn activate_loaded_theme(&mut self, theme: ThemeDocument, path: PathBuf) {
        let language = self.language();
        self.theme = theme;
        self.theme_path = Some(path.clone());
        self.settings.active_theme_path = Some(path.to_string_lossy().into_owned());
        self.settings.custom_theme_enabled = true;
        self.selection = Selection::Surface(0);
        self.preview_dirty = true;
        self.dirty = false;
        self.theme_error = None;
        self.reset_history();
        self.expression_helper = None;
        self.action_helper = None;
        self.text_template_helper = None;
        self.asset_picker = None;
        self.preview_mouse_overrides.clear();
        self.preview_hover_target = None;
        self.preview_pending_click = None;
        if let Err(error) = app_settings::save_settings(&self.settings) {
            self.theme_error = Some(format!(
                "{}: {error}",
                language.text("The theme could not be activated")
            ));
        } else {
            self.notify_owner();
        }
    }

    pub(super) fn import_theme_path(&mut self, source: &Path) {
        let language = self.language();
        if self.dirty || (theme_package::is_theme_package(source) && self.context_menu_dirty) {
            self.theme_error = Some(
                language
                    .text("Save or discard changes before importing")
                    .into(),
            );
            return;
        }
        match theme_package::import_path(source) {
            Ok(imported) => {
                if let Some((menu, path)) = imported.context_menu {
                    self.context_menu = menu;
                    self.context_menu_path = Some(path);
                    self.context_menu_dirty = false;
                    self.context_menu_selection = None;
                    self.context_menu_action_helper = None;
                }
                if imported.imported_assets > 0 {
                    self.asset_thumbnails.clear();
                    self.asset_page_selected = None;
                }
                self.activate_loaded_theme(imported.theme, imported.theme_path);
            }
            Err(error) => {
                self.theme_error = Some(format!(
                    "{}: {error}",
                    language.text("Unable to import theme")
                ));
            }
        }
    }

    pub(super) fn import_theme_from_dialog(&mut self) {
        let language = self.language();
        let filter = format!(
            "{}\0*.zip;*.json\0{}\0*.zip\0{}\0*.json\0{}\0*.*\0\0",
            language.text("Theme Studio packages and themes"),
            language.text("Theme packages"),
            language.text("Theme files"),
            language.text("All files")
        );
        if let Some(path) = choose_file(
            self.owner,
            language.text("Import a theme or package"),
            &filter,
        ) {
            self.import_theme_path(&path);
        }
    }

    pub(super) fn export_theme_from_dialog(&mut self) {
        let language = self.language();
        let default_name = format!("{}.zip", safe_file_name(&self.theme.name, "theme"));
        let filter = format!(
            "{}\0*.zip\0{}\0*.*\0\0",
            language.text("Theme Studio packages"),
            language.text("All files")
        );
        let Some(path) = choose_save_file(
            self.owner,
            language.text("Export theme package"),
            &filter,
            &default_name,
            "zip",
        ) else {
            return;
        };
        match theme_package::export_package(&path, &self.theme, &self.context_menu) {
            Ok(_) => self.theme_error = None,
            Err(error) => {
                self.theme_error = Some(format!(
                    "{}: {error}",
                    language.text("Unable to export theme package")
                ));
            }
        }
    }

    pub(super) fn request_activate_theme(&mut self, path: PathBuf) {
        if self.theme_path.as_deref() == Some(path.as_path()) {
            return;
        }
        if self.pending_unsaved_action.is_some() {
            return;
        }
        if self.dirty {
            self.pending_unsaved_action = Some(PendingUnsavedAction::ActivateTheme(path));
        } else {
            self.activate_theme(path);
        }
    }

    pub(super) fn request_new_theme(&mut self) {
        if self.pending_unsaved_action.is_some() {
            return;
        }
        if self.dirty {
            self.pending_unsaved_action = Some(PendingUnsavedAction::NewTheme);
        } else {
            self.new_theme_name = Some(self.language().text("Untitled theme").into());
        }
    }

    pub(super) fn changed(&mut self) {
        if self.theme_is_read_only() {
            return;
        }
        self.theme.prepare_runtime();
        if serde_json::to_string(&self.theme).ok()
            != serde_json::to_string(&self.history_snapshot).ok()
        {
            self.undo_stack.push(self.history_snapshot.clone());
            if self.undo_stack.len() > 100 {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
            self.history_snapshot = self.theme.clone();
        }
        self.preview_mouse_overrides.clear();
        self.preview_hover_target = None;
        self.preview_pending_click = None;
        self.mark_theme_changed();
    }

    pub(super) fn mark_theme_changed(&mut self) {
        self.dirty = true;
        self.preview_dirty = true;
        if self.live_apply {
            self.save_theme();
        }
    }

    pub(super) fn undo_theme(&mut self) {
        if self.theme_is_read_only() {
            return;
        }
        let Some(previous) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(self.theme.clone());
        self.theme = previous;
        self.history_snapshot = self.theme.clone();
        self.normalize_selection();
        self.mark_theme_changed();
    }

    pub(super) fn redo_theme(&mut self) {
        if self.theme_is_read_only() {
            return;
        }
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(self.theme.clone());
        self.theme = next;
        self.history_snapshot = self.theme.clone();
        self.normalize_selection();
        self.mark_theme_changed();
    }

    pub(super) fn normalize_selection(&mut self) {
        if self.theme.surfaces.is_empty() {
            self.theme.prepare_runtime();
        }
        let surface = match self.selection {
            Selection::Surface(surface) | Selection::Object(surface, _) => {
                surface.min(self.theme.surfaces.len().saturating_sub(1))
            }
        };
        self.selection = match self.selection {
            Selection::Object(_, object_index)
                if object_index < self.theme.surfaces[surface].children.len() =>
            {
                Selection::Object(surface, object_index)
            }
            _ => Selection::Surface(surface),
        };
    }

    pub(super) fn reset_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.history_snapshot = self.theme.clone();
    }

    pub(super) fn duplicate_theme(&mut self, name: String) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let mut duplicate = self.theme.clone();
        duplicate.id = unique_id("theme");
        duplicate.name = name.to_string();
        duplicate.prepare_runtime();
        self.theme = duplicate;
        self.theme_path = None;
        self.selection = Selection::Surface(0);
        self.preview_dirty = true;
        self.dirty = true;
        self.expression_helper = None;
        self.action_helper = None;
        self.text_template_helper = None;
        self.asset_picker = None;
        self.preview_mouse_overrides.clear();
        self.preview_hover_target = None;
        self.preview_pending_click = None;
        self.reset_history();
        let _ = self.save_theme();
    }

    pub(super) fn new_theme(&mut self, name: String) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.theme = blank_theme(name);
        self.theme_path = None;
        self.selection = Selection::Surface(0);
        self.preview_dirty = true;
        self.dirty = true;
        self.expression_helper = None;
        self.action_helper = None;
        self.text_template_helper = None;
        self.asset_picker = None;
        self.preview_mouse_overrides.clear();
        self.preview_hover_target = None;
        self.preview_pending_click = None;
        self.reset_history();
        let _ = self.save_theme();
    }

    pub(super) fn continue_unsaved_action(
        &mut self,
        action: PendingUnsavedAction,
        context: &egui::Context,
    ) {
        match action {
            PendingUnsavedAction::Update { install } => self.send_update_action(install),
            PendingUnsavedAction::Close => {
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            PendingUnsavedAction::ActivateTheme(path) => self.activate_theme(path),
            PendingUnsavedAction::NewTheme => {
                self.new_theme_name = Some(self.language().text("Untitled theme").into());
            }
        }
    }

    pub(super) fn unsaved_changes_dialog(&mut self, context: &egui::Context) {
        let Some(action) = self.pending_unsaved_action.take() else {
            return;
        };
        let language = self.language();
        let mut choice = 0;
        let dialog_height = if self.theme_error.is_some() {
            124.0
        } else {
            92.0
        };
        crate::ui::components::modal::Modal::new(
            language.text("Save changes?"),
            "unsaved-theme-changes-dialog",
        )
        .width(420.0)
        .fixed_height(dialog_height)
        .show(context, |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} {}",
                    self.theme.name,
                    language.text("has been changed, what would you like to do?")
                ))
                .size(17.0),
            );
            if let Some(error) = &self.theme_error {
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::from_rgb(232, 119, 95), error);
            }
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(language.text("Save and continue")).clicked() {
                    choice = 1;
                }
                if ui.button(language.text("Discard changes")).clicked() {
                    choice = 2;
                }
                if ui.button(language.text("Cancel")).clicked() {
                    choice = 3;
                }
            });
        });

        match choice {
            1 if self.save_theme() => self.continue_unsaved_action(action, context),
            1 => self.pending_unsaved_action = Some(action),
            2 => {
                self.dirty = false;
                self.theme_error = None;
                self.continue_unsaved_action(action, context);
            }
            3 => {}
            _ => self.pending_unsaved_action = Some(action),
        }
    }

    pub(super) fn new_theme_dialog(&mut self, context: &egui::Context) {
        let Some(mut name) = self.new_theme_name.take() else {
            return;
        };
        let language = self.language();
        let mut action = 0;
        crate::ui::components::modal::Modal::new(language.text("New theme"), "new-theme-dialog")
            .show(context, |ui| {
                ui.label(language.text("Name the new theme"));
                let response = ui.add(
                    singleline_text_edit(&mut name)
                        .desired_width(ui.available_width())
                        .hint_text(language.text("Theme name")),
                );
                response.request_focus();
                let enter =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !name.trim().is_empty(),
                            egui::Button::new(language.text("Create")),
                        )
                        .clicked()
                        || enter
                    {
                        action = 1;
                    }
                    if ui.button(language.text("Cancel")).clicked() {
                        action = 2;
                    }
                });
            });
        match action {
            1 => self.new_theme(name),
            2 => {}
            _ => self.new_theme_name = Some(name),
        }
    }

    pub(super) fn duplicate_theme_dialog(&mut self, context: &egui::Context) {
        let Some(mut name) = self.duplicate_theme_name.take() else {
            return;
        };
        let language = self.language();
        let mut action = 0;
        crate::ui::components::modal::Modal::new(
            language.text("Duplicate theme"),
            "duplicate-theme-dialog",
        )
        .show(context, |ui| {
            ui.label(language.text("Name the editable copy"));
            let response = ui.add(
                singleline_text_edit(&mut name)
                    .desired_width(ui.available_width())
                    .hint_text(language.text("Theme name")),
            );
            response.request_focus();
            let enter =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !name.trim().is_empty(),
                        egui::Button::new(language.text("Create copy")),
                    )
                    .clicked()
                    || enter
                {
                    action = 1;
                }
                if ui.button(language.text("Cancel")).clicked() {
                    action = 2;
                }
            });
        });
        match action {
            1 => self.duplicate_theme(name),
            2 => {}
            _ => self.duplicate_theme_name = Some(name),
        }
    }

    pub(super) fn delete_theme(&mut self, deletion: ThemeDeletionConfirmation) {
        if self.theme_is_read_only() {
            return;
        }
        if self.theme_path.as_deref() != Some(deletion.path.as_path()) {
            return;
        }

        let fallback_path = match theme_engine::ensure_starter_theme() {
            Ok(path) => path,
            Err(_) => return,
        };
        let fallback_theme = match theme_engine::load_theme(&fallback_path) {
            Ok(theme) => theme,
            Err(_) => return,
        };
        if theme_engine::delete_theme(&deletion.path).is_err() {
            return;
        }

        self.theme = fallback_theme;
        self.theme_path = Some(fallback_path.clone());
        self.settings.active_theme_path = Some(fallback_path.to_string_lossy().into_owned());
        self.settings.custom_theme_enabled = true;
        self.selection = Selection::Surface(0);
        self.preview_dirty = true;
        self.dirty = false;
        self.expression_helper = None;
        self.action_helper = None;
        self.text_template_helper = None;
        self.asset_picker = None;
        self.preview_mouse_overrides.clear();
        self.preview_hover_target = None;
        self.preview_pending_click = None;
        self.reset_history();
        if let Err(error) = app_settings::save_settings(&self.settings) {
            self.theme_error = Some(format!(
                "The Classic theme was restored, but it could not be activated: {error}"
            ));
        } else {
            self.notify_owner();
        }
    }

    pub(super) fn delete_theme_dialog(&mut self, context: &egui::Context) {
        let Some(deletion) = self.delete_theme_confirmation.take() else {
            return;
        };
        let language = self.language();
        let mut action = 0;
        crate::ui::components::modal::Modal::new(
            language.text("Delete theme?"),
            "delete-theme-dialog",
        )
        .width(310.0)
        .fixed_height(110.0)
        .show(context, |ui| {
            ui.label(
                language
                    .text("Are you sure you want to delete {name}?")
                    .replace("{name}", &deletion.name),
            );
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(language.text("Delete theme"))
                                .color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(178, 48, 48)),
                    )
                    .clicked()
                {
                    action = 1;
                }
                if ui.button(language.text("Cancel")).clicked() {
                    action = 2;
                }
            });
        });
        match action {
            1 => self.delete_theme(deletion),
            2 => {}
            _ => self.delete_theme_confirmation = Some(deletion),
        }
    }

    pub(super) fn update_usage_cache(&mut self, mut cache: UsageCache) -> bool {
        cache.data.select_accounts(&self.settings.accounts);
        let poll_ok = cache.poll_ok && !cache.data.is_empty();
        let has_error = !cache.poll_ok
            || (cache.data.is_empty()
                && cache
                    .data
                    .accounts
                    .iter()
                    .any(|account| account.error.is_some()));
        let changed = self.usage.as_ref() != Some(&cache.data)
            || self.usage_poll_ok != poll_ok
            || self.usage_has_error != has_error;
        if changed {
            crate::diagnose::log_lazy(|| {
                format!(
                    "dashboard loaded usage cache: updated={} accounts={} poll_ok={poll_ok}",
                    cache.updated_unix,
                    cache.data.accounts.len()
                )
            });
            self.usage = Some(cache.data);
            self.usage_poll_ok = poll_ok;
            self.usage_has_error = has_error;
        }
        changed
    }

    pub(super) fn refresh_usage_cache(&mut self) {
        if self.last_cache_read.elapsed() < Duration::from_secs(1) {
            return;
        }
        let now = Instant::now();
        self.last_cache_read = now;
        self.update_status = crate::dashboard::read_update_status(self.owner);
        self.sync_poll_interval(app_settings::load_settings().poll_interval_ms);
        let usage_changed =
            app_settings::load_usage_cache().is_some_and(|cache| self.update_usage_cache(cache));
        let countdown_due = self
            .next_preview_countdown_refresh
            .is_some_and(|deadline| now >= deadline);
        let clock_interval = self.theme.current_time_refresh_interval();
        let clock_due = clock_interval.is_some()
            && self
                .next_preview_clock_refresh
                .is_none_or(|deadline| now >= deadline);
        if usage_changed || countdown_due || clock_due {
            self.preview_dirty = true;
            self.next_preview_countdown_refresh =
                preview_countdown_refresh_delay(self.usage.as_ref())
                    .and_then(|delay| now.checked_add(delay));
        }
        if clock_interval.is_none() {
            self.next_preview_clock_refresh = None;
        } else if clock_due {
            self.next_preview_clock_refresh = clock_interval
                .map(clock_refresh_delay)
                .and_then(|delay| now.checked_add(delay));
        }
    }

    pub(super) fn request_update_action(&mut self) {
        if self.update_status.is_busy() || self.pending_unsaved_action.is_some() {
            return;
        }
        let install = matches!(
            self.update_status,
            crate::dashboard::UpdateStatus::Available(_)
        );
        // The usual check can offer to install immediately. Resolve unsaved
        // edits first so they cannot keep the dashboard executable locked.
        if self.dirty {
            self.pending_unsaved_action = Some(PendingUnsavedAction::Update { install });
        } else {
            self.send_update_action(install);
        }
    }

    fn send_update_action(&mut self, install: bool) {
        // A refresh click must still check and prompt even if an automatic
        // check has found a release since the dashboard last read the status.
        let message = if install {
            native_interop::WM_APP_UPDATE_ACTION
        } else {
            native_interop::WM_APP_CHECK_FOR_UPDATES
        };
        match studio_diagnostics::send_owner_message(self.owner, message) {
            Ok(()) => {
                self.update_status = if install {
                    crate::dashboard::UpdateStatus::Applying
                } else {
                    crate::dashboard::UpdateStatus::Checking
                };
                self.last_cache_read = Instant::now();
            }
            Err(error) => self.theme_error = Some(error),
        }
    }

    pub(super) fn version_button(&mut self, ui: &mut egui::Ui) -> egui::Response {
        use crate::dashboard::UpdateStatus;
        let language = self.language();
        let (icon, tooltip) = match &self.update_status {
            UpdateStatus::Available(version) => (
                LucideIcon::Download,
                language
                    .text("Click to update to v{version}")
                    .replace("{version}", version),
            ),
            UpdateStatus::Checking => (
                LucideIcon::RefreshCw,
                language.strings().checking_for_updates.to_string(),
            ),
            UpdateStatus::Applying => (
                LucideIcon::Download,
                language.strings().applying_update.to_string(),
            ),
            UpdateStatus::Idle => (
                LucideIcon::RefreshCw,
                language.text("Check for updates").to_string(),
            ),
        };
        let response = ui
            .scope(|ui| {
                ui.add_enabled_ui(!self.update_status.is_busy(), |ui| {
                    let icon_id = ui.id().with("version-update-icon");
                    let version = format!("v{}", env!("CARGO_PKG_VERSION"));
                    let background = ui.painter().add(egui::Shape::Noop);
                    let button = egui::AtomLayout::new((
                        egui::RichText::new(&version).size(16.0).color(muted()),
                        egui::Atom::custom(icon_id, egui::vec2(12.0, 12.0)),
                    ))
                    .gap(4.0)
                    // The footer row is bottom-aligned; centre the contents
                    // independently so its extra height is not all above them.
                    .align2(egui::Align2::LEFT_CENTER)
                    .sense(egui::Sense::click())
                    .min_size(egui::vec2(0.0, CONTROL_HEIGHT))
                    .frame(egui::Frame::new().inner_margin(egui::Margin {
                        left: 5,
                        right: 5,
                        top: 2,
                        bottom: 4,
                    }))
                    .show(ui);
                    if button.response.hovered() || button.response.has_focus() {
                        ui.painter().set(
                            background,
                            egui::Shape::rect_filled(
                                button.response.rect,
                                4.0,
                                crate::ui::theme::menu_hover(),
                            ),
                        );
                    }
                    button.response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            &version,
                        )
                    });
                    if let Some(rect) = button.rect(icon_id) {
                        crate::ui::components::icon::paint_centered_icon(
                            ui,
                            rect.translate(egui::vec2(0.0, 1.0)),
                            icon,
                            12.0,
                            muted(),
                        );
                    }
                    button.response
                })
                .inner
            })
            .inner;
        let response = response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(&tooltip)
            .on_disabled_hover_text(&tooltip);
        if response.clicked() {
            self.request_update_action();
        }
        response
    }

    pub(super) fn shell(&mut self, ui: &mut egui::Ui) {
        const GITHUB_URL: &str = "https://github.com/CodeZeno/Claude-Code-Usage-Monitor";

        let language = self.language();
        let full_height = ui.available_height();
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(DEFAULT_MENU_WIDTH, full_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(DEFAULT_MENU_WIDTH);
                    ui.set_min_height(full_height);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, menu_surface());
                    egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: 8,
                            right: 8,
                            top: 20,
                            bottom: 4,
                        })
                        .show(ui, |ui| {
                            ui.set_width(DEFAULT_MENU_WIDTH - 16.0);
                            nav(
                                ui,
                                &mut self.page,
                                Page::Settings,
                                language.text("Settings"),
                            );
                            nav(
                                ui,
                                &mut self.page,
                                Page::Studio,
                                language.text("Theme Studio"),
                            );
                            nav(
                                ui,
                                &mut self.page,
                                Page::ContextMenus,
                                language.text("Context Menus"),
                            );
                            nav(ui, &mut self.page, Page::Assets, language.text("Assets"));
                            ui.separator();
                            nav(
                                ui,
                                &mut self.page,
                                Page::Diagnostics,
                                language.text("Diagnostics"),
                            );
                            ui.allocate_ui_with_layout(
                                ui.available_size(),
                                egui::Layout::bottom_up(egui::Align::Min),
                                |ui| {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(
                                            ui.available_width(),
                                            crate::ui::components::navigation::ITEM_HEIGHT,
                                        ),
                                        egui::Layout::left_to_right(egui::Align::Max),
                                        |ui| {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            crate::ui::components::navigation::github_link(
                                                ui, GITHUB_URL,
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Max),
                                                |ui| {
                                                    self.version_button(ui);
                                                },
                                            );
                                        },
                                    );
                                },
                            );
                        });
                },
            );
            ui.add(egui::Separator::default().spacing(2.0));
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), full_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_height(full_height);
                    if let Some(error) = self.theme_error.clone() {
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgb(70, 34, 34))
                            .corner_radius(egui::CornerRadius::same(5))
                            .inner_margin(egui::Margin::symmetric(10, 7))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.colored_label(egui::Color32::from_rgb(255, 190, 178), error);
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add(lucide_icon_button(LucideIcon::X))
                                                .on_hover_text(language.text("Dismiss error"))
                                                .clicked()
                                            {
                                                self.theme_error = None;
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(8.0);
                    }
                    match self.page {
                        Page::Settings => self.settings_page(ui),
                        Page::Studio => self.studio_page(ui),
                        Page::ContextMenus => self.context_menus_page(ui),
                        Page::Assets => self.assets_page(ui),
                        Page::Diagnostics => self.diagnostics_page(ui),
                    }
                },
            );
        });
    }

    pub(super) fn page_header(ui: &mut egui::Ui, title: &str, detail: &str) {
        ui.add_space(12.0);
        ui.label(egui::RichText::new(title).size(25.0).strong());
        ui.label(egui::RichText::new(detail).color(muted()));
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
    }
}
