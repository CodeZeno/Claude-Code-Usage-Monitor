use super::*;

impl StudioApp {
    pub(super) fn assets_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        Self::page_header(
            ui,
            language.text("Assets"),
            language.text(
                "Add images once, reuse them across themes, or drop image files here to import them.",
            ),
        );
        let assets = match theme_engine::list_assets() {
            Ok(assets) => assets,
            Err(error) => {
                self.asset_error = Some(error);
                Vec::new()
            }
        };
        self.ensure_asset_thumbnails(ui.ctx(), &assets);
        if self
            .asset_page_selected
            .as_ref()
            .is_some_and(|selected| !assets.iter().any(|asset| &asset.relative_path == selected))
        {
            self.asset_page_selected = None;
        }

        let mut filter = self.asset_page_filter.clone();
        let mut selected = self.asset_page_selected.clone();
        let selected_asset = selected
            .as_ref()
            .and_then(|path| assets.iter().find(|asset| &asset.relative_path == path))
            .cloned();
        let mut add_image = false;
        let mut delete_asset = false;
        ui.horizontal(|ui| {
            ui.add(
                singleline_text_edit(&mut filter)
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
            let delete = ui
                .add_enabled(
                    selected_asset.is_some(),
                    lucide_labeled_button(LucideIcon::Trash, language.text("Delete")),
                )
                .on_disabled_hover_text(language.text("Select an asset to delete"));
            if delete.clicked() {
                delete_asset = true;
            }
        });
        if let Some(error) = &self.asset_error {
            ui.add_space(8.0);
            ui.colored_label(egui::Color32::from_rgb(232, 119, 95), error);
        }
        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .id_salt("assets-page-grid")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                asset_grid(
                    ui,
                    &assets,
                    &self.asset_thumbnails,
                    &filter,
                    &mut selected,
                    &self.theme,
                    language,
                );
            });
        self.asset_page_filter = filter;
        self.asset_page_selected = selected;

        if add_image {
            if let Some(asset) = self.import_asset_from_dialog() {
                self.asset_page_selected = Some(asset.relative_path);
            }
        }
        if delete_asset {
            if let Some(asset) = selected_asset {
                self.asset_delete_confirmation = Some(AssetDeletionConfirmation { asset });
            }
        }
        self.asset_delete_dialog(ui.ctx());
    }

    pub(super) fn asset_delete_dialog(&mut self, context: &egui::Context) {
        let Some(confirmation) = self.asset_delete_confirmation.take() else {
            return;
        };
        let language = self.language();
        let mut action = 0;
        crate::ui::components::modal::Modal::new(
            language.text("Delete asset?"),
            "delete-asset-confirmation",
        )
        .width(310.0)
        .fixed_height(110.0)
        .show(context, |ui| {
            ui.label(
                language
                    .text(
                        "Are you sure you want to delete {name} from the asset library and all themes using it?",
                    )
                    .replace("{name}", &confirmation.asset.name),
            );
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(language.text("Delete"))
                                .color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(178, 48, 48)),
                    )
                    .clicked()
                {
                    action = 2;
                }
                if ui.button(language.text("Cancel")).clicked() {
                    action = 1;
                }
            });
        });
        match action {
            1 => {}
            2 => {
                let path = &confirmation.asset.relative_path;
                if let Err(error) = theme_engine::delete_asset(path) {
                    self.asset_error = Some(error);
                } else {
                    if theme_engine::remove_asset_references(&mut self.theme, path) > 0 {
                        self.theme.prepare_runtime();
                        self.preview_dirty = true;
                        self.reset_history();
                    }
                    self.asset_error = None;
                    self.asset_thumbnails.remove(path);
                    if self.asset_page_selected.as_deref() == Some(path) {
                        self.asset_page_selected = None;
                    }
                }
            }
            _ => self.asset_delete_confirmation = Some(confirmation),
        }
    }
}
