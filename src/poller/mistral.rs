use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::windows_credentials;
use super::{build_agent, get_header_i64, PollError};
use crate::diagnose;
use crate::models::{UsageData, UsageSection};

const MISTRAL_MODELS_URL: &str = "https://api.mistral.ai/v1/models";
const MISTRAL_BILLING_USAGE_URL: &str = "https://api.mistral.ai/v1/billing/usage";
const MISTRAL_BILLING_SUBSCRIPTION_URL: &str = "https://api.mistral.ai/v1/billing/subscription";
/// The precedence the Vibe CLI itself gives its key variable.
const API_KEY_ENVIRONMENT: &str = "MISTRAL_API_KEY";
/// Where the monitor keeps a key the user entered for it; the Vibe CLI keeps
/// its own copy in the OS keyring, which on Windows is Credential Manager.
const CREDENTIAL_TARGET: &str = "mistral:vibe-api-key";

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Deserialize)]
struct BillingUsageResponse {
    #[serde(default)]
    data: Vec<BillingUsageEntry>,
}

#[derive(Debug, Deserialize)]
struct BillingUsageEntry {
    #[serde(default)]
    total_cost: Option<f64>,
    #[serde(default)]
    cost: Option<f64>,
    #[serde(default)]
    start_date: Option<String>,
    #[serde(default)]
    end_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BillingSubscriptionResponse {
    #[serde(default)]
    monthly_budget: Option<f64>,
}

struct ApiKeyCandidate {
    source: String,
    key: String,
}

pub(super) fn poll_mistral() -> Result<UsageData, PollError> {
    let candidates = api_key_candidates();
    if candidates.is_empty() {
        diagnose::log(
            "Mistral usage poll failed: no API key found (set MISTRAL_API_KEY or store a key in Windows Credential Manager)",
        );
        return Err(PollError::NoCredentials);
    }

    let mut last_error = PollError::AuthRequired;
    for candidate in &candidates {
        match fetch_mistral_usage(&candidate.key) {
            Ok(usage) => return Ok(usage),
            Err(error @ PollError::AuthRequired) => {
                diagnose::log(format!(
                    "Mistral rejected the key from {}: {error:?}",
                    candidate.source
                ));
                last_error = error;
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

/// Fingerprints only: secrets are hashed, never logged.
pub(super) fn credential_watch_snapshot(_all_sources: bool) -> Vec<String> {
    let environment = match non_empty_environment(API_KEY_ENVIRONMENT) {
        Some(value) => secret_signature("environment", &value),
        None => format!("{API_KEY_ENVIRONMENT}|missing"),
    };
    let stored = match windows_credentials::read_generic(CREDENTIAL_TARGET) {
        Some(secret) => secret_signature(CREDENTIAL_TARGET, &secret),
        None => format!("{CREDENTIAL_TARGET}|missing"),
    };
    vec![environment, stored]
}

fn api_key_candidates() -> Vec<ApiKeyCandidate> {
    let mut candidates = Vec::new();
    if let Some(key) = non_empty_environment(API_KEY_ENVIRONMENT) {
        candidates.push(ApiKeyCandidate {
            source: format!("environment variable {API_KEY_ENVIRONMENT}"),
            key,
        });
    }
    if let Some(secret) = windows_credentials::read_generic(CREDENTIAL_TARGET) {
        candidates.push(ApiKeyCandidate {
            source: format!("Windows credential {CREDENTIAL_TARGET}"),
            key: secret.trim().to_string(),
        });
    }
    candidates.into_iter().filter(|candidate| is_header_safe_key(&candidate.key)).collect()
}

fn fetch_mistral_usage(api_key: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;

    let mut models_response = match agent
        .get(MISTRAL_MODELS_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .call()
        .and_then(super::check_http_status)
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401 | 403)) => return Err(PollError::AuthRequired),
        Err(ureq::Error::StatusCode(code)) => {
            diagnose::log(format!("Mistral models endpoint returned status {code}"));
            return Err(PollError::HttpStatus(code));
        }
        Err(error) => {
            diagnose::log_error("Mistral models request failed", error);
            return Err(PollError::NetworkError);
        }
    };
    // The model list itself is only a carrier for the rate-limit headers; the
    // body is not needed, but draining it keeps the connection reusable.
    let _ = models_response.body_mut().read_json::<serde_json::Value>();

    let session = rate_limit_section(&models_response);

    // Billing routes need a key with billing scope; without it the account
    // still gets its rate-limit window instead of a hard failure.
    let (spend, budget) = match fetch_billing(&agent, api_key) {
        Ok(billing) => billing,
        Err(PollError::AuthRequired) => return Err(PollError::AuthRequired),
        Err(error) => {
            diagnose::log(format!("Mistral billing endpoints unavailable: {error:?}"));
            (None, None)
        }
    };

    let monthly = spend.map(|spend| monthly_section(spend, budget, SystemTime::now()));
    match (session.is_some(), monthly.is_some()) {
        (false, None) => {
            diagnose::log("Mistral response carried neither rate-limit headers nor spend data");
            Err(PollError::UnexpectedResponse)
        }
        _ => Ok(UsageData {
            limits: Vec::new(),
            session: session.unwrap_or_default(),
            weekly: monthly.clone().unwrap_or_default(),
            weekly_label: monthly.as_ref().map(|_| "30d".into()),
            monthly,
            credits: None,
            stale: false,
            ..Default::default()
        }),
    }
}

/// Monthly spend and budget are optional; a 403 on billing routes simply means
/// the key cannot read them.
fn fetch_billing(
    agent: &ureq::Agent,
    api_key: &str,
) -> Result<(Option<f64>, Option<f64>), PollError> {
    let mut usage_response = match agent
        .get(MISTRAL_BILLING_USAGE_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .call()
        .and_then(super::check_http_status)
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401)) => return Err(PollError::AuthRequired),
        Err(ureq::Error::StatusCode(403 | 404)) => return Ok((None, None)),
        Err(ureq::Error::StatusCode(code)) => return Err(PollError::HttpStatus(code)),
        Err(error) => {
            diagnose::log_error("Mistral billing usage request failed", error);
            return Err(PollError::NetworkError);
        }
    };
    let usage: BillingUsageResponse = usage_response.body_mut().read_json().map_err(|error| {
        diagnose::log_error("unable to parse Mistral billing usage response", error);
        PollError::UnexpectedResponse
    })?;
    let spend = monthly_spend(&usage.data, SystemTime::now());

    let mut subscription_response = match agent
        .get(MISTRAL_BILLING_SUBSCRIPTION_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .call()
        .and_then(super::check_http_status)
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401)) => return Err(PollError::AuthRequired),
        Err(ureq::Error::StatusCode(403 | 404)) => return Ok((Some(spend), None)),
        Err(ureq::Error::StatusCode(code)) => return Err(PollError::HttpStatus(code)),
        Err(error) => {
            diagnose::log_error("Mistral billing subscription request failed", error);
            return Err(PollError::NetworkError);
        }
    };
    let subscription: BillingSubscriptionResponse = subscription_response
        .body_mut()
        .read_json()
        .map_err(|error| {
            diagnose::log_error("unable to parse Mistral billing subscription response", error);
            PollError::UnexpectedResponse
        })?;
    let budget = subscription
        .monthly_budget
        .filter(|value| value.is_finite() && *value > 0.0);
    Ok((Some(spend), budget))
}

/// Mistral rate limits are reported per minute as response headers on every
/// endpoint; `/v1/models` is the cheapest one to ask.
fn rate_limit_section(response: &super::HttpResponse) -> Option<UsageSection> {
    let requests = window_from_headers(
        get_header_i64(response, "x-ratelimit-limit-requests"),
        get_header_i64(response, "x-ratelimit-remaining-requests"),
        response
            .headers()
            .get("x-ratelimit-reset-requests")
            .and_then(|value| value.to_str().ok()),
    );
    if requests.is_some() {
        return requests;
    }
    window_from_headers(
        get_header_i64(response, "x-ratelimit-limit-tokens"),
        get_header_i64(response, "x-ratelimit-remaining-tokens"),
        response
            .headers()
            .get("x-ratelimit-reset-tokens")
            .and_then(|value| value.to_str().ok()),
    )
}

fn window_from_headers(
    limit: Option<i64>,
    remaining: Option<i64>,
    reset: Option<&str>,
) -> Option<UsageSection> {
    let limit = limit?;
    let remaining = remaining?;
    if limit <= 0 || remaining < 0 {
        return None;
    }
    let percentage = ((limit - remaining) as f64 / limit as f64 * 100.0).clamp(0.0, 100.0);
    let resets_at =
        parse_reset_seconds(reset).map(|seconds| SystemTime::now() + Duration::from_secs(seconds));
    Some(UsageSection {
        available: true,
        percentage,
        resets_at,
    })
}

/// `x-ratelimit-reset-*` is a duration until the window refills, either a bare
/// number of seconds or a suffixed string such as `30s` or `1m`.
fn parse_reset_seconds(value: Option<&str>) -> Option<u64> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds);
    }
    let (digits, unit) = value.split_at(value.len().saturating_sub(1));
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        _ => return None,
    };
    digits.parse::<u64>().ok().map(|value| value * multiplier)
}

/// Sum the cost entries that overlap the current calendar month. Entries
/// without dates belong to the month the endpoint already scoped to.
fn monthly_spend(entries: &[BillingUsageEntry], now: SystemTime) -> f64 {
    let (month_start, month_end) = current_month_bounds(now);
    entries
        .iter()
        .filter(|entry| entry_overlaps_month(entry, month_start, month_end))
        .filter_map(|entry| {
            entry
                .total_cost
                .or(entry.cost)
                .filter(|value| value.is_finite() && *value >= 0.0)
        })
        .sum()
}

fn entry_overlaps_month(entry: &BillingUsageEntry, month_start: u64, month_end: u64) -> bool {
    let start = entry
        .start_date
        .as_deref()
        .and_then(|value| super::parse_iso8601(Some(value)))
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    let end = entry
        .end_date
        .as_deref()
        .and_then(|value| super::parse_iso8601(Some(value)))
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    match (start, end) {
        (None, None) => true,
        (start, end) => {
            let start = start.unwrap_or(0);
            let end = end.unwrap_or(u64::MAX);
            start < month_end && end >= month_start
        }
    }
}

/// Spend against the monthly budget, on the long-window row like Copilot's
/// calendar-month pool; the budgetless reading still reports the reset time.
fn monthly_section(spend: f64, budget: Option<f64>, now: SystemTime) -> UsageSection {
    let percentage = match budget {
        Some(budget) => (spend / budget * 100.0).clamp(0.0, 100.0),
        None => 0.0,
    };
    UsageSection {
        available: true,
        percentage,
        resets_at: Some(UNIX_EPOCH + Duration::from_secs(next_month_start(now))),
    }
}

fn current_month_bounds(now: SystemTime) -> (u64, u64) {
    let now_secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (year, month, _) = civil_from_days(now_secs / 86_400);
    let start = days_from_civil(year, month, 1) * 86_400;
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let end = days_from_civil(next_year, next_month, 1) * 86_400;
    (start, end)
}

fn next_month_start(now: SystemTime) -> u64 {
    current_month_bounds(now).1
}

/// Days since 1970-01-01 to civil date, after Howard Hinnant's algorithm.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era as u64 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    let y = if month <= 2 { year - 1 } else { year } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe as i64 - 719_468) as u64
}

/// Keys go into a header, so anything that could split it is unusable.
fn is_header_safe_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_graphic())
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

    fn entry(cost: Option<f64>, start: Option<&str>, end: Option<&str>) -> BillingUsageEntry {
        BillingUsageEntry {
            total_cost: cost,
            cost: None,
            start_date: start.map(str::to_string),
            end_date: end.map(str::to_string),
        }
    }

    #[test]
    fn billing_entries_sum_within_the_current_month() {
        // 2026-11-03T00:00:00Z is inside November 2026.
        let now = UNIX_EPOCH + Duration::from_secs(1_793_664_000);
        let entries = vec![
            entry(Some(10.5), Some("2026-11-01T00:00:00Z"), Some("2026-11-03T00:00:00Z")),
            entry(Some(2.0), Some("2026-10-31T00:00:00Z"), Some("2026-11-01T00:00:00Z")),
            entry(Some(99.0), Some("2026-10-01T00:00:00Z"), Some("2026-10-31T00:00:00Z")),
            entry(Some(1.25), None, None),
            entry(Some(-5.0), None, None),
            entry(None, None, None),
        ];
        assert_eq!(monthly_spend(&entries, now), 13.75);
    }

    #[test]
    fn monthly_section_measures_spend_against_the_budget() {
        let now = UNIX_EPOCH + Duration::from_secs(1_793_664_000);
        let section = monthly_section(25.0, Some(100.0), now);
        assert!(section.available);
        assert_eq!(section.percentage, 25.0);
        // Resets at midnight UTC on 2026-12-01.
        assert_eq!(
            section.resets_at,
            Some(UNIX_EPOCH + Duration::from_secs(1_796_083_200))
        );

        let uncapped = monthly_section(25.0, None, now);
        assert_eq!(uncapped.percentage, 0.0);
        assert!(uncapped.resets_at.is_some());

        let over = monthly_section(250.0, Some(100.0), now);
        assert_eq!(over.percentage, 100.0);
    }

    #[test]
    fn month_bounds_wrap_the_year() {
        // 2026-11-30T00:00:00Z: November runs 2026-11-01 .. 2026-12-01.
        let november = UNIX_EPOCH + Duration::from_secs(1_796_083_200 - 86_400);
        assert_eq!(current_month_bounds(november), (1_793_491_200, 1_796_083_200));
        // 2026-12-31T00:00:00Z: December wraps into January 2027.
        let december = UNIX_EPOCH + Duration::from_secs(1_798_761_600 - 86_400);
        assert_eq!(current_month_bounds(december), (1_796_083_200, 1_798_761_600));
    }

    #[test]
    fn rate_limit_windows_prefer_requests_and_clamp_to_the_limit() {
        let window = window_from_headers(Some(100), Some(40), Some("30")).unwrap();
        assert!(window.available);
        assert_eq!(window.percentage, 60.0);
        assert!(window.resets_at.is_some());

        let saturated = window_from_headers(Some(100), Some(-5), None).unwrap();
        assert_eq!(saturated.percentage, 100.0);
        assert!(saturated.resets_at.is_none());

        assert!(window_from_headers(Some(0), Some(0), None).is_none());
        assert!(window_from_headers(None, Some(5), None).is_none());
        assert!(window_from_headers(Some(100), None, None).is_none());
    }

    #[test]
    fn reset_headers_accept_seconds_and_duration_strings() {
        assert_eq!(parse_reset_seconds(Some("45")), Some(45));
        assert_eq!(parse_reset_seconds(Some("30s")), Some(30));
        assert_eq!(parse_reset_seconds(Some("2m")), Some(120));
        assert_eq!(parse_reset_seconds(Some("1h")), Some(3_600));
        assert_eq!(parse_reset_seconds(Some("soon")), None);
        assert_eq!(parse_reset_seconds(Some("")), None);
        assert_eq!(parse_reset_seconds(None), None);
    }

    #[test]
    fn keys_that_could_split_a_header_are_refused() {
        assert!(is_header_safe_key("abc123XYZ"));
        assert!(!is_header_safe_key(""));
        assert!(!is_header_safe_key("abc\r\nInjected: yes"));
        assert!(!is_header_safe_key("two words"));
    }

    #[test]
    fn civil_date_round_trips_through_the_days_algorithms() {
        for days in [0_u64, 1, 365, 19_000, 21_000, 56_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(21_018), (2027, 7, 19));
    }
}
