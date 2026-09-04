use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::{build_agent, PollError};
use crate::diagnose;
use crate::models::{UsageData, UsageSection};

const DASHBOARD_URL_PREFIX: &str = "https://opencode.ai/workspace/";
const DASHBOARD_URL_SUFFIX: &str = "/go";
const DASHBOARD_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/126.0 Safari/537.36";
const WORKSPACE_ID_ENV: &str = "OPENCODE_GO_WORKSPACE_ID";
const AUTH_COOKIE_ENV: &str = "OPENCODE_GO_AUTH_COOKIE";
const CONFIG_FILE_ENV: &str = "OPENCODE_GO_CONFIG_FILE";

#[derive(Deserialize)]
struct DashboardConfig {
    #[serde(alias = "workspaceId", alias = "workspaceID")]
    workspace_id: String,
    #[serde(alias = "authCookie", alias = "cookie")]
    auth_cookie: String,
}

struct DashboardCredentials {
    workspace_id: String,
    auth_cookie: String,
    source: String,
}

#[derive(Clone, Debug, PartialEq)]
struct UsageWindow {
    usage_percent: f64,
    reset_in_sec: i64,
}

#[derive(Debug, Default, PartialEq)]
struct DashboardUsage {
    rolling: Option<UsageWindow>,
    weekly: Option<UsageWindow>,
    monthly: Option<UsageWindow>,
}

pub(super) fn poll_opencode() -> Result<UsageData, PollError> {
    let credentials = read_dashboard_credentials().ok_or_else(|| {
        diagnose::log("OpenCode usage poll failed: no dashboard credentials found");
        PollError::NoCredentials
    })?;
    poll_dashboard(&credentials)
}

pub(super) fn credential_watch_snapshot(_all_sources: bool) -> Vec<String> {
    vec![credential_watch_signature()]
}

fn poll_dashboard(credentials: &DashboardCredentials) -> Result<UsageData, PollError> {
    let usage = fetch_dashboard_usage(credentials).inspect_err(|error| {
        diagnose::log(format!(
            "OpenCode dashboard poll failed via {}: {error:?}",
            credentials.source
        ));
    })?;

    if usage.rolling.is_none() && usage.weekly.is_none() && usage.monthly.is_none() {
        diagnose::log(format!(
            "OpenCode dashboard returned no usage windows from {}",
            credentials.source
        ));
        return Err(PollError::RequestFailed);
    }

    let now = SystemTime::now();
    let session = usage
        .rolling
        .as_ref()
        .map(|window| section_from_window(window, now))
        .unwrap_or_default();
    let (weekly, weekly_label) = select_long_window(&usage, now);

    Ok(UsageData {
        session,
        weekly,
        weekly_label,
        // The monthly window is kept available to themes alongside the
        // auto-selected `weekly` slot (which prefers the more constrained
        // of the two windows, as before).
        monthly: usage
            .monthly
            .as_ref()
            .map(|window| section_from_window(window, now)),
        fable: None,
        credits: None,
        stale: false,
    })
}

fn select_long_window(usage: &DashboardUsage, now: SystemTime) -> (UsageSection, Option<String>) {
    match (&usage.weekly, &usage.monthly) {
        (Some(weekly), Some(monthly)) if monthly.usage_percent > weekly.usage_percent => {
            (section_from_window(monthly, now), Some("30d".to_string()))
        }
        (Some(weekly), _) => (section_from_window(weekly, now), Some("7d".to_string())),
        (None, Some(monthly)) => (section_from_window(monthly, now), Some("30d".to_string())),
        (None, None) => (UsageSection::default(), None),
    }
}

fn section_from_window(window: &UsageWindow, now: SystemTime) -> UsageSection {
    UsageSection {
        percentage: window.usage_percent.clamp(0.0, 100.0),
        resets_at: now.checked_add(Duration::from_secs(window.reset_in_sec.max(0) as u64)),
    }
}

fn read_dashboard_credentials() -> Option<DashboardCredentials> {
    if let (Some(workspace_id), Some(auth_cookie)) = (
        non_empty_environment(WORKSPACE_ID_ENV),
        non_empty_environment(AUTH_COOKIE_ENV),
    ) {
        if valid_workspace_id(&workspace_id) && valid_cookie(&auth_cookie) {
            return Some(DashboardCredentials {
                workspace_id,
                auth_cookie,
                source: "environment".to_string(),
            });
        }
    }

    dashboard_config_paths()
        .into_iter()
        .find_map(|path| read_dashboard_config(&path))
}

fn read_dashboard_config(path: &Path) -> Option<DashboardCredentials> {
    let content = std::fs::read_to_string(path).ok()?;
    let config: DashboardConfig = serde_json::from_str(&content).ok()?;
    let workspace_id = config.workspace_id.trim().to_string();
    let auth_cookie = config.auth_cookie.trim().to_string();
    if !valid_workspace_id(&workspace_id) || !valid_cookie(&auth_cookie) {
        return None;
    }
    Some(DashboardCredentials {
        workspace_id,
        auth_cookie,
        source: path.display().to_string(),
    })
}

fn fetch_dashboard_usage(credentials: &DashboardCredentials) -> Result<DashboardUsage, PollError> {
    let url = format!(
        "{DASHBOARD_URL_PREFIX}{}{DASHBOARD_URL_SUFFIX}",
        credentials.workspace_id
    );
    let cookie = if credentials
        .auth_cookie
        .split(';')
        .any(|part| part.trim_start().starts_with("auth="))
    {
        credentials.auth_cookie.clone()
    } else {
        format!("auth={}", credentials.auth_cookie)
    };

    let mut response = match build_agent()?
        .get(&url)
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Cookie", &cookie)
        .header("User-Agent", DASHBOARD_USER_AGENT)
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401 | 403)) => return Err(PollError::AuthRequired),
        Err(error) => {
            diagnose::log_error("OpenCode Go dashboard request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let html = response.body_mut().read_to_string().map_err(|error| {
        diagnose::log_error("OpenCode Go dashboard response is not UTF-8", error);
        PollError::RequestFailed
    })?;
    Ok(parse_dashboard_html(&html))
}

fn parse_dashboard_html(html: &str) -> DashboardUsage {
    let normalized = html
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .replace("\\\"", "\"")
        .replace("\\u0022", "\"");
    DashboardUsage {
        rolling: parse_window("rollingUsage", &normalized),
        weekly: parse_window("weeklyUsage", &normalized),
        monthly: parse_window("monthlyUsage", &normalized),
    }
}

fn parse_window(field_name: &str, text: &str) -> Option<UsageWindow> {
    text.match_indices(field_name)
        .find_map(|(index, _)| parse_window_value(field_value_at(text, field_name, index)?))
}

fn parse_window_value(mut value: &str) -> Option<UsageWindow> {
    if let Some(rest) = value.strip_prefix("$R[") {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        value = rest.get(digits..)?.strip_prefix(']')?.trim_start();
        value = value.strip_prefix('=')?.trim_start();
    }
    let body = value.strip_prefix('{')?.split_once('}')?.0;
    Some(UsageWindow {
        usage_percent: numeric_field(body, "usagePercent")?,
        reset_in_sec: numeric_field(body, "resetInSec")?.max(0.0) as i64,
    })
}

fn field_value<'a>(text: &'a str, field_name: &str) -> Option<&'a str> {
    text.match_indices(field_name)
        .find_map(|(index, _)| field_value_at(text, field_name, index))
}

fn field_value_at<'a>(text: &'a str, field_name: &str, index: usize) -> Option<&'a str> {
    let preceding = text[..index].bytes().next_back();
    if preceding.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        return None;
    }

    let mut remainder = &text[index + field_name.len()..];
    if remainder.starts_with(['\'', '"']) {
        remainder = &remainder[1..];
    }
    remainder
        .trim_start()
        .strip_prefix(':')
        .map(str::trim_start)
}

fn numeric_field(text: &str, field_name: &str) -> Option<f64> {
    let mut value = field_value(text, field_name)?;
    if value.starts_with(['\'', '"']) {
        value = &value[1..];
    }

    let bytes = value.as_bytes();
    let mut end = usize::from(bytes.first() == Some(&b'-'));
    let integer_start = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if end == integer_start {
        return None;
    }
    if bytes.get(end) == Some(&b'.') {
        let fraction_start = end + 1;
        end = fraction_start;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == fraction_start {
            return None;
        }
    }
    value[..end].parse().ok()
}

fn dashboard_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = non_empty_environment(CONFIG_FILE_ENV).map(PathBuf::from) {
        paths.push(path);
    }
    if let Some(app_data) = non_empty_environment("APPDATA").map(PathBuf::from) {
        paths.push(app_data.join("opencode-go").join("config.json"));
    }
    if let Some(config_home) = non_empty_environment("XDG_CONFIG_HOME").map(PathBuf::from) {
        paths.push(config_home.join("opencode-bar").join("opencode-go.json"));
        paths.push(config_home.join("opencode-quota").join("opencode-go.json"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(
            home.join(".config")
                .join("opencode-bar")
                .join("opencode-go.json"),
        );
        paths.push(
            home.join(".config")
                .join("opencode-quota")
                .join("opencode-go.json"),
        );
    }
    paths
}

fn non_empty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn valid_workspace_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_cookie(value: &str) -> bool {
    !value.is_empty() && !value.bytes().any(|byte| matches!(byte, b'\r' | b'\n'))
}

fn credential_watch_signature() -> String {
    let mut parts = Vec::new();
    match read_dashboard_credentials() {
        Some(credentials) => {
            let mut hasher = DefaultHasher::new();
            credentials.workspace_id.hash(&mut hasher);
            credentials.auth_cookie.hash(&mut hasher);
            parts.push(format!(
                "dashboard|present|{}|{}|{:x}|{}",
                credentials.workspace_id.len(),
                credentials.auth_cookie.len(),
                hasher.finish(),
                credentials.source
            ));
        }
        None => parts.push("dashboard|missing".to_string()),
    }
    for path in dashboard_config_paths() {
        parts.push(path_signature("config", &path));
    }
    parts.join(";;")
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
    fn dashboard_parser_accepts_serialized_and_html_escaped_windows() {
        let html = r#"rollingUsage:{usagePercent:12.5,resetInSec:300},&quot;weeklyUsage&quot;:{&quot;usagePercent&quot;:&quot;45&quot;,&quot;resetInSec&quot;:7200},monthlyUsage:$R[7]={usagePercent:60,resetInSec:9000}"#;
        let usage = parse_dashboard_html(html);
        assert_eq!(usage.rolling.unwrap().usage_percent, 12.5);
        assert_eq!(usage.weekly.unwrap().reset_in_sec, 7_200);
        assert_eq!(usage.monthly.unwrap().usage_percent, 60.0);
    }

    #[test]
    fn dashboard_parser_rejects_lookalike_and_malformed_fields() {
        let html = r#"notrollingUsage:{usagePercent:1,resetInSec:2},rollingUsage:null,rollingUsage:{usagePercent:7,resetInSec:8},weeklyUsage:$R[x]={usagePercent:3,resetInSec:4},monthlyUsage:{usagePercent:.5,resetInSec:6}"#;
        let usage = parse_dashboard_html(html);
        assert_eq!(
            usage.rolling,
            Some(UsageWindow {
                usage_percent: 7.0,
                reset_in_sec: 8,
            })
        );
        assert!(usage.weekly.is_none());
        assert!(usage.monthly.is_none());
    }

    #[test]
    fn most_constrained_long_window_is_selected() {
        let usage = DashboardUsage {
            weekly: Some(UsageWindow {
                usage_percent: 40.0,
                reset_in_sec: 60,
            }),
            monthly: Some(UsageWindow {
                usage_percent: 70.0,
                reset_in_sec: 120,
            }),
            ..Default::default()
        };
        let (section, label) = select_long_window(&usage, UNIX_EPOCH);
        assert_eq!(section.percentage, 70.0);
        assert_eq!(label.as_deref(), Some("30d"));
    }

    #[test]
    fn dashboard_identifiers_and_cookie_headers_reject_request_injection() {
        assert!(valid_workspace_id("wrk_01-test"));
        assert!(!valid_workspace_id("../other"));
        assert!(valid_cookie("auth=abc; theme=dark"));
        assert!(!valid_cookie("auth=abc\r\nX-Test: injected"));
    }
}
