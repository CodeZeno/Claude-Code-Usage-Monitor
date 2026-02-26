use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::models::{UsageData, UsageSection};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_SCOPES: &str = "user:profile user:inference user:sessions:claude_code user:mcp_servers";

const MODEL_FALLBACK_CHAIN: &[&str] = &[
    "claude-3-haiku-20240307",
    "claude-haiku-4-5-20251001",
];

#[derive(Debug)]
pub enum PollError {
    NoCredentials,
    TokenExpired,
    AllModelsFailed,
}

pub fn poll() -> Result<UsageData, PollError> {
    let creds = read_credentials().ok_or(PollError::NoCredentials)?;

    let token = if is_token_expired(creds.expires_at) {
        // Token expired — try to refresh it
        let refresh = creds.refresh_token.ok_or(PollError::TokenExpired)?;
        refresh_access_token(&refresh).map_err(|_| PollError::TokenExpired)?
    } else {
        creds.access_token
    };

    fetch_usage_with_fallback(&token)
}

fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    for model in MODEL_FALLBACK_CHAIN {
        if let Some(data) = try_model(token, model) {
            return Ok(data);
        }
    }

    Err(PollError::AllModelsFailed)
}

struct Credentials {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
}

fn read_credentials() -> Option<Credentials> {
    let home = dirs::home_dir()?;
    let cred_path: PathBuf = home.join(".claude").join(".credentials.json");

    let content = std::fs::read_to_string(&cred_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    let oauth = json.get("claudeAiOauth")?;
    Some(Credentials {
        access_token: oauth.get("accessToken")?.as_str()?.to_string(),
        refresh_token: oauth.get("refreshToken").and_then(|v| v.as_str()).map(String::from),
        expires_at: oauth.get("expiresAt").and_then(|v| v.as_i64()),
    })
}

fn is_token_expired(expires_at: Option<i64>) -> bool {
    let Some(exp) = expires_at else { return false };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    now >= exp
}

/// Refresh the OAuth token using the refresh grant.
/// Returns the new access token in-memory only — does not write to disk.
fn refresh_access_token(refresh_token: &str) -> Result<String, String> {
    let tls = std::sync::Arc::new(
        native_tls::TlsConnector::new().map_err(|e| e.to_string())?
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .tls_connector(tls)
        .build();

    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
        "scope": OAUTH_SCOPES,
    });

    let resp = agent
        .post(TOKEN_URL)
        .set("Content-Type", "application/json")
        .send_json(&body)
        .map_err(|e| e.to_string())?;

    let resp_body: serde_json::Value = resp
        .into_json()
        .map_err(|e| e.to_string())?;

    resp_body.get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("missing access_token in refresh response".to_string())
        .map(String::from)
}

fn try_model(token: &str, model: &str) -> Option<UsageData> {
    let tls = std::sync::Arc::new(
        native_tls::TlsConnector::new().ok()?
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .tls_connector(tls)
        .build();

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "."}]
    });

    let response = match agent
        .post(API_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-version", "2023-06-01")
        .set("anthropic-beta", "oauth-2025-04-20")
        .send_json(&body)
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(_, resp)) => resp,
        Err(_) => return None,
    };

    let has_rate_limit_headers = response.header("anthropic-ratelimit-unified-5h-utilization").is_some()
        || response.header("anthropic-ratelimit-unified-7d-utilization").is_some()
        || response.header("anthropic-ratelimit-unified-status").is_some();

    if has_rate_limit_headers {
        Some(parse_headers(&response))
    } else {
        None
    }
}

fn parse_headers(response: &ureq::Response) -> UsageData {
    let mut data = UsageData::default();

    // Session (5-hour window)
    data.session.percentage = get_header_f64(response, "anthropic-ratelimit-unified-5h-utilization") * 100.0;
    data.session.resets_at = unix_to_system_time(get_header_i64(response, "anthropic-ratelimit-unified-5h-reset"));

    // Weekly (7-day window)
    data.weekly.percentage = get_header_f64(response, "anthropic-ratelimit-unified-7d-utilization") * 100.0;
    data.weekly.resets_at = unix_to_system_time(get_header_i64(response, "anthropic-ratelimit-unified-7d-reset"));

    // Overall reset/status fallback
    let overall_reset = get_header_i64(response, "anthropic-ratelimit-unified-reset");

    if data.session.percentage == 0.0 && data.weekly.percentage == 0.0 {
        let status = get_header_str(response, "anthropic-ratelimit-unified-status");
        if status.as_deref() == Some("rejected") {
            let claim = get_header_str(response, "anthropic-ratelimit-unified-representative-claim");
            match claim.as_deref() {
                Some("five_hour") => data.session.percentage = 100.0,
                Some("seven_day") => data.weekly.percentage = 100.0,
                _ => {}
            }
        }

        if data.session.resets_at.is_none() && overall_reset.is_some() {
            data.session.resets_at = unix_to_system_time(overall_reset);
        }
    }

    data
}

fn get_header_f64(response: &ureq::Response, name: &str) -> f64 {
    response
        .header(name)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn get_header_i64(response: &ureq::Response, name: &str) -> Option<i64> {
    response
        .header(name)
        .and_then(|s| s.parse::<i64>().ok())
}

fn get_header_str(response: &ureq::Response, name: &str) -> Option<String> {
    response.header(name).map(String::from)
}

fn unix_to_system_time(unix_secs: Option<i64>) -> Option<SystemTime> {
    let secs = unix_secs?;
    if secs < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(secs as u64))
}

/// Format a usage section as "X% · Yh" style text
pub fn format_line(section: &UsageSection) -> String {
    let pct = format!("{:.0}%", section.percentage);
    let cd = format_countdown(section.resets_at);
    if cd.is_empty() {
        pct
    } else {
        format!("{pct} \u{00b7} {cd}")
    }
}

fn format_countdown(resets_at: Option<SystemTime>) -> String {
    let reset = match resets_at {
        Some(t) => t,
        None => return String::new(),
    };

    let remaining = match reset.duration_since(SystemTime::now()) {
        Ok(d) => d,
        Err(_) => return "now".to_string(),
    };

    let total_secs = remaining.as_secs();
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3600;
    let total_days = total_secs / 86400;

    if total_days >= 1 {
        format!("{total_days}d")
    } else if total_mins > 61 {
        format!("{total_hours}h")
    } else if total_secs > 60 {
        format!("{total_mins}m")
    } else {
        format!("{total_secs}")
    }
}

/// Calculate how long until the display text would change
pub fn time_until_display_change(resets_at: Option<SystemTime>) -> Option<Duration> {
    let reset = resets_at?;
    let remaining = reset.duration_since(SystemTime::now()).ok()?;

    let total_secs = remaining.as_secs();
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3600;
    let total_days = total_secs / 86400;

    if total_secs <= 60 {
        // Update every second during final countdown
        return Some(Duration::from_secs(1));
    }

    let next_boundary = if total_days >= 1 {
        Duration::from_secs(total_days * 86400)
    } else if total_mins > 61 {
        if total_hours > 1 {
            Duration::from_secs(total_hours * 3600)
        } else {
            Duration::from_secs(61 * 60)
        }
    } else {
        Duration::from_secs(total_mins * 60)
    };

    let delay = remaining.saturating_sub(next_boundary);
    if delay > Duration::ZERO {
        Some(delay + Duration::from_secs(1))
    } else {
        Some(Duration::from_secs(1))
    }
}

/// Returns true if either section has reached "now" (reset time has passed).
pub fn is_past_reset(data: &UsageData) -> bool {
    let now = SystemTime::now();
    let past = |s: &UsageSection| matches!(s.resets_at, Some(t) if now.duration_since(t).is_ok());
    past(&data.session) || past(&data.weekly)
}
