use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::{build_agent, parse_iso8601, PollError};
use crate::diagnose;
use crate::models::{UsageData, UsageSection};

const GO_STATUS_URL: &str = "https://opencode.ai/console/api/go/status";
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
    resets_at: Option<SystemTime>,
}

#[derive(Debug, Default, PartialEq)]
struct DashboardUsage {
    rolling: Option<UsageWindow>,
    weekly: Option<UsageWindow>,
    monthly: Option<UsageWindow>,
}

// The console JSON API serializes microcent amounts as strings (JavaScript BigInts).
#[derive(Deserialize)]
struct GoStatus {
    access: Option<GoAccess>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoAccess {
    ends_at: String,
    meters: GoMeters,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoMeters {
    five_hour: GoTimedMeter,
    week: GoTimedMeter,
    month: GoMeter,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoTimedMeter {
    #[serde(flatten)]
    meter: GoMeter,
    // An unused rolling window has no reset time until the first request.
    resets_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoMeter {
    limit_micro_cents: String,
    used_micro_cents: String,
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

    let session = usage
        .rolling
        .as_ref()
        .map(section_from_window)
        .unwrap_or_default();
    let (weekly, weekly_label) = select_long_window(&usage);

    Ok(UsageData {
        limits: Vec::new(),
        session,
        weekly,
        weekly_label,
        // The monthly window is kept available to themes alongside the
        // auto-selected `weekly` slot (which prefers the more constrained
        // of the two windows, as before).
        monthly: usage.monthly.as_ref().map(section_from_window),
        credits: None,
        stale: false,
    })
}

fn select_long_window(usage: &DashboardUsage) -> (UsageSection, Option<String>) {
    match (&usage.weekly, &usage.monthly) {
        (Some(weekly), Some(monthly)) if monthly.usage_percent > weekly.usage_percent => {
            (section_from_window(monthly), Some("30d".to_string()))
        }
        (Some(weekly), _) => (section_from_window(weekly), Some("7d".to_string())),
        (None, Some(monthly)) => (section_from_window(monthly), Some("30d".to_string())),
        (None, None) => (UsageSection::default(), None),
    }
}

fn section_from_window(window: &UsageWindow) -> UsageSection {
    UsageSection {
        available: true,
        percentage: window.usage_percent.clamp(0.0, 100.0),
        resets_at: window.resets_at,
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
    fetch_go_status(credentials, GO_STATUS_URL)
}

fn fetch_go_status(
    credentials: &DashboardCredentials,
    url: &str,
) -> Result<DashboardUsage, PollError> {
    let cookie = if credentials.auth_cookie.split(';').any(|part| {
        let part = part.trim_start();
        part.starts_with("auth=") || part.starts_with("__Host-console_session=")
    }) {
        credentials.auth_cookie.clone()
    } else {
        // Bare legacy auth values may contain '=' padding; that alone does
        // not identify a complete Cookie header.
        format!("auth={}", credentials.auth_cookie)
    };

    let mut response = match build_agent()?
        .get(url)
        .header("Accept", "application/json")
        .header("x-org-id", &credentials.workspace_id)
        .header("Cookie", &cookie)
        .header("User-Agent", DASHBOARD_USER_AGENT)
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401 | 403)) => return Err(PollError::AuthRequired),
        Err(error) => {
            diagnose::log_error("OpenCode Go status request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let status = response
        .body_mut()
        .read_json::<Option<GoStatus>>()
        .map_err(|error| {
            diagnose::log_error("OpenCode Go status response is not valid JSON", error);
            PollError::RequestFailed
        })?;
    usage_from_status(status)
}

fn usage_from_status(status: Option<GoStatus>) -> Result<DashboardUsage, PollError> {
    let access = status.and_then(|status| status.access).ok_or_else(|| {
        diagnose::log("OpenCode Go status returned no active subscription access");
        PollError::RequestFailed
    })?;
    Ok(DashboardUsage {
        rolling: Some(window_from_meter(
            &access.meters.five_hour.meter,
            access.meters.five_hour.resets_at.as_deref(),
        )?),
        weekly: Some(window_from_meter(
            &access.meters.week.meter,
            access.meters.week.resets_at.as_deref(),
        )?),
        monthly: Some(window_from_meter(
            &access.meters.month,
            Some(&access.ends_at),
        )?),
    })
}

fn window_from_meter(meter: &GoMeter, resets_at: Option<&str>) -> Result<UsageWindow, PollError> {
    let limit = meter.limit_micro_cents.parse::<u128>().map_err(|_| {
        diagnose::log("OpenCode Go status contains an invalid usage limit");
        PollError::RequestFailed
    })?;
    let used = meter.used_micro_cents.parse::<u128>().map_err(|_| {
        diagnose::log("OpenCode Go status contains an invalid usage amount");
        PollError::RequestFailed
    })?;
    let resets_at = resets_at
        .map(|value| {
            parse_iso8601(Some(value)).ok_or_else(|| {
                diagnose::log("OpenCode Go status contains an invalid reset time");
                PollError::RequestFailed
            })
        })
        .transpose()?;
    Ok(UsageWindow {
        usage_percent: if limit == 0 {
            0.0
        } else {
            used as f64 / limit as f64 * 100.0
        },
        resets_at,
    })
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

    // Wire shape verified against the console's Go status schema and usage UI.
    const GO_STATUS_JSON: &str = r#"{
        "subscriberUserID": "usr_example",
        "useBalance": false,
        "access": {
            "startsAt": "2026-09-01T00:00:00.000Z",
            "endsAt": "2026-10-01T00:00:00.000Z",
            "meters": {
                "fiveHour": {
                    "limitMicroCents": "2000000000",
                    "usedMicroCents": "250000000",
                    "resetsAt": "2026-09-17T05:00:00.000Z"
                },
                "week": {
                    "limitMicroCents": "10000000000",
                    "usedMicroCents": "4500000000",
                    "resetsAt": "2026-09-21T00:00:00.000Z"
                },
                "month": {
                    "limitMicroCents": "20000000000",
                    "usedMicroCents": "12000000000"
                }
            }
        }
    }"#;

    #[test]
    fn console_status_maps_meter_percentages_and_absolute_resets() {
        let usage = usage_from_status(serde_json::from_str(GO_STATUS_JSON).unwrap()).unwrap();
        let rolling = usage.rolling.as_ref().unwrap();
        assert_eq!(rolling.usage_percent, 12.5);
        assert_eq!(
            rolling.resets_at,
            parse_iso8601(Some("2026-09-17T05:00:00Z"))
        );
        let weekly = usage.weekly.as_ref().unwrap();
        assert_eq!(weekly.usage_percent, 45.0);
        assert_eq!(
            weekly.resets_at,
            parse_iso8601(Some("2026-09-21T00:00:00Z"))
        );
        let (long, label) = select_long_window(&usage);
        assert_eq!(long.percentage, 60.0);
        assert_eq!(label.as_deref(), Some("30d"));
        assert_eq!(long.resets_at, parse_iso8601(Some("2026-10-01T00:00:00Z")));
    }

    #[test]
    fn console_status_preserves_idle_rolling_window_without_reset() {
        let json = GO_STATUS_JSON
            .replace("\"250000000\"", "\"0\"")
            .replace("\"2026-09-17T05:00:00.000Z\"", "null");
        let usage = usage_from_status(serde_json::from_str(&json).unwrap()).unwrap();
        let section = section_from_window(usage.rolling.as_ref().unwrap());
        assert!(section.available);
        assert_eq!(section.percentage, 0.0);
        assert_eq!(section.resets_at, None);
    }

    #[test]
    fn console_status_rejects_missing_access_and_invalid_meter_values() {
        for json in ["null", r#"{"access":null}"#] {
            assert_eq!(
                usage_from_status(serde_json::from_str(json).unwrap()),
                Err(PollError::RequestFailed)
            );
        }
        for json in [
            GO_STATUS_JSON.replace("250000000", "-1"),
            GO_STATUS_JSON.replace("250000000", "NaN"),
            GO_STATUS_JSON.replace("250000000", "1.5"),
            GO_STATUS_JSON.replace("2026-09-17T05:00:00.000Z", "invalid-date"),
        ] {
            assert_eq!(
                usage_from_status(serde_json::from_str(&json).unwrap()),
                Err(PollError::RequestFailed)
            );
        }
    }

    #[test]
    fn console_meter_handles_zero_limits_large_amounts_and_overages() {
        let meter = |used: &str, limit: &str| GoMeter {
            used_micro_cents: used.into(),
            limit_micro_cents: limit.into(),
        };
        assert_eq!(
            window_from_meter(&meter("0", "0"), None)
                .unwrap()
                .usage_percent,
            0.0
        );
        assert_eq!(
            window_from_meter(&meter("10000000000000000000", "20000000000000000000"), None)
                .unwrap()
                .usage_percent,
            50.0
        );
        let window = window_from_meter(&meter("120", "100"), None).unwrap();
        assert_eq!(section_from_window(&window).percentage, 100.0);
    }

    fn mock_status_request(
        status: u16,
        body: &str,
        cookie: &str,
    ) -> (Result<DashboardUsage, PollError>, String) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::Duration;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/console/api/go/status",
            listener.local_addr().unwrap()
        );
        let response = format!(
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&buffer[..count]);
            }
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8(request).unwrap()
        });
        let result = fetch_go_status(
            &DashboardCredentials {
                workspace_id: "wrk_example".into(),
                auth_cookie: cookie.into(),
                source: "test".into(),
            },
            &url,
        );
        (result, server.join().unwrap())
    }

    #[test]
    fn console_request_sends_workspace_context_and_normalizes_auth_cookie() {
        for (cookie, expected) in [
            ("test-token", "auth=test-token"),
            ("test-token==", "auth=test-token=="),
            ("auth=test-token; theme=dark", "auth=test-token; theme=dark"),
            (
                "__Host-console_session=SessionToken==",
                "__Host-console_session=SessionToken==",
            ),
            (
                "__Host-console_session=SessionToken; __stripe_mid=StripeValue",
                "__Host-console_session=SessionToken; __stripe_mid=StripeValue",
            ),
            (
                "theme=dark; __Host-console_session=SessionToken",
                "theme=dark; __Host-console_session=SessionToken",
            ),
            (
                "auth=; __Host-console_session=SessionToken",
                "auth=; __Host-console_session=SessionToken",
            ),
        ] {
            let (result, request) = mock_status_request(200, GO_STATUS_JSON, cookie);
            assert_eq!(result.unwrap().rolling.unwrap().usage_percent, 12.5);
            let sent_cookie = request
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("cookie")
                        .then_some(value.trim_start())
                })
                .unwrap();
            assert_eq!(sent_cookie, expected);
            let request = request.to_ascii_lowercase();
            assert!(request.starts_with("get /console/api/go/status http/1.1\r\n"));
            assert!(request.contains("\r\nx-org-id: wrk_example\r\n"));
            assert!(request.contains("\r\naccept: application/json\r\n"));
        }
    }

    #[test]
    fn console_request_distinguishes_auth_errors_from_bad_responses() {
        for status in [401, 403] {
            assert_eq!(
                mock_status_request(status, "{}", "test-token").0,
                Err(PollError::AuthRequired)
            );
        }
        for (status, body) in [
            (400, "{}"),
            (500, "{}"),
            (200, "<html><div id=app></div></html>"),
            (200, "{invalid"),
            (200, "{}"),
            (200, "null"),
            (200, r#"{"access":{"meters":{}}}"#),
        ] {
            assert_eq!(
                mock_status_request(status, body, "test-token").0,
                Err(PollError::RequestFailed)
            );
        }
    }

    #[test]
    fn most_constrained_long_window_is_selected() {
        let usage = DashboardUsage {
            weekly: Some(UsageWindow {
                usage_percent: 40.0,
                resets_at: Some(UNIX_EPOCH),
            }),
            monthly: Some(UsageWindow {
                usage_percent: 70.0,
                resets_at: Some(UNIX_EPOCH),
            }),
            ..Default::default()
        };
        let (section, label) = select_long_window(&usage);
        assert_eq!(section.percentage, 70.0);
        assert_eq!(label.as_deref(), Some("30d"));
    }

    #[test]
    fn idle_opencode_windows_are_available_and_missing_windows_are_not() {
        let window = UsageWindow {
            usage_percent: 0.0,
            resets_at: None,
        };
        let section = section_from_window(&window);
        assert!(section.available);
        assert_eq!(section.percentage, 0.0);
        let usage = DashboardUsage {
            monthly: Some(window),
            ..Default::default()
        };
        let (section, label) = select_long_window(&usage);
        assert!(section.available);
        assert_eq!(label.as_deref(), Some("30d"));
        assert!(!select_long_window(&DashboardUsage::default()).0.available);
    }

    #[test]
    fn dashboard_identifiers_and_cookie_headers_reject_request_injection() {
        assert!(valid_workspace_id("wrk_01-test"));
        assert!(!valid_workspace_id("../other"));
        assert!(valid_cookie("auth=abc; theme=dark"));
        assert!(!valid_cookie("auth=abc\r\nX-Test: injected"));
    }
}
