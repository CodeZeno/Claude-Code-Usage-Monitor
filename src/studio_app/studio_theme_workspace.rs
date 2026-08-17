use super::*;

impl StudioApp {
    pub(super) fn studio_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        let themes = available_themes(self.theme_path.as_deref(), &self.theme);
        let mut requested_theme = None;
        let read_only = self.theme_is_read_only();
        let can_delete = !read_only
            && self
                .theme_path
                .as_deref()
                .is_some_and(theme_engine::is_managed_theme_path);
        let mut save_after_enabling_live_apply = false;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(language.text("Theme")).color(muted()));
            Dropdown::from_id_salt("studio-theme")
                .width(240.0)
                .selected_text(if read_only {
                    format!("{} ({})", self.theme.name, language.text("built-in"))
                } else {
                    self.theme.name.clone()
                })
                .show_ui(ui, |ui| {
                    for theme in &themes {
                        let selected = self.theme_path.as_deref() == Some(theme.path.as_path());
                        if dropdown_selectable_label(ui, selected, &theme.label).clicked() {
                            requested_theme = Some(theme.path.clone());
                        }
                    }
                });
            if ui
                .add(lucide_icon_button(LucideIcon::FilePlus))
                .on_hover_text(language.text("New"))
                .clicked()
            {
                self.request_new_theme();
            }
            if ui
                .add(lucide_icon_button(LucideIcon::Copy))
                .on_hover_text(language.text("Duplicate..."))
                .clicked()
            {
                self.duplicate_theme_name =
                    Some(format!("{} ({})", self.theme.name, language.text("copy")));
            }
            if ui
                .add_enabled(
                    !read_only && self.dirty,
                    lucide_icon_button(LucideIcon::Save),
                )
                .on_hover_text(language.text("Save"))
                .clicked()
            {
                let _ = self.save_theme();
            }
            let delete = ui
                .add_enabled(can_delete, lucide_icon_button(LucideIcon::Trash))
                .on_hover_text(language.text("Delete..."))
                .on_disabled_hover_text(if read_only {
                    language.text("Built-in themes cannot be deleted")
                } else {
                    language.text("Only themes saved in Theme Studio can be deleted here")
                });
            if delete.clicked() {
                if let Some(path) = self.theme_path.clone() {
                    self.delete_theme_confirmation = Some(ThemeDeletionConfirmation {
                        path,
                        name: self.theme.name.clone(),
                    });
                }
            }
            if ui
                .add(lucide_icon_button(LucideIcon::Upload))
                .on_hover_text(language.text("Import..."))
                .clicked()
            {
                self.import_theme_from_dialog();
            }
            if ui
                .add(lucide_icon_button(LucideIcon::Download))
                .on_hover_text(language.text("Export..."))
                .clicked()
            {
                self.export_theme_from_dialog();
            }
            if ui
                .add_enabled(
                    !read_only && !self.undo_stack.is_empty(),
                    lucide_icon_button(LucideIcon::Undo),
                )
                .on_hover_text(language.text("Undo the last theme change (Ctrl+Z)"))
                .clicked()
            {
                self.undo_theme();
            }
            if ui
                .add_enabled(
                    !read_only && !self.redo_stack.is_empty(),
                    lucide_icon_button(LucideIcon::Redo),
                )
                .on_hover_text(language.text("Restore the undone change (Ctrl+Y)"))
                .clicked()
            {
                self.redo_theme();
            }
            if !read_only {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let changed = Toggle::new(&mut self.live_apply)
                        .labels(language.text("Enabled"), language.text("Disabled"))
                        .show(ui)
                        .changed();
                    save_after_enabling_live_apply = changed && self.live_apply && self.dirty;
                    ui.label(language.text("Live apply"));
                });
            }
        });
        if save_after_enabling_live_apply {
            let _ = self.save_theme();
        }
        if let Some(path) = requested_theme {
            self.request_activate_theme(path);
        }
        self.new_theme_dialog(ui.ctx());
        self.duplicate_theme_dialog(ui.ctx());
        self.delete_theme_dialog(ui.ctx());
        let read_only = self.theme_is_read_only();
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        if self.text_template_helper.is_some() {
            self.text_template_helper_ui(ui);
            return;
        }
        if self.action_helper.is_some() {
            self.action_helper_ui(ui);
            return;
        }
        if self.expression_helper.is_some() {
            self.expression_helper_ui(ui);
            return;
        }
        if self.asset_picker.is_some() {
            self.asset_picker_ui(ui);
            return;
        }

        let row_height = (ui.available_height() - 8.0).max(1.0);
        let total_width = ui.available_width();
        const SPLITTER_WIDTH: f32 = 8.0;
        const MIN_SCENE_WIDTH: f32 = 170.0;
        const MIN_CANVAS_WIDTH: f32 = 240.0;
        const MIN_INSPECTOR_WIDTH: f32 = 320.0;
        let usable = (total_width - SPLITTER_WIDTH * 2.0).max(1.0);
        let (scene_width, canvas_width) =
            if usable >= MIN_SCENE_WIDTH + MIN_CANVAS_WIDTH + MIN_INSPECTOR_WIDTH {
                self.scene_width = self.scene_width.clamp(
                    MIN_SCENE_WIDTH,
                    usable - MIN_CANVAS_WIDTH - MIN_INSPECTOR_WIDTH,
                );
                self.inspector_width = self.inspector_width.clamp(
                    MIN_INSPECTOR_WIDTH,
                    usable - MIN_CANVAS_WIDTH - self.scene_width,
                );
                (
                    self.scene_width,
                    usable - self.scene_width - self.inspector_width,
                )
            } else {
                // The native minimum normally keeps us above this point, but
                // proportional fallback guarantees no pane can force overflow.
                let scene = usable * 0.23;
                let inspector = usable * 0.39;
                (scene, usable - scene - inspector)
            };
        let mut scene_delta = 0.0;
        let mut inspector_delta = 0.0;
        let (workspace_rect, _) =
            ui.allocate_exact_size(egui::vec2(total_width, row_height), egui::Sense::hover());
        let scene_rect =
            egui::Rect::from_min_size(workspace_rect.min, egui::vec2(scene_width, row_height));
        let left_splitter_rect = egui::Rect::from_min_size(
            egui::pos2(scene_rect.right(), workspace_rect.top()),
            egui::vec2(SPLITTER_WIDTH, row_height),
        );
        let canvas_rect = egui::Rect::from_min_size(
            egui::pos2(left_splitter_rect.right(), workspace_rect.top()),
            egui::vec2(canvas_width, row_height),
        );
        let right_splitter_rect = egui::Rect::from_min_size(
            egui::pos2(canvas_rect.right(), workspace_rect.top()),
            egui::vec2(SPLITTER_WIDTH, row_height),
        );
        let inspector_rect = egui::Rect::from_min_max(
            egui::pos2(right_splitter_rect.right(), workspace_rect.top()),
            workspace_rect.right_bottom(),
        );

        let mut scene_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("scene-pane")
                .max_rect(scene_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        scene_ui.set_clip_rect(scene_rect);
        studio_region(&mut scene_ui, scene_width, row_height, |ui| {
            self.scene_tree(ui, read_only)
        });

        let left_splitter = workspace_splitter(ui, left_splitter_rect, "left-splitter");
        if left_splitter.dragged() {
            scene_delta = ui.input(|input| input.pointer.delta().x);
        }

        let mut canvas_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("canvas-pane")
                .max_rect(canvas_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        canvas_ui.set_clip_rect(canvas_rect);
        studio_region(&mut canvas_ui, canvas_width, row_height, |ui| {
            self.canvas_preview(ui)
        });

        let right_splitter = workspace_splitter(ui, right_splitter_rect, "right-splitter");
        if right_splitter.dragged() {
            inspector_delta = -ui.input(|input| input.pointer.delta().x);
        }

        let mut inspector_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("inspector-pane")
                .max_rect(inspector_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        inspector_ui.set_clip_rect(inspector_rect);
        studio_region(
            &mut inspector_ui,
            inspector_rect.width(),
            row_height,
            |ui| self.inspector(ui, read_only),
        );
        self.scene_width += scene_delta;
        self.inspector_width += inspector_delta;
    }

    pub(super) fn open_asset_picker(&mut self, target: Selection, background: &LayerBackground) {
        let selected_path = match background {
            LayerBackground::Image { path, .. } if !path.is_empty() => Some(path.clone()),
            _ => None,
        };
        self.asset_error = None;
        self.asset_picker = Some(AssetPickerState {
            target,
            selected_path,
            filter: String::new(),
        });
    }

    pub(super) fn import_asset_from_dialog(&mut self) -> Option<theme_engine::ManagedAsset> {
        let language = self.language();
        let filter = format!(
            "{}\0*.png;*.jpg;*.jpeg;*.gif;*.bmp;*.webp\0{}\0*.*\0\0",
            language.text("Images"),
            language.text("All files")
        );
        let source = choose_file(
            self.owner,
            language.text("Add an image to the asset library"),
            &filter,
        )?;
        self.import_asset_path(&source)
    }

    pub(super) fn import_asset_path(
        &mut self,
        source: &Path,
    ) -> Option<theme_engine::ManagedAsset> {
        match theme_engine::import_asset(source) {
            Ok(asset) => {
                self.asset_thumbnails.remove(&asset.relative_path);
                self.asset_error = None;
                Some(asset)
            }
            Err(error) => {
                self.asset_error = Some(error);
                None
            }
        }
    }

    pub(super) fn handle_dropped_files(&mut self, context: &egui::Context) {
        let dropped = context.input(|input| input.raw.dropped_files.clone());
        for file in dropped {
            let Some(path) = file.path else {
                continue;
            };
            if theme_package::is_theme_package(&path) {
                self.import_theme_path(&path);
            } else if self.page == Page::Assets {
                if let Some(asset) = self.import_asset_path(&path) {
                    self.asset_page_selected = Some(asset.relative_path);
                }
            }
        }
    }

    pub(super) fn ensure_asset_thumbnails(
        &mut self,
        context: &egui::Context,
        assets: &[theme_engine::ManagedAsset],
    ) {
        for asset in assets {
            if self.asset_thumbnails.contains_key(&asset.relative_path) {
                continue;
            }
            let Ok(source) = image::open(&asset.absolute_path) else {
                continue;
            };
            let thumbnail = source.thumbnail(320, 200).to_rgba8();
            let size = [thumbnail.width() as usize, thumbnail.height() as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, thumbnail.as_raw());
            let texture = context.load_texture(
                format!("asset-thumbnail:{}", asset.relative_path),
                image,
                egui::TextureOptions::LINEAR,
            );
            self.asset_thumbnails
                .insert(asset.relative_path.clone(), texture);
        }
    }

    pub(super) fn apply_asset_to_selection(&mut self, target: Selection, path: String) {
        let changed = match target {
            Selection::Surface(index) => {
                self.theme.surfaces.get_mut(index).is_some_and(|surface| {
                    match &mut surface.background {
                        LayerBackground::Image { path: current, .. } => {
                            if *current == path {
                                false
                            } else {
                                *current = path;
                                true
                            }
                        }
                        _ => false,
                    }
                })
            }
            Selection::Object(surface_index, object_index) => self
                .theme
                .surfaces
                .get_mut(surface_index)
                .and_then(|surface| surface.children.get_mut(object_index))
                .is_some_and(|object| match &mut object.background {
                    LayerBackground::Image { path: current, .. } => {
                        if *current == path {
                            false
                        } else {
                            *current = path;
                            true
                        }
                    }
                    _ => false,
                }),
        };
        if changed {
            self.selection = target;
            self.changed();
        }
    }

    pub(super) fn asset_picker_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut picker) = self.asset_picker.take() else {
            return;
        };
        let language = self.language();
        let assets = match theme_engine::list_assets() {
            Ok(assets) => assets,
            Err(error) => {
                self.asset_error = Some(error);
                Vec::new()
            }
        };
        self.ensure_asset_thumbnails(ui.ctx(), &assets);
        let mut close = false;
        let mut add_image = false;
        let mut apply_path = None;
        let width = ui.available_width();
        let height = ui.available_height();

        egui::Frame::new()
            .fill(egui::Color32::from_rgb(21, 22, 26))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 50, 57)))
            .corner_radius(egui::CornerRadius::same(7))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_width((width - 28.0).max(1.0));
                ui.set_min_height((height - 28.0).max(1.0));
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(language.text("Asset library"))
                                .size(20.0)
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new(
                                language
                                    .text("Choose a managed image or add one from your computer."),
                            )
                            .color(muted()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if ui
                            .add(lucide_icon_button(LucideIcon::X))
                            .on_hover_text(language.text("Close asset library"))
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add(
                        singleline_text_edit(&mut picker.filter)
                            .desired_width(300.0)
                            .hint_text(language.text("Search assets...")),
                    );
                    if ui
                        .add(lucide_labeled_button(
                            LucideIcon::ImagePlus,
                            language.text("Add image"),
                        ))
                        .clicked()
                    {
                        add_image = true;
                    }
                });
                if let Some(error) = &self.asset_error {
                    ui.add_space(8.0);
                    ui.colored_label(egui::Color32::from_rgb(232, 119, 95), error);
                }
                ui.add_space(12.0);
                let grid_height = (ui.available_height() - 52.0).max(120.0);
                egui::ScrollArea::vertical()
                    .id_salt("asset-picker-grid")
                    .auto_shrink([false, false])
                    .max_height(grid_height)
                    .show(ui, |ui| {
                        if let Some(path) = asset_grid(
                            ui,
                            &assets,
                            &self.asset_thumbnails,
                            &picker.filter,
                            &mut picker.selected_path,
                            &self.theme,
                            language,
                        ) {
                            apply_path = Some(path);
                        }
                    });
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            picker.selected_path.is_some(),
                            lucide_labeled_button(LucideIcon::Check, language.text("Use asset")),
                        )
                        .clicked()
                    {
                        apply_path = picker.selected_path.clone();
                    }
                });
            });

        if add_image {
            if let Some(asset) = self.import_asset_from_dialog() {
                picker.selected_path = Some(asset.relative_path);
            }
        }
        if let Some(path) = apply_path {
            self.apply_asset_to_selection(picker.target, path);
        } else if !close {
            self.asset_picker = Some(picker);
        }
    }

    pub(super) fn text_template_helper_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut helper) = self.text_template_helper.take() else {
            return;
        };
        let language = self.language();
        let context = match &helper.target {
            TextTemplateHelperTarget::Theme(selection) => self.expression_context(*selection),
            TextTemplateHelperTarget::ContextMenu(_) => DataContext::from_usage_with_runtime(
                self.usage.as_ref(),
                &Canvas::default(),
                self.selected_theme_runtime(),
            ),
        };
        let action = show_text_helper(
            ui,
            &mut helper.editor,
            language,
            |draft| theme_engine::validate_template(draft, &context),
            |draft| theme_engine::format_template(draft, &context),
            |ui, editor, panel_height| {
                let panel_gap = ui.spacing().item_spacing.x;
                let usable_width = (ui.available_width() - panel_gap * 2.0).max(1.0);
                let values_width = usable_width * 0.45;
                let formats_width = usable_width * 0.31;
                let guide_width = (usable_width - values_width - formats_width).max(1.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), panel_height),
                    egui::Layout::left_to_right(egui::Align::Min),
                    |ui| {
                        text_template_values_panel(
                            ui,
                            egui::vec2(values_width, panel_height),
                            &context,
                            &mut editor.value_filter,
                            &mut editor.selected_value,
                            &mut editor.selected_format,
                            language,
                        );
                        text_template_formats_panel(
                            ui,
                            egui::vec2(formats_width, panel_height),
                            &context,
                            editor.selected_value,
                            &mut editor.selected_format,
                            &mut editor.draft,
                            language,
                        );
                        text_template_guide_panel(ui, guide_width, panel_height, language);
                    },
                );
            },
        );

        match action {
            TextHelperAction::Close => {}
            TextHelperAction::Apply => match helper.target {
                TextTemplateHelperTarget::Theme(selection) => {
                    let applied = match selection {
                        Selection::Surface(surface_index) => self
                            .theme
                            .surfaces
                            .get_mut(surface_index)
                            .is_some_and(|surface| {
                                set_text_template(&mut surface.content, helper.editor.draft.clone())
                            }),
                        Selection::Object(surface_index, object_index) => self
                            .theme
                            .surfaces
                            .get_mut(surface_index)
                            .and_then(|surface| surface.children.get_mut(object_index))
                            .is_some_and(|object| {
                                set_text_template(&mut object.content, helper.editor.draft.clone())
                            }),
                    };
                    if applied {
                        self.selection = selection;
                        self.changed();
                    }
                }
                TextTemplateHelperTarget::ContextMenu(path) => {
                    if let Some(item) = context_menu_item_mut(&mut self.context_menu.items, &path) {
                        item.label = helper.editor.draft;
                        self.context_menu_selection = Some(path);
                        self.context_menu_dirty = true;
                    }
                }
            },
            TextHelperAction::Continue => self.text_template_helper = Some(helper),
        }
    }

    pub(super) fn expression_helper_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut helper) = self.expression_helper.take() else {
            return;
        };
        let language = self.language();
        let context = self.expression_context(helper.selection);
        let action = show_expression_helper(
            ui,
            &mut helper.editor,
            language,
            |draft| {
                theme_engine::evaluate(draft, &context).and_then(|value| {
                    value
                        .is_finite()
                        .then(|| format_expression_result(helper.field, value, language))
                        .ok_or_else(|| language.text("Expression result is not finite").to_string())
                })
            },
            |ui, editor, panel_height| {
                let panel_gap = ui.spacing().item_spacing.x;
                let panel_width = ((ui.available_width() - panel_gap * 2.0) / 3.0).max(1.0);
                ui.horizontal(|ui| {
                    expression_variables_panel(
                        ui,
                        panel_width,
                        panel_height,
                        &context,
                        &mut editor.variable_filter,
                        &mut editor.draft,
                        language,
                    );
                    expression_functions_panel(
                        ui,
                        panel_width,
                        panel_height,
                        &mut editor.function_filter,
                        &mut editor.draft,
                        language,
                    );
                    expression_operators_panel(
                        ui,
                        panel_width,
                        panel_height,
                        &mut editor.draft,
                        language,
                    );
                });
            },
        );

        match action {
            ExpressionHelperAction::Close => {}
            ExpressionHelperAction::Apply => {
                let expression = Expression(helper.editor.draft);
                let applied = match helper.selection {
                    Selection::Surface(surface_index) => self
                        .theme
                        .surfaces
                        .get_mut(surface_index)
                        .map(|surface| match helper.field {
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
                        })
                        .unwrap_or(false),
                    Selection::Object(surface_index, object_index) => self
                        .theme
                        .surfaces
                        .get_mut(surface_index)
                        .and_then(|surface| surface.children.get_mut(object_index))
                        .is_some_and(|object| {
                            set_object_expression(object, helper.field, expression)
                        }),
                };
                if applied {
                    self.selection = helper.selection;
                    self.changed();
                }
            }
            ExpressionHelperAction::Continue => self.expression_helper = Some(helper),
        }
    }

    pub(super) fn action_helper_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut helper) = self.action_helper.take() else {
            return;
        };
        let language = self.language();
        let surface_index = match helper.selection {
            Selection::Surface(surface) | Selection::Object(surface, _) => surface,
        };
        let Some(surface) = self.theme.surfaces.get(surface_index) else {
            return;
        };
        let self_id = match helper.selection {
            Selection::Surface(_) => surface.id.clone(),
            Selection::Object(_, object) => surface
                .children
                .get(object)
                .map(|object| object.id.clone())
                .unwrap_or_else(|| surface.id.clone()),
        };
        let targets = self
            .theme
            .surfaces
            .iter()
            .flat_map(|root| {
                std::iter::once((root.id.clone(), root.name.clone())).chain(
                    root.children
                        .iter()
                        .map(|object| (object.id.clone(), object.name.clone())),
                )
            })
            .collect::<Vec<_>>();
        let context = self.expression_context(helper.selection);
        let mut target = helper.target.clone();
        let mut property = helper.property;
        let mut value = helper.value.clone();
        let context_menus = context_menu::list_context_menus().unwrap_or_default();
        let mut context_menu_reference = helper.context_menu_reference.clone();
        let action = show_action_helper(
            ui,
            &mut helper.editor,
            language,
            "Build safe mouse actions that affect layers at runtime.",
            |draft| {
                let errors = theme_engine::validate_mouse_action_script(
                    draft,
                    &self.theme,
                    surface_index,
                    &self_id,
                    &context,
                );
                if errors.is_empty() {
                    let count = theme_engine::parse_mouse_actions(draft)?.len();
                    Ok(format!("{count} {}", language.text("actions")))
                } else {
                    Err(errors.join("\n"))
                }
            },
            |ui, editor, panel_height| {
                action_reference_panels(
                    ui,
                    panel_height,
                    &targets,
                    &self_id,
                    &mut target,
                    &mut property,
                    &mut value,
                    &context_menus,
                    &mut context_menu_reference,
                    &mut editor.draft,
                    language,
                );
            },
        );

        match action {
            ExpressionHelperAction::Close => {}
            ExpressionHelperAction::Apply => {
                let draft = helper.editor.draft;
                let applied = match helper.selection {
                    Selection::Surface(index) => self.theme.surfaces.get_mut(index),
                    Selection::Object(surface, object) => self
                        .theme
                        .surfaces
                        .get_mut(surface)
                        .and_then(|surface| surface.children.get_mut(object)),
                }
                .is_some_and(|object| {
                    let events = object.mouse_events.get_or_insert_with(MouseEvents::default);
                    *events.handler_mut(helper.field.kind()) = draft;
                    true
                });
                if applied {
                    self.selection = helper.selection;
                    self.changed();
                }
            }
            ExpressionHelperAction::Continue => {
                helper.target = target;
                helper.property = property;
                helper.value = value;
                helper.context_menu_reference = context_menu_reference;
                self.action_helper = Some(helper);
            }
        }
    }

    pub(super) fn context_menu_action_helper_ui(&mut self, ui: &mut egui::Ui) {
        let Some(mut helper) = self.context_menu_action_helper.take() else {
            return;
        };
        let language = self.language();
        let layer_targets = self
            .theme
            .surfaces
            .iter()
            .flat_map(|root| {
                std::iter::once((root.id.clone(), root.name.clone())).chain(
                    root.children
                        .iter()
                        .map(|object| (object.id.clone(), object.name.clone())),
                )
            })
            .collect::<Vec<_>>();
        let action = show_action_helper(
            ui,
            &mut helper.editor,
            language,
            "Choose one action for this context menu item.",
            |draft| {
                parse_context_menu_action_script(draft)?;
                Ok(language.text("One menu action").into())
            },
            |ui, editor, panel_height| {
                context_menu_action_reference_panels(
                    ui,
                    panel_height,
                    &layer_targets,
                    &mut helper.target,
                    &mut helper.property,
                    &mut helper.value,
                    &mut editor.draft,
                    language,
                );
            },
        );

        match action {
            ExpressionHelperAction::Close => {}
            ExpressionHelperAction::Apply => {
                match parse_context_menu_action_script(&helper.editor.draft) {
                    Ok(action) => {
                        if let Some(item) =
                            context_menu_item_mut(&mut self.context_menu.items, &helper.path)
                        {
                            item.kind = ContextMenuItemKind::Action { action };
                            self.context_menu_selection = Some(helper.path);
                            self.context_menu_dirty = true;
                        }
                    }
                    Err(error) => self.theme_error = Some(error),
                }
            }
            ExpressionHelperAction::Continue => self.context_menu_action_helper = Some(helper),
        }
    }

    pub(super) fn expression_context(&self, selection: Selection) -> DataContext {
        let surface_index = match selection {
            Selection::Surface(surface) | Selection::Object(surface, _) => surface,
        };
        let Some(surface) = self.theme.surfaces.get(surface_index) else {
            return DataContext::default();
        };
        let runtime = self.theme_runtime_for_surface(surface_index);
        let (width, height) = theme_engine::resolve_surface_size(
            &self.theme,
            surface_index,
            self.usage.as_ref(),
            runtime,
        );
        let canvas = Canvas {
            width,
            width_expression: Some(surface.width.clone()),
            height,
            height_expression: Some(surface.height.clone()),
            background: match &surface.background {
                theme_engine::LayerBackground::Colour { colour } => colour.clone(),
                theme_engine::LayerBackground::None
                | theme_engine::LayerBackground::Gradient { .. }
                | theme_engine::LayerBackground::Image { .. } => Paint::default(),
            },
        };
        DataContext::from_usage_with_runtime(self.usage.as_ref(), &canvas, runtime)
    }

    pub(super) fn scene_tree(&mut self, ui: &mut egui::Ui, read_only: bool) {
        let language = self.language();
        self.hovered_scene_item = None;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(language.text("Layers"))
                    .size(16.0)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        !read_only,
                        lucide_labeled_button(LucideIcon::Plus, language.text("Add layer")),
                    )
                    .clicked()
                {
                    self.add_layer();
                }
            });
        });
        ui.add_space(6.0);
        // Keep enough fixed space for the selected-object actions so they are
        // never pushed below the pane when the window is short.
        let footer_height = CONTROL_HEIGHT + ui.spacing().item_spacing.y * 3.0 + 1.0;
        let tree_height = (ui.available_height() - footer_height).max(80.0);
        let mut pending_drop = None;
        egui::ScrollArea::vertical()
            .id_salt("scene_tree")
            .auto_shrink([false, false])
            .content_margin(egui::Margin {
                right: 12,
                ..egui::Margin::ZERO
            })
            .max_height(tree_height)
            .show(ui, |ui| {
                let surface_count = self.theme.surfaces.len();
                for surface_index in 0..surface_count {
                    let selected = self.selection == Selection::Surface(surface_index);
                    let mut surface = self.theme.surfaces[surface_index].clone();
                    let (preview_width, preview_height) = theme_engine::resolve_surface_size(
                        &self.theme,
                        surface_index,
                        self.usage.as_ref(),
                        self.theme_runtime_for_surface(surface_index),
                    );
                    let preview_size = egui::vec2(preview_width as f32, preview_height as f32);
                    let roots: Vec<usize> = surface
                        .children
                        .iter()
                        .enumerate()
                        .filter(|(_, object)| object.parent.is_none())
                        .map(|(index, _)| index)
                        .collect();
                    let id = ui.make_persistent_id(("scene-surface", surface.id.clone()));
                    let selection = Selection::Surface(surface_index);
                    paint_scene_row_background(ui, selected);
                    if roots.is_empty() {
                        let responses = scene_row_style(ui, selected, |ui| {
                            scene_row_contents(
                                ui,
                                &mut surface,
                                selection,
                                preview_size,
                                None,
                                !read_only,
                                language,
                            )
                        });
                        if responses.name_changed {
                            self.theme.surfaces[surface_index].name = surface.name.clone();
                            self.changed();
                        }
                        if responses.item.hovered() || responses.drag_handle.hovered() {
                            self.hovered_scene_item = Some(selection);
                        }
                        if responses.item.clicked() || responses.drag_handle.drag_started() {
                            self.selection = selection;
                        }
                        if !read_only && pending_drop.is_none() {
                            pending_drop = scene_drop_from_response(
                                ui,
                                &responses.item,
                                selection,
                                surface_index,
                            );
                        }
                    } else {
                        let body_visuals = ui.visuals().clone();
                        let state =
                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                ui.ctx(),
                                id,
                                true,
                            );
                        let is_open = state.is_open();
                        let responses = scene_row_style(ui, selected, |ui| {
                            scene_row_contents(
                                ui,
                                &mut surface,
                                selection,
                                preview_size,
                                Some(is_open),
                                !read_only,
                                language,
                            )
                        });
                        if responses.name_changed {
                            self.theme.surfaces[surface_index].name = surface.name.clone();
                            self.changed();
                        }
                        let row_response = responses.item.clone();
                        if row_response.hovered() || responses.drag_handle.hovered() {
                            self.hovered_scene_item = Some(selection);
                        }
                        if row_response.clicked() || responses.drag_handle.drag_started() {
                            self.selection = selection;
                        }
                        if responses.expand_button.clicked() {
                            toggle_scene_node(ui, id, true);
                        }
                        if !read_only && pending_drop.is_none() {
                            pending_drop = scene_drop_from_response(
                                ui,
                                &row_response,
                                selection,
                                surface_index,
                            );
                        }
                        let mut state =
                            egui::collapsing_header::CollapsingState::load_with_default_open(
                                ui.ctx(),
                                id,
                                true,
                            );
                        if state.is_open() {
                            state.show_body_indented(&responses.expand_button, ui, |ui| {
                                *ui.visuals_mut() = body_visuals;
                                for index in roots {
                                    if pending_drop.is_none() {
                                        pending_drop = self.object_tree_row(
                                            ui,
                                            surface_index,
                                            index,
                                            1,
                                            read_only,
                                        );
                                    } else {
                                        self.object_tree_row(
                                            ui,
                                            surface_index,
                                            index,
                                            1,
                                            read_only,
                                        );
                                    }
                                }
                            });
                        }
                    }
                    ui.add_space(8.0);
                }
            });
        if !read_only {
            if let Some((source, target)) = pending_drop {
                if self.apply_scene_drop(source, target) {
                    self.changed();
                }
            }
        }
        ui.separator();
        let footer_width = (ui.clip_rect().right() - ui.cursor().min.x).max(1.0);
        let button_width = ((footer_width - ui.spacing().item_spacing.x) / 2.0).max(1.0);
        let can_delete =
            !matches!(self.selection, Selection::Surface(_)) || self.theme.surfaces.len() > 1;
        let mut action = 0;
        ui.horizontal(|ui| {
            if ui
                .add_enabled_ui(!read_only, |ui| {
                    ui.add_sized(
                        [button_width, CONTROL_HEIGHT],
                        lucide_labeled_button(LucideIcon::Copy, language.text("Duplicate")),
                    )
                })
                .inner
                .clicked()
            {
                action = 1;
            }
            if ui
                .add_enabled_ui(!read_only && can_delete, |ui| {
                    ui.add_sized(
                        [button_width, CONTROL_HEIGHT],
                        lucide_labeled_button(LucideIcon::Trash, language.text("Delete")),
                    )
                })
                .inner
                .clicked()
            {
                action = 2;
            }
        });
        if action == 1 {
            if self.duplicate_selection() {
                self.changed();
            }
        } else if action == 2 && self.delete_selection() {
            self.changed();
        }
    }

    pub(super) fn object_tree_row(
        &mut self,
        ui: &mut egui::Ui,
        surface: usize,
        index: usize,
        depth: usize,
        read_only: bool,
    ) -> Option<(Selection, SceneDropTarget)> {
        let language = self.language();
        let mut object = self
            .theme
            .surfaces
            .get(surface)
            .and_then(|s| s.children.get(index))
            .cloned()?;
        let preview_size = theme_engine::resolve_object_bounds_with_runtime(
            &self.theme,
            surface,
            index,
            self.usage.as_ref(),
            self.theme_runtime_for_surface(surface),
        )
        .map(|(_, _, width, height)| egui::vec2(width as f32, height as f32))
        .unwrap_or_else(|| egui::vec2(24.0, 24.0));
        let children: Vec<usize> = self.theme.surfaces[surface]
            .children
            .iter()
            .enumerate()
            .filter(|(_, child)| child.parent.as_deref() == Some(object.id.as_str()))
            .map(|(index, _)| index)
            .collect();
        if !children.is_empty() {
            let selected = self.selection == Selection::Object(surface, index);
            let id = ui.make_persistent_id(("scene-object", surface, object.id.clone()));
            paint_scene_row_background(ui, selected);
            let body_visuals = ui.visuals().clone();
            let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                depth == 1,
            );
            let is_open = state.is_open();
            let responses = scene_row_style(ui, selected, |ui| {
                scene_row_contents(
                    ui,
                    &mut object,
                    Selection::Object(surface, index),
                    preview_size,
                    Some(is_open),
                    !read_only,
                    language,
                )
            });
            let selection = Selection::Object(surface, index);
            if responses.name_changed {
                self.theme.surfaces[surface].children[index].name = object.name.clone();
                self.changed();
            }
            let row_response = responses.item.clone();
            if row_response.hovered() || responses.drag_handle.hovered() {
                self.hovered_scene_item = Some(selection);
            }
            if row_response.clicked() || responses.drag_handle.drag_started() {
                self.selection = selection;
            }
            if responses.expand_button.clicked() {
                toggle_scene_node(ui, id, depth == 1);
            }
            let mut pending_drop = if read_only {
                None
            } else {
                scene_drop_from_response(ui, &row_response, selection, surface)
            };
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                depth == 1,
            );
            if state.is_open() {
                state.show_body_indented(&responses.expand_button, ui, |ui| {
                    *ui.visuals_mut() = body_visuals;
                    for child in children {
                        if pending_drop.is_none() {
                            pending_drop =
                                self.object_tree_row(ui, surface, child, depth + 1, read_only);
                        } else {
                            self.object_tree_row(ui, surface, child, depth + 1, read_only);
                        }
                    }
                });
            }
            return if read_only { None } else { pending_drop };
        }
        let selection = Selection::Object(surface, index);
        let selected = self.selection == selection;
        paint_scene_row_background(ui, selected);
        let responses = scene_row_style(ui, selected, |ui| {
            scene_row_contents(
                ui,
                &mut object,
                selection,
                preview_size,
                None,
                !read_only,
                language,
            )
        });
        if responses.name_changed {
            self.theme.surfaces[surface].children[index].name = object.name;
            self.changed();
        }
        if responses.item.hovered() || responses.drag_handle.hovered() {
            self.hovered_scene_item = Some(selection);
        }
        if responses.item.clicked() || responses.drag_handle.drag_started() {
            self.selection = selection;
        }
        if read_only {
            None
        } else {
            scene_drop_from_response(ui, &responses.item, selection, surface)
        }
    }

    pub(super) fn canvas_preview(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(language.text("Preview"))
                    .size(16.0)
                    .strong(),
            );
            crate::ui::components::zoom::zoom_control(
                ui,
                &mut self.zoom,
                MIN_CANVAS_ZOOM..=MAX_CANVAS_ZOOM,
                1.0,
                language,
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.dirty {
                    ui.colored_label(
                        egui::Color32::from_rgb(232, 188, 95),
                        language.text("Unsaved changes"),
                    );
                }
            });
        });
        let surface_index = match self.selection {
            Selection::Surface(index) | Selection::Object(index, _) => {
                index.min(self.theme.surfaces.len().saturating_sub(1))
            }
        };
        if self.preview_dirty {
            let runtime = self.theme_runtime_for_surface(surface_index);
            let preview_theme = theme_engine::apply_mouse_action_overrides(
                &self.theme,
                &self.preview_mouse_overrides,
            );
            let should_render = theme_engine::surface_should_render(
                &preview_theme,
                surface_index,
                self.usage.as_ref(),
                runtime,
            );
            let mut rendered = theme_engine::render_theme_surface_with_runtime(
                &preview_theme,
                surface_index,
                self.usage.as_ref(),
                runtime,
            );
            if !should_render {
                rendered.pixels.fill(0);
            }
            let _render_warnings = &rendered.warnings;
            let mut rgba = Vec::with_capacity(rendered.pixels.len() * 4);
            for pixel in rendered.pixels {
                rgba.extend_from_slice(&[
                    ((pixel >> 16) & 0xff) as u8,
                    ((pixel >> 8) & 0xff) as u8,
                    (pixel & 0xff) as u8,
                    ((pixel >> 24) & 0xff) as u8,
                ]);
            }
            let image = egui::ColorImage::from_rgba_premultiplied(
                [rendered.width as usize, rendered.height as usize],
                &rgba,
            );
            match self.preview.as_mut() {
                Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                None => {
                    self.preview = Some(ui.ctx().load_texture(
                        "theme-preview",
                        image,
                        egui::TextureOptions::LINEAR,
                    ))
                }
            }
            self.preview_dirty = false;
        }
        let available = ui.available_size();
        let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click());
        if response.hovered() {
            let scroll_delta = ui.input(|input| input.smooth_scroll_delta().y);
            if scroll_delta.abs() > f32::EPSILON {
                self.zoom = (self.zoom * (scroll_delta * 0.002).exp())
                    .clamp(MIN_CANVAS_ZOOM, MAX_CANVAS_ZOOM);
                ui.ctx().request_repaint();
            }
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(13, 14, 17));
        let cell = 16.0;
        let mut y = rect.top();
        let mut row = 0;
        while y < rect.bottom() {
            let mut x = rect.left();
            let mut col = 0;
            while x < rect.right() {
                if (row + col) % 2 == 0 {
                    painter.rect_filled(
                        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell, cell)),
                        0.0,
                        egui::Color32::from_rgb(25, 27, 31),
                    );
                }
                x += cell;
                col += 1;
            }
            y += cell;
            row += 1;
        }
        if let Some(texture) = &self.preview {
            let texture_size = texture.size_vec2();
            let fit_factor = (rect.width() / texture_size.x)
                .min(rect.height() / texture_size.y)
                .min(1.0);
            let size = texture_size * fit_factor * self.zoom;
            let image_rect = egui::Rect::from_center_size(rect.center(), size);
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            self.draw_scene_hover_outline(ui, image_rect, surface_index);
            self.handle_preview_mouse(ui, &response, image_rect, surface_index);
        }
    }

    pub(super) fn preview_mouse_target(
        &self,
        pointer: egui::Pos2,
        canvas_rect: egui::Rect,
        surface_index: usize,
    ) -> Option<String> {
        if !canvas_rect.contains(pointer) {
            return None;
        }
        let theme =
            theme_engine::apply_mouse_action_overrides(&self.theme, &self.preview_mouse_overrides);
        let runtime = self.theme_runtime_for_surface(surface_index);
        let (width, height) =
            theme_engine::resolve_surface_size(&theme, surface_index, self.usage.as_ref(), runtime);
        let x = (pointer.x - canvas_rect.left()) as f64 / canvas_rect.width().max(1.0) as f64
            * width as f64;
        let y = (pointer.y - canvas_rect.top()) as f64 / canvas_rect.height().max(1.0) as f64
            * height as f64;
        theme_engine::hit_test_mouse_event(
            &theme,
            surface_index,
            x,
            y,
            self.usage.as_ref(),
            runtime,
        )
    }

    pub(super) fn execute_preview_mouse_event(
        &mut self,
        surface_index: usize,
        object_id: &str,
        event: MouseEventKind,
    ) -> bool {
        let Some(source) =
            theme_engine::mouse_event_script(&self.theme, surface_index, object_id, event)
                .map(str::to_string)
        else {
            return false;
        };
        let runtime = self.theme_runtime_for_surface(surface_index);
        match theme_engine::execute_mouse_actions(
            &self.theme,
            surface_index,
            object_id,
            &source,
            self.usage.as_ref(),
            runtime,
            &mut self.preview_mouse_overrides,
        ) {
            Ok(_) => {
                self.preview_dirty = true;
                true
            }
            Err(error) => {
                self.theme_error = Some(error);
                false
            }
        }
    }

    pub(super) fn handle_preview_mouse(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        canvas_rect: egui::Rect,
        surface_index: usize,
    ) {
        if self
            .preview_pending_click
            .as_ref()
            .is_some_and(|(started, _, _)| started.elapsed() >= Duration::from_millis(500))
        {
            if let Some((_, pending_surface, pending_object)) = self.preview_pending_click.take() {
                self.execute_preview_mouse_event(
                    pending_surface,
                    &pending_object,
                    MouseEventKind::Click,
                );
            }
        }
        let pointer = response.hover_pos();
        for _ in 0..4 {
            let target = pointer
                .and_then(|pointer| self.preview_mouse_target(pointer, canvas_rect, surface_index))
                .map(|object| (surface_index, object));
            if self.preview_hover_target == target {
                break;
            }
            let previous = std::mem::replace(&mut self.preview_hover_target, target.clone());
            if let Some((surface, object)) = previous {
                self.execute_preview_mouse_event(surface, &object, MouseEventKind::MouseLeave);
            }
            if let Some((surface, object)) = &target {
                self.execute_preview_mouse_event(*surface, object, MouseEventKind::MouseEnter);
            }
        }
        let Some(pointer) = pointer else {
            return;
        };
        let Some(object) = self.preview_mouse_target(pointer, canvas_rect, surface_index) else {
            return;
        };
        let clickable = [
            MouseEventKind::Click,
            MouseEventKind::DoubleClick,
            MouseEventKind::RightClick,
        ]
        .into_iter()
        .any(|event| {
            theme_engine::mouse_event_script(&self.theme, surface_index, &object, event).is_some()
        });
        if clickable {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.double_clicked_by(egui::PointerButton::Primary) {
            self.preview_pending_click = None;
            self.execute_preview_mouse_event(surface_index, &object, MouseEventKind::DoubleClick);
        } else if response.clicked_by(egui::PointerButton::Primary) {
            let has_click = theme_engine::mouse_event_script(
                &self.theme,
                surface_index,
                &object,
                MouseEventKind::Click,
            )
            .is_some();
            let has_double_click = theme_engine::mouse_event_script(
                &self.theme,
                surface_index,
                &object,
                MouseEventKind::DoubleClick,
            )
            .is_some();
            if has_click && has_double_click {
                self.preview_pending_click = Some((Instant::now(), surface_index, object.clone()));
            } else if has_click {
                self.execute_preview_mouse_event(surface_index, &object, MouseEventKind::Click);
            }
        }
        if response.clicked_by(egui::PointerButton::Secondary) {
            self.execute_preview_mouse_event(surface_index, &object, MouseEventKind::RightClick);
        }
    }

    pub(super) fn draw_scene_hover_outline(
        &mut self,
        ui: &mut egui::Ui,
        canvas_rect: egui::Rect,
        surface_index: usize,
    ) {
        let Some(hovered) = self.hovered_scene_item else {
            return;
        };
        let outline = match hovered {
            Selection::Surface(hovered_surface) if hovered_surface == surface_index => canvas_rect,
            Selection::Object(hovered_surface, object_index)
                if hovered_surface == surface_index =>
            {
                let runtime = self.theme_runtime_for_surface(surface_index);
                let Some((x, y, width, height)) = theme_engine::resolve_object_bounds_with_runtime(
                    &self.theme,
                    surface_index,
                    object_index,
                    self.usage.as_ref(),
                    runtime,
                ) else {
                    return;
                };
                let (canvas_width, canvas_height) = theme_engine::resolve_surface_size(
                    &self.theme,
                    surface_index,
                    self.usage.as_ref(),
                    runtime,
                );
                let sx = canvas_rect.width() / canvas_width.max(1) as f32;
                let sy = canvas_rect.height() / canvas_height.max(1) as f32;
                egui::Rect::from_min_size(
                    egui::pos2(
                        canvas_rect.left() + x as f32 * sx,
                        canvas_rect.top() + y as f32 * sy,
                    ),
                    egui::vec2(width as f32 * sx, height as f32 * sy),
                )
            }
            _ => return,
        };
        ui.painter().rect_stroke(
            outline,
            1.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 0, 255)),
            egui::StrokeKind::Inside,
        );
    }

    pub(super) fn inspector(&mut self, ui: &mut egui::Ui, read_only: bool) {
        // TextEdit and DragValue keep in-progress text by widget id. Namespace
        // the complete inspector so a focus change cannot hand that draft to
        // the newly selected object at the same screen position.
        let inspector_identity = match self.selection {
            Selection::Surface(index) => self
                .theme
                .surfaces
                .get(index)
                .map(|surface| format!("surface:{}", surface.id))
                .unwrap_or_else(|| format!("surface-index:{index}")),
            Selection::Object(surface_index, object_index) => self
                .theme
                .surfaces
                .get(surface_index)
                .and_then(|surface| {
                    surface
                        .children
                        .get(object_index)
                        .map(|object| format!("object:{}:{}", surface.id, object.id))
                })
                .unwrap_or_else(|| format!("object-index:{surface_index}:{object_index}")),
        };
        egui::ScrollArea::vertical()
            .id_salt("inspector")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if read_only {
                    let name = match self.selection {
                        Selection::Surface(index) => self
                            .theme
                            .surfaces
                            .get(index)
                            .map(|surface| surface.name.as_str()),
                        Selection::Object(surface, object) => self
                            .theme
                            .surfaces
                            .get(surface)
                            .and_then(|surface| surface.children.get(object))
                            .map(|object| object.name.as_str()),
                    };
                    if let Some(name) = name {
                        inspector_heading(ui, name);
                        ui.add_space(6.0);
                    }
                }
                ui.add_enabled_ui(!read_only, |ui| {
                    ui.push_id(inspector_identity, |ui| {
                        let changed = match self.selection {
                            Selection::Surface(index) => {
                                self.surface_inspector(ui, index, !read_only)
                            }
                            Selection::Object(surface, object_index) => {
                                self.object_inspector(ui, surface, object_index, !read_only)
                            }
                        };
                        if changed {
                            self.changed();
                        }
                    });
                });
            });
    }

    pub(super) fn surface_inspector(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        editable_name: bool,
    ) -> bool {
        let language = self.language();
        let Some(mut surface) = self.theme.surfaces.get(index).cloned() else {
            return false;
        };
        let before = serde_json::to_string(&surface).unwrap_or_default();
        let expression_context = self.expression_context(Selection::Surface(index));
        let render_controls_id =
            ui.make_persistent_id(("surface-render-controls", surface.id.clone()));
        let object_controls_id =
            ui.make_persistent_id(("root-object-controls", surface.id.clone()));
        let placement_controls_id =
            ui.make_persistent_id(("placement-expression-controls", surface.id.clone()));
        let mut requested_expression = None;
        let mut requested_text_template = false;
        let mut requested_asset = false;
        let mut requested_mouse_event = None;
        if editable_name {
            inspector_name_editor(
                ui,
                &mut surface.name,
                object_controls_id.with("name"),
                language,
            );
            ui.add_space(6.0);
        }

        crate::ui::components::collapsible::inspector_section(
            ui,
            object_controls_id.with("layer-section"),
            language.text("Layer"),
            |ui| {
                if let Some(request) = layer_properties_inspector(
                    ui,
                    object_controls_id.with("layer"),
                    &expression_context,
                    &mut surface,
                    language,
                ) {
                    match request {
                        LayerInspectorRequest::Expression(field) => {
                            requested_expression = Some(field)
                        }
                        LayerInspectorRequest::TextTemplate => requested_text_template = true,
                    }
                }
            },
        );

        crate::ui::components::collapsible::inspector_section(
            ui,
            object_controls_id.with("appearance-section"),
            language.text("Appearance"),
            |ui| {
                if let Some(field) = render_controls(
                    ui,
                    render_controls_id,
                    &mut surface.render,
                    &mut surface.visibility,
                    &expression_context,
                    language,
                ) {
                    requested_expression = Some(field);
                }
                if let Some(field) = appearance_inspector(
                    ui,
                    object_controls_id.with("appearance"),
                    &expression_context,
                    &mut surface,
                    &mut requested_asset,
                    language,
                ) {
                    requested_expression = Some(field);
                }
                requested_mouse_event = mouse_events_inspector(
                    ui,
                    object_controls_id.with("mouse-events"),
                    &mut surface,
                    language,
                );
            },
        );

        crate::ui::components::collapsible::inspector_section(
            ui,
            object_controls_id.with("positioning-section"),
            language.text("Positioning"),
            |ui| {
                labeled(ui, language.text("Host"), |ui| {
                    Dropdown::from_id_salt(("surface-nest", index))
                        .width(inspector_control_width(ui))
                        .selected_text(surface_nest_name(language, surface.placement.nest))
                        .show_ui(ui, |ui| {
                            for nest in [
                                SurfaceNest::Taskbar,
                                SurfaceNest::TrayIcon,
                                SurfaceNest::Desktop,
                                SurfaceNest::Floating,
                            ] {
                                dropdown_selectable_value(
                                    ui,
                                    &mut surface.placement.nest,
                                    nest,
                                    surface_nest_name(language, nest),
                                );
                            }
                        });
                });
                if surface.placement.nest == SurfaceNest::TrayIcon {
                    ui.label(
                        egui::RichText::new(
                            language.text("Explorer controls this icon's position, overflow, drag order, and final display size. The root's width and height control its source artwork."),
                        )
                        .small()
                        .color(muted()),
                    );
                }
                labeled(ui, language.text("Reference"), |ui| {
                    const REFERENCE_POINT_WIDTH: f32 = 35.0;
                    let reference_width = (inspector_control_width(ui)
                        - REFERENCE_POINT_WIDTH
                        - ui.spacing().item_spacing.x)
                        .max(1.0);
                    Dropdown::from_id_salt(("reference", index))
                        .width(reference_width)
                        .selected_text(reference_target_name(language, surface.placement.reference))
                        .show_ui(ui, |ui| {
                            let display_count = native_interop::find_monitors().len().max(1);
                            for display in 0..display_count {
                                for region in [
                                    ReferenceRegion::Monitor,
                                    ReferenceRegion::Taskbar,
                                    ReferenceRegion::SystemTray,
                                ] {
                                    let target = ReferenceTarget { region, display };
                                    dropdown_selectable_value(
                                        ui,
                                        &mut surface.placement.reference,
                                        target,
                                        reference_target_name(language, target),
                                    );
                                }
                            }
                        });
                    anchor_point_picker(
                        ui,
                        ("reference-point", index),
                        &mut surface.placement.horizontal,
                        &mut surface.placement.vertical,
                    );
                });
                let mut surface_horizontal = surface
                    .placement
                    .surface_horizontal
                    .unwrap_or(surface.placement.horizontal);
                let mut surface_vertical = surface
                    .placement
                    .surface_vertical
                    .unwrap_or(surface.placement.vertical);
                labeled(ui, language.text("Anchor"), |ui| {
                    anchor_point_picker(
                        ui,
                        ("surface-point", index),
                        &mut surface_horizontal,
                        &mut surface_vertical,
                    );
                });
                surface.placement.surface_horizontal = Some(surface_horizontal);
                surface.placement.surface_vertical = Some(surface_vertical);
                for (label, field, value) in [
                    (
                        language.text("Height"),
                        ExpressionField::ObjectHeight,
                        &mut surface.height,
                    ),
                    (
                        language.text("Width"),
                        ExpressionField::ObjectWidth,
                        &mut surface.width,
                    ),
                ] {
                    if numeric_expression_control(
                        ui,
                        object_controls_id.with(label),
                        label,
                        value,
                        &expression_context,
                    ) {
                        requested_expression = Some(field);
                    }
                }
                if placement_offset_expression_control(
                    ui,
                    placement_controls_id.with("x"),
                    language.text("X offset"),
                    &mut surface.placement.offset_x,
                    &mut surface.placement.offset_x_expression,
                    &expression_context,
                ) {
                    requested_expression = Some(ExpressionField::PlacementOffsetX);
                }
                if placement_offset_expression_control(
                    ui,
                    placement_controls_id.with("y"),
                    language.text("Y offset"),
                    &mut surface.placement.offset_y,
                    &mut surface.placement.offset_y_expression,
                    &expression_context,
                ) {
                    requested_expression = Some(ExpressionField::PlacementOffsetY);
                }
                if numeric_expression_control(
                    ui,
                    object_controls_id.with("rotation"),
                    language.text("Rotation"),
                    &mut surface.rotation,
                    &expression_context,
                ) {
                    requested_expression = Some(ExpressionField::ObjectRotation);
                }
            },
        );

        if let Some(field) = requested_expression {
            let draft = match field {
                ExpressionField::PlacementOffsetX => surface
                    .placement
                    .offset_x_expression
                    .as_ref()
                    .map(|expression| expression.0.clone()),
                ExpressionField::PlacementOffsetY => surface
                    .placement
                    .offset_y_expression
                    .as_ref()
                    .map(|expression| expression.0.clone()),
                _ => scene_object_expression(&surface, field),
            };
            if let Some(draft) = draft {
                self.expression_helper = Some(ExpressionHelperState::new(
                    Selection::Surface(index),
                    field,
                    draft,
                ));
            }
        }
        if let Some(field) = requested_mouse_event {
            let draft = surface
                .mouse_events
                .as_ref()
                .map(|events| events.handler(field.kind()).to_string())
                .unwrap_or_default();
            self.action_helper = Some(ActionHelperState::new(
                Selection::Surface(index),
                field,
                draft,
            ));
        }
        if requested_text_template {
            if let SceneContent::Text { template, .. } = &surface.content {
                self.text_template_helper = Some(TextTemplateHelperState::for_theme(
                    Selection::Surface(index),
                    template.clone(),
                ));
            }
        }
        if requested_asset {
            self.open_asset_picker(Selection::Surface(index), &surface.background);
        }
        let changed = before != serde_json::to_string(&surface).unwrap_or_default();
        if changed {
            self.theme.surfaces[index] = surface;
        }
        changed
    }

    pub(super) fn object_inspector(
        &mut self,
        ui: &mut egui::Ui,
        surface_index: usize,
        object_index: usize,
        editable_name: bool,
    ) -> bool {
        let language = self.language();
        let Some(surface) = self.theme.surfaces.get(surface_index) else {
            return false;
        };
        let Some(mut object) = surface.children.get(object_index).cloned() else {
            return false;
        };
        let expression_context =
            self.expression_context(Selection::Object(surface_index, object_index));
        let before = serde_json::to_string(&object).unwrap_or_default();
        let render_controls_id =
            ui.make_persistent_id(("object-render-controls", object.id.clone()));
        let managed_by_parent = object.parent.as_ref().map_or_else(
            || matches!(surface.layout, ChildLayout::Row | ChildLayout::Column),
            |parent_id| {
                surface.children.iter().any(|candidate| {
                    candidate.id == *parent_id
                        && matches!(candidate.layout, ChildLayout::Row | ChildLayout::Column)
                })
            },
        );
        let expression_controls_id =
            ui.make_persistent_id(("object-expression-controls", object.id.clone()));
        let mut requested_expression = None;
        let mut requested_text_template = false;
        let mut requested_asset = false;
        let mut requested_mouse_event = None;
        if editable_name {
            inspector_name_editor(
                ui,
                &mut object.name,
                expression_controls_id.with("name"),
                language,
            );
            ui.add_space(6.0);
        }

        crate::ui::components::collapsible::inspector_section(
            ui,
            expression_controls_id.with("layer-section"),
            language.text("Layer"),
            |ui| {
                if let Some(request) = layer_properties_inspector(
                    ui,
                    expression_controls_id.with("layer"),
                    &expression_context,
                    &mut object,
                    language,
                ) {
                    match request {
                        LayerInspectorRequest::Expression(field) => {
                            requested_expression = Some(field)
                        }
                        LayerInspectorRequest::TextTemplate => requested_text_template = true,
                    }
                }
            },
        );

        crate::ui::components::collapsible::inspector_section(
            ui,
            expression_controls_id.with("appearance-section"),
            language.text("Appearance"),
            |ui| {
                if let Some(field) = render_controls(
                    ui,
                    render_controls_id,
                    &mut object.render,
                    &mut object.visibility,
                    &expression_context,
                    language,
                ) {
                    requested_expression = Some(field);
                }
                if let Some(field) = appearance_inspector(
                    ui,
                    expression_controls_id.with("appearance"),
                    &expression_context,
                    &mut object,
                    &mut requested_asset,
                    language,
                ) {
                    requested_expression = Some(field);
                }
                requested_mouse_event = mouse_events_inspector(
                    ui,
                    expression_controls_id.with("mouse-events"),
                    &mut object,
                    language,
                );
            },
        );

        crate::ui::components::collapsible::inspector_section(
            ui,
            expression_controls_id.with("positioning-section"),
            language.text("Positioning"),
            |ui| {
                if managed_by_parent {
                    ui.label(
                        egui::RichText::new(
                            language.text("Position is managed by the parent container. X and Y are fine offsets."),
                        )
                        .small()
                        .color(muted()),
                    );
                }
                labeled(ui, language.text("Anchor"), |ui| {
                    object_anchor_picker(
                        ui,
                        ("object-anchor", surface_index, object.id.clone()),
                        &mut object.anchor,
                    );
                });
                for (label, field, value) in [
                    (
                        language.text("Height"),
                        ExpressionField::ObjectHeight,
                        &mut object.height,
                    ),
                    (
                        language.text("Width"),
                        ExpressionField::ObjectWidth,
                        &mut object.width,
                    ),
                ] {
                    if numeric_expression_control(
                        ui,
                        expression_controls_id.with(label),
                        label,
                        value,
                        &expression_context,
                    ) {
                        requested_expression = Some(field);
                    }
                }
                for (label, field, value) in [
                    (
                        language.text("X offset"),
                        ExpressionField::ObjectX,
                        &mut object.x,
                    ),
                    (
                        language.text("Y offset"),
                        ExpressionField::ObjectY,
                        &mut object.y,
                    ),
                    (
                        language.text("Rotation"),
                        ExpressionField::ObjectRotation,
                        &mut object.rotation,
                    ),
                ] {
                    if numeric_expression_control(
                        ui,
                        expression_controls_id.with(label),
                        label,
                        value,
                        &expression_context,
                    ) {
                        requested_expression = Some(field);
                    }
                }
            },
        );

        if let Some(field) = requested_expression {
            if let Some(draft) = scene_object_expression(&object, field) {
                self.expression_helper = Some(ExpressionHelperState::new(
                    Selection::Object(surface_index, object_index),
                    field,
                    draft,
                ));
            }
        }
        if let Some(field) = requested_mouse_event {
            let draft = object
                .mouse_events
                .as_ref()
                .map(|events| events.handler(field.kind()).to_string())
                .unwrap_or_default();
            self.action_helper = Some(ActionHelperState::new(
                Selection::Object(surface_index, object_index),
                field,
                draft,
            ));
        }
        if requested_text_template {
            if let SceneContent::Text { template, .. } = &object.content {
                self.text_template_helper = Some(TextTemplateHelperState::for_theme(
                    Selection::Object(surface_index, object_index),
                    template.clone(),
                ));
            }
        }
        if requested_asset {
            self.open_asset_picker(
                Selection::Object(surface_index, object_index),
                &object.background,
            );
        }
        let changed = before != serde_json::to_string(&object).unwrap_or_default();
        if changed {
            self.theme.surfaces[surface_index].children[object_index] = object;
        }
        changed
    }

    pub(super) fn duplicate_selection(&mut self) -> bool {
        let language = self.language();
        match self.selection {
            Selection::Surface(surface_index) => {
                let Some(mut root) = self.theme.surfaces.get(surface_index).cloned() else {
                    return false;
                };
                let descendants = std::mem::take(&mut root.children);
                let mut moving = vec![root];
                moving.extend(descendants);
                remap_scene_ids(&mut moving, &std::collections::HashSet::new(), true);
                let mut duplicate = moving.remove(0);
                duplicate.name = format!("{} ({})", duplicate.name, language.text("copy"));
                duplicate.children = moving;
                let insert_at = surface_index + 1;
                self.theme.surfaces.insert(insert_at, duplicate);
                self.selection = Selection::Surface(insert_at);
                true
            }
            Selection::Object(surface_index, object_index) => {
                let Some(surface) = self.theme.surfaces.get(surface_index) else {
                    return false;
                };
                let Some(root_id) = surface
                    .children
                    .get(object_index)
                    .map(|object| object.id.clone())
                else {
                    return false;
                };
                let ids = scene_subtree_ids(&surface.children, &root_id);
                let mut moving: Vec<SceneObject> = surface
                    .children
                    .iter()
                    .filter(|object| ids.contains(&object.id))
                    .cloned()
                    .collect();
                let Some(root_position) = moving.iter().position(|object| object.id == root_id)
                else {
                    return false;
                };
                remap_scene_ids(&mut moving, &std::collections::HashSet::new(), true);
                moving[root_position].name =
                    format!("{} ({})", moving[root_position].name, language.text("copy"));
                let insert_at = scene_subtree_end(&surface.children, &root_id);
                let selected_index = insert_at + root_position;
                self.theme.surfaces[surface_index]
                    .children
                    .splice(insert_at..insert_at, moving);
                self.selection = Selection::Object(surface_index, selected_index);
                true
            }
        }
    }

    pub(super) fn delete_selection(&mut self) -> bool {
        match self.selection {
            Selection::Surface(surface_index) if self.theme.surfaces.len() > 1 => {
                self.theme.surfaces.remove(surface_index);
                self.selection = Selection::Surface(
                    surface_index.min(self.theme.surfaces.len().saturating_sub(1)),
                );
                true
            }
            Selection::Surface(_) => false,
            Selection::Object(surface_index, object_index) => {
                let Some(root_id) = self
                    .theme
                    .surfaces
                    .get(surface_index)
                    .and_then(|surface| surface.children.get(object_index))
                    .map(|object| object.id.clone())
                else {
                    return false;
                };
                take_scene_subtree(&mut self.theme.surfaces[surface_index].children, &root_id);
                self.selection = Selection::Surface(surface_index);
                true
            }
        }
    }

    pub(super) fn apply_scene_drop(&mut self, source: Selection, target: SceneDropTarget) -> bool {
        if matches!(target, SceneDropTarget::Into(selection) if selection == source)
            || matches!(target, SceneDropTarget::Before(selection) if selection == source)
            || matches!(target, SceneDropTarget::After(selection) if selection == source)
        {
            return false;
        }

        if let SceneDropTarget::RootAt(mut insert_at) = target {
            return match source {
                Selection::Surface(source_index) => {
                    if source_index >= self.theme.surfaces.len() {
                        return false;
                    }
                    let surface = self.theme.surfaces.remove(source_index);
                    if source_index < insert_at {
                        insert_at = insert_at.saturating_sub(1);
                    }
                    insert_at = insert_at.min(self.theme.surfaces.len());
                    self.theme.surfaces.insert(insert_at, surface);
                    self.selection = Selection::Surface(insert_at);
                    source_index != insert_at
                }
                Selection::Object(surface_index, object_index) => {
                    let Some(root_id) = self
                        .theme
                        .surfaces
                        .get(surface_index)
                        .and_then(|surface| surface.children.get(object_index))
                        .map(|object| object.id.clone())
                    else {
                        return false;
                    };
                    let mut moving = take_scene_subtree(
                        &mut self.theme.surfaces[surface_index].children,
                        &root_id,
                    );
                    let Some(root_position) = moving.iter().position(|object| object.id == root_id)
                    else {
                        return false;
                    };
                    let mut root = moving.remove(root_position);
                    root.parent = None;
                    root.placement = Placement::default();
                    root.anchor = ObjectAnchor::default();
                    root.x = 0.0.into();
                    root.y = 0.0.into();
                    root.children.clear();

                    let reserved: std::collections::HashSet<String> = self
                        .theme
                        .surfaces
                        .iter()
                        .map(|surface| surface.id.to_ascii_lowercase())
                        .collect();
                    let mut promoted = vec![root];
                    promoted.extend(moving);
                    remap_scene_ids(&mut promoted, &reserved, false);
                    let mut root = promoted.remove(0);
                    for child in &mut promoted {
                        if child.parent.as_deref() == Some(root.id.as_str()) {
                            child.parent = None;
                        }
                    }
                    root.children = promoted;
                    insert_at = insert_at.min(self.theme.surfaces.len());
                    self.theme.surfaces.insert(insert_at, root);
                    self.selection = Selection::Surface(insert_at);
                    true
                }
            };
        }

        let target_selection = match target {
            SceneDropTarget::Into(selection)
            | SceneDropTarget::Before(selection)
            | SceneDropTarget::After(selection) => selection,
            SceneDropTarget::RootAt(_) => unreachable!(),
        };
        let target_surface_index = match target_selection {
            Selection::Surface(index) | Selection::Object(index, _) => index,
        };
        let Some(target_surface) = self.theme.surfaces.get(target_surface_index) else {
            return false;
        };
        let target_surface_id = target_surface.id.clone();
        let (target_object_id, target_parent) = match target_selection {
            Selection::Surface(_) => (None, None),
            Selection::Object(_, object_index) => {
                let Some(object) = target_surface.children.get(object_index) else {
                    return false;
                };
                (Some(object.id.clone()), object.parent.clone())
            }
        };

        if let Selection::Object(source_surface, source_object) = source {
            if source_surface == target_surface_index {
                let Some(source_id) = self.theme.surfaces[source_surface]
                    .children
                    .get(source_object)
                    .map(|object| object.id.clone())
                else {
                    return false;
                };
                if target_object_id.as_ref().is_some_and(|target_id| {
                    scene_subtree_ids(&self.theme.surfaces[source_surface].children, &source_id)
                        .contains(target_id)
                }) {
                    return false;
                }
            }
        }
        if matches!(source, Selection::Surface(index) if index == target_surface_index) {
            return false;
        }

        let mut moving = match source {
            Selection::Surface(source_index) => {
                if source_index >= self.theme.surfaces.len() {
                    return false;
                }
                let mut root = self.theme.surfaces.remove(source_index);
                let mut descendants = std::mem::take(&mut root.children);
                for child in &mut descendants {
                    if child.parent.is_none() {
                        child.parent = Some(root.id.clone());
                    }
                }
                root.placement = Placement::default();
                root.anchor = ObjectAnchor::default();
                root.x = 0.0.into();
                root.y = 0.0.into();
                let mut moving = vec![root];
                moving.extend(descendants);
                moving
            }
            Selection::Object(source_surface, source_object) => {
                let Some(source_id) = self
                    .theme
                    .surfaces
                    .get(source_surface)
                    .and_then(|surface| surface.children.get(source_object))
                    .map(|object| object.id.clone())
                else {
                    return false;
                };
                take_scene_subtree(
                    &mut self.theme.surfaces[source_surface].children,
                    &source_id,
                )
            }
        };
        let Some(root_position) = moving.iter().position(|object| {
            object.parent.is_none()
                || !moving
                    .iter()
                    .any(|candidate| Some(&candidate.id) == object.parent.as_ref())
        }) else {
            return false;
        };

        let Some(adjusted_target_surface) = self
            .theme
            .surfaces
            .iter()
            .position(|surface| surface.id == target_surface_id)
        else {
            return false;
        };
        let crosses_surfaces = match source {
            Selection::Surface(_) => true,
            Selection::Object(source_surface, _) => source_surface != adjusted_target_surface,
        };
        if crosses_surfaces {
            let reserved = reserved_scene_ids(&self.theme.surfaces[adjusted_target_surface]);
            remap_scene_ids(&mut moving, &reserved, false);
        }

        let target_object = target_object_id.as_ref().and_then(|id| {
            self.theme.surfaces[adjusted_target_surface]
                .children
                .iter()
                .find(|object| object.id == *id)
                .cloned()
        });
        let parent = match target {
            SceneDropTarget::Into(Selection::Surface(_)) => None,
            SceneDropTarget::Into(Selection::Object(_, _)) => target_object_id.clone(),
            SceneDropTarget::Before(_) | SceneDropTarget::After(_) => target_parent,
            SceneDropTarget::RootAt(_) => unreachable!(),
        };
        moving[root_position].parent = parent;
        let moved_root_id = moving[root_position].id.clone();
        let insert_at = match target {
            SceneDropTarget::Into(Selection::Surface(_)) => {
                self.theme.surfaces[adjusted_target_surface].children.len()
            }
            SceneDropTarget::Into(Selection::Object(_, _)) | SceneDropTarget::After(_) => {
                let Some(target_object) = target_object.as_ref() else {
                    return false;
                };
                scene_subtree_end(
                    &self.theme.surfaces[adjusted_target_surface].children,
                    &target_object.id,
                )
            }
            SceneDropTarget::Before(_) => {
                let Some(target_object) = target_object.as_ref() else {
                    return false;
                };
                self.theme.surfaces[adjusted_target_surface]
                    .children
                    .iter()
                    .position(|object| object.id == target_object.id)
                    .unwrap_or(0)
            }
            SceneDropTarget::RootAt(_) => unreachable!(),
        };
        self.theme.surfaces[adjusted_target_surface]
            .children
            .splice(insert_at..insert_at, moving);
        let moved_index = self.theme.surfaces[adjusted_target_surface]
            .children
            .iter()
            .position(|object| object.id == moved_root_id)
            .unwrap_or(insert_at);
        self.selection = Selection::Object(adjusted_target_surface, moved_index);
        true
    }

    pub(super) fn add_layer(&mut self) {
        let language = self.language();
        match self.selection {
            Selection::Surface(selected) if selected < self.theme.surfaces.len() => {
                self.insert_root_layer(selected + 1);
            }
            Selection::Object(surface_index, object_index) => {
                let sibling = self
                    .theme
                    .surfaces
                    .get(surface_index)
                    .and_then(|surface| surface.children.get(object_index))
                    .map(|selected| (selected.id.clone(), selected.parent.clone()));
                let Some((selected_id, parent)) = sibling else {
                    self.insert_root_layer(0);
                    self.changed();
                    return;
                };
                let surface = &mut self.theme.surfaces[surface_index];
                let insert_at = scene_subtree_end(&surface.children, &selected_id);
                let mut layer = SceneObject::object(
                    unique_id("layer"),
                    format!("{} {}", language.text("Layer"), surface.children.len() + 1),
                );
                layer.parent = parent;
                surface.children.insert(insert_at, layer);
                self.selection = Selection::Object(surface_index, insert_at);
            }
            Selection::Surface(_) => self.insert_root_layer(0),
        }
        self.changed();
    }

    pub(super) fn insert_root_layer(&mut self, index: usize) {
        let language = self.language();
        let index = index.min(self.theme.surfaces.len());
        self.theme.surfaces.insert(
            index,
            SceneObject::root(
                unique_id("layer"),
                format!(
                    "{} {}",
                    language.text("Layer"),
                    self.theme.surfaces.len() + 1
                ),
                292.0.into(),
                104.0.into(),
                Placement::default(),
            ),
        );
        self.selection = Selection::Surface(index);
    }
}
