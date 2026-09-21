use super::*;
use crate::providers::ProviderId;

impl StudioApp {
    pub(super) fn settings_page(&mut self, ui: &mut egui::Ui) {
        let language = self.language();
        let mut changed = false;
        let mut requested_theme = None;
        let mut open_theme_studio = false;
        let themes = available_themes(self.theme_path.as_deref(), &self.theme);
        if let Some(error) = &self.settings_error {
            ui.colored_label(egui::Color32::from_rgb(196, 64, 64), error);
            ui.add_space(8.0);
        }
        settings_scroll_area(ui, |ui| {
            section(ui, language.text("General"), |ui| {
                setting_row(
                    ui,
                    language.text("Update frequency"),
                    language.text("How often provider usage is refreshed"),
                    |ui| {
                        ui.label(language.text("minutes"));
                        let mut minutes = self.settings.poll_interval_ms / POLL_1_MIN;
                        if ui
                            .push_id(
                                ("poll_interval", self.poll_interval_editor_generation),
                                |ui| {
                                    NumberField::new(&mut minutes)
                                        .range(1..=app_settings::MAX_POLL_MINUTES)
                                        .speed(1.0)
                                        .show(ui, 100.0)
                                },
                            )
                            .inner
                            .changed()
                        {
                            self.settings.poll_interval_ms = minutes * POLL_1_MIN;
                            changed = true;
                        }
                        if ui.button(language.text("Refresh now")).clicked() {
                            self.request_refresh();
                        }
                    },
                );
                setting_separator(ui);
                setting_row(
                    ui,
                    language.text("Start with Windows"),
                    language.text("Launch the monitor when you sign in"),
                    |ui| {
                        if Toggle::new(&mut self.startup_enabled)
                            .labels(language.text("Enabled"), language.text("Disabled"))
                            .show(ui)
                            .changed()
                        {
                            crate::window::set_startup_enabled(self.startup_enabled);
                        }
                    },
                );
            });
            section(ui, language.text("Providers"), |ui| {
                for (index, descriptor) in PROVIDER_DESCRIPTORS.iter().enumerate() {
                    if index > 0 {
                        setting_separator(ui);
                    }
                    setting_row(
                        ui,
                        language.text(descriptor.display_name),
                        language.text(descriptor.settings_description),
                        |ui| {
                            let mut enabled = self.settings.provider_enabled(descriptor.id);
                            if Toggle::new(&mut enabled)
                                .labels(language.text("Enabled"), language.text("Disabled"))
                                .show(ui)
                                .changed()
                            {
                                changed |= self.settings.toggle_provider(descriptor.id);
                            }
                        },
                    );
                    if matches!(descriptor.id, ProviderId::Claude | ProviderId::Codex) {
                        let enabled = self.settings.provider_enabled(descriptor.id);
                        let accounts = match descriptor.id {
                            ProviderId::Claude => &mut self.settings.accounts.claude,
                            _ => &mut self.settings.accounts.codex,
                        };
                        ui.push_id(descriptor.key, |ui| {
                            changed |= account_settings(
                                ui,
                                descriptor.id,
                                accounts,
                                self.usage.as_ref(),
                                enabled,
                                language,
                                self.owner,
                            );
                        });
                    }
                }
            });
            section(ui, language.text("Display"), |ui| {
                setting_row(
                    ui,
                    language.text("Usage direction"),
                    language.text("Count down what is left in supported themes"),
                    |ui| {
                        changed |= Toggle::new(&mut self.settings.usage_countdown)
                            .labels(language.text("Remaining"), language.text("Used"))
                            .show(ui)
                            .changed();
                    },
                );
                setting_separator(ui);
                setting_row(
                    ui,
                    language.text("Language"),
                    language.text("Language used by the app and widget"),
                    |ui| {
                        let language_code = self
                            .settings
                            .language
                            .get_or_insert_with(|| "system".into());
                        Dropdown::from_id_salt("language")
                            .width(220.0)
                            .selected_text(language_name(language, language_code))
                            .show_ui(ui, |ui| {
                                for (code, name) in languages(language) {
                                    changed |= dropdown_selectable_value(
                                        ui,
                                        language_code,
                                        code.into(),
                                        name,
                                    )
                                    .changed();
                                }
                            });
                    },
                );
            });
            section(ui, language.text("Appearance"), |ui| {
                setting_row(
                    ui,
                    language.text("Active theme"),
                    language.text("The widget is managed in Theme Studio"),
                    |ui| {
                        Dropdown::from_id_salt("active_theme")
                            .width(220.0)
                            .selected_text(&self.theme.name)
                            .show_ui(ui, |ui| {
                                for theme in &themes {
                                    let selected =
                                        self.theme_path.as_deref() == Some(theme.path.as_path());
                                    if dropdown_selectable_label(ui, selected, &theme.label)
                                        .clicked()
                                    {
                                        requested_theme = Some(theme.path.clone());
                                    }
                                }
                            });
                    },
                );
                setting_separator(ui);
                setting_row(
                    ui,
                    language.text("Theme editor"),
                    language.text("Create and edit themes"),
                    |ui| {
                        if ui.button(language.text("Open Theme Studio")).clicked() {
                            open_theme_studio = true;
                        }
                    },
                );
            });
        });
        if changed {
            self.settings.accounts.claude.normalize();
            self.settings.accounts.codex.normalize();
            if let Some(data) = self.usage.as_mut() {
                data.select_accounts(&self.settings.accounts);
            }
            let new_language = self.language();
            if new_language != language {
                configure_style(ui.ctx(), new_language);
            }
            self.preview_dirty = true;
            self.save_settings();
        }
        if let Some(path) = requested_theme {
            self.request_activate_theme(path);
        }
        if open_theme_studio {
            self.page = Page::Studio;
        }
    }
}

fn account_settings(
    ui: &mut egui::Ui,
    provider: ProviderId,
    accounts: &mut crate::accounts::ProviderAccounts,
    data: Option<&AppUsageData>,
    provider_enabled: bool,
    language: LanguageId,
    owner: isize,
) -> bool {
    let mut changed = false;
    let header = egui::CollapsingHeader::new(language.text("Accounts"))
        .default_open(accounts.profiles.len() > 1);
    header.show(ui, |ui| {
        ui.label(language.text("Themes use the default account unless they specify an account. Custom themes can display multiple accounts."));
        ui.horizontal(|ui| {
            ui.label(language.text("Default account"));
            let selected = accounts.selected().map(|p| p.name.as_str()).unwrap_or("None");
            Dropdown::from_id_salt("widget_account")
                .width(220.0)
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for profile in accounts.profiles.iter().filter(|p| p.enabled) {
                        changed |= dropdown_selectable_value(ui, &mut accounts.selected,
                            profile.id.clone(), &profile.name).changed();
                    }
                });
        });
        let mut remove = None;
        for (index, profile) in accounts.profiles.iter_mut().enumerate() {
            ui.push_id(profile.id.clone(), |ui| {
                ui.separator();
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut profile.enabled, language.text("Monitor")).changed();
                    let name = ui.add(crate::ui::components::text_field::singleline(&mut profile.name)
                        .desired_width(220.0).hint_text(language.text("Account name")));
                    changed |= name.lost_focus();
                    if ui.button(language.text("Remove")).clicked() { remove = Some(index); }
                });
                ui.label(language.text("Config directory"));
                let directory = ui.add(crate::ui::components::text_field::singleline(&mut profile.config_dir)
                    .hint_text(if provider == ProviderId::Claude { "~/.claude-work" } else { "~/.codex-work" }));
                changed |= directory.lost_focus();
                ui.collapsing(language.text("Custom credentials file"), |ui| {
                    let file = ui.add(crate::ui::components::text_field::singleline(&mut profile.credentials_path)
                        .hint_text(language.text("Optional; overrides the config directory")));
                    changed |= file.lost_focus();
                    if ui.button(language.text("Browse...")).clicked() {
                        if let Some(path) = choose_file(owner, language.text("Select credentials file"), "JSON files\0*.json\0All files\0*.*\0\0") {
                            profile.credentials_path = path.to_string_lossy().into_owned();
                            changed = true;
                        }
                    }
                    ui.weak(format!("{}: accounts.{}.{}", language.text("Theme binding"), provider.descriptor().key, profile.id));
                });
                match profile.credential_path(provider) {
                    Err(error) => { ui.colored_label(egui::Color32::from_rgb(196, 64, 64), error); }
                    Ok(None) => { ui.weak(if provider == ProviderId::Claude {
                        language.text("Uses CLAUDE_CONFIG_DIR when set; otherwise detects CLI, Desktop or WSL credentials.")
                    } else {
                        language.text("Uses CODEX_HOME when set; otherwise ~/.codex/auth.json.")
                    }); }
                    Ok(Some(path)) => { ui.weak(path.display().to_string()); }
                }
                if !provider_enabled || !profile.enabled {
                    ui.weak(language.text("Monitoring disabled"));
                } else if let Some(account) = data.and_then(|data| data.accounts.iter().find(|account| {
                    account.provider == provider && account.profile.same_source(profile)
                })) {
                    if let Some(error) = account.error {
                        ui.colored_label(egui::Color32::from_rgb(196, 112, 32), account_error_message(error, language));
                    }
                    if let Some(usage) = &account.usage {
                        if usage.stale { ui.weak(language.text("Last known usage")); }
                        ui.horizontal_wrapped(|ui| {
                            for (label, section) in [("Session", &usage.session), ("Weekly", &usage.weekly)] {
                                if section.available {
                                    ui.label(format!("{}: {:.0}%", language.text(label), section.percentage));
                                    if let Some(reset) = section.resets_at {
                                        let minutes = reset.duration_since(std::time::SystemTime::now()).unwrap_or_default().as_secs() / 60;
                                        ui.weak(format!("{} {}h {}m", language.text("Resets in"), minutes / 60, minutes % 60));
                                    }
                                }
                            }
                            if let Some(credits) = &usage.credits {
                                ui.label(format!("{}: {:.0}%", language.text("Credits"), credits.percentage));
                            }
                        });
                    } else if account.error.is_none() {
                        ui.weak(language.text("Waiting for usage"));
                    }
                } else {
                    ui.weak(language.text("Waiting for usage"));
                }
            });
        }
        if let Some(index) = remove {
            accounts.profiles.remove(index);
            changed = true;
        }
        if ui.button(language.text("Add account")).clicked() {
            accounts.add();
            changed = true;
        }
    });
    changed
}

fn account_error_message(error: crate::poller::PollError, language: LanguageId) -> String {
    error.message(language)
}

#[cfg(test)]
mod account_status_tests {
    use super::*;

    #[test]
    fn status_explains_http_failure_and_correct_recovery_action() {
        use crate::poller::PollError;
        assert_eq!(
            account_error_message(PollError::HttpStatus(429), LanguageId::English),
            "HTTP 429: Too Many Requests. Retrying at the next refresh"
        );
        assert_eq!(
            account_error_message(PollError::HttpStatus(401), LanguageId::English),
            "HTTP 401: Unauthorized. Sign in again for this account"
        );
        assert!(
            account_error_message(PollError::HttpStatus(503), LanguageId::English)
                .contains("Service Unavailable")
        );
    }
}
