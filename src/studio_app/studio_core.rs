use super::*;

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
    }

    pub(super) fn theme_runtime_for_surface(&self, surface_index: usize) -> ThemeRuntime {
        crate::window::theme_runtime_for_surface(&self.theme, surface_index, self.theme_runtime())
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
        let usage_cache = app_settings::load_usage_cache();
        let usage_poll_ok = usage_cache.as_ref().is_some_and(|cache| cache.poll_ok);
        let usage_has_error = usage_cache.as_ref().is_some_and(|cache| !cache.poll_ok);
        let usage = usage_cache.map(|cache| cache.data);
        let next_preview_countdown_refresh = preview_countdown_refresh_delay(usage.as_ref())
            .and_then(|delay| Instant::now().checked_add(delay));
        Self {
            owner,
            page: initial_page,
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
                    HWND(self.owner as *mut _),
                    WM_APP_SETTINGS_UPDATED,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
    }

    pub(super) fn post_owner(&self, message: u32) {
        if self.owner != 0 {
            unsafe {
                let _ = PostMessageW(HWND(self.owner as *mut _), message, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub(super) fn save_settings(&mut self) {
        match app_settings::save_settings(&self.settings) {
            Ok(()) => {
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

    pub(super) fn update_usage_cache(&mut self, cache: UsageCache) -> bool {
        let poll_ok = cache.poll_ok;
        let has_error = !poll_ok;
        let changed = self.usage.as_ref() != Some(&cache.data)
            || self.usage_poll_ok != poll_ok
            || self.usage_has_error != has_error;
        if changed {
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
        let usage_changed =
            app_settings::load_usage_cache().is_some_and(|cache| self.update_usage_cache(cache));
        let countdown_due = self
            .next_preview_countdown_refresh
            .is_some_and(|deadline| now >= deadline);
        if usage_changed || countdown_due {
            self.preview_dirty = true;
            self.next_preview_countdown_refresh =
                preview_countdown_refresh_delay(self.usage.as_ref())
                    .and_then(|delay| now.checked_add(delay));
        }
    }

    pub(super) fn shell(&mut self, ui: &mut egui::Ui) {
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
                            bottom: 0,
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
