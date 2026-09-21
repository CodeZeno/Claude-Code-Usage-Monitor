use std::collections::BTreeMap;
use std::time::SystemTime;

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::providers::ProviderId;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UsageSection {
    /// The provider reported this window, even if unused and without a reset time.
    pub available: bool,
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
}

impl<'de> Deserialize<'de> for UsageSection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct StoredSection {
            available: Option<bool>,
            percentage: f64,
            resets_at: Option<SystemTime>,
        }
        let stored = StoredSection::deserialize(deserializer)?;
        Ok(Self {
            // Older caches lost the distinction between an idle window and an
            // absent one. Preserve evidence of presence until a fresh poll.
            available: stored
                .available
                .unwrap_or(stored.resets_at.is_some() || stored.percentage != 0.0),
            percentage: stored.percentage,
            resets_at: stored.resets_at,
        })
    }
}

/// Paid credits that carry a provider past its included allowance.
///
/// `None` on [`UsageData`] means the provider has nothing to show: credits are
/// switched off, unavailable on the plan, or not yet in play because the
/// included allowance still has room.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CreditsSection {
    /// Share of the current allowance already consumed, 0 to 100.
    pub percentage: f64,
    /// What is left, in whole currency units.
    pub remaining: f64,
    /// What `percentage` is measured against, in whole currency units: a
    /// plan's cap where there is one, otherwise the balance recorded at the
    /// last top-up.
    pub total: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageData {
    pub session: UsageSection,
    pub weekly: UsageSection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_label: Option<String>,
    /// Optional longer-window usage (e.g. the OpenCode Go monthly window).
    /// Kept separate from `weekly` so themes can choose how to display it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly: Option<UsageSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<CreditsSection>,
    /// Additional API quotas, exposed only through opt-in theme bindings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<UsageLimit>,
    /// True when this reading was carried over from an earlier poll because
    /// the provider failed this cycle. The figures are real, just not current.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageLimit {
    pub key: String,
    pub kind: String,
    pub label: String,
    pub model: Option<String>,
    pub model_id: Option<String>,
    pub scope: Option<serde_json::Value>,
    pub is_active: bool,
    pub usage: UsageSection,
}

impl UsageData {
    pub fn sections(&self) -> impl Iterator<Item = &UsageSection> {
        [&self.session, &self.weekly]
            .into_iter()
            .chain(self.monthly.iter())
            .chain(self.limits.iter().map(|limit| &limit.usage))
    }
}

/// Codex reports a credit balance with no ceiling, so the denominator has to
/// be learned: any rise in the balance is a top-up, and the balance recorded
/// at that moment becomes what the gauge measures against until the next one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexCreditsState {
    /// Account whose balance this state belongs to. Older state files did not
    /// record it and are deliberately re-seeded when an account ID is now
    /// available, rather than risking a gauge based on another account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Balance seen at the previous poll, in raw credits.
    pub balance: f64,
    /// Balance recorded at the last observed top-up, in raw credits. Seeded
    /// from the first balance we ever see.
    pub baseline: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppUsageData {
    providers: BTreeMap<ProviderId, UsageData>,
    pub accounts: Vec<AccountUsage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountUsage {
    pub provider: ProviderId,
    pub profile: crate::accounts::AccountProfile,
    pub source_signature: String,
    #[serde(default)]
    pub source_path: Option<std::path::PathBuf>,
    pub usage: Option<UsageData>,
    pub error: Option<crate::poller::PollError>,
    #[serde(default)]
    pub selected: bool,
}

impl AppUsageData {
    /// Authentication failures stay paused until this source changes or the
    /// user explicitly asks to retry. Other accounts remain independently live.
    pub fn auth_error_for_source(
        &self,
        provider: ProviderId,
        profile: &crate::accounts::AccountProfile,
        signature: &str,
    ) -> Option<crate::poller::PollError> {
        self.accounts.iter().find_map(|account| {
            (account.provider == provider
                && account.profile.same_source(profile)
                && account.source_signature == signature)
                .then_some(account.error)
                .flatten()
                .filter(|error| error.is_auth())
        })
    }

    pub fn new_auth_failures(&self, previous: Option<&Self>, force: bool) -> Vec<&AccountUsage> {
        self.accounts
            .iter()
            .filter(|account| {
                account.error.is_some_and(crate::poller::PollError::is_auth)
                    && (force
                        || previous
                            .and_then(|previous| {
                                previous.auth_error_for_source(
                                    account.provider,
                                    &account.profile,
                                    &account.source_signature,
                                )
                            })
                            .is_none())
            })
            .collect()
    }

    pub fn get(&self, provider: ProviderId) -> Option<&UsageData> {
        self.providers.get(&provider)
    }

    pub fn insert(&mut self, provider: ProviderId, usage: UsageData) -> Option<UsageData> {
        self.providers.insert(provider, usage)
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.accounts.iter().all(|account| account.usage.is_none())
    }

    pub fn iter(&self) -> impl Iterator<Item = (ProviderId, &UsageData)> {
        self.providers
            .iter()
            .map(|(provider, usage)| (*provider, usage))
    }

    pub fn all_usage(&self) -> impl Iterator<Item = &UsageData> {
        self.providers.values().chain(
            self.accounts
                .iter()
                .filter_map(|account| account.usage.as_ref()),
        )
    }

    /// Rebuild the legacy provider bindings from the user's selected accounts.
    /// A missing selection must never display another account's cached usage.
    pub fn select_accounts(&mut self, settings: &crate::accounts::AccountSettings) {
        for account in &mut self.accounts {
            if let Some(profile) = settings.get(account.provider).and_then(|configured| {
                configured
                    .profiles
                    .iter()
                    .find(|profile| profile.enabled && profile.same_source(&account.profile))
            }) {
                account.profile = profile.clone();
            }
            account.selected = settings
                .get(account.provider)
                .and_then(|configured| configured.selected())
                .is_some_and(|selected| *selected == account.profile);
        }
        for provider in [ProviderId::Claude, ProviderId::Codex] {
            let configured = settings.get(provider).unwrap();
            let tracked = self
                .accounts
                .iter()
                .any(|account| account.provider == provider);
            if tracked || configured != &crate::accounts::ProviderAccounts::default() {
                self.providers.remove(&provider);
                if let Some(selected) = configured.selected() {
                    if let Some(usage) = self
                        .accounts
                        .iter()
                        .find(|account| {
                            account.provider == provider && account.profile == *selected
                        })
                        .and_then(|account| account.usage.clone())
                    {
                        self.providers.insert(provider, usage);
                    }
                }
            }
        }
        self.accounts.retain(|account| {
            settings.get(account.provider).is_some_and(|configured| {
                configured
                    .profiles
                    .iter()
                    .any(|profile| profile.enabled && *profile == account.profile)
            })
        });
    }

    pub fn selected_account_name(&self, provider: ProviderId) -> Option<&str> {
        self.accounts
            .iter()
            .find(|account| account.provider == provider && account.selected)
            .map(|account| account.profile.name.as_str())
    }

    /// A cached reading must not outlive a login change or a different inherited
    /// config directory. This only stats files; it never starts a CLI or WSL.
    pub fn invalidate_changed_credentials(&mut self) {
        for account in &mut self.accounts {
            let expected = account
                .profile
                .credential_path(account.provider)
                .ok()
                .flatten()
                .or_else(|| crate::accounts::default_credential_path(account.provider));
            let changed = match &account.source_path {
                Some(path) => {
                    expected.as_ref().is_none_or(|expected| {
                        crate::accounts::source_key(expected) != crate::accounts::source_key(path)
                    }) || crate::poller::account_source_signature(account.provider, path)
                        != account.source_signature
                }
                None => crate::accounts::environment_directory(account.provider).is_some(),
            };
            if changed {
                account.usage = None;
                account.error = None;
                self.providers.remove(&account.provider);
            }
        }
    }
}

impl FromIterator<(ProviderId, UsageData)> for AppUsageData {
    fn from_iter<T: IntoIterator<Item = (ProviderId, UsageData)>>(iter: T) -> Self {
        Self {
            providers: iter.into_iter().collect(),
            accounts: Vec::new(),
        }
    }
}

impl Serialize for AppUsageData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        for (provider, usage) in &self.providers {
            map.serialize_entry(provider.descriptor().cache_key, usage)?;
        }
        if !self.accounts.is_empty() {
            map.serialize_entry("accounts", &self.accounts)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for AppUsageData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut values = BTreeMap::<String, serde_json::Value>::deserialize(deserializer)?;
        let accounts = values
            .remove("accounts")
            .map(serde_json::from_value)
            .transpose()
            .map_err(serde::de::Error::custom)?
            .unwrap_or_default();
        let mut data: Self = values
            .into_iter()
            .filter_map(|(key, usage)| {
                let usage = serde_json::from_value::<Option<UsageData>>(usage).ok()??;
                ProviderId::from_cache_key(&key)
                    .or_else(|| ProviderId::from_key(&key))
                    .map(|provider| (provider, usage))
            })
            .collect();
        data.accounts = accounts;
        Ok(data)
    }
}

pub fn limit_slug(value: &str) -> String {
    let mut result = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            result.push(ch.to_ascii_lowercase());
        } else if !result.is_empty() && !result.ends_with('_') {
            result.push('_');
        }
    }
    let result = result.trim_end_matches('_');
    if result.is_empty() {
        "unnamed".to_string()
    } else {
        result.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_claude_cache_keeps_poll_results_until_source_changes() {
        let provider = ProviderId::Claude;
        let path = crate::accounts::default_credential_path(provider).unwrap();
        // Fingerprints only: no tokens are decrypted, used, or changed.
        let signature = crate::poller::account_source_signature(provider, &path);
        for error in [None, Some(crate::poller::PollError::HttpStatus(429))] {
            let mut data = AppUsageData::default();
            data.accounts.push(AccountUsage {
                provider,
                profile: crate::accounts::AccountProfile::default(),
                source_signature: signature.clone(),
                source_path: Some(path.clone()),
                usage: error.is_none().then(UsageData::default),
                error,
                selected: true,
            });
            let json = serde_json::to_string(&data).unwrap();
            let mut cached: AppUsageData = serde_json::from_str(&json).unwrap();
            cached.invalidate_changed_credentials();
            assert_eq!(cached.accounts, data.accounts);
            cached.accounts[0].source_signature.push_str("changed");
            cached.invalidate_changed_credentials();
            assert!(cached.accounts[0].usage.is_none());
            assert!(cached.accounts[0].error.is_none());
        }
    }

    #[test]
    fn usage_cache_preserves_idle_window_presence_and_reads_legacy_sections() {
        for available in [false, true] {
            let section = UsageSection {
                available,
                ..Default::default()
            };
            let json = serde_json::to_value(&section).unwrap();
            assert_eq!(json["available"], available);
            assert_eq!(
                serde_json::from_value::<UsageSection>(json).unwrap(),
                section
            );
        }
        for (json, expected) in [
            (r#"{"percentage":0,"resets_at":null}"#, false),
            (r#"{"percentage":42,"resets_at":null}"#, true),
            (
                r#"{"percentage":0,"resets_at":{"secs_since_epoch":0,"nanos_since_epoch":0}}"#,
                true,
            ),
            (
                r#"{"available":false,"percentage":42,"resets_at":null}"#,
                false,
            ),
        ] {
            let section: UsageSection = serde_json::from_str(json).unwrap();
            assert_eq!(section.available, expected);
        }
    }

    #[test]
    fn usage_cache_keeps_legacy_provider_keys() {
        let data: AppUsageData = [
            (ProviderId::Claude, UsageData::default()),
            (ProviderId::Codex, UsageData::default()),
            (
                ProviderId::OpenCode,
                UsageData {
                    weekly_label: Some("30d".into()),
                    monthly: Some(UsageSection {
                        available: true,
                        percentage: 43.0,
                        resets_at: None,
                    }),
                    ..Default::default()
                },
            ),
        ]
        .into_iter()
        .collect();

        let json = serde_json::to_value(&data).unwrap();
        assert!(json.get("claude_code").is_some());
        assert!(json.get("codex").is_some());
        assert_eq!(json["opencode"]["weekly_label"], "30d");
        assert_eq!(json["opencode"]["monthly"]["percentage"], 43.0);
        assert!(json.get("claude").is_none());

        let decoded: AppUsageData = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn usage_cache_accepts_nulls_from_the_legacy_struct_format() {
        let decoded: AppUsageData = serde_json::from_str(
            r#"{
                "claude_code": null,
                "codex": {"session":{"percentage":42.0,"resets_at":null},"weekly":{"percentage":0.0,"resets_at":null}},
                "antigravity": null
                ,"opencode": null
            }"#,
        )
        .unwrap();

        assert!(decoded.get(ProviderId::Claude).is_none());
        assert_eq!(
            decoded.get(ProviderId::Codex).unwrap().session.percentage,
            42.0
        );
        assert!(decoded.get(ProviderId::Antigravity).is_none());
        assert!(decoded.get(ProviderId::OpenCode).is_none());
    }
}
