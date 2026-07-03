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
const DB_ENV: &str = "OPENCODE_DB";
const PROVIDER_ID: &str = "opencode-go";
const FIVE_HOUR_LIMIT_USD: f64 = 12.0;
const WEEKLY_LIMIT_USD: f64 = 30.0;
const MONTHLY_LIMIT_USD: f64 = 60.0;
const FIVE_HOUR_WINDOW_MS: i64 = 5 * 60 * 60 * 1_000;
const WEEKLY_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const MONTHLY_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1_000;

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
    if let Some(credentials) = read_dashboard_credentials() {
        return poll_dashboard(&credentials);
    }

    if let Some(path) = database_path() {
        return poll_local_database(&path);
    }

    diagnose::log(
        "OpenCode usage poll failed: no dashboard credentials and no local database found",
    );
    Err(PollError::NoCredentials)
}

pub(super) fn credential_watch_snapshot(_all_sources: bool) -> Vec<String> {
    vec![credential_watch_signature()]
}

fn poll_dashboard(credentials: &DashboardCredentials) -> Result<UsageData, PollError> {
    let usage = fetch_dashboard_usage(credentials).map_err(|error| {
        diagnose::log(format!(
            "OpenCode dashboard poll failed via {}: {error:?}",
            credentials.source
        ));
        error
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

fn poll_local_database(path: &Path) -> Result<UsageData, PollError> {
    let (five_hour, weekly, monthly) = local_database_totals(path).map_err(|error| {
        diagnose::log(format!(
            "OpenCode local database poll failed at {}: {error:?}",
            path.display()
        ));
        error
    })?;

    let now = SystemTime::now();
    let session = local_section(five_hour, FIVE_HOUR_LIMIT_USD, FIVE_HOUR_WINDOW_MS, now);
    let weekly_section = local_section(weekly, WEEKLY_LIMIT_USD, WEEKLY_WINDOW_MS, now);
    let monthly_section = local_section(monthly, MONTHLY_LIMIT_USD, MONTHLY_WINDOW_MS, now);
    let (weekly, weekly_label) = if monthly_section.percentage > weekly_section.percentage {
        (monthly_section, Some("30d".to_string()))
    } else {
        (weekly_section, Some("7d".to_string()))
    };

    Ok(UsageData {
        session,
        weekly,
        weekly_label,
    })
}

fn local_section(cost: f64, limit: f64, window_ms: i64, now: SystemTime) -> UsageSection {
    UsageSection {
        percentage: ((cost / limit) * 100.0).clamp(0.0, 100.0),
        resets_at: now.checked_add(Duration::from_millis(window_ms as u64)),
    }
}

fn local_database_totals(path: &Path) -> Result<(f64, f64, f64), PollError> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let five_hour_cutoff = now_ms.saturating_sub(FIVE_HOUR_WINDOW_MS);
    let weekly_cutoff = now_ms.saturating_sub(WEEKLY_WINDOW_MS);
    let monthly_cutoff = now_ms.saturating_sub(MONTHLY_WINDOW_MS);

    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        diagnose::log_error("unable to open OpenCode database", error);
        PollError::RequestFailed
    })?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| {
            diagnose::log_error("unable to set OpenCode database busy timeout", error);
            PollError::RequestFailed
        })?;

    let mut statement = connection
        .prepare(
            "SELECT \
                 COALESCE(SUM(CASE WHEN time_updated >= ?1 THEN cost ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN time_updated >= ?2 THEN cost ELSE 0 END), 0), \
                 COALESCE(SUM(CASE WHEN time_updated >= ?3 THEN cost ELSE 0 END), 0) \
             FROM session \
             WHERE json_extract(model, '$.providerID') = ?4 \
               AND time_archived IS NULL",
        )
        .map_err(|error| {
            diagnose::log_error("unable to query OpenCode session usage", error);
            PollError::RequestFailed
        })?;

    statement
        .query_row(
            rusqlite::params![five_hour_cutoff, weekly_cutoff, monthly_cutoff, PROVIDER_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| {
            diagnose::log_error("unable to read OpenCode session totals", error);
            PollError::RequestFailed
        })
}

fn read_dashboard_credentials() -> Option<DashboardCredentials> {
    if let (Some(workspace_id), Some(auth_cookie)) = (
        non_empty_environment(WORKSPACE_ID_ENV),
        non_empty_environment(AUTH_COOKIE_ENV),
    ) {
        return Some(DashboardCredentials {
            workspace_id,
            auth_cookie,
            source: "environment".to_string(),
        });
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
    if workspace_id.is_empty() || auth_cookie.is_empty() {
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
    let cookie = if credentials.auth_cookie.contains("auth=") {
        credentials.auth_cookie.clone()
    } else {
        format!("auth={}", credentials.auth_cookie)
    };

    let response = match build_agent()?
        .get(&url)
        .set("Accept", "text/html,application/xhtml+xml")
        .set("Cookie", &cookie)
        .set("User-Agent", DASHBOARD_USER_AGENT)
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(401 | 403, _)) => return Err(PollError::AuthRequired),
        Err(error) => {
            diagnose::log_error("OpenCode Go dashboard request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let html = response.into_string().map_err(|error| {
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
    use std::sync::OnceLock;

    static WINDOW: OnceLock<regex::Regex> = OnceLock::new();
    static PERCENTAGE: OnceLock<regex::Regex> = OnceLock::new();
    static RESET: OnceLock<regex::Regex> = OnceLock::new();
    let window = WINDOW.get_or_init(|| {
        regex::Regex::new(
            r#"["']?(rollingUsage|weeklyUsage|monthlyUsage)["']?\s*:\s*(?:\$R\[\d+\]\s*=\s*)?\{(?P<body>[^{}]*)\}"#,
        )
        .expect("valid OpenCode window regex")
    });
    let percentage = PERCENTAGE.get_or_init(|| {
        regex::Regex::new(r#"["']?usagePercent["']?\s*:\s*"?(-?\d+(?:\.\d+)?)"?"#)
            .expect("valid OpenCode percentage regex")
    });
    let reset = RESET.get_or_init(|| {
        regex::Regex::new(r#"["']?resetInSec["']?\s*:\s*"?(-?\d+(?:\.\d+)?)"?"#)
            .expect("valid OpenCode reset regex")
    });

    let body = window
        .captures_iter(text)
        .find(|capture| {
            capture
                .get(1)
                .is_some_and(|value| value.as_str() == field_name)
        })?
        .name("body")?
        .as_str();
    Some(UsageWindow {
        usage_percent: percentage.captures(body)?.get(1)?.as_str().parse().ok()?,
        reset_in_sec: reset
            .captures(body)?
            .get(1)?
            .as_str()
            .parse::<f64>()
            .ok()?
            .max(0.0) as i64,
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

fn database_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(DB_ENV).map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    if let Some(home) = dirs::home_dir() {
        let path = home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db");
        if path.is_file() {
            return Some(path);
        }
    }
    let path = dirs::data_dir()?.join("opencode").join("opencode.db");
    path.is_file().then_some(path)
}

fn non_empty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
    match database_path() {
        Some(path) => parts.push(path_signature("database", &path)),
        None => parts.push("database|missing".to_string()),
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
}
