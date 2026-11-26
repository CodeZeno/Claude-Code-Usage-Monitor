//! The tray icon: its usage summary tooltip and themed icon.

use super::*;

#[cfg(test)]
fn tray_usage_summary_lines(
    data: &AppUsageData,
    providers: ProviderSet,
    language: LanguageId,
    countdown: bool,
) -> Vec<String> {
    tray_usage_summary_lines_with_pools(data, providers, language, countdown, true, true)
}

fn tray_usage_summary_lines_with_pools(
    data: &AppUsageData,
    providers: ProviderSet,
    language: LanguageId,
    countdown: bool,
    show_antigravity_gemini: bool,
    show_antigravity_external: bool,
) -> Vec<String> {
    usage_summary_lines(
        data,
        providers,
        language,
        countdown,
        show_antigravity_gemini,
        show_antigravity_external,
        None,
    )
}

pub(super) fn usage_summary_lines(
    data: &AppUsageData,
    providers: ProviderSet,
    language: LanguageId,
    countdown: bool,
    show_antigravity_gemini: bool,
    show_antigravity_external: bool,
    detailed_at: Option<SystemTime>,
) -> Vec<String> {
    let strings = language.strings();
    let shown = |percentage: f64| {
        if countdown {
            100.0 - percentage
        } else {
            percentage
        }
    };
    let window_text = |section: &crate::models::UsageSection| {
        if section.available {
            if let Some(now) = detailed_at {
                let percentage = format!("{:.1}%", shown(section.percentage));
                match section.resets_at {
                    Some(reset) => format!(
                        "{percentage} ({})",
                        widget_tooltip::reset_countdown(reset, now, &strings)
                    ),
                    None => percentage,
                }
            } else {
                format!("{:.0}%", shown(section.percentage))
            }
        } else {
            "--".to_string()
        }
    };
    let mut lines = Vec::new();
    for provider in providers.iter() {
        if provider == ProviderId::Antigravity {
            let base_name = language.text(provider.descriptor().display_name);
            let acct = data.selected_account_name(provider);
            if show_antigravity_gemini {
                if let Some(usage) = data.get(provider) {
                    let weekly_label = usage
                        .weekly_label
                        .as_deref()
                        .unwrap_or(strings.weekly_window);
                    let session_label = usage
                        .session_label
                        .as_deref()
                        .unwrap_or(strings.session_window);
                    let label = match acct {
                        Some(name) => format!("{base_name} (Gemini, {name})"),
                        None => format!("{base_name} (Gemini)"),
                    };
                    lines.push(format!(
                        "{} {}: {} | {}: {}",
                        label,
                        session_label,
                        window_text(&usage.session),
                        weekly_label,
                        window_text(&usage.weekly),
                    ));
                }
            }
            if show_antigravity_external {
                if data.get(provider).is_some() && data.antigravity_external().is_none() {
                    let label = match acct {
                        Some(name) => format!("{base_name} (Claude/GPT, {name})"),
                        None => format!("{base_name} (Claude/GPT)"),
                    };
                    lines.push(format!("{label}: --"));
                }
                if let Some(usage) = data.antigravity_external() {
                    let weekly_label = usage
                        .weekly_label
                        .as_deref()
                        .unwrap_or(strings.weekly_window);
                    let session_label = usage
                        .session_label
                        .as_deref()
                        .unwrap_or(strings.session_window);
                    let label = match acct {
                        Some(name) => format!("{base_name} (Claude/GPT, {name})"),
                        None => format!("{base_name} (Claude/GPT)"),
                    };
                    lines.push(format!(
                        "{} {}: {} | {}: {}",
                        label,
                        session_label,
                        window_text(&usage.session),
                        weekly_label,
                        window_text(&usage.weekly),
                    ));
                }
            }
        } else {
            let Some(usage) = data.get(provider) else {
                continue;
            };
            let descriptor = provider.descriptor();
            let weekly_label = usage
                .weekly_label
                .as_deref()
                .unwrap_or(strings.weekly_window);
            let session_label = usage
                .session_label
                .as_deref()
                .unwrap_or(strings.session_window);
            let label = match data.selected_account_name(provider) {
                Some(name) => format!("{} ({name})", language.text(descriptor.display_name)),
                None => language.text(descriptor.display_name).to_string(),
            };
            if provider == ProviderId::Claude && !usage.session.available {
                lines.push(format!(
                    "{label} {weekly_label}: {}",
                    window_text(&usage.weekly)
                ));
                continue;
            }
            lines.push(format!(
                "{} {}: {} | {}: {}",
                label,
                session_label,
                if detailed_at.is_some() {
                    window_text(&usage.session)
                } else {
                    format!("{:.0}%", shown(usage.session.percentage))
                },
                weekly_label,
                if detailed_at.is_some() {
                    window_text(&usage.weekly)
                } else {
                    format!("{:.0}%", shown(usage.weekly.percentage))
                },
            ));
        }
    }
    lines
}

fn tray_usage_summary_from_state() -> Option<String> {
    tray_usage_summary_for_provider_from_state(None)
}

fn tray_usage_summary_for_provider_from_state(provider: Option<ProviderId>) -> Option<String> {
    let state = lock_state();
    let state = state.as_ref()?;
    let providers = provider
        .map(|provider| ProviderSet::from_enabled([provider]))
        .unwrap_or(state.providers);
    let errors = tray_error_lines(
        state.data.as_ref(),
        state.last_poll_failure,
        providers,
        state.language,
    );
    if !errors.is_empty() {
        return Some(errors.join("\n"));
    }
    if !state.last_poll_ok {
        return None;
    }
    let lines = tray_usage_summary_lines_with_pools(
        state.data.as_ref()?,
        providers,
        state.language,
        state.usage_countdown,
        state.show_antigravity_gemini,
        state.show_antigravity_external,
    );
    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub(super) fn tray_error_lines(
    data: Option<&AppUsageData>,
    failure: Option<poller::PollFailure>,
    providers: ProviderSet,
    language: LanguageId,
) -> Vec<String> {
    let mut accounts: Vec<_> = data
        .into_iter()
        .flat_map(|data| &data.accounts)
        .filter(|account| providers.contains(account.provider) && account.profile.enabled)
        .filter_map(|account| account.error.map(|error| (account, error)))
        .collect();
    // Windows truncates tray tooltips: put the selected account's error first.
    accounts.sort_by_key(|(account, _)| !account.selected);
    let mut lines: Vec<_> = accounts
        .into_iter()
        .map(|(account, error)| {
            format!(
                "{} ({}): {}",
                language.text(account.provider.descriptor().display_name),
                account.profile.name,
                error.message(language),
            )
        })
        .collect();
    if let Some(failure) = failure.filter(|failure| providers.contains(failure.provider)) {
        if !data.is_some_and(|data| {
            data.accounts
                .iter()
                .any(|account| account.provider == failure.provider && account.error.is_some())
        }) {
            lines.push(format!(
                "{}: {}",
                language.text(failure.provider.descriptor().display_name),
                failure.error.message(language)
            ));
        }
    }
    lines
}

fn tray_icon_tooltip_from_state() -> String {
    tray_usage_summary_from_state().unwrap_or_else(|| {
        lock_state()
            .as_ref()
            .map(|state| state.language.strings().window_title.to_string())
            .unwrap_or_else(|| "Claude Code Usage Monitor".to_string())
    })
}

pub(super) fn sync_tray_icon(hwnd: HWND) {
    let hwnd = controller_for(hwnd);
    widget_tooltip::refresh();
    let usage_tooltip = tray_usage_summary_from_state();
    let themed = {
        let state = lock_state();
        state.as_ref().and_then(|state| {
            effective_theme_from_state(state)
                .map(|theme| (theme, state.data.clone(), theme_runtime_from_state(state)))
        })
    };
    if let Some((theme, data, runtime)) = themed {
        let has_tray_surfaces = theme.surfaces.iter().any(|surface| {
            surface
                .placement
                .nest
                .resolve(surface.placement.reference.region)
                == SurfaceNest::TrayIcon
        });
        if has_tray_surfaces {
            let icons = theme
                .surfaces
                .iter()
                .enumerate()
                .filter(|(surface_index, surface)| {
                    let surface_runtime =
                        theme_runtime_for_surface(&theme, *surface_index, runtime);
                    surface
                        .placement
                        .nest
                        .resolve(surface.placement.reference.region)
                        == SurfaceNest::TrayIcon
                        && theme_engine::surface_should_render(
                            &theme,
                            *surface_index,
                            data.as_ref(),
                            surface_runtime,
                        )
                })
                .filter_map(|(surface_index, surface)| {
                    let surface_runtime = theme_runtime_for_surface(&theme, surface_index, runtime);
                    let (logical_width, logical_height) = theme_engine::resolve_surface_size(
                        &theme,
                        surface_index,
                        data.as_ref(),
                        surface_runtime,
                    );
                    let max_dimension = logical_width.max(logical_height) as f64;
                    let scale =
                        theme_surface_scale(&theme, surface_index).min(if max_dimension > 0.0 {
                            512.0 / max_dimension
                        } else {
                            1.0
                        });
                    if scale < 0.25 {
                        diagnose::log(format!(
                            "tray-icon theme surface '{}' exceeds the 512px source limit",
                            surface.name
                        ));
                        return None;
                    }
                    let rendered = theme_engine::render_theme_surface_with_runtime_at_scale(
                        &theme,
                        surface_index,
                        data.as_ref(),
                        surface_runtime,
                        scale,
                    );
                    Some(tray_icon::ThemedTrayIcon {
                        surface_index,
                        surface_id: surface.id.clone(),
                        label: tray_icon::surface_label(&surface.id, &surface.name),
                        tooltip: match tray_icon::surface_provider(&surface.id) {
                            Some(provider) => {
                                tray_usage_summary_for_provider_from_state(Some(provider))
                                    .unwrap_or_default()
                            }
                            None => usage_tooltip
                                .clone()
                                .unwrap_or_else(|| surface.name.clone()),
                        },
                        width: rendered.width,
                        height: rendered.height,
                        pixels: rendered.pixels,
                    })
                })
                .collect::<Vec<_>>();
            tray_icon::sync_themed(hwnd, &icons);
            return;
        }
    }
    tray_icon::sync(hwnd, &tray_icon_tooltip_from_state());
}

pub(super) fn theme_tray_uses_current_time(theme: &ThemeDocument) -> bool {
    theme
        .surfaces
        .iter()
        .enumerate()
        .filter(|(_, surface)| {
            surface
                .placement
                .nest
                .resolve(surface.placement.reference.region)
                == SurfaceNest::TrayIcon
        })
        .any(|(surface_index, _)| {
            theme
                .surface_current_time_refresh_interval(surface_index)
                .is_some()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{UsageData, UsageSection};

    fn usage(session: f64, weekly: f64, weekly_label: Option<&str>) -> UsageData {
        UsageData {
            session: UsageSection {
                available: true,
                percentage: session,
                resets_at: None,
            },
            weekly: UsageSection {
                available: true,
                percentage: weekly,
                resets_at: None,
            },
            weekly_label: weekly_label.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn tray_errors_distinguish_causes_and_disappear_after_recovery() {
        use crate::poller::{PollError, PollFailure};
        let providers = ProviderSet::from_enabled([ProviderId::Claude]);
        for (error, expected) in [
            (PollError::TokenExpired, "Login expired"),
            (PollError::NoCredentials, "No usable login"),
            (PollError::AuthRequired, "Login rejected"),
            (PollError::RequestFailed, "Usage request failed"),
            (PollError::NetworkError, "Service unreachable"),
            (PollError::UnexpectedResponse, "Unexpected usage response"),
            (PollError::HttpStatus(429), "HTTP 429"),
        ] {
            let failure = PollFailure {
                provider: ProviderId::Claude,
                error,
            };
            let lines = tray_error_lines(None, Some(failure), providers, LanguageId::English);
            assert_eq!(lines.len(), 1);
            assert!(lines[0].contains(expected), "{:?}", lines);
            assert!(tray_error_lines(
                None,
                Some(failure),
                ProviderSet::from_enabled([ProviderId::Codex]),
                LanguageId::English
            )
            .is_empty());
        }
        assert!(tray_error_lines(None, None, providers, LanguageId::English).is_empty());
        let mut data = AppUsageData::default();
        for (name, selected) in [("Work", false), ("Personal", true)] {
            data.accounts.push(crate::models::AccountUsage {
                provider: ProviderId::Claude,
                profile: crate::accounts::AccountProfile {
                    name: name.into(),
                    enabled: true,
                    ..Default::default()
                },
                source_signature: String::new(),
                source_path: None,
                usage: None,
                error: Some(PollError::TokenExpired),
                selected,
            });
        }
        let lines = tray_error_lines(Some(&data), None, providers, LanguageId::English);
        assert!(lines[0].starts_with("Claude Code (Personal): Login expired"));
        assert!(lines[1].contains("Work"));
        for account in &mut data.accounts {
            account.error = None;
        }
        assert!(tray_error_lines(Some(&data), None, providers, LanguageId::English).is_empty());
        let instruction = LanguageId::English
            .provider_auth_error(ProviderId::Claude)
            .1;
        assert!(instruction.contains("desktop app") && instruction.contains("/login"));
    }

    #[test]
    fn claude_summaries_omit_an_absent_five_hour_window() {
        let mut reading = usage(0.0, 26.0, None);
        reading.session = UsageSection::default();
        let data = [(ProviderId::Claude, reading)].into_iter().collect();
        let providers = ProviderSet::from_enabled([ProviderId::Claude]);
        for (countdown, expected) in [(false, "26"), (true, "74")] {
            assert_eq!(
                tray_usage_summary_lines(&data, providers, LanguageId::English, countdown),
                [format!("Claude Code 7d: {expected}%")]
            );
            assert_eq!(
                usage_summary_lines(
                    &data,
                    providers,
                    LanguageId::English,
                    countdown,
                    true,
                    true,
                    Some(UNIX_EPOCH),
                ),
                [format!("Claude Code 7d: {expected}.0%")]
            );
        }
    }

    #[test]
    fn tray_summary_formats_enabled_provider_usage() {
        let data = [(ProviderId::Claude, usage(4.6, 42.4, None))]
            .into_iter()
            .collect();

        assert_eq!(
            tray_usage_summary_lines(
                &data,
                ProviderSet::from_enabled([ProviderId::Claude]),
                LanguageId::English,
                false,
            ),
            ["Claude Code 5h: 5% | 7d: 42%"]
        );
    }

    #[test]
    fn widget_summary_adds_precision_and_reset_times_without_changing_tray_text() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut reading = usage(4.64, 42.37, Some("30d"));
        reading.session.resets_at = Some(now + Duration::from_secs(11_520));
        reading.weekly.resets_at = Some(now + Duration::from_secs(183_600));
        let data = [(ProviderId::Claude, reading)].into_iter().collect();
        let providers = ProviderSet::from_enabled([ProviderId::Claude]);
        assert_eq!(
            usage_summary_lines(
                &data,
                providers,
                LanguageId::English,
                false,
                true,
                true,
                Some(now)
            ),
            ["Claude Code 5h: 4.6% (3h 12m) | 30d: 42.4% (2d 3h)"]
        );
        assert_eq!(
            usage_summary_lines(
                &data,
                providers,
                LanguageId::English,
                true,
                true,
                true,
                Some(now)
            ),
            ["Claude Code 5h: 95.4% (3h 12m) | 30d: 57.6% (2d 3h)"]
        );
        assert_eq!(
            tray_usage_summary_lines(&data, providers, LanguageId::English, false),
            ["Claude Code 5h: 5% | 30d: 42%"]
        );
    }

    #[test]
    fn widget_summary_preserves_pool_selection_and_missing_windows() {
        let now = UNIX_EPOCH;
        let mut reading = usage(12.34, 56.78, None);
        reading.weekly = UsageSection::default();
        let data = [(ProviderId::Antigravity, reading)].into_iter().collect();
        let providers = ProviderSet::from_enabled([ProviderId::Antigravity]);
        let lines = usage_summary_lines(
            &data,
            providers,
            LanguageId::English,
            false,
            true,
            true,
            Some(now),
        );
        assert!(
            lines[0].ends_with("(Gemini) 5h: 12.3% | 7d: --"),
            "{lines:?}"
        );
        assert!(lines[1].ends_with("(Claude/GPT): --"), "{lines:?}");
        assert!(usage_summary_lines(
            &data,
            providers,
            LanguageId::English,
            false,
            false,
            false,
            Some(now)
        )
        .is_empty());
        assert!(usage_summary_lines(
            &data,
            ProviderSet::from_enabled([ProviderId::Claude]),
            LanguageId::English,
            false,
            true,
            true,
            Some(now)
        )
        .is_empty());
    }

    #[test]
    fn tray_summary_counts_down_when_the_widget_shows_what_is_left() {
        let data = [(ProviderId::Claude, usage(4.6, 42.4, None))]
            .into_iter()
            .collect();

        assert_eq!(
            tray_usage_summary_lines(
                &data,
                ProviderSet::from_enabled([ProviderId::Claude]),
                LanguageId::English,
                true,
            ),
            ["Claude Code 5h: 95% | 7d: 58%"]
        );
    }

    #[test]
    fn tray_summary_uses_provider_window_labels_and_selection() {
        let data = [
            (ProviderId::Claude, usage(10.0, 20.0, None)),
            (ProviderId::OpenCode, usage(30.0, 40.0, Some("30d"))),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            tray_usage_summary_lines(
                &data,
                ProviderSet::from_enabled([ProviderId::OpenCode]),
                LanguageId::English,
                false,
            ),
            ["OpenCode 5h: 30% | 30d: 40%"]
        );
    }

    #[test]
    fn tray_summary_prefers_the_reported_session_window_label() {
        // Cursor's included-usage window is not the default five-hour session.
        let mut data = usage(7.1, 0.0, Some("API"));
        data.session_label = Some("7d".into());
        let data = [(ProviderId::Cursor, data)].into_iter().collect();

        assert_eq!(
            tray_usage_summary_lines(
                &data,
                ProviderSet::from_enabled([ProviderId::Cursor]),
                LanguageId::English,
                false,
            ),
            ["Cursor 7d: 7% | API: 0%"]
        );
        assert_eq!(
            usage_summary_lines(
                &data,
                ProviderSet::from_enabled([ProviderId::Cursor]),
                LanguageId::English,
                false,
                true,
                true,
                Some(UNIX_EPOCH),
            ),
            ["Cursor 7d: 7.1% | API: 0.0%"]
        );
    }
}
