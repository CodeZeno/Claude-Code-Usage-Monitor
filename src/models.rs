use std::collections::BTreeMap;
use std::time::SystemTime;

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::providers::ProviderId;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSection {
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<CreditsSection>,
}

/// Codex reports a credit balance with no ceiling, so the denominator has to
/// be learned: any rise in the balance is a top-up, and the balance recorded
/// at that moment becomes what the gauge measures against until the next one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexCreditsState {
    /// Balance seen at the previous poll, in raw credits.
    pub balance: f64,
    /// Balance recorded at the last observed top-up, in raw credits. Seeded
    /// from the first balance we ever see.
    pub baseline: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppUsageData {
    providers: BTreeMap<ProviderId, UsageData>,
}

impl AppUsageData {
    pub fn get(&self, provider: ProviderId) -> Option<&UsageData> {
        self.providers.get(&provider)
    }

    pub fn insert(&mut self, provider: ProviderId, usage: UsageData) -> Option<UsageData> {
        self.providers.insert(provider, usage)
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (ProviderId, &UsageData)> {
        self.providers
            .iter()
            .map(|(provider, usage)| (*provider, usage))
    }
}

impl FromIterator<(ProviderId, UsageData)> for AppUsageData {
    fn from_iter<T: IntoIterator<Item = (ProviderId, UsageData)>>(iter: T) -> Self {
        Self {
            providers: iter.into_iter().collect(),
        }
    }
}

impl Serialize for AppUsageData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.providers.len()))?;
        for (provider, usage) in &self.providers {
            map.serialize_entry(provider.descriptor().cache_key, usage)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for AppUsageData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = BTreeMap::<String, Option<UsageData>>::deserialize(deserializer)?;
        Ok(values
            .into_iter()
            .filter_map(|(key, usage)| {
                let usage = usage?;
                ProviderId::from_cache_key(&key)
                    .or_else(|| ProviderId::from_key(&key))
                    .map(|provider| (provider, usage))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_cache_keeps_legacy_provider_keys() {
        let data: AppUsageData = [
            (ProviderId::Claude, UsageData::default()),
            (ProviderId::Codex, UsageData::default()),
            (
                ProviderId::OpenCode,
                UsageData {
                    weekly_label: Some("30d".into()),
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
