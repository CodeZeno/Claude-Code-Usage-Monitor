use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::time::SystemTime;

use serde::Deserialize;

use super::windows_credentials::{self, StoredCredential};
use super::{build_agent, parse_iso8601, PollError};
use crate::diagnose;
use crate::models::{limit_slug, UsageData, UsageLimit, UsageSection};

/// The quota endpoint the Copilot editors and CLI read for their own usage
/// panels. A GitHub OAuth token from any Copilot-enabled login can read it.
const COPILOT_USER_URL: &str = "https://api.github.com/copilot_internal/user";
/// The precedence the Copilot CLI itself gives its token variables.
const TOKEN_ENVIRONMENT: [&str; 3] = ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"];
/// Newer Copilot CLI builds store `https://github.com:<login>.copilot-cli`,
/// older ones `copilot-cli/https://github.com:<login>`.
const COPILOT_CLI_TARGET_PREFIX: &str = "https://github.com:";
const COPILOT_CLI_TARGET_SUFFIX: &str = ".copilot-cli";
const LEGACY_COPILOT_CLI_TARGET_PREFIX: &str = "copilot-cli/https://github.com:";
/// The GitHub CLI keeps the active account under `gh:github.com:` and every
/// signed-in account under `gh:github.com:<login>`.
const GH_CLI_TARGET_PREFIX: &str = "gh:github.com:";
const GH_CLI_ACTIVE_TARGET: &str = "gh:github.com:";
const PREMIUM_QUOTA: &str = "premium_interactions";
const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Deserialize)]
struct CopilotUserResponse {
    #[serde(default)]
    copilot_plan: Option<String>,
    #[serde(default)]
    quota_reset_date_utc: Option<String>,
    /// Date-only fallback, e.g. `2026-11-01`.
    #[serde(default)]
    quota_reset_date: Option<String>,
    /// Kept as raw values so one unfamiliar snapshot cannot fail the rest.
    #[serde(default)]
    quota_snapshots: BTreeMap<String, serde_json::Value>,
    /// Copilot Free accounts without snapshots report what is left of each
    /// monthly allowance here, against the totals in `monthly_quotas`.
    #[serde(default)]
    limited_user_quotas: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    monthly_quotas: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    limited_user_reset_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotQuotaSnapshot {
    #[serde(default)]
    entitlement: Option<f64>,
    #[serde(default)]
    remaining: Option<f64>,
    #[serde(default)]
    percent_remaining: Option<f64>,
    #[serde(default)]
    unlimited: bool,
}

impl CopilotQuotaSnapshot {
    fn used_percentage(&self) -> Option<f64> {
        if self.unlimited {
            return Some(0.0);
        }
        if let Some(remaining) = self.percent_remaining.filter(|value| value.is_finite()) {
            return Some((100.0 - remaining).clamp(0.0, 100.0));
        }
        used_from_remaining(self.entitlement?, self.remaining?)
    }
}

struct TokenCandidate {
    source: String,
    token: String,
}

pub(super) fn poll_copilot() -> Result<UsageData, PollError> {
    let candidates = token_candidates();
    if candidates.is_empty() {
        diagnose::log(
            "Copilot usage poll failed: no GitHub login found (sign in with 'copilot' or 'gh auth login', or set GH_TOKEN)",
        );
        return Err(PollError::NoCredentials);
    }

    let mut last_error = PollError::AuthRequired;
    for candidate in &candidates {
        match fetch_copilot_usage(&candidate.token) {
            Ok(usage) => return Ok(usage),
            // Another stored login may still hold a Copilot seat; a 404 means
            // this one is valid but has no Copilot access.
            Err(error @ (PollError::AuthRequired | PollError::HttpStatus(404))) => {
                diagnose::log(format!(
                    "Copilot rejected the login from {}: {error:?}",
                    candidate.source
                ));
                last_error = error;
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

/// Fingerprints only: secrets are hashed, stored logins reduced to their
/// target names and write times.
pub(super) fn credential_watch_snapshot(_all_sources: bool) -> Vec<String> {
    TOKEN_ENVIRONMENT
        .iter()
        .map(|name| match non_empty_environment(name) {
            Some(value) => secret_signature(name, &value),
            None => format!("{name}|missing"),
        })
        .chain(
            copilot_cli_credentials()
                .into_iter()
                .chain(gh_cli_credentials())
                .map(|credential| format!("{}|{}", credential.target, credential.last_written)),
        )
        .collect()
}

fn token_candidates() -> Vec<TokenCandidate> {
    let environment = TOKEN_ENVIRONMENT.iter().filter_map(|name| {
        non_empty_environment(name).map(|token| TokenCandidate {
            source: format!("environment variable {name}"),
            token,
        })
    });
    let stored = copilot_cli_credentials()
        .into_iter()
        .chain(gh_cli_credentials())
        .map(|credential| TokenCandidate {
            source: format!("Windows credential {}", credential.target),
            token: credential.secret.trim().to_string(),
        });

    let mut candidates: Vec<TokenCandidate> = Vec::new();
    for candidate in environment.chain(stored) {
        if is_header_safe_token(&candidate.token)
            && !candidates
                .iter()
                .any(|existing| existing.token == candidate.token)
        {
            candidates.push(candidate);
        }
    }
    candidates
}

fn copilot_cli_credentials() -> Vec<StoredCredential> {
    let mut credentials = windows_credentials::enumerate_generic(COPILOT_CLI_TARGET_PREFIX)
        .into_iter()
        .filter(|credential| credential.target.ends_with(COPILOT_CLI_TARGET_SUFFIX))
        .chain(windows_credentials::enumerate_generic(
            LEGACY_COPILOT_CLI_TARGET_PREFIX,
        ))
        .collect::<Vec<_>>();
    credentials.sort_by_key(|credential| std::cmp::Reverse(credential.last_written));
    credentials
}

/// The active account first, then the most recently written.
fn gh_cli_credentials() -> Vec<StoredCredential> {
    let mut credentials = windows_credentials::enumerate_generic(GH_CLI_TARGET_PREFIX);
    credentials.sort_by_key(|credential| {
        (
            credential.target != GH_CLI_ACTIVE_TARGET,
            std::cmp::Reverse(credential.last_written),
        )
    });
    credentials
}

fn fetch_copilot_usage(token: &str) -> Result<UsageData, PollError> {
    let mut response = match build_agent()?
        .get(COPILOT_USER_URL)
        .header("Authorization", &format!("token {token}"))
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .call()
        .and_then(super::check_http_status)
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401 | 403)) => return Err(PollError::AuthRequired),
        Err(ureq::Error::StatusCode(code)) => {
            diagnose::log(format!("Copilot usage endpoint returned status {code}"));
            return Err(PollError::HttpStatus(code));
        }
        Err(error) => {
            diagnose::log_error("Copilot usage request failed", error);
            return Err(PollError::NetworkError);
        }
    };

    let response: CopilotUserResponse = response.body_mut().read_json().map_err(|error| {
        diagnose::log_error("unable to parse Copilot usage response", error);
        PollError::UnexpectedResponse
    })?;
    if let Some(plan) = response.copilot_plan.as_deref() {
        diagnose::log(format!("Copilot plan: {plan}"));
    }
    copilot_usage_from_user(response).ok_or_else(|| {
        diagnose::log("Copilot usage response missing quota snapshots");
        PollError::UnexpectedResponse
    })
}

fn copilot_usage_from_user(response: CopilotUserResponse) -> Option<UsageData> {
    let snapshots = response
        .quota_snapshots
        .into_iter()
        .filter_map(|(key, value)| {
            Some((
                key,
                serde_json::from_value::<CopilotQuotaSnapshot>(value).ok()?,
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let reset = parse_iso8601(response.quota_reset_date_utc.as_deref())
        .or_else(|| parse_reset_date(response.quota_reset_date.as_deref()));

    let premium = snapshots
        .get(PREMIUM_QUOTA)
        .and_then(CopilotQuotaSnapshot::used_percentage);
    let (pool, limits, resets_at) = match premium {
        Some(premium) => {
            let metered = snapshots
                .iter()
                .filter(|(_, snapshot)| !snapshot.unlimited)
                .filter_map(|(key, snapshot)| Some((key.clone(), snapshot.used_percentage()?)))
                .collect();
            (premium, quota_limits(metered, reset), reset)
        }
        None => {
            // Older Copilot Free responses carry no snapshots at all; chat is
            // the allowance those accounts run out of.
            let reset =
                reset.or_else(|| parse_reset_date(response.limited_user_reset_date.as_deref()));
            let free = free_quota_usage(&response.limited_user_quotas, &response.monthly_quotas);
            let pool = *free.get("chat")?;
            (pool, quota_limits(free, reset), reset)
        }
    };

    // Copilot spends one pool per calendar month and has no short window, so
    // the pool lands in `weekly`, which is what tray badges read.
    let window = UsageSection {
        available: true,
        percentage: pool,
        resets_at,
    };
    Some(UsageData {
        limits,
        session: UsageSection::default(),
        weekly: window.clone(),
        weekly_label: Some("30d".into()),
        monthly: Some(window),
        credits: None,
        stale: false,
    })
}

/// Chat and completions are unlimited on paid plans; only metered ones are
/// worth exposing, through the opt-in `copilot.limits.<key>` bindings.
fn quota_limits(quotas: BTreeMap<String, f64>, resets_at: Option<SystemTime>) -> Vec<UsageLimit> {
    quotas
        .into_iter()
        .filter(|(key, _)| key != PREMIUM_QUOTA)
        .map(|(key, percentage)| UsageLimit {
            key: limit_slug(&key),
            kind: "quota".into(),
            label: quota_label(&key),
            model: None,
            model_id: None,
            scope: None,
            is_active: percentage > 0.0,
            usage: UsageSection {
                available: true,
                percentage,
                resets_at,
            },
        })
        .collect()
}

fn free_quota_usage(
    remaining: &BTreeMap<String, serde_json::Value>,
    totals: &BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, f64> {
    totals
        .iter()
        .filter_map(|(key, total)| {
            let used = used_from_remaining(total.as_f64()?, remaining.get(key)?.as_f64()?)?;
            Some((key.clone(), used))
        })
        .collect()
}

fn used_from_remaining(entitlement: f64, remaining: f64) -> Option<f64> {
    if !entitlement.is_finite() || entitlement <= 0.0 || !remaining.is_finite() {
        return None;
    }
    Some(((entitlement - remaining) / entitlement * 100.0).clamp(0.0, 100.0))
}

fn quota_label(key: &str) -> String {
    match key {
        "chat" => "Chat".into(),
        "completions" => "Completions".into(),
        other => other.replace('_', " "),
    }
}

/// Copilot quotas reset at midnight UTC on the given day.
fn parse_reset_date(date: Option<&str>) -> Option<SystemTime> {
    let date = date?.trim();
    parse_iso8601(Some(&format!("{date}T00:00:00Z")))
}

/// Tokens go into a header, so anything that could split it is unusable.
fn is_header_safe_token(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_graphic())
}

fn non_empty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn secret_signature(source: &str, value: &str) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{source}|present|{}|{:x}", value.len(), hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn usage(json: serde_json::Value) -> Option<UsageData> {
        copilot_usage_from_user(serde_json::from_value(json).unwrap())
    }

    #[test]
    fn a_live_enterprise_response_maps_premium_requests_to_the_monthly_pool() {
        // Shape captured from api.github.com on an Enterprise seat; identifiers
        // and organisation details dropped.
        let data = usage(serde_json::json!({
            "access_type_sku": "copilot_enterprise_seat_quota",
            "copilot_plan": "enterprise",
            "quota_reset_date": "2026-11-01",
            "quota_reset_date_utc": "2026-11-01T00:00:00.000Z",
            "token_based_billing": true,
            "quota_snapshots": {
                "chat": {
                    "entitlement": 0, "remaining": 0, "percent_remaining": 100.0,
                    "quota_id": "chat", "unlimited": true, "overage_permitted": false
                },
                "completions": {
                    "entitlement": 0, "remaining": 0, "percent_remaining": 100.0,
                    "quota_id": "completions", "unlimited": true
                },
                "premium_interactions": {
                    "entitlement": 30000, "remaining": 22500, "percent_remaining": 75.0,
                    "quota_id": "premium_interactions", "unlimited": false,
                    "overage_permitted": true, "credits_used": 0
                }
            }
        }))
        .unwrap();

        assert!(data.weekly.available);
        assert_eq!(data.weekly.percentage, 25.0);
        assert_eq!(data.weekly_label.as_deref(), Some("30d"));
        assert_eq!(data.monthly.as_ref(), Some(&data.weekly));
        assert_eq!(
            data.weekly.resets_at,
            Some(UNIX_EPOCH + Duration::from_secs(1_793_491_200))
        );
        // Copilot has no short window, so nothing claims the session slot.
        assert!(!data.session.available);
        assert!(data.credits.is_none());
        // Unlimited chat and completions have nothing to gauge.
        assert!(data.limits.is_empty());
    }

    #[test]
    fn metered_chat_and_completions_become_opt_in_limits() {
        let data = usage(serde_json::json!({
            "copilot_plan": "free",
            "quota_reset_date_utc": "2026-11-01T00:00:00.000Z",
            "quota_snapshots": {
                "chat": {"entitlement": 50, "remaining": 10, "percent_remaining": 20.0},
                "completions": {"entitlement": 2000, "remaining": 2000, "percent_remaining": 100.0},
                "premium_interactions": {"entitlement": 50, "remaining": 45, "percent_remaining": 90.0}
            }
        }))
        .unwrap();
        assert_eq!(data.weekly.percentage, 10.0);
        assert_eq!(
            data.limits
                .iter()
                .map(|limit| (
                    limit.key.as_str(),
                    limit.label.as_str(),
                    limit.usage.percentage,
                    limit.is_active
                ))
                .collect::<Vec<_>>(),
            vec![
                ("chat", "Chat", 80.0, true),
                ("completions", "Completions", 0.0, false)
            ]
        );
        assert!(data
            .limits
            .iter()
            .all(|limit| limit.usage.resets_at == data.weekly.resets_at));
    }

    #[test]
    fn overage_and_missing_percentages_fall_back_to_counts() {
        let data = usage(serde_json::json!({
            "quota_reset_date": "2026-11-01",
            "quota_snapshots": {
                "premium_interactions": {"entitlement": 300, "remaining": -12}
            }
        }))
        .unwrap();
        assert_eq!(data.weekly.percentage, 100.0);
        // The date-only field still yields a reset time.
        assert_eq!(
            data.weekly.resets_at,
            Some(UNIX_EPOCH + Duration::from_secs(1_793_491_200))
        );

        let data = usage(serde_json::json!({
            "quota_snapshots": {
                "premium_interactions": {"entitlement": 300, "remaining": 225}
            }
        }))
        .unwrap();
        assert_eq!(data.weekly.percentage, 25.0);
        assert!(data.weekly.resets_at.is_none());
    }

    #[test]
    fn a_legacy_free_response_reads_the_chat_allowance() {
        let data = usage(serde_json::json!({
            "copilot_plan": "individual",
            "limited_user_reset_date": "2026-10-15",
            "limited_user_quotas": {"chat": 40, "completions": 1500},
            "monthly_quotas": {"chat": 50, "completions": 2000}
        }))
        .unwrap();
        assert_eq!(data.weekly.percentage, 20.0);
        assert!(data.weekly.resets_at.is_some());
        let completions = data
            .limits
            .iter()
            .find(|limit| limit.key == "completions")
            .unwrap();
        assert_eq!(completions.usage.percentage, 25.0);
        assert_eq!(completions.label, "Completions");
    }

    #[test]
    fn responses_without_a_readable_pool_are_rejected() {
        assert!(usage(serde_json::json!({})).is_none());
        assert!(usage(serde_json::json!({
            "quota_snapshots": {
                "premium_interactions": "unexpected",
                "chat": {"unlimited": true}
            }
        }))
        .is_none());
        assert!(usage(serde_json::json!({
            "quota_snapshots": {"premium_interactions": {"entitlement": 0, "remaining": 0}}
        }))
        .is_none());
        assert!(usage(serde_json::json!({
            "limited_user_quotas": {"chat": 1},
            "monthly_quotas": {"chat": 0}
        }))
        .is_none());
    }

    #[test]
    fn an_unlimited_premium_pool_reads_as_idle() {
        let data = usage(serde_json::json!({
            "quota_snapshots": {"premium_interactions": {"unlimited": true}}
        }))
        .unwrap();
        assert!(data.weekly.available);
        assert_eq!(data.weekly.percentage, 0.0);
    }

    #[test]
    fn tokens_that_could_split_a_header_are_refused() {
        assert!(is_header_safe_token("gho_abc123"));
        assert!(!is_header_safe_token(""));
        assert!(!is_header_safe_token("gho_abc\r\nInjected: yes"));
        assert!(!is_header_safe_token("two words"));
    }
}
