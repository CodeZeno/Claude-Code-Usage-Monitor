use super::*;
use crate::models::{limit_slug, UsageData, UsageLimit};

impl DataContext {
    pub(super) fn insert_limits(&mut self, name: &str, usage: Option<&UsageData>, countdown: bool) {
        let limits = usage
            .map(|usage| usage.limits.as_slice())
            .unwrap_or_default();
        self.insert(&format!("{name}.limits.count"), limits.len() as f64);
        for limit in limits {
            self.insert_limit(&format!("{name}.limits.{}", limit.key), limit, countdown);
        }
        // The model shortcut means a weekly model cap. Other kinds remain
        // individually addressable through limits.<key>, avoiding ambiguity.
        let mut models = std::collections::BTreeMap::<String, Vec<&UsageLimit>>::new();
        for limit in limits.iter().filter(|limit| {
            matches!(
                limit.kind.as_str(),
                "weekly_scoped" | "seven_day_opus" | "seven_day_sonnet"
            )
        }) {
            if let Some(model) = &limit.model {
                models.entry(limit_slug(model)).or_default().push(limit);
            }
        }
        for (model, candidates) in models {
            // Never select an arbitrary quota when two distinct scopes share
            // a model name. The full limit keys remain available in Studio.
            if let [limit] = candidates.as_slice() {
                self.insert_limit(&format!("{name}.model.{model}"), limit, countdown);
            }
        }
        let active: Vec<_> = limits
            .iter()
            .filter(|limit| limit.is_active && (limit.scope.is_some() || limit.model.is_some()))
            .collect();
        // Preserve ambiguity rather than guessing which active cap binds.
        if let [limit] = active.as_slice() {
            self.insert_limit(&format!("{name}.scoped"), limit, countdown);
        }
    }

    fn insert_limit(&mut self, base: &str, limit: &UsageLimit, countdown: bool) {
        let percentage = limit.usage.percentage;
        for (metric, value) in [
            ("available", limit.usage.available as u8 as f64),
            ("percentage", percentage),
            ("remaining", 100.0 - percentage),
            (
                "display",
                if countdown {
                    100.0 - percentage
                } else {
                    percentage
                },
            ),
            ("is_active", limit.is_active as u8 as f64),
        ] {
            self.insert(&format!("{base}.{metric}"), value);
        }
        for (field, value) in [
            ("key", limit.key.as_str()),
            ("label", limit.label.as_str()),
            ("kind", limit.kind.as_str()),
            ("model_id", limit.model_id.as_deref().unwrap_or("")),
        ] {
            self.insert_string(&format!("{base}.{field}"), value);
        }
        self.insert_string(
            &format!("{base}.scope"),
            limit
                .scope
                .as_ref()
                .map(serde_json::Value::to_string)
                .unwrap_or_default(),
        );
        let unix = limit
            .usage
            .resets_at
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0);
        let seconds = limit
            .usage
            .resets_at
            .and_then(|at| at.duration_since(std::time::SystemTime::now()).ok())
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(0.0);
        for (unit, value) in [
            ("unix", unix),
            ("seconds", seconds),
            ("minutes", seconds / 60.0),
            ("hours", seconds / 3600.0),
            ("days", seconds / 86400.0),
        ] {
            self.insert(&format!("{base}.reset.{unit}"), value);
        }
    }

    /// Valid dynamic quota paths still validate when the account is offline or
    /// no longer reports the quota. Unknown fields continue to catch typos.
    pub(super) fn limit_field(name: &str) -> Option<(&str, &str)> {
        let (owner, field) = if let Some(rest) = name.strip_prefix("accounts.") {
            let (provider, rest) = rest.split_once('.')?;
            let (id, field) = rest.split_once('.')?;
            if !matches!(provider, "claude" | "codex") || !valid_key(id) {
                return None;
            }
            (&name[..name.len() - field.len() - 1], field)
        } else {
            let (owner, field) = name.split_once('.')?;
            if !matches!(
                owner,
                "claude" | "codex" | "antigravity" | "opencode" | "cursor" | "active"
            ) {
                return None;
            }
            (owner, field)
        };
        if let Some(field) = field.strip_prefix("scoped.") {
            return Some((owner, field));
        }
        let rest = field
            .strip_prefix("limits.")
            .or_else(|| field.strip_prefix("model."))?;
        let (key, field) = rest.split_once('.')?;
        valid_key(key).then_some((owner, field))
    }

    pub(super) fn limit_default(&self, name: &str) -> Option<f64> {
        let (_, field) = Self::limit_field(name)?;
        match field {
            "available" | "percentage" | "is_active" | "reset.unix" | "reset.seconds"
            | "reset.minutes" | "reset.hours" | "reset.days" => Some(0.0),
            "remaining" => Some(100.0),
            "display" => Some(
                if self.values.get("display.countdown").copied().unwrap_or(0.0) != 0.0 {
                    100.0
                } else {
                    0.0
                },
            ),
            _ => None,
        }
    }

    pub(super) fn limit_string_default(name: &str) -> Option<&'static str> {
        let (_, field) = Self::limit_field(name)?;
        matches!(field, "key" | "label" | "kind" | "model_id" | "scope").then_some("")
    }

    pub fn limit_variables(&self) -> Vec<&str> {
        let mut keys: Vec<_> = self
            .values
            .keys()
            .chain(self.strings.keys())
            .map(String::as_str)
            .filter(|key| Self::limit_field(key).is_some() || key.ends_with(".limits.count"))
            .collect();
        keys.sort_unstable();
        keys
    }
}

fn valid_key(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AccountUsage, UsageSection};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn limit() -> UsageLimit {
        UsageLimit {
            key: "weekly_scoped_fable".into(),
            kind: "weekly_scoped".into(),
            label: "Fable".into(),
            model: Some("Fable".into()),
            is_active: true,
            usage: UsageSection {
                available: true,
                percentage: 91.0,
                resets_at: Some(SystemTime::now() + Duration::from_secs(7200)),
            },
            ..Default::default()
        }
    }

    #[test]
    fn account_limits_are_independent_and_missing_fields_have_typed_defaults() {
        let mut data = AppUsageData::default();
        for (id, percentage) in [("personal", 91.0), ("work", 12.0)] {
            let mut limit = limit();
            limit.usage.percentage = percentage;
            data.accounts.push(AccountUsage {
                provider: ProviderId::Claude,
                profile: crate::accounts::AccountProfile {
                    id: id.into(),
                    name: id.into(),
                    enabled: true,
                    ..Default::default()
                },
                source_signature: String::new(),
                source_path: None,
                usage: Some(UsageData {
                    limits: vec![limit],
                    ..Default::default()
                }),
                error: None,
                selected: id == "personal",
            });
        }
        let context = DataContext::from_usage(Some(&data), &Canvas::default());
        assert_eq!(
            context.get("accounts.claude.personal.model.fable.percentage"),
            Some(91.0)
        );
        assert_eq!(
            context.get("accounts.claude.work.model.fable.percentage"),
            Some(12.0)
        );
        assert_eq!(
            format_template("{accounts.claude.work.model.fable:usage_badge}", &context),
            "12%"
        );
        assert!(
            context
                .get("accounts.claude.work.model.fable.reset.seconds")
                .unwrap()
                > 7100.0
        );
        assert!(context
            .limit_variables()
            .contains(&"accounts.claude.work.limits.weekly_scoped_fable.label"));

        for data in [None, Some(&AppUsageData::default())] {
            let context = DataContext::from_usage_with_runtime(
                data,
                &Canvas::default(),
                ThemeRuntime::default().with_countdown(true),
            );
            for base in [
                "claude.model.future_model",
                "claude.limits.weekly_scoped_future_model",
                "claude.scoped",
                "accounts.claude.deleted.model.fable",
                "accounts.claude.work.scoped",
            ] {
                assert_eq!(
                    evaluate(&format!("{base}.available"), &context).unwrap(),
                    0.0
                );
                assert_eq!(
                    evaluate(&format!("{base}.remaining"), &context).unwrap(),
                    100.0
                );
                assert_eq!(
                    evaluate(&format!("{base}.display"), &context).unwrap(),
                    100.0
                );
                assert_eq!(
                    evaluate(&format!("{base}.reset.unix"), &context).unwrap(),
                    0.0
                );
                assert_eq!(format_template(&format!("{{{base}.label}}"), &context), "");
                assert!(evaluate(&format!("{base}.percentge"), &context).is_err());
            }
            assert!(evaluate("claud.model.fable.percentage", &context).is_err());
        }
    }

    #[test]
    fn ambiguous_and_absent_quotas_never_choose_an_arbitrary_model_or_active_cap() {
        let first = limit();
        let mut second = first.clone();
        second.key.push_str("_other");
        second.model_id = Some("other-account-scope".into());
        let data = AppUsageData::from_iter([(
            ProviderId::Claude,
            UsageData {
                limits: vec![first, second],
                ..Default::default()
            },
        )]);
        let context = DataContext::from_usage(Some(&data), &Canvas::default());
        assert_eq!(context.get("claude.model.fable.available"), Some(0.0));
        assert_eq!(context.get("claude.scoped.available"), Some(0.0));
        assert_eq!(context.get("claude.limits.count"), Some(2.0));
        assert_eq!(
            format_template("{claude.model.missing:usage_line}", &context),
            "--"
        );
        assert_eq!(
            context.get("claude.limits.weekly_scoped_fable_other.available"),
            Some(1.0)
        );
    }

    #[test]
    fn extra_limits_do_not_change_builtin_rendering_or_headlines() {
        let plain_usage = UsageData {
            session: UsageSection {
                available: true,
                percentage: 29.0,
                resets_at: None,
            },
            weekly: UsageSection {
                available: true,
                percentage: 26.0,
                resets_at: None,
            },
            ..Default::default()
        };
        let plain = AppUsageData::from_iter([(ProviderId::Claude, plain_usage.clone())]);
        let extra = AppUsageData::from_iter([(
            ProviderId::Claude,
            UsageData {
                limits: vec![limit()],
                ..plain_usage
            },
        )]);
        for (_, source) in BUILTIN_THEME_SOURCES {
            let mut theme: ThemeDocument = serde_json::from_str(source).unwrap();
            theme.prepare_runtime();
            for countdown in [false, true] {
                let runtime = ThemeRuntime::new(true, false, false).with_countdown(countdown);
                for index in 0..theme.surfaces.len() {
                    let before =
                        render_theme_surface_with_runtime(&theme, index, Some(&plain), runtime);
                    let after =
                        render_theme_surface_with_runtime(&theme, index, Some(&extra), runtime);
                    assert_eq!((before.width, before.height), (after.width, after.height));
                    assert_eq!(
                        before.pixels, after.pixels,
                        "{} surface {index}",
                        theme.name
                    );
                    assert_eq!(before.warnings, after.warnings);
                }
            }
        }
        let context = DataContext::from_usage(Some(&extra), &Canvas::default());
        assert_eq!(context.get("claude.headline.percentage"), Some(29.0));
    }

    #[test]
    fn additional_resets_and_stale_quota_caches_follow_existing_poll_rules() {
        let mut limit = limit();
        limit.usage.resets_at = Some(UNIX_EPOCH);
        let usage = UsageData {
            limits: vec![limit],
            ..Default::default()
        };
        assert!(crate::poller::is_past_reset(&usage));
        let previous = AppUsageData::from_iter([(ProviderId::Claude, usage)]);
        let carried = crate::poller::carry_forward_failures(
            AppUsageData::default(),
            &previous,
            ProviderSet::from_enabled([ProviderId::Claude]),
        );
        assert_eq!(
            carried.get(ProviderId::Claude).unwrap().limits,
            previous.get(ProviderId::Claude).unwrap().limits
        );
        assert!(!crate::poller::app_is_past_reset(&carried));
        let mut account = AccountUsage {
            provider: ProviderId::Claude,
            profile: Default::default(),
            source_signature: String::new(),
            source_path: None,
            usage: carried.get(ProviderId::Claude).cloned(),
            error: Some(crate::poller::PollError::UnexpectedResponse),
            selected: true,
        };
        let restored: AccountUsage =
            serde_json::from_str(&serde_json::to_string(&account).unwrap()).unwrap();
        assert_eq!(restored, account);
        account.error = None;
        assert_ne!(restored, account);
        let old: UsageData = serde_json::from_str(r#"{"session":{"percentage":0,"resets_at":null},"weekly":{"percentage":0,"resets_at":null}}"#).unwrap();
        assert!(old.limits.is_empty());
    }
}
