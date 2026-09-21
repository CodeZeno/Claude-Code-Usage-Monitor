use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;

use crate::models::{limit_slug as slug, UsageLimit, UsageSection};

#[derive(Deserialize)]
struct Limit {
    kind: String,
    #[serde(alias = "utilization")]
    percent: f64,
    resets_at: Option<String>,
    scope: Option<Value>,
    #[serde(default)]
    is_active: bool,
}

pub(super) fn parse(entries: &[Value], legacy: &BTreeMap<String, Value>) -> Vec<UsageLimit> {
    let mut limits = Vec::new();
    for entry in entries {
        match serde_json::from_value::<Limit>(entry.clone()) {
            Ok(limit)
                if !limit.kind.trim().is_empty()
                    && limit.percent.is_finite()
                    && limit.percent >= 0.0 =>
            {
                let model = limit.scope.as_ref().and_then(|scope| scope.get("model"));
                let model_id = model
                    .and_then(|model| model.get("id"))
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                    .map(str::to_string);
                let model_name = model
                    .and_then(|model| model.get("display_name"))
                    .and_then(Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                    .map(str::to_string);
                let label = model_name
                    .clone()
                    .or_else(|| model_id.clone())
                    .unwrap_or_else(|| limit.kind.replace('_', " "));
                let mut key = slug(&limit.kind);
                if let Some(model) = model_name.as_ref().or(model_id.as_ref()) {
                    key.push('_');
                    key.push_str(&slug(model));
                } else if let Some(scope) = &limit.scope {
                    key.push('_');
                    key.push_str(&stable_suffix(&scope.to_string()));
                }
                limits.push(UsageLimit {
                    key,
                    kind: limit.kind,
                    label,
                    model: model_name.or_else(|| model_id.clone()),
                    model_id,
                    scope: limit.scope,
                    is_active: limit.is_active,
                    usage: UsageSection {
                        available: true,
                        percentage: limit.percent,
                        resets_at: super::parse_iso8601(limit.resets_at.as_deref()),
                    },
                });
            }
            _ => crate::diagnose::log("ignored malformed entry in Claude usage limits array"),
        }
    }

    // Keep older optional buckets (including future ones) available too. Only
    // objects with an actual numeric utilization are quota buckets.
    for (key, value) in legacy {
        let Some(percentage) = value
            .get("utilization")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
        else {
            continue;
        };
        let model = match key.as_str() {
            "seven_day_opus" => Some("Opus".to_string()),
            "seven_day_sonnet" => Some("Sonnet".to_string()),
            _ => None,
        };
        // The array is authoritative when both representations are present.
        if model.as_ref().is_some_and(|model| {
            limits.iter().any(|limit| {
                limit.kind == "weekly_scoped"
                    && limit
                        .model
                        .as_ref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(model))
            })
        }) {
            continue;
        }
        limits.push(UsageLimit {
            key: slug(key),
            kind: key.clone(),
            label: model.clone().unwrap_or_else(|| key.replace('_', " ")),
            model,
            model_id: None,
            scope: None,
            is_active: value
                .get("is_active")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            usage: UsageSection {
                available: true,
                percentage,
                resets_at: super::parse_iso8601(value.get("resets_at").and_then(Value::as_str)),
            },
        });
    }

    // Names must not depend on array order or silently overwrite another quota.
    let mut identities: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for limit in &limits {
        identities
            .entry(limit.key.clone())
            .or_default()
            .insert(identity(limit));
    }
    for limit in &mut limits {
        if identities[&limit.key].len() > 1 {
            limit.key = format!("{}_{}", limit.key, stable_suffix(&identity(limit)));
        }
    }
    limits.sort_by(|a, b| {
        a.key
            .cmp(&b.key)
            .then(b.is_active.cmp(&a.is_active))
            .then(b.usage.percentage.total_cmp(&a.usage.percentage))
    });
    limits.dedup_by(|a, b| a.key == b.key);
    limits
}

fn identity(limit: &UsageLimit) -> String {
    format!(
        "{}|{}|{}|{}",
        limit.kind,
        limit.model.as_deref().unwrap_or(""),
        limit.model_id.as_deref().unwrap_or(""),
        limit
            .scope
            .as_ref()
            .map(Value::to_string)
            .unwrap_or_default()
    )
}

fn stable_suffix(value: &str) -> String {
    // FNV-1a is stable across runs and Rust versions; this is an identifier,
    // not a security hash. Keep the full width to avoid short-name collisions.
    let hash = value.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn quotas_keep_model_metadata_active_flags_and_unknown_kinds() {
        let values = vec![
            json!({"kind":"weekly_scoped","percent":43,"resets_at":"2030-01-01T00:00:00Z","scope":{"model":{"id":null,"display_name":"Fable"}},"is_active":true}),
            json!({"kind":"monthly_scoped","percent":0,"scope":{"model":{"id":"future-v2","display_name":"Future Model 2"}}}),
            json!({"kind":"daily_team","percent":112.5,"scope":{"team":"example"}}),
        ];
        let limits = parse(&values, &BTreeMap::new());
        assert_eq!(limits.len(), 3);
        let fable = limits
            .iter()
            .find(|limit| limit.key == "weekly_scoped_fable")
            .unwrap();
        assert_eq!(fable.label, "Fable");
        assert_eq!(fable.usage.percentage, 43.0);
        assert!(fable.is_active && fable.usage.resets_at.is_some());
        let future = limits
            .iter()
            .find(|limit| limit.key == "monthly_scoped_future_model_2")
            .unwrap();
        assert!(future.usage.available);
        assert_eq!(future.model_id.as_deref(), Some("future-v2"));
        assert!(!future.is_active);
        assert!(limits
            .iter()
            .any(|limit| limit.kind == "daily_team" && limit.usage.percentage == 112.5));
    }

    #[test]
    fn malformed_entries_do_not_hide_valid_quotas() {
        let values = vec![
            Value::Null,
            json!({"kind":"weekly_scoped"}),
            json!({"kind":"","percent":3}),
            json!({"kind":"bad","percent":-1}),
            json!({"kind":"bad","percent":"43"}),
            json!({"kind":"weekly_all","percent":0}),
        ];
        let limits = parse(&values, &BTreeMap::new());
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].key, "weekly_all");
        assert_eq!(limits[0].usage.percentage, 0.0);
    }

    #[test]
    fn legacy_buckets_and_new_array_coexist_without_duplicate_model_caps() {
        let legacy = BTreeMap::from([
            (
                "seven_day_opus".into(),
                json!({"utilization":15,"resets_at":null}),
            ),
            ("seven_day_sonnet".into(), json!({"utilization":25})),
            ("seven_day_cowork".into(), json!({"utilization":35})),
            ("unrelated".into(), json!({"enabled":true})),
            ("absent".into(), Value::Null),
        ]);
        let array = vec![
            json!({"kind":"weekly_scoped","percent":50,"scope":{"model":{"display_name":"Sonnet"}}}),
        ];
        let limits = parse(&array, &legacy);
        assert_eq!(limits.len(), 3);
        assert!(limits
            .iter()
            .any(|limit| limit.key == "seven_day_opus" && limit.model.as_deref() == Some("Opus")));
        assert!(limits
            .iter()
            .any(|limit| limit.key == "weekly_scoped_sonnet" && limit.usage.percentage == 50.0));
        assert!(limits.iter().any(|limit| limit.key == "seven_day_cowork"));
    }

    #[test]
    fn colliding_names_and_duplicates_have_order_independent_keys() {
        let mut array = vec![
            json!({"kind":"weekly_scoped","percent":25,"scope":{"model":{"id":"a","display_name":"A/B"}}}),
            json!({"kind":"weekly_scoped","percent":35,"scope":{"model":{"id":"b","display_name":"A B"}}}),
            json!({"kind":"weekly_scoped","percent":45,"scope":{"model":{"id":"c","display_name":"同じ"}}}),
        ];
        array.push(array[0].clone());
        let original = parse(&array, &BTreeMap::new());
        array.reverse();
        assert_eq!(parse(&array, &BTreeMap::new()), original);
        assert_eq!(original.len(), 3);
        assert_eq!(
            original
                .iter()
                .map(|limit| &limit.key)
                .collect::<BTreeSet<_>>()
                .len(),
            3
        );
    }
}
