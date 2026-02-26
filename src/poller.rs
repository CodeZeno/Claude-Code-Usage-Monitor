use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::models::{UsageData, UsageSection};

const API_URL: &str = "https://api.anthropic.com/v1/messages";


const MODEL_FALLBACK_CHAIN: &[&str] = &[
    "claude-3-haiku-20240307",
    "claude-haiku-4-5-20251001",
];

#[derive(Debug)]
pub enum PollError {
    NoCredentials,
    AllModelsFailed(String),
}

impl std::fmt::Display for PollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PollError::NoCredentials => write!(f, "No Claude credentials found"),
            PollError::AllModelsFailed(msg) => write!(f, "All models failed:\n{msg}"),
        }
    }
}

pub fn poll() -> Result<UsageData, PollError> {
    let token = read_access_token().ok_or(PollError::NoCredentials)?;
    fetch_usage_with_fallback(&token)
}

fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    let mut errors = Vec::new();

    for model in MODEL_FALLBACK_CHAIN {
        match try_model(token, model) {
            Ok(data) => return Ok(data),
            Err(msg) => errors.push(format!("{model}: {msg}")),
        }
    }

    let combined = errors.join("\n");
    Err(PollError::AllModelsFailed(combined))
}

fn read_access_token() -> Option<String> {
    let home = dirs::home_dir()?;
    let cred_path: PathBuf = home.join(".claude").join(".credentials.json");

    let content = std::fs::read_to_string(&cred_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;

    json.get("claudeAiOauth")?
        .get("accessToken")?
        .as_str()
        .map(String::from)
}

fn try_model(token: &str, model: &str) -> Result<UsageData, String> {
    let tls = std::sync::Arc::new(
        native_tls::TlsConnector::new().map_err(|e| e.to_string())?
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

    let (status, response) = match agent
        .post(API_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-version", "2023-06-01")
        .set("anthropic-beta", "oauth-2025-04-20")
        .send_json(&body)
    {
        Ok(resp) => (resp.status(), resp),
        Err(ureq::Error::Status(code, resp)) => (code, resp),
        Err(e) => return Err(e.to_string()),
    };

    let has_rate_limit_headers = response.header("anthropic-ratelimit-unified-5h-utilization").is_some()
        || response.header("anthropic-ratelimit-unified-7d-utilization").is_some()
        || response.header("anthropic-ratelimit-unified-status").is_some();

    if has_rate_limit_headers {
        let data = parse_headers(&response);
        return Ok(data);
    }

    let body_text = response.into_string().unwrap_or_default();
    Err(extract_error_message(&body_text, status))
}

fn extract_error_message(body: &str, status: u16) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(msg) = json.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str())
        {
            return format!("HTTP {status}: {msg}");
        }
    }
    let truncated = if body.len() > 200 { &body[..200] } else { body };
    format!("HTTP {status}: {truncated}")
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
    } else {
        format!("{total_mins}m")
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
