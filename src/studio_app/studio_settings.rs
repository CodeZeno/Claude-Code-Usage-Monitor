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
                    setting_row_with_mark(
                        ui,
                        provider_mark_glyph(descriptor.id),
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
                    language.text("Lock in taskbar"),
                    language.text("Keep the widget docked; never float automatically"),
                    |ui| {
                        changed |= Toggle::new(&mut self.settings.lock_taskbar)
                            .labels(language.text("Locked"), language.text("Floating"))
                            .show(ui)
                            .changed();
                    },
                );
                match self
                    .theme
                    .edge_surface()
                    .map(|surface| surface.placement.clone())
                {
                    Some(placement) => {
                        changed |= edge_settings(ui, &mut self.settings, &placement, language);
                    }
                    // The alarm's sound works with every theme.
                    None => changed |= alarm_setting(ui, &mut self.settings, language),
                }
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

/// The choices for "Hide until hover": the theme's own setting, or forced on or off.
const HIDE_CHOICES: [Option<bool>; 3] = [None, Some(true), Some(false)];

/// Record the Dashboard's hiding choice. `None` hands the decision back to the theme.
fn choose_hide(settings: &mut SettingsFile, choice: Option<bool>) -> bool {
    let changed = settings.hide_until_hover != choice;
    settings.hide_until_hover = choice;
    changed
}

/// Give hiding back to the theme, as the Studio's "Use theme setting" does.
pub(super) fn clear_hide_override(settings: &mut SettingsFile) -> bool {
    choose_hide(settings, None)
}

/// The display the widget really sits on, which is what the selector shows.
fn shown_display(saved: Option<usize>, authored: usize, count: usize) -> usize {
    native_interop::display_or_first(saved.unwrap_or(authored), count)
}

/// An explicit display choice is always saved, even when it equals what the
/// selector already showed for a display that has gone.
fn choose_display(settings: &mut SettingsFile, index: usize) -> bool {
    settings.edge_display = Some(index);
    true
}

/// Hiding and monitor choice for the active theme's edge-docked surfaces.
/// Settings hold them because built-in themes are read-only.
fn edge_settings(
    ui: &mut egui::Ui,
    settings: &mut SettingsFile,
    placement: &Placement,
    language: LanguageId,
) -> bool {
    let mut changed = false;
    setting_separator(ui);
    setting_row(
        ui,
        language.text("Hide until hover"),
        language.text("Keep edge-docked widgets tucked away until you point at them"),
        |ui| {
            let name = |choice: Option<bool>| match choice {
                None => language.text("Use theme setting"),
                Some(true) => language.text("Enabled"),
                Some(false) => language.text("Disabled"),
            };
            Dropdown::from_id_salt("hide_until_hover")
                .width(220.0)
                .selected_text(name(settings.hide_until_hover))
                .show_ui(ui, |ui| {
                    for choice in HIDE_CHOICES {
                        let selected = settings.hide_until_hover == choice;
                        if dropdown_selectable_label(ui, selected, name(choice)).clicked() {
                            changed |= choose_hide(settings, choice);
                        }
                    }
                });
        },
    );
    changed |= alarm_setting(ui, settings, language);
    setting_separator(ui);
    setting_row(
        ui,
        language.text("Widget display"),
        language.text("Monitor used by edge-docked widgets"),
        |ui| {
            let count = native_interop::find_monitors().len().max(1);
            let name = |index: usize| format!("{} {}", language.text("Display"), index + 1);
            let display = shown_display(settings.edge_display, placement.reference.display, count);
            Dropdown::from_id_salt("edge_display")
                .width(220.0)
                .selected_text(name(display))
                .show_ui(ui, |ui| {
                    for index in 0..count {
                        // Clicking the value on show still saves it, so a
                        // saved display that is gone can be settled.
                        if dropdown_selectable_label(ui, display == index, name(index)).clicked() {
                            changed |= choose_display(settings, index);
                        }
                    }
                });
        },
    );
    changed
}

fn alarm_setting(ui: &mut egui::Ui, settings: &mut SettingsFile, language: LanguageId) -> bool {
    setting_separator(ui);
    let mut changed = false;
    setting_row(
        ui,
        language.text("Alarm at 5% remaining"),
        language.text("Sound once and keep edge-docked widgets open while a limit is almost spent"),
        |ui| {
            changed = Toggle::new(&mut settings.low_usage_alarm)
                .labels(language.text("Enabled"), language.text("Disabled"))
                .show(ui)
                .changed();
        },
    );
    changed
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

#[cfg(test)]
mod edge_setting_tests {
    use super::*;

    #[test]
    fn the_dashboard_can_hand_hiding_back_to_the_theme_and_the_theme_wins_again() {
        let mut theme: crate::theme_engine::ThemeDocument =
            serde_json::from_str(include_str!("../themes/top-bar.json")).unwrap();
        theme.prepare_runtime();
        let authored = theme.surfaces[0].placement.auto_hide;
        assert!(authored, "the Top Bar hides by itself");
        let mut settings = SettingsFile::default();
        let effective = |settings: &SettingsFile| {
            let mut theme = theme.clone();
            theme.apply_edge_preferences(settings.hide_until_hover, None);
            theme.surfaces[0].placement.auto_hide
        };
        assert_eq!(effective(&settings), authored);
        // Forced off, then forced on, then back to the theme's own.
        assert!(choose_hide(&mut settings, Some(false)));
        assert!(!effective(&settings));
        assert!(!choose_hide(&mut settings, Some(false)), "nothing to save");
        assert!(choose_hide(&mut settings, Some(true)));
        assert!(effective(&settings));
        assert!(choose_hide(&mut settings, Some(false)));
        assert!(clear_hide_override(&mut settings));
        assert_eq!(settings.hide_until_hover, None);
        assert_eq!(effective(&settings), authored);
        assert!(!clear_hide_override(&mut settings), "already the theme's");
    }

    #[test]
    fn a_saved_display_that_is_gone_shows_where_the_widget_is_and_a_click_settles_it() {
        // Two displays connected; the saved third is gone. The runtime puts the
        // widget on the first, and so does the selector.
        assert_eq!(shown_display(Some(2), 0, 2), 0);
        assert_eq!(shown_display(None, 3, 2), 0);
        assert_eq!(shown_display(Some(1), 0, 2), 1);
        assert_eq!(shown_display(None, 0, 1), 0);
        let mut settings = SettingsFile::default();
        settings.edge_display = Some(2);
        // Clicking the display it already shows still saves it.
        assert!(choose_display(&mut settings, 0));
        assert_eq!(settings.edge_display, Some(0));
    }
}
