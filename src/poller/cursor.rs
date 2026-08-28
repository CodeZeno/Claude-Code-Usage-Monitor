use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::{build_agent, parse_iso8601, PollError};
use crate::diagnose;
use crate::models::{UsageData, UsageSection};

const CURSOR_USAGE_SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";
const CURSOR_SESSION_TOKEN_ENV: &str = "CURSOR_SESSION_TOKEN";
const CURSOR_ACCESS_TOKEN_KEY: &str = "cursorAuth/accessToken";

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
    #[serde(rename = "autoPercentUsed")]
    auto_percent_used: Option<f64>,
    #[serde(rename = "apiPercentUsed")]
    api_percent_used: Option<f64>,
    #[serde(rename = "totalPercentUsed")]
    total_percent_used: Option<f64>,
}

pub(super) fn poll_cursor() -> Result<UsageData, PollError> {
    let cookie = read_cursor_session_cookie().ok_or_else(|| {
        diagnose::log(
            "Cursor usage poll failed: no Cursor session found (sign in to Cursor or set CURSOR_SESSION_TOKEN)",
        );
        PollError::NoCredentials
    })?;
    fetch_cursor_usage(&cookie)
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

/// Resolve a Cursor dashboard session cookie. An explicit environment value
/// takes priority over the access token persisted by Cursor itself.
fn read_cursor_session_cookie() -> Option<String> {
    if let Some(token) = non_empty_environment(CURSOR_SESSION_TOKEN_ENV) {
        return normalize_cursor_session_cookie(&token);
    }

    let access_token = read_cursor_access_token_from_state_db()?;
    cursor_cookie_from_access_token(&access_token)
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
    let path = dirs::config_dir()?
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

fn fetch_cursor_usage(cookie: &str) -> Result<UsageData, PollError> {
    let cookie_header = format!("WorkosCursorSessionToken={cookie}");
    let mut response = match build_agent()?
        .get(CURSOR_USAGE_SUMMARY_URL)
        .header("Cookie", &cookie_header)
        .header("User-Agent", "Mozilla/5.0")
        .call()
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

fn cursor_usage_from_summary(response: CursorUsageSummaryResponse) -> Option<UsageData> {
    let plan = response.individual_usage?.plan?;
    let reset = parse_iso8601(response.billing_cycle_end.as_deref());
    let auto = plan
        .auto_percent_used
        .or(plan.total_percent_used)
        .unwrap_or(0.0)
        .clamp(0.0, 100.0);
    let api = plan.api_percent_used.unwrap_or(0.0).clamp(0.0, 100.0);
    Some(UsageData {
        session: UsageSection {
            percentage: auto,
            resets_at: reset,
        },
        weekly: UsageSection {
            percentage: api,
            resets_at: reset,
        },
        weekly_label: Some("API".into()),
        monthly: None,
        credits: None,
        stale: false,
    })
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
    }
}
