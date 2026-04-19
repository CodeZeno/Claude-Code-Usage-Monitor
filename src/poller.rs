use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use std::os::windows::process::CommandExt;

use crate::diagnose;
use crate::localization::Strings;
use crate::models::{UsageData, UsageSection};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const CREATE_NO_WINDOW: u32 = 0x08000000;

const MODEL_FALLBACK_CHAIN: &[&str] = &["claude-3-haiku-20240307", "claude-haiku-4-5-20251001"];

#[derive(Debug)]
pub enum PollError {
    AuthRequired,
    NoCredentials,
    TokenExpired,
    RequestFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialWatchMode {
    ActiveSource,
    AllSources,
}

pub type CredentialWatchSnapshot = Vec<String>;

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageBucket>,
    seven_day: Option<UsageBucket>,
}

#[derive(Deserialize)]
struct UsageBucket {
    utilization: f64,
    resets_at: Option<String>,
}

pub fn poll() -> Result<UsageData, PollError> {
    let mut creds = match read_credentials() {
        Some(c) => c,
        None => {
            diagnose::log("poll failed: no Claude credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    if is_token_expired(creds.expires_at) {
        cli_refresh_token(&creds.source);

        match read_credentials_from_source(&creds.source) {
            Some(refreshed) => creds = refreshed,
            None => {
                diagnose::log("poll failed: credentials still unavailable after refresh attempt");
                return Err(PollError::NoCredentials);
            }
        }

        if is_token_expired(creds.expires_at) {
            diagnose::log("poll failed: token is still expired after refresh attempt");
            return Err(PollError::TokenExpired);
        }
    }

    fetch_usage_with_fallback(&creds.access_token)
}

/// Invoke the Claude CLI with a minimal prompt to force its internal
/// OAuth token refresh.
fn cli_refresh_token(source: &CredentialSource) {
    match source {
        CredentialSource::Windows(_) => cli_refresh_windows_token(),
        CredentialSource::Wsl { distro } => cli_refresh_wsl_token(distro),
    }
}

fn cli_refresh_windows_token() {
    let claude_path = resolve_windows_claude_path();
    let is_cmd = claude_path.to_lowercase().ends_with(".cmd");
    diagnose::log(format!(
        "attempting Windows Claude token refresh via {claude_path}"
    ));

    let args: &[&str] = &["-p", "."];

    let mut cmd = if is_cmd {
        let mut c = Command::new("cmd.exe");
        c.arg("/c").arg(&claude_path).args(args);
        c
    } else {
        let mut c = Command::new(&claude_path);
        c.args(args);
        c
    };
    cmd.env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("unable to spawn Windows Claude token refresh", error);
            return;
        }
    };

    // Wait up to 30 seconds — don't block the poll thread forever
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(30) {
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(_) => break,
        }
    }
}

fn cli_refresh_wsl_token(distro: &str) {
    diagnose::log(format!(
        "attempting WSL Claude token refresh in distro {distro}"
    ));
    let mut cmd = Command::new("wsl.exe");
    cmd.arg("-d")
        .arg(distro)
        .arg("--")
        .arg("bash")
        .arg("-lic")
        .arg("if command -v claude >/dev/null 2>&1; then claude -p .; elif [ -x \"$HOME/.local/bin/claude\" ]; then \"$HOME/.local/bin/claude\" -p .; else exit 127; fi")
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("unable to spawn WSL Claude token refresh", error);
            return;
        }
    };

    wait_for_refresh(&mut child);
}

/// Spawn a command and wait up to `timeout` for it to finish.
/// Returns None if the process fails to start or exceeds the deadline.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<std::process::Output> {
    let mut child = cmd.spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
}

fn wait_for_refresh(child: &mut std::process::Child) {
    // Wait up to 30 seconds; don't block the poll thread forever.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(30) {
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(_) => break,
        }
    }
}

/// Resolve the full path to the `claude` CLI executable.
fn resolve_windows_claude_path() -> String {
    for name in &["claude.cmd", "claude"] {
        if Command::new(name)
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return name.to_string();
        }
    }

    for name in &["claude.cmd", "claude"] {
        if let Ok(output) = Command::new("where.exe")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(first_line) = stdout.lines().next() {
                    let path = first_line.trim().to_string();
                    if !path.is_empty() {
                        return path;
                    }
                }
            }
        }
    }

    "claude.cmd".to_string()
}

fn build_agent() -> Result<ureq::Agent, PollError> {
    let tls = native_tls::TlsConnector::new().map_err(|_| PollError::RequestFailed)?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .tls_connector(std::sync::Arc::new(tls))
        .build())
}

pub fn credential_watch_snapshot(mode: CredentialWatchMode) -> CredentialWatchSnapshot {
    let sources = match mode {
        CredentialWatchMode::ActiveSource => read_credentials()
            .map(|creds| vec![creds.source])
            .unwrap_or_else(all_known_credential_sources),
        CredentialWatchMode::AllSources => all_known_credential_sources(),
    };

    let mut snapshot: CredentialWatchSnapshot = sources
        .into_iter()
        .filter_map(|source| credential_watch_signature(&source))
        .collect();
    snapshot.sort();
    snapshot.dedup();
    snapshot
}

fn all_known_credential_sources() -> Vec<CredentialSource> {
    let mut sources = Vec::new();
    if let Some(source) = windows_credential_source() {
        sources.push(source);
    }
    for distro in list_wsl_distros() {
        sources.push(CredentialSource::Wsl { distro });
    }
    sources
}

fn windows_credential_source() -> Option<CredentialSource> {
    let home = dirs::home_dir()?;
    Some(CredentialSource::Windows(
        home.join(".claude").join(".credentials.json"),
    ))
}

fn credential_watch_signature(source: &CredentialSource) -> Option<String> {
    match source {
        CredentialSource::Windows(path) => Some(windows_credential_watch_signature(path)),
        CredentialSource::Wsl { distro } => wsl_credential_watch_signature(distro),
    }
}

fn windows_credential_watch_signature(path: &PathBuf) -> String {
    let key = format!("win:{}", path.display());
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs())
                .unwrap_or(0);
            format!("{key}|present|{}|{modified}", metadata.len())
        }
        Err(_) => format!("{key}|missing"),
    }
}

fn wsl_credential_watch_signature(distro: &str) -> Option<String> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg(
                "if [ -f ~/.claude/.credentials.json ]; then \
                 stat -c 'present|%s|%Y' ~/.claude/.credentials.json; \
                 else echo missing; fi",
            )
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    )?;

    let state = if output.status.success() {
        decode_wsl_text(&output.stdout).trim().to_string()
    } else {
        format!("status-{}", output.status)
    };

    Some(format!("wsl:{distro}|{state}"))
}

fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    // Try the dedicated usage endpoint first
    match try_usage_endpoint(token)? {
        Some(data) => {
        // If reset timers are missing, fill them in from the Messages API
            if data.session.resets_at.is_none() || data.weekly.resets_at.is_none() {
                if let Ok(fallback) = fetch_usage_via_messages(token) {
                    let mut merged = data;
                    if merged.session.resets_at.is_none() {
                        merged.session.resets_at = fallback.session.resets_at;
                    }
                    if merged.weekly.resets_at.is_none() {
                        merged.weekly.resets_at = fallback.weekly.resets_at;
                    }
                    return Ok(merged);
                }
            }
            return Ok(data);
        }
        None => {}
    }

    // Fall back to Messages API with rate limit headers
    let result = fetch_usage_via_messages(token);
    if result.is_err() {
        diagnose::log("usage endpoint and Messages API fallback both failed");
    }
    result
}

fn try_usage_endpoint(token: &str) -> Result<Option<UsageData>, PollError> {
    let agent = build_agent()?;

    let resp = match agent
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call()
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "usage endpoint returned auth error status {code}; re-login required"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(_) => return Ok(None),
    };

    let response: UsageResponse = match resp.into_json() {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    let mut data = UsageData::default();

    if let Some(bucket) = &response.five_hour {
        data.session.percentage = bucket.utilization;
        data.session.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    if let Some(bucket) = &response.seven_day {
        data.weekly.percentage = bucket.utilization;
        data.weekly.resets_at = parse_iso8601(bucket.resets_at.as_deref());
    }

    Ok(Some(data))
}

fn fetch_usage_via_messages(token: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;

    for model in MODEL_FALLBACK_CHAIN {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "."}]
        });

        let response = match agent
            .post(MESSAGES_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("anthropic-version", "2023-06-01")
            .set("anthropic-beta", "oauth-2025-04-20")
            .send_json(&body)
        {
            Ok(resp) => resp,
            Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
                diagnose::log(format!(
                    "messages endpoint returned auth error status {code}; re-login required"
                ));
                return Err(PollError::AuthRequired);
            }
            Err(ureq::Error::Status(_code, resp)) => resp,
            Err(_) => continue,
        };

        let h5 = response.header("anthropic-ratelimit-unified-5h-utilization");
        let h7 = response.header("anthropic-ratelimit-unified-7d-utilization");
        let hs = response.header("anthropic-ratelimit-unified-status");

        if h5.is_some() || h7.is_some() || hs.is_some() {
            return Ok(parse_rate_limit_headers(&response));
        }
    }

    Err(PollError::RequestFailed)
}

fn parse_rate_limit_headers(response: &ureq::Response) -> UsageData {
    let mut data = UsageData::default();

    data.session.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-5h-utilization") * 100.0;
    data.session.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-5h-reset",
    ));

    data.weekly.percentage =
        get_header_f64(response, "anthropic-ratelimit-unified-7d-utilization") * 100.0;
    data.weekly.resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-7d-reset",
    ));

    let overall_reset = get_header_i64(response, "anthropic-ratelimit-unified-reset");

    if data.session.percentage == 0.0 && data.weekly.percentage == 0.0 {
        let status = response.header("anthropic-ratelimit-unified-status");
        if status == Some("rejected") {
            let claim = response.header("anthropic-ratelimit-unified-representative-claim");
            match claim {
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
    response.header(name).and_then(|s| s.parse::<i64>().ok())
}

fn unix_to_system_time(unix_secs: Option<i64>) -> Option<SystemTime> {
    let secs = unix_secs?;
    if secs < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(secs as u64))
}

struct Credentials {
    access_token: String,
    expires_at: Option<i64>,
    source: CredentialSource,
}

#[derive(Clone, Debug)]
enum CredentialSource {
    Windows(PathBuf),
    Wsl { distro: String },
}

fn read_credentials() -> Option<Credentials> {
    let mut candidates = Vec::new();

    if let Some(creds) = read_windows_credentials() {
        candidates.push(creds);
    }

    for distro in list_wsl_distros() {
        if let Some(creds) = read_wsl_credentials(&distro) {
            candidates.push(creds);
        }
    }

    choose_best_credentials(candidates)
}

fn read_windows_credentials() -> Option<Credentials> {
    let CredentialSource::Windows(cred_path) = windows_credential_source()? else {
        return None;
    };
    let content = match std::fs::read_to_string(&cred_path) {
        Ok(content) => content,
        Err(error) => {
            if diagnose::is_enabled() {
                diagnose::log_error(
                    &format!("unable to read Windows credentials at {}", cred_path.display()),
                    error,
                );
            }
            return None;
        }
    };
    parse_credentials(&content, CredentialSource::Windows(cred_path))
}

fn read_credentials_from_source(source: &CredentialSource) -> Option<Credentials> {
    match source {
        CredentialSource::Windows(path) => {
            let content = std::fs::read_to_string(path).ok()?;
            parse_credentials(&content, source.clone())
        }
        CredentialSource::Wsl { distro } => read_wsl_credentials(distro),
    }
}

fn read_wsl_credentials(distro: &str) -> Option<Credentials> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg("cat ~/.claude/.credentials.json")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    )?;

    if !output.status.success() {
        diagnose::log(format!(
            "WSL credentials probe failed for distro {distro} with status {}",
            output.status
        ));
        return None;
    }

    let content = String::from_utf8(output.stdout).ok()?;
    parse_credentials(
        &content,
        CredentialSource::Wsl {
            distro: distro.to_string(),
        },
    )
}

fn parse_credentials(content: &str, source: CredentialSource) -> Option<Credentials> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;

    let oauth = json.get("claudeAiOauth")?;
    let access_token = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())?
        .to_string();
    let expires_at = oauth.get("expiresAt").and_then(|v| v.as_i64());

    Some(Credentials {
        access_token,
        expires_at,
        source,
    })
}

fn choose_best_credentials(mut candidates: Vec<Credentials>) -> Option<Credentials> {
    if candidates.is_empty() {
        return None;
    }

    candidates.sort_by_key(|creds| is_token_expired(creds.expires_at));
    candidates.into_iter().next()
}

fn list_wsl_distros() -> Vec<String> {
    let output = match run_with_timeout(
        Command::new("wsl.exe")
            .args(["-l", "-q"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    ) {
        Some(output) if output.status.success() => output,
        _ => {
            diagnose::log("unable to enumerate WSL distros");
            return Vec::new();
        }
    };

    let stdout = decode_wsl_text(&output.stdout);
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn decode_wsl_text(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    if let Some(decoded) = decode_utf16le(bytes) {
        return decoded;
    }

    String::from_utf8_lossy(bytes).into_owned()
}

fn decode_utf16le(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 2 || bytes.len() % 2 != 0 {
        return None;
    }

    let body = if bytes.starts_with(&[0xFF, 0xFE]) {
        &bytes[2..]
    } else if looks_like_utf16le(bytes) {
        bytes
    } else {
        return None;
    };

    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    Some(String::from_utf16_lossy(&units))
}

fn looks_like_utf16le(bytes: &[u8]) -> bool {
    let sample_len = bytes.len().min(128);
    let units = sample_len / 2;
    if units == 0 {
        return false;
    }

    let nul_high_bytes = bytes[..sample_len]
        .chunks_exact(2)
        .filter(|chunk| chunk[1] == 0)
        .count();

    nul_high_bytes * 2 >= units
}

fn is_token_expired(expires_at: Option<i64>) -> bool {
    let Some(exp) = expires_at else { return false };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    now >= exp
}

/// Parse an ISO 8601 timestamp string into a SystemTime.
fn parse_iso8601(s: Option<&str>) -> Option<SystemTime> {
    let s = s?;
    // Strip timezone offset to get "YYYY-MM-DDTHH:MM:SS" or with fractional seconds
    // The API returns formats like "2026-03-05T08:00:00.321598+00:00"
    let datetime_part = s.split('+').next().unwrap_or(s);
    let datetime_part = datetime_part.split('Z').next().unwrap_or(datetime_part);

    // Try parsing with and without fractional seconds
    let formats = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"];
    for fmt in &formats {
        if let Ok(secs) = parse_datetime_to_unix(datetime_part, fmt) {
            return Some(UNIX_EPOCH + Duration::from_secs(secs));
        }
    }
    None
}

/// Minimal datetime parser — avoids pulling in chrono/time crates.
fn parse_datetime_to_unix(s: &str, _fmt: &str) -> Result<u64, ()> {
    // Extract date and time parts from "YYYY-MM-DDTHH:MM:SS[.frac]"
    let (date_str, time_str) = s.split_once('T').ok_or(())?;
    let date_parts: Vec<&str> = date_str.split('-').collect();
    if date_parts.len() != 3 {
        return Err(());
    }

    let year: u64 = date_parts[0].parse().map_err(|_| ())?;
    let month: u64 = date_parts[1].parse().map_err(|_| ())?;
    let day: u64 = date_parts[2].parse().map_err(|_| ())?;

    // Strip fractional seconds
    let time_base = time_str.split('.').next().unwrap_or(time_str);
    let time_parts: Vec<&str> = time_base.split(':').collect();
    if time_parts.len() != 3 {
        return Err(());
    }

    let hour: u64 = time_parts[0].parse().map_err(|_| ())?;
    let min: u64 = time_parts[1].parse().map_err(|_| ())?;
    let sec: u64 = time_parts[2].parse().map_err(|_| ())?;

    // Days from year (using a simplified calculation for dates after 1970)
    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }

    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += month_days[m as usize];
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }
    days += day - 1;

    Ok(days * 86400 + hour * 3600 + min * 60 + sec)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Format a usage section as "X% · Yh" style text
pub fn format_line(section: &UsageSection, strings: Strings) -> String {
    let pct = format!("{:.0}%", section.percentage);
    let cd = format_countdown(section.resets_at, strings);
    if cd.is_empty() {
        pct
    } else {
        format!("{pct} \u{00b7} {cd}")
    }
}

fn format_countdown(resets_at: Option<SystemTime>, strings: Strings) -> String {
    let reset = match resets_at {
        Some(t) => t,
        None => return String::new(),
    };

    let remaining = match reset.duration_since(SystemTime::now()) {
        Ok(d) => d,
        Err(_) => return strings.now.to_string(),
    };

    format_countdown_from_secs(remaining.as_secs(), strings)
}

/// Calculate how long until the display text would change
pub fn time_until_display_change(resets_at: Option<SystemTime>) -> Option<Duration> {
    let reset = resets_at?;
    let remaining = reset.duration_since(SystemTime::now()).ok()?;
    Some(time_until_display_change_from_secs(remaining.as_secs()))
}

fn format_countdown_from_secs(total_secs: u64, strings: Strings) -> String {
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3600;
    let total_days = total_secs / 86400;

    if total_days >= 1 {
        format!("{total_days}{}", strings.day_suffix)
    } else if total_hours >= 1 {
        format!("{total_hours}{}", strings.hour_suffix)
    } else if total_mins >= 1 {
        format!("{total_mins}{}", strings.minute_suffix)
    } else {
        format!("{total_secs}{}", strings.second_suffix)
    }
}

fn time_until_display_change_from_secs(total_secs: u64) -> Duration {
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3600;
    let total_days = total_secs / 86400;

    let current_bucket_start = if total_days >= 1 {
        total_days * 86400
    } else if total_hours >= 1 {
        total_hours * 3600
    } else if total_mins >= 1 {
        total_mins * 60
    } else {
        total_secs
    };

    Duration::from_secs(total_secs.saturating_sub(current_bucket_start) + 1)
}

/// Returns true if either section has reached "now" (reset time has passed).
pub fn is_past_reset(data: &UsageData) -> bool {
    let now = SystemTime::now();
    let past = |s: &UsageSection| matches!(s.resets_at, Some(t) if now.duration_since(t).is_ok());
    past(&data.session) || past(&data.weekly)
}
