use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer};

use super::{build_agent, parse_iso8601, PollError};
use crate::diagnose;
use crate::models::{UsageData, UsageSection};

const CURSOR_USAGE_SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";
/// This internal application service can report the current allowance while
/// the dashboard summary describes a stale period for the same account.
const CURSOR_CURRENT_PERIOD_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";
const CURSOR_SESSION_TOKEN_ENV: &str = "CURSOR_SESSION_TOKEN";
const CURSOR_ACCESS_TOKEN_KEY: &str = "cursorAuth/accessToken";

/// Bearer token plus the dashboard cookie, both derived from the same local
/// login. Each Cursor endpoint authenticates differently.
struct CursorAuth {
    access_token: String,
    cookie: String,
}

#[derive(Deserialize)]
struct CursorUsageSummaryResponse {
    #[serde(rename = "billingCycleEnd")]
    billing_cycle_end: Option<String>,
    #[serde(rename = "individualUsage")]
    individual_usage: Option<CursorIndividualUsage>,
}

#[derive(Deserialize)]
struct CursorIndividualUsage {
    plan: Option<CursorPlanUsage>,
}

#[derive(Deserialize)]
struct CursorPlanUsage {
    #[serde(
        default,
        rename = "autoPercentUsed",
        alias = "auto_percent_used",
        deserialize_with = "deserialize_optional_f64"
    )]
    auto_percent_used: Option<f64>,
    #[serde(
        default,
        rename = "apiPercentUsed",
        alias = "api_percent_used",
        deserialize_with = "deserialize_optional_f64"
    )]
    api_percent_used: Option<f64>,
    #[serde(
        default,
        rename = "totalPercentUsed",
        alias = "total_percent_used",
        deserialize_with = "deserialize_optional_f64"
    )]
    total_percent_used: Option<f64>,
}

pub(super) fn poll_cursor() -> Result<UsageData, PollError> {
    let auth = read_cursor_auth().ok_or_else(|| {
        diagnose::log(
            "Cursor usage poll failed: no Cursor session found (sign in to Cursor or set CURSOR_SESSION_TOKEN)",
        );
        PollError::NoCredentials
    })?;
    fetch_cursor_usage(&auth)
}

pub(super) fn credential_watch_snapshot(_all_sources: bool) -> Vec<String> {
    let environment = non_empty_environment(CURSOR_SESSION_TOKEN_ENV)
        .map(|value| secret_signature("environment", &value))
        .unwrap_or_else(|| "environment|missing".into());
    let database = cursor_state_db_path()
        .map(|path| path_signature("database", &path))
        .unwrap_or_else(|| "database|missing".into());
    vec![environment, database]
}

/// Resolve a Cursor login. An explicit environment value takes priority over
/// the access token persisted by Cursor itself. The environment accepts a raw
/// access token, a `WorkosCursorSessionToken` cookie value, or the combined
/// `user_id%3A%3Aaccess_token` form.
fn read_cursor_auth() -> Option<CursorAuth> {
    if let Some(token) = non_empty_environment(CURSOR_SESSION_TOKEN_ENV) {
        return cursor_auth_from_environment(&token);
    }

    let access_token = read_cursor_access_token_from_state_db()?;
    let cookie = cursor_cookie_from_access_token(&access_token)?;
    Some(CursorAuth {
        access_token,
        cookie,
    })
}

fn cursor_auth_from_environment(token: &str) -> Option<CursorAuth> {
    let normalized = normalize_cursor_session_cookie(token)?;
    let access_token = access_token_from_cookie(&normalized).unwrap_or_else(|| normalized.clone());
    Some(CursorAuth {
        access_token,
        cookie: normalized,
    })
}

fn normalize_cursor_session_cookie(token: &str) -> Option<String> {
    if token.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return None;
    }
    let token = token
        .trim()
        .strip_prefix("WorkosCursorSessionToken=")
        .unwrap_or(token.trim())
        .trim();
    if token.is_empty() {
        None
    } else if token.contains("%3A%3A") {
        Some(token.to_string())
    } else if token.contains("::") {
        Some(token.replace("::", "%3A%3A"))
    } else {
        cursor_cookie_from_access_token(token).or_else(|| Some(token.to_string()))
    }
}

fn access_token_from_cookie(cookie: &str) -> Option<String> {
    cookie
        .split_once("%3A%3A")
        .map(|(_, token)| token.to_string())
        .or_else(|| cookie.split_once("::").map(|(_, token)| token.to_string()))
        .filter(|token| !token.is_empty())
}

fn cursor_cookie_from_access_token(access_token: &str) -> Option<String> {
    let user_id = extract_cursor_user_id(access_token)?;
    Some(format!("{user_id}%3A%3A{access_token}"))
}

fn extract_cursor_user_id(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let decoded = base64_url_decode(payload)?;
    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let subject = json.get("sub")?.as_str()?;
    Some(
        subject
            .rsplit_once('|')
            .map(|(_, id)| id.to_string())
            .unwrap_or_else(|| subject.to_string()),
    )
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    if input.len() % 4 == 1 {
        return None;
    }
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    let padding_mask = (1u32 << bits).saturating_sub(1);
    (buffer & padding_mask == 0).then_some(output)
}

fn cursor_state_db_path() -> Option<PathBuf> {
    let path = crate::known_folders::roaming_app_data_dir()?
        .join("Cursor")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb");
    path.is_file().then_some(path)
}

fn read_cursor_access_token_from_state_db() -> Option<String> {
    let path = cursor_state_db_path()?;
    match query_cursor_access_token(&path) {
        Ok(token) => token,
        Err(error) => {
            diagnose::log(format!(
                "Cursor state DB direct read failed ({error}); retrying via temp copy"
            ));
            query_cursor_access_token_from_copy(&path)
        }
    }
}

fn query_cursor_access_token_from_copy(path: &Path) -> Option<String> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = std::env::temp_dir().join(format!(
        "claude-monitor-cursor-state-{}-{unique}.vscdb",
        std::process::id()
    ));
    if let Err(error) = std::fs::copy(path, &temporary) {
        diagnose::log(format!("Cursor state DB temp copy failed: {error}"));
        return None;
    }
    let result = query_cursor_access_token(&temporary);
    let _ = std::fs::remove_file(&temporary);
    match result {
        Ok(token) => token,
        Err(error) => {
            diagnose::log(format!("Cursor state DB temp-copy read failed: {error}"));
            None
        }
    }
}

fn query_cursor_access_token(path: &Path) -> Result<Option<String>, crate::winsqlite::Error> {
    crate::winsqlite::query_optional_text(
        path,
        "SELECT value FROM ItemTable WHERE key = ?1",
        CURSOR_ACCESS_TOKEN_KEY,
    )
    .map(|token| token.filter(|token| !token.is_empty()))
}

fn fetch_cursor_usage(auth: &CursorAuth) -> Result<UsageData, PollError> {
    fetch_cursor_usage_with(
        || fetch_cursor_usage_current_period(auth),
        || fetch_cursor_usage_summary(&auth.cookie),
    )
}

fn fetch_cursor_usage_with(
    current_period: impl FnOnce() -> Result<UsageData, PollError>,
    summary: impl FnOnce() -> Result<UsageData, PollError>,
) -> Result<UsageData, PollError> {
    // Prefer the application service window: it matches the allowance Cursor
    // itself displays. The dashboard summary is kept as a fallback because the
    // application service is internal and may reject some accounts.
    match current_period() {
        Ok(data) => Ok(data),
        Err(current_error) => {
            diagnose::log(format!(
                "Cursor GetCurrentPeriodUsage unavailable; falling back to usage-summary ({current_error:?})"
            ));
            // Rejection by the optional internal service does not prove the
            // dashboard credentials are invalid. Preserve the fallback error
            // so temporary summary outages still retry normally.
            summary()
        }
    }
}

fn fetch_cursor_usage_current_period(auth: &CursorAuth) -> Result<UsageData, PollError> {
    let body = serde_json::json!({});
    let mut response = match build_agent()?
        .post(CURSOR_CURRENT_PERIOD_URL)
        .header("Authorization", &format!("Bearer {}", auth.access_token))
        .header("Content-Type", "application/json")
        .header("User-Agent", "Mozilla/5.0")
        .send_json(&body)
        .and_then(super::check_http_status)
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401 | 403)) => return Err(PollError::AuthRequired),
        Err(ureq::Error::StatusCode(code)) => return Err(PollError::HttpStatus(code)),
        Err(error) => {
            diagnose::log_error("Cursor GetCurrentPeriodUsage request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    // The endpoint is undocumented and has drifted before, so parse loosely
    // and report the top-level keys when the expected shape is missing.
    let value: serde_json::Value = response.body_mut().read_json().map_err(|error| {
        diagnose::log_error(
            "unable to parse Cursor GetCurrentPeriodUsage response",
            error,
        );
        PollError::UnexpectedResponse
    })?;
    cursor_usage_from_current_period(&value).ok_or_else(|| {
        diagnose::log(format!(
            "Cursor GetCurrentPeriodUsage response missing plan usage; keys: {:?}",
            value
                .as_object()
                .map(|object| { object.keys().map(String::as_str).collect::<Vec<_>>() })
        ));
        PollError::UnexpectedResponse
    })
}

fn fetch_cursor_usage_summary(cookie: &str) -> Result<UsageData, PollError> {
    let cookie_header = format!("WorkosCursorSessionToken={cookie}");
    let mut response = match build_agent()?
        .get(CURSOR_USAGE_SUMMARY_URL)
        .header("Cookie", &cookie_header)
        .header("User-Agent", "Mozilla/5.0")
        .call()
        .and_then(super::check_http_status)
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401 | 403)) => return Err(PollError::AuthRequired),
        Err(error) => {
            diagnose::log_error("Cursor usage-summary request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: CursorUsageSummaryResponse =
        response.body_mut().read_json().map_err(|error| {
            diagnose::log_error("unable to parse Cursor usage-summary response", error);
            PollError::RequestFailed
        })?;
    cursor_usage_from_summary(response).ok_or_else(|| {
        diagnose::log("Cursor usage-summary response missing plan usage");
        PollError::RequestFailed
    })
}

fn cursor_usage_from_current_period(value: &serde_json::Value) -> Option<UsageData> {
    if value.get("enabled").and_then(|value| value.as_bool()) == Some(false) {
        return None;
    }
    let plan_value = value
        .get("planUsage")
        .or_else(|| value.get("plan"))
        .or_else(|| value.pointer("/individualUsage/plan"))?;
    let plan: CursorPlanUsage = serde_json::from_value(plan_value.clone()).ok()?;
    if plan.auto_percent_used.is_none()
        && plan.api_percent_used.is_none()
        && plan.total_percent_used.is_none()
    {
        return None;
    }
    let start_ms = epoch_millis_from_value(value.get("billingCycleStart"));
    let end_ms = epoch_millis_from_value(value.get("billingCycleEnd"));
    cursor_usage_from_plan(
        plan,
        epoch_millis_to_system_time(end_ms),
        period_label(start_ms, end_ms),
    )
}

fn cursor_usage_from_summary(response: CursorUsageSummaryResponse) -> Option<UsageData> {
    let plan = response.individual_usage?.plan?;
    let reset = parse_iso8601(response.billing_cycle_end.as_deref());
    cursor_usage_from_plan(plan, reset, None)
}

fn cursor_usage_from_plan(
    plan: CursorPlanUsage,
    reset: Option<SystemTime>,
    session_label: Option<String>,
) -> Option<UsageData> {
    let section = |percentage: Option<f64>| {
        percentage
            .map(|percentage| UsageSection {
                available: true,
                percentage: percentage.clamp(0.0, 100.0),
                resets_at: reset,
            })
            .unwrap_or_default()
    };
    Some(UsageData {
        limits: Vec::new(),
        session: section(plan.auto_percent_used.or(plan.total_percent_used)),
        session_label,
        weekly: section(plan.api_percent_used),
        weekly_label: Some("API".into()),
        monthly: None,
        credits: None,
        stale: false,
        ..Default::default()
    })
}

/// Human-readable window length from the period bounds, e.g. "7d" or "5h".
fn period_label(start_ms: Option<i64>, end_ms: Option<i64>) -> Option<String> {
    let duration_ms = end_ms?.checked_sub(start_ms?)?;
    if duration_ms <= 0 {
        return None;
    }
    let duration_secs = duration_ms / 1000;
    if duration_secs < 60 {
        return None;
    }
    // Whole units only: a seven-day window reads "7d", not "168h".
    let days = duration_secs / 86_400;
    if days >= 1 {
        return Some(format!("{days}d"));
    }
    let hours = duration_secs / 3_600;
    if hours >= 1 {
        return Some(format!("{hours}h"));
    }
    Some(format!("{}m", duration_secs / 60))
}

fn epoch_millis_to_system_time(millis: Option<i64>) -> Option<SystemTime> {
    let millis = millis?;
    if millis < 0 {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::from_millis(millis as u64))
}

fn epoch_millis_from_value(value: Option<&serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value
        .and_then(|value| match value {
            serde_json::Value::Number(number) => number.as_f64(),
            serde_json::Value::String(text) => text.parse().ok(),
            _ => None,
        })
        .filter(|number| number.is_finite()))
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

fn path_signature(kind: &str, path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs())
                .unwrap_or(0);
            format!(
                "{kind}:{}|present|{}|{modified}",
                path.display(),
                metadata.len()
            )
        }
        Err(_) => format!("{kind}:{}|missing", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_cursor_user_id_from_a_jwt() {
        let jwt = "header.eyJzdWIiOiJhdXRoMHx1c2VyXzEyMyJ9.signature";
        assert_eq!(extract_cursor_user_id(jwt).as_deref(), Some("user_123"));
        assert_eq!(
            cursor_cookie_from_access_token(jwt).as_deref(),
            Some("user_123%3A%3Aheader.eyJzdWIiOiJhdXRoMHx1c2VyXzEyMyJ9.signature")
        );
    }

    #[test]
    fn rejects_malformed_base64_and_cookie_header_injection() {
        assert!(base64_url_decode("a").is_none());
        assert!(normalize_cursor_session_cookie("value\r\nInjected: yes").is_none());
    }

    #[test]
    fn cursor_usage_maps_auto_and_api_percentages() {
        let response: CursorUsageSummaryResponse = serde_json::from_str(
            r#"{
                "billingCycleEnd": "2026-08-25T19:27:24.000Z",
                "individualUsage": {
                    "plan": {
                        "autoPercentUsed": 12.5,
                        "apiPercentUsed": 3.0,
                        "totalPercentUsed": 10.0
                    }
                }
            }"#,
        )
        .unwrap();

        let data = cursor_usage_from_summary(response).unwrap();
        assert_eq!(data.session.percentage, 12.5);
        assert_eq!(data.weekly.percentage, 3.0);
        assert_eq!(data.weekly_label.as_deref(), Some("API"));
        assert!(data.session.resets_at.is_some());
        assert_eq!(data.session.resets_at, data.weekly.resets_at);
        assert!(data.session_label.is_none());
    }

    #[test]
    fn idle_cursor_windows_are_distinct_from_missing_metrics() {
        for (plan, session, weekly) in [
            (r#"{"autoPercentUsed":0,"apiPercentUsed":0}"#, true, true),
            (r#"{"totalPercentUsed":0}"#, true, false),
            (r#"{"apiPercentUsed":42}"#, false, true),
            ("{}", false, false),
        ] {
            for end in ["null", r#""2026-08-25T19:27:24Z""#] {
                let response = serde_json::from_str(&format!(
                    r#"{{"billingCycleEnd":{end},"individualUsage":{{"plan":{plan}}}}}"#,
                ))
                .unwrap();
                let data = cursor_usage_from_summary(response).unwrap();
                assert_eq!(data.session.available, session);
                assert_eq!(data.weekly.available, weekly);
                assert!(data.monthly.is_none());
            }
        }
    }

    #[test]
    fn current_period_maps_plan_usage_and_derives_the_window_label() {
        // Shapes observed against the live application service: epoch-millisecond
        // bounds as strings and percentages nested under `planUsage`.
        let value: serde_json::Value = serde_json::from_str(
            r#"{
                "enabled": true,
                "billingCycleStart": "1792405460651",
                "billingCycleEnd": "1793010260651",
                "planUsage": {
                    "autoPercentUsed": 7.085590465872156,
                    "apiPercentUsed": 0,
                    "totalPercentUsed": 7.085590465872156
                }
            }"#,
        )
        .unwrap();

        let data = cursor_usage_from_current_period(&value).unwrap();
        assert!(data.session.available);
        assert!((data.session.percentage - 7.085590465872156).abs() < 1e-9);
        assert!(data.weekly.available);
        assert_eq!(data.weekly.percentage, 0.0);
        assert_eq!(data.weekly_label.as_deref(), Some("API"));
        // 1792405460651..1793010260651 is exactly a seven-day window.
        assert_eq!(data.session_label.as_deref(), Some("7d"));
        let reset = data.session.resets_at.unwrap();
        assert_eq!(
            reset.duration_since(UNIX_EPOCH).unwrap().as_millis(),
            1_793_010_260_651
        );
    }

    #[test]
    fn current_period_rejects_disabled_or_empty_payloads_so_summary_can_fallback() {
        for json in [
            r#"{"enabled": false, "planUsage": {"autoPercentUsed": 7}}"#,
            r#"{"enabled": true, "billingCycleEnd": "1793010260651"}"#,
            r#"{"planUsage": {}}"#,
            r#"{"planUsage": {"autoPercentUsed": null}}"#,
            r#"{"planUsage": {"autoPercentUsed": "NaN", "apiPercentUsed": "inf"}}"#,
            r#"{"planUsage": {"autoPercentUsed": "invalid", "apiPercentUsed": false}}"#,
            r#"{}"#,
        ] {
            let value: serde_json::Value = serde_json::from_str(json).unwrap();
            assert!(cursor_usage_from_current_period(&value).is_none());
        }
    }

    #[test]
    fn current_period_accepts_numeric_millis_and_string_percentages() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{
                "billingCycleStart": 0,
                "billingCycleEnd": 18000000,
                "planUsage": {
                    "auto_percent_used": "12.5",
                    "api_percent_used": 2
                }
            }"#,
        )
        .unwrap();
        let data = cursor_usage_from_current_period(&value).unwrap();
        assert_eq!(data.session.percentage, 12.5);
        assert_eq!(data.weekly.percentage, 2.0);
        // 18000 seconds is a five-hour session window.
        assert_eq!(data.session_label.as_deref(), Some("5h"));
    }

    #[test]
    fn current_period_invalid_auto_percentage_uses_finite_total() {
        let value = serde_json::json!({
            "planUsage": {"autoPercentUsed": "NaN", "totalPercentUsed": "7.1"}
        });
        let data = cursor_usage_from_current_period(&value).unwrap();
        assert_eq!(data.session.percentage, 7.1);
        assert!(!data.weekly.available);
    }

    #[test]
    fn current_period_success_skips_summary_and_empty_plan_falls_back() {
        let data = cursor_usage_from_current_period(&serde_json::json!({
            "planUsage": {"autoPercentUsed": 0, "apiPercentUsed": 0}
        }))
        .unwrap();
        assert_eq!(
            fetch_cursor_usage_with(|| Ok(data.clone()), || panic!("unexpected fallback")),
            Ok(data.clone())
        );
        assert_eq!(
            fetch_cursor_usage_with(
                || cursor_usage_from_current_period(&serde_json::json!({"planUsage": {}}))
                    .ok_or(PollError::UnexpectedResponse),
                || Ok(data.clone()),
            ),
            Ok(data)
        );
    }

    #[test]
    fn internal_endpoint_rejection_keeps_summary_retry_and_auth_semantics() {
        for summary_error in [
            PollError::RequestFailed,
            PollError::HttpStatus(429),
            PollError::AuthRequired,
        ] {
            assert_eq!(
                fetch_cursor_usage_with(|| Err(PollError::AuthRequired), || Err(summary_error),),
                Err(summary_error)
            );
        }
        assert!(fetch_cursor_usage_with(
            || Err(PollError::AuthRequired),
            || Ok(UsageData::default()),
        )
        .is_ok());
    }

    #[test]
    fn invalid_epoch_millis_do_not_panic() {
        assert!(epoch_millis_to_system_time(Some(-1)).is_none());
        let _ = epoch_millis_to_system_time(Some(i64::MAX));
        assert!(epoch_millis_from_value(Some(&serde_json::json!(u64::MAX))).is_none());
        assert!(epoch_millis_from_value(Some(&serde_json::json!("invalid"))).is_none());
    }

    #[test]
    fn period_label_reflects_the_reported_bounds() {
        let start = 1_792_405_460_651_i64;
        let week = start + 7 * 86_400_000;
        assert_eq!(period_label(Some(start), Some(week)).as_deref(), Some("7d"));
        assert_eq!(
            period_label(Some(start), Some(start + 5 * 3_600_000)).as_deref(),
            Some("5h")
        );
        assert_eq!(
            period_label(Some(start), Some(start + 30 * 86_400_000)).as_deref(),
            Some("30d")
        );
        assert_eq!(
            period_label(Some(start), Some(start + 45 * 60_000)).as_deref(),
            Some("45m")
        );
        assert!(period_label(Some(start), Some(start)).is_none());
        assert!(period_label(None, Some(week)).is_none());
        assert!(period_label(Some(start), None).is_none());
    }

    #[test]
    fn environment_auth_extracts_a_bearer_token_from_cookie_forms() {
        let jwt = "header.eyJzdWIiOiJhdXRoMHx1c2VyXzEyMyJ9.signature";
        let cookie = format!("user_123%3A%3A{jwt}");
        let auth = cursor_auth_from_environment(&cookie).unwrap();
        assert_eq!(auth.access_token, jwt);
        assert_eq!(auth.cookie, cookie);

        let auth = cursor_auth_from_environment(jwt).unwrap();
        assert_eq!(auth.access_token, jwt);
        assert_eq!(auth.cookie, format!("user_123%3A%3A{jwt}"));

        let auth =
            cursor_auth_from_environment(&format!("WorkosCursorSessionToken={cookie}")).unwrap();
        assert_eq!(auth.access_token, jwt);
    }
}
