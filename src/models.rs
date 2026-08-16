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
                    monthly: Some(UsageSection {
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
