//! The live values theme expressions read: provider usage, runtime state and
//! per-object layout, bound into a `DataContext`.

use super::*;

#[derive(Clone, Debug, Default)]
pub struct DataContext {
    pub(super) values: HashMap<String, f64>,
    pub(super) strings: HashMap<String, String>,
    pub(super) force_segoe_ui: bool,
}

/// Runtime environment used by theme expressions and layout. Provider state is
/// deliberately independent of polling availability so a temporary provider
/// error never causes the widget to jump or resize.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeRuntime {
    providers: ProviderSet,
    pub poll_ok: bool,
    pub has_error: bool,
    pub language: LanguageId,
    /// Present each allowance as what is left rather than what is spent. Only
    /// the `.display` values and summaries using them follow this; `.percentage`
    /// always means consumption so severity thresholds keep their meaning.
    pub countdown: bool,
    pub use_windows_default_font: bool,
    pub surface_nest: SurfaceNest,
    pub floating_card_opacity: u8,
    pub antigravity_gemini_enabled: bool,
    pub antigravity_external_enabled: bool,
    pub crossfade_opacity: f64,
    host_width: u32,
    host_height: u32,
}

impl Default for ThemeRuntime {
    fn default() -> Self {
        Self {
            providers: ProviderSet::default(),
            poll_ok: true,
            has_error: false,
            language: LanguageId::English,
            countdown: false,
            use_windows_default_font: true,
            surface_nest: SurfaceNest::Taskbar,
            floating_card_opacity: 85,
            antigravity_gemini_enabled: true,
            antigravity_external_enabled: false,
            crossfade_opacity: 1.0,
            host_width: default_canvas_width(),
            host_height: default_canvas_height(),
        }
    }
}

impl ThemeRuntime {
    #[cfg(test)]
    pub fn new(claude_enabled: bool, codex_enabled: bool, antigravity_enabled: bool) -> Self {
        let providers = ProviderSet::from_enabled(
            [
                (ProviderId::Claude, claude_enabled),
                (ProviderId::Codex, codex_enabled),
                (ProviderId::Antigravity, antigravity_enabled),
            ]
            .into_iter()
            .filter_map(|(provider, enabled)| enabled.then_some(provider)),
        );
        Self::from_providers(providers)
    }

    pub fn from_providers(providers: ProviderSet) -> Self {
        let agy = providers.contains(ProviderId::Antigravity);
        Self {
            providers,
            poll_ok: true,
            has_error: false,
            language: LanguageId::English,
            countdown: false,
            use_windows_default_font: true,
            surface_nest: SurfaceNest::Taskbar,
            floating_card_opacity: 85,
            antigravity_gemini_enabled: agy,
            antigravity_external_enabled: false,
            crossfade_opacity: 1.0,
            host_width: default_canvas_width(),
            host_height: default_canvas_height(),
        }
    }

    pub fn with_antigravity_pools(
        mut self,
        gemini: bool,
        external: bool,
        crossfade_opacity: f64,
    ) -> Self {
        self.antigravity_gemini_enabled = gemini;
        self.antigravity_external_enabled = external;
        self.crossfade_opacity = crossfade_opacity;
        self
    }

    pub fn with_nest(mut self, nest: SurfaceNest) -> Self {
        self.surface_nest = nest;
        self
    }

    pub fn with_floating_card_opacity(mut self, opacity: u8) -> Self {
        self.floating_card_opacity = opacity;
        self
    }

    pub fn with_poll_state(mut self, poll_ok: bool, has_error: bool) -> Self {
        self.poll_ok = poll_ok;
        self.has_error = has_error;
        self
    }

    pub fn with_language(mut self, language: LanguageId) -> Self {
        self.language = language;
        self
    }

    /// Count each allowance down towards its limit instead of up from zero.
    pub fn with_countdown(mut self, countdown: bool) -> Self {
        self.countdown = countdown;
        self
    }

    pub fn with_windows_default_font(mut self, enabled: bool) -> Self {
        self.use_windows_default_font = enabled;
        self
    }

    /// Supply the selected native host's 96-DPI logical dimensions. Theme
    /// expressions consume these as `host.width` and `host.height`.
    pub fn with_host_dimensions(mut self, width: u32, height: u32) -> Self {
        self.host_width = width.max(1);
        self.host_height = height.max(1);
        self
    }

    pub fn host_dimensions(self) -> (u32, u32) {
        (self.host_width, self.host_height)
    }

    pub fn provider_count(self) -> usize {
        self.providers.len().max(1)
    }

    pub fn provider_enabled(self, provider: ProviderId) -> bool {
        self.providers.contains(provider)
    }
}

impl DataContext {
    pub fn from_usage(data: Option<&AppUsageData>, canvas: &Canvas) -> Self {
        Self::from_usage_with_runtime(data, canvas, ThemeRuntime::default())
    }

    pub fn from_usage_with_runtime(
        data: Option<&AppUsageData>,
        canvas: &Canvas,
        runtime: ThemeRuntime,
    ) -> Self {
        let mut context = Self {
            force_segoe_ui: !runtime.use_windows_default_font,
            ..Self::default()
        };
        context.insert("canvas.width", canvas.width as f64);
        context.insert("canvas.height", canvas.height as f64);
        context.insert("parent.width", canvas.width as f64);
        context.insert("parent.height", canvas.height as f64);
        context.insert("host.width", runtime.host_width as f64);
        context.insert("host.height", runtime.host_height as f64);
        context.insert("pi", std::f64::consts::PI);
        context.insert("e", std::f64::consts::E);
        context.insert("true", 1.0);
        context.insert("false", 0.0);
        context.insert_string("app.version", env!("CARGO_PKG_VERSION"));
        let mut version_parts = env!("CARGO_PKG_VERSION")
            .split(['.', '-', '+'])
            .filter_map(|part| part.parse::<f64>().ok());
        context.insert("app.version.major", version_parts.next().unwrap_or(0.0));
        context.insert("app.version.minor", version_parts.next().unwrap_or(0.0));
        context.insert("app.version.patch", version_parts.next().unwrap_or(0.0));
        context.insert("system.dark", render_dark_mode() as u8 as f64);
        context.insert("data.poll_ok", runtime.poll_ok as u8 as f64);
        context.insert("data.has_error", runtime.has_error as u8 as f64);
        context.insert(
            "data.loading",
            (!runtime.poll_ok && !runtime.has_error) as u8 as f64,
        );
        let strings = runtime.language.strings();
        context.insert_string("i18n.locale", runtime.language.code());
        context.insert_string("i18n.session_window", strings.session_window);
        context.insert_string("i18n.weekly_window", strings.weekly_window);
        context.insert_string("i18n.cursor_auto_window", strings.cursor_auto_window);
        context.insert_string("i18n.cursor_api_window", strings.cursor_api_window);
        context.insert_string("i18n.now", strings.now);
        context.insert_string("i18n.day_suffix", strings.day_suffix);
        context.insert_string("i18n.hour_suffix", strings.hour_suffix);
        context.insert_string("i18n.minute_suffix", strings.minute_suffix);
        context.insert_string("i18n.second_suffix", strings.second_suffix);
        context.insert("providers.count", runtime.provider_count() as f64);
        let now = render_now_unix();
        context.insert("time.now.unix", now);
        context.insert("time.now.milliseconds", now * 1000.0);
        for (zone, local) in [("local", true), ("utc", false)] {
            if let Some(value) = timestamp_parts(now, local) {
                for (component, value) in [
                    ("year", value.year),
                    ("month", value.month),
                    ("day", value.day),
                    ("weekday", value.weekday),
                    ("hour", value.hour),
                    ("minute", value.minute),
                    ("second", value.second),
                ] {
                    context.insert(&format!("time.{zone}.{component}"), f64::from(value));
                }
            }
        }
        for descriptor in PROVIDER_DESCRIPTORS {
            context.insert_string(&format!("{}.account.name", descriptor.key), "");
            context.insert(
                &format!("providers.{}.enabled", descriptor.key),
                runtime.provider_enabled(descriptor.id) as u8 as f64,
            );
        }
        context.insert("display.countdown", runtime.countdown as u8 as f64);
        context.insert(
            "antigravity.gemini.enabled",
            runtime.antigravity_gemini_enabled as u8 as f64,
        );
        context.insert(
            "antigravity.external.enabled",
            runtime.antigravity_external_enabled as u8 as f64,
        );
        let agy_dual_mode =
            runtime.antigravity_gemini_enabled && runtime.antigravity_external_enabled;
        context.insert("antigravity.dual_mode", agy_dual_mode as u8 as f64);
        context.insert(
            "antigravity.crossfade_opacity",
            runtime.crossfade_opacity.clamp(0.0, 1.0),
        );
        if let Some(data) = data {
            for account in &data.accounts {
                let key = format!(
                    "accounts.{}.{}",
                    account.provider.descriptor().key,
                    account.profile.id
                );
                context.insert_string(&format!("{key}.name"), &account.profile.name);
                context.insert(&format!("{key}.selected"), account.selected as u8 as f64);
                context.insert(
                    &format!("{key}.has_error"),
                    account.error.is_some() as u8 as f64,
                );
                context.insert_provider(
                    &key,
                    account.usage.as_ref(),
                    account.provider == ProviderId::Codex,
                    runtime.countdown,
                );
            }
            for descriptor in PROVIDER_DESCRIPTORS {
                if let Some(account) = data
                    .accounts
                    .iter()
                    .find(|account| account.provider == descriptor.id && account.selected)
                {
                    context.insert(
                        &format!("{}.has_error", descriptor.key),
                        account.error.is_some() as u8 as f64,
                    );
                }
                context.insert_string(
                    &format!("{}.account.name", descriptor.key),
                    data.selected_account_name(descriptor.id).unwrap_or(""),
                );
                context.insert_provider(
                    descriptor.key,
                    data.get(descriptor.id),
                    descriptor.id == ProviderId::Codex,
                    runtime.countdown,
                );
            }
            if let Some(agy) = data.get(ProviderId::Antigravity) {
                // Antigravity answered this poll; pool-level gaps are "no limit
                // data" ("--"), not a provider error.
                context.insert("antigravity.reported", 1.0);
                context.insert_provider("antigravity.gemini", Some(agy), false, runtime.countdown);
                // A missing external pool stays empty ("--"); never show Gemini figures in its place.
                context.insert_provider(
                    "antigravity.external",
                    agy.external_pool.as_deref(),
                    false,
                    runtime.countdown,
                );
            } else {
                context.insert_provider("antigravity.gemini", None, false, runtime.countdown);
                context.insert_provider("antigravity.external", None, false, runtime.countdown);
            }
            let active = ProviderId::ALL
                .into_iter()
                .find_map(|provider| data.get(provider).map(|usage| (provider, usage)));
            context.insert_provider(
                "active",
                active.map(|(_, usage)| usage),
                active.is_some_and(|(provider, _)| provider == ProviderId::Codex),
                runtime.countdown,
            );
        } else {
            for descriptor in PROVIDER_DESCRIPTORS {
                context.insert_provider(
                    descriptor.key,
                    None,
                    descriptor.id == ProviderId::Codex,
                    runtime.countdown,
                );
            }
            context.insert_provider("antigravity.gemini", None, false, runtime.countdown);
            context.insert_provider("antigravity.external", None, false, runtime.countdown);
            context.insert_provider("active", None, false, runtime.countdown);
        }
        context
    }

    fn insert_provider(
        &mut self,
        name: &str,
        usage: Option<&crate::models::UsageData>,
        codex_compatibility: bool,
        countdown: bool,
    ) {
        // What a gauge or a badge should show. `percentage` stays the share
        // that has been spent so warning thresholds keep working, while
        // `display` follows the countdown setting.
        self.insert_limits(name, usage, countdown);
        let display = |percentage: f64| {
            if countdown {
                100.0 - percentage
            } else {
                percentage
            }
        };
        let weekly_label = usage
            .and_then(|usage| usage.weekly_label.as_deref())
            .or_else(|| self.get_string("i18n.weekly_window"))
            .unwrap_or("7d")
            .to_string();
        self.insert_string(&format!("{name}.weekly.label"), weekly_label);
        let session_label = usage
            .and_then(|usage| usage.session_label.as_deref())
            .or_else(|| self.get_string("i18n.session_window"))
            .unwrap_or("5h")
            .to_string();
        self.insert_string(&format!("{name}.session.label"), session_label);
        let (five_hour, weekly) = usage
            .map(|usage| (usage.session.percentage, usage.weekly.percentage))
            .unwrap_or((0.0, 0.0));
        // Before Codex windows were classified by duration, a weekly-only
        // response arrived in the primary slot and was exposed to themes as
        // `codex.session`. Keep that established binding working for existing
        // custom themes, while `codex.five_hour` always means the real window.
        let use_codex_session_fallback = codex_compatibility
            && usage.is_some_and(|usage| !usage.session.available && usage.weekly.available);
        let session = if use_codex_session_fallback {
            weekly
        } else {
            five_hour
        };
        let (session_available, weekly_available) = usage
            .map(|usage| {
                (
                    usage.session.available || use_codex_session_fallback,
                    usage.weekly.available,
                )
            })
            .unwrap_or((false, false));
        self.insert(
            &format!("{name}.session.available"),
            session_available as u8 as f64,
        );
        self.insert(
            &format!("{name}.five_hour.available"),
            usage.is_some_and(|usage| usage.session.available) as u8 as f64,
        );
        self.insert(
            &format!("{name}.weekly.available"),
            weekly_available as u8 as f64,
        );
        self.insert(&format!("{name}.session.percentage"), session);
        self.insert(&format!("{name}.session.remaining"), 100.0 - session);
        // An Antigravity pool window that was not reported draws an empty
        // gauge in either usage direction instead of a full "remaining" one.
        let shown_session = if name.starts_with("antigravity") && !session_available {
            0.0
        } else {
            display(session)
        };
        self.insert(&format!("{name}.session.display"), shown_session);
        self.insert(&format!("{name}.five_hour.percentage"), five_hour);
        self.insert(&format!("{name}.five_hour.remaining"), 100.0 - five_hour);
        // An Antigravity pool window that was not reported draws an empty
        // gauge in either usage direction instead of a full "remaining" one.
        let shown_five_hour = if name.starts_with("antigravity")
            && !usage.is_some_and(|usage| usage.session.available)
        {
            0.0
        } else {
            display(five_hour)
        };
        self.insert(&format!("{name}.five_hour.display"), shown_five_hour);
        self.insert(&format!("{name}.weekly.percentage"), weekly);
        self.insert(&format!("{name}.weekly.remaining"), 100.0 - weekly);
        // An Antigravity pool window that was not reported draws an empty
        // gauge in either usage direction instead of a full "remaining" one.
        let shown_weekly = if name.starts_with("antigravity") && !weekly_available {
            0.0
        } else {
            display(weekly)
        };
        self.insert(&format!("{name}.weekly.display"), shown_weekly);
        let monthly = usage.and_then(|usage| usage.monthly.as_ref());
        self.insert_string(&format!("{name}.monthly.label"), "30d");
        if let Some(monthly) = monthly {
            self.insert(&format!("{name}.monthly.percentage"), monthly.percentage);
            self.insert(
                &format!("{name}.monthly.remaining"),
                100.0 - monthly.percentage,
            );
            self.insert(
                &format!("{name}.monthly.display"),
                display(monthly.percentage),
            );
        } else {
            self.insert(&format!("{name}.monthly.percentage"), 0.0);
            self.insert(&format!("{name}.monthly.remaining"), 100.0);
            self.insert(&format!("{name}.monthly.display"), display(0.0));
        }
        self.insert(
            &format!("{name}.monthly.available"),
            monthly.is_some() as u8 as f64,
        );
        self.insert(&format!("{name}.available"), usage.is_some() as u8 as f64);
        // Carried over from an earlier poll: real figures, not current ones.
        self.insert(
            &format!("{name}.stale"),
            usage.is_some_and(|usage| usage.stale) as u8 as f64,
        );
        // Credits are absent for most accounts, so `credits.available` is what
        // a theme should gate the overlay on rather than `available`.
        let credits = usage.and_then(|usage| usage.credits.as_ref());
        let credits_percentage = credits.map(|credits| credits.percentage).unwrap_or(0.0);
        self.insert(&format!("{name}.credits.percentage"), credits_percentage);
        self.insert(
            &format!("{name}.credits.remaining"),
            100.0 - credits_percentage,
        );
        self.insert(
            &format!("{name}.credits.display"),
            display(credits_percentage),
        );
        // Currency, unlike the percentages either side of it.
        self.insert(
            &format!("{name}.credits.balance"),
            credits.map(|credits| credits.remaining).unwrap_or(0.0),
        );
        self.insert(
            &format!("{name}.credits.total"),
            credits.map(|credits| credits.total).unwrap_or(0.0),
        );
        self.insert(
            &format!("{name}.credits.available"),
            credits.is_some() as u8 as f64,
        );
        // The single figure a badge should show: whatever is closest to its
        // limit. A provider can switch a window off entirely -- Codex has its
        // five-hour window disabled -- so binding a badge to one window alone
        // leaves it reporting 0% while another allowance is spent.
        let headline = match credits {
            Some(credits) => credits.percentage,
            None => five_hour.max(weekly),
        };
        self.insert(&format!("{name}.headline.percentage"), headline);
        self.insert(&format!("{name}.headline.remaining"), 100.0 - headline);
        self.insert(&format!("{name}.headline.display"), display(headline));
        let reset_value = |reset: Option<std::time::SystemTime>| {
            let unix = reset
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|value| value.as_secs_f64())
                .unwrap_or(0.0);
            let seconds = reset
                .and_then(|value| value.duration_since(render_now()).ok())
                .map(|value| value.as_secs_f64())
                .unwrap_or(0.0);
            (unix, seconds)
        };
        let (five_hour_unix, five_hour_seconds) =
            reset_value(usage.and_then(|value| value.session.resets_at));
        let (weekly_unix, weekly_seconds) =
            reset_value(usage.and_then(|value| value.weekly.resets_at));
        let (session_unix, session_seconds) = if use_codex_session_fallback {
            (weekly_unix, weekly_seconds)
        } else {
            (five_hour_unix, five_hour_seconds)
        };
        let five_hour_available = usage.is_some_and(|value| value.session.available);
        let weekly_available = usage.is_some_and(|value| value.weekly.available);
        let session_available = if use_codex_session_fallback {
            weekly_available
        } else {
            five_hour_available
        };
        // Presence is independent of percentage and reset times. Monthly
        // availability remains based on its explicitly optional section.
        for (window, available) in [
            ("session", session_available),
            ("five_hour", five_hour_available),
            ("weekly", weekly_available),
        ] {
            self.insert(
                &format!("{name}.{window}.available"),
                available as u8 as f64,
            );
        }
        let (monthly_unix, monthly_seconds) =
            reset_value(monthly.and_then(|value| value.resets_at));
        for (window, unix, seconds) in [
            ("session", session_unix, session_seconds),
            ("five_hour", five_hour_unix, five_hour_seconds),
            ("weekly", weekly_unix, weekly_seconds),
            ("monthly", monthly_unix, monthly_seconds),
        ] {
            self.insert(&format!("{name}.{window}.reset.unix"), unix);
            self.insert(&format!("{name}.{window}.reset.seconds"), seconds);
            self.insert(&format!("{name}.{window}.reset.minutes"), seconds / 60.0);
            self.insert(&format!("{name}.{window}.reset.hours"), seconds / 3600.0);
            self.insert(&format!("{name}.{window}.reset.days"), seconds / 86400.0);
        }
    }

    pub fn insert(&mut self, name: &str, value: f64) {
        self.values.insert(name.to_ascii_lowercase(), value);
    }

    /// Whether a Builder catalogue can be reused. Clock rows are refreshed
    /// separately because the fractional Unix timestamp changes every frame.
    pub(crate) fn same_catalogue_data(&self, other: &Self) -> bool {
        self.strings == other.strings
            && self.values.len() == other.values.len()
            && self.values.iter().all(|(name, value)| {
                other.values.get(name).is_some_and(|other| {
                    name.starts_with("time.") || value.to_bits() == other.to_bits()
                })
            })
    }

    pub fn get(&self, name: &str) -> Option<f64> {
        let name = name.to_ascii_lowercase();
        self.values
            .get(&name)
            .copied()
            .or_else(|| self.limit_default(&name))
            .or_else(|| {
                let key = Self::account_default_key(&name)?;
                let value = *Self::account_defaults().values.get(&key)?;
                Some(
                    if key.ends_with(".display")
                        && self.values.get("display.countdown").copied().unwrap_or(0.0) != 0.0
                    {
                        100.0 - value
                    } else {
                        value
                    },
                )
            })
    }

    /// Every number and text name set in this context, sorted. Names that only
    /// resolve through defaults (offline accounts and quotas) are not listed.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<_> = self
            .values
            .keys()
            .chain(self.strings.keys())
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    pub fn insert_string(&mut self, name: &str, value: impl Into<String>) {
        self.strings.insert(name.to_ascii_lowercase(), value.into());
    }

    pub fn get_string(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.strings
            .get(&name)
            .map(String::as_str)
            .or_else(|| Self::limit_string_default(&name))
            .or_else(|| {
                let key = Self::account_default_key(&name)?;
                Self::account_defaults()
                    .strings
                    .get(&key)
                    .map(String::as_str)
            })
    }

    // Account IDs are dynamic, but their fields use the same schema as provider
    // bindings. Supply typed defaults during validation, startup and removal;
    // unknown fields must still report typos instead of silently becoming zero.
    fn account_default_key(name: &str) -> Option<String> {
        let (provider, rest) = name.strip_prefix("accounts.")?.split_once('.')?;
        let (id, field) = rest.split_once('.')?;
        (matches!(provider, "claude" | "codex")
            && !id.is_empty()
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
        .then(|| format!("account.{field}"))
    }

    fn account_defaults() -> &'static Self {
        static DEFAULTS: OnceLock<DataContext> = OnceLock::new();
        DEFAULTS.get_or_init(|| {
            let mut context = Self::default();
            context.insert_provider("account", None, false, false);
            context.insert_string("account.name", "");
            context.insert("account.selected", 0.0);
            context.insert("account.has_error", 0.0);
            context
        })
    }

    pub fn with_object(mut self, object: &ResolvedObject<'_>) -> Self {
        self.insert("object.x", object.x);
        self.insert("object.y", object.y);
        self.insert("object.width", object.width);
        self.insert("object.height", object.height);
        self.insert("this.x", object.x);
        self.insert("this.y", object.y);
        self.insert("this.width", object.width);
        self.insert("this.height", object.height);
        self.insert("this.gap", object.gap);
        self.insert("parent.width", object.parent_width);
        self.insert("parent.height", object.parent_height);
        self
    }
}
