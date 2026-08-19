use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::claude_desktop;
use super::{
    build_agent, get_header_f64, get_header_i64, parse_iso8601, unix_to_system_time, PollError,
};
use crate::diagnose;
use crate::models::UsageData;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const MODEL_FALLBACK_CHAIN: &[&str] = &["claude-3-haiku-20240307", "claude-haiku-4-5-20251001"];
const CREATE_NO_WINDOW: u32 = 0x08000000;

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

struct Credentials {
    access_token: String,
    expires_at: Option<i64>,
    source: CredentialSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CredentialSource {
    Windows(PathBuf),
    /// The Claude desktop app's own token cache, used when Claude Code has
    /// only ever run inside the desktop app and no CLI login wrote
    /// `~/.claude/.credentials.json`.
    DesktopApp(PathBuf),
    Wsl {
        distro: String,
    },
}

pub(super) fn poll_claude_code() -> Result<UsageData, PollError> {
    let creds = match read_first_credentials() {
        Some(c) => c,
        None => {
            diagnose::log("poll failed: no Claude credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    let creds = refresh_or_fallback(creds)?;

    fetch_usage_with_fallback(&creds.access_token)
}

pub(super) fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    // Try the dedicated usage endpoint first
    if let Some(data) = try_usage_endpoint(token)? {
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

    // Fall back to Messages API with rate limit headers
    let result = fetch_usage_via_messages(token);
    if result.is_err() {
        diagnose::log("usage endpoint and Messages API fallback both failed");
    }
    result
}

pub(super) fn try_usage_endpoint(token: &str) -> Result<Option<UsageData>, PollError> {
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

pub(super) fn fetch_usage_via_messages(token: &str) -> Result<UsageData, PollError> {
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

pub(super) fn parse_rate_limit_headers(response: &ureq::Response) -> UsageData {
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

pub(super) fn credential_watch_snapshot(all_sources: bool) -> Vec<String> {
    let sources = if all_sources {
        all_known_credential_sources()
    } else {
        read_first_credentials()
            .map(|credentials| vec![credentials.source])
            .unwrap_or_else(all_known_credential_sources)
    };

    let mut snapshot: Vec<String> = sources
        .into_iter()
        .filter_map(|source| credential_watch_signature(&source))
        .collect();
    snapshot.sort();
    snapshot.dedup();
    snapshot
}

fn refresh_or_fallback(mut credentials: Credentials) -> Result<Credentials, PollError> {
    loop {
        if !is_token_expired(credentials.expires_at) {
            return Ok(credentials);
        }

        let source = credentials.source.clone();
        cli_refresh_token(&source);

        match read_credentials_from_source(&source) {
            Some(refreshed) if !is_token_expired(refreshed.expires_at) => return Ok(refreshed),
            Some(_) => diagnose::log(format!(
                "credentials from {source:?} still expired after refresh attempt"
            )),
            None => diagnose::log(format!(
                "credentials from {source:?} unavailable after refresh attempt"
            )),
        }

        match read_next_credentials_after(&source) {
            Some(next) => credentials = next,
            None => return Err(PollError::TokenExpired),
        }
    }
}

fn cli_refresh_token(source: &CredentialSource) {
    match source {
        CredentialSource::Windows(_) => cli_refresh_windows_token(),
        // The desktop app owns this token and refreshes it itself, so there is
        // nothing to drive from here; re-reading the cache is the whole retry.
        CredentialSource::DesktopApp(_) => {
            diagnose::log("Claude desktop app refreshes its own token; re-reading the cache")
        }
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
    let mut command = if is_cmd {
        let mut command = Command::new("cmd.exe");
        command.arg("/c").arg(&claude_path).args(args);
        command
    } else {
        let mut command = Command::new(&claude_path);
        command.args(args);
        command
    };
    command
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            diagnose::log_error("unable to spawn Windows Claude token refresh", error);
            return;
        }
    };
    wait_for_refresh(&mut child);
}

fn cli_refresh_wsl_token(distro: &str) {
    diagnose::log(format!(
        "attempting WSL Claude token refresh in distro {distro}"
    ));
    let mut command = Command::new("wsl.exe");
    command
        .arg("-d")
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

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            diagnose::log_error("unable to spawn WSL Claude token refresh", error);
            return;
        }
    };
    wait_for_refresh(&mut child);
}

fn resolve_windows_claude_path() -> String {
    for name in ["claude.cmd", "claude"] {
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

    for name in ["claude.cmd", "claude"] {
        if let Ok(output) = Command::new("where.exe")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(path) = stdout
                    .lines()
                    .next()
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                {
                    return path.to_string();
                }
            }
        }
    }

    if let Some(bundled) = bundled_desktop_claude_path() {
        return bundled.to_string_lossy().into_owned();
    }

    "claude.cmd".to_string()
}

/// The desktop app ships its own Claude Code build under
/// `%APPDATA%\Claude\claude-code\<version>\claude.exe`, which is the only
/// Claude binary present when the standalone CLI was never installed.
fn bundled_desktop_claude_path() -> Option<PathBuf> {
    let versions = dirs::config_dir()?.join("Claude").join("claude-code");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(versions)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join("claude.exe"))
        .filter(|path| path.is_file())
        .collect();
    // Directory order is not version order; the newest install wins.
    candidates.sort_by(|left, right| {
        bundled_claude_version(left)
            .cmp(&bundled_claude_version(right))
            .then_with(|| left.cmp(right))
    });
    candidates.pop()
}

fn bundled_claude_version(path: &Path) -> Option<Vec<u64>> {
    path.parent()?
        .file_name()?
        .to_str()?
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()
}

fn read_first_credentials() -> Option<Credentials> {
    credential_sources_in_order().find_map(|source| read_credentials_from_source(&source))
}

fn read_windows_credentials(path: &Path) -> Option<Credentials> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            if diagnose::is_enabled() {
                diagnose::log_error(
                    &format!("unable to read Windows credentials at {}", path.display()),
                    error,
                );
            }
            return None;
        }
    };
    parse_credentials(&content, CredentialSource::Windows(path.to_path_buf()))
}

fn read_desktop_app_credentials(path: &Path) -> Option<Credentials> {
    let token = claude_desktop::read_token(path)?;
    diagnose::log("using the Claude desktop app token cache");
    Some(Credentials {
        access_token: token.access_token,
        expires_at: token.expires_at,
        source: CredentialSource::DesktopApp(path.to_path_buf()),
    })
}

fn read_credentials_from_source(source: &CredentialSource) -> Option<Credentials> {
    match source {
        CredentialSource::Windows(path) => read_windows_credentials(path),
        CredentialSource::DesktopApp(path) => read_desktop_app_credentials(path),
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
    Some(Credentials {
        access_token: oauth.get("accessToken")?.as_str()?.to_string(),
        expires_at: oauth.get("expiresAt").and_then(|value| value.as_i64()),
        source,
    })
}

fn read_next_credentials_after(source: &CredentialSource) -> Option<Credentials> {
    credential_sources_in_order()
        .skip_while(|candidate| candidate != source)
        .skip(1)
        .find_map(|candidate| read_credentials_from_source(&candidate))
}

/// Credential sources, cheapest first. The WSL probe stays lazy so a machine
/// that resolves a token locally never has to spawn `wsl.exe`.
fn credential_sources_in_order() -> impl Iterator<Item = CredentialSource> {
    windows_credential_source()
        .into_iter()
        .chain(desktop_app_credential_source())
        .chain(
            std::iter::once_with(list_wsl_distros)
                .flatten()
                .map(|distro| CredentialSource::Wsl { distro }),
        )
}

fn all_known_credential_sources() -> Vec<CredentialSource> {
    credential_sources_in_order().collect()
}

fn windows_credential_source() -> Option<CredentialSource> {
    Some(CredentialSource::Windows(
        dirs::home_dir()?.join(".claude").join(".credentials.json"),
    ))
}

fn desktop_app_credential_source() -> Option<CredentialSource> {
    claude_desktop::config_path().map(CredentialSource::DesktopApp)
}

fn credential_watch_signature(source: &CredentialSource) -> Option<String> {
    match source {
        CredentialSource::Windows(path) => Some(windows_credential_watch_signature(path)),
        CredentialSource::DesktopApp(path) => Some(claude_desktop::watch_signature(path)),
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
                "if [ -f ~/.claude/.credentials.json ]; then stat -c 'present|%s|%Y' ~/.claude/.credentials.json; else echo missing; fi",
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
    decode_wsl_text(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn decode_wsl_text(bytes: &[u8]) -> String {
    decode_utf16le(bytes).unwrap_or_else(|| String::from_utf8_lossy(bytes).into_owned())
}

fn decode_utf16le(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let body = if bytes.starts_with(&[0xFF, 0xFE]) {
        &bytes[2..]
    } else if looks_like_utf16le(bytes) {
        bytes
    } else {
        return None;
    };
    Some(String::from_utf16_lossy(
        &body
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>(),
    ))
}

fn looks_like_utf16le(bytes: &[u8]) -> bool {
    let sample_len = bytes.len().min(128);
    let units = sample_len / 2;
    units > 0
        && bytes[..sample_len]
            .chunks_exact(2)
            .filter(|chunk| chunk[1] == 0)
            .count()
            * 2
            >= units
}

fn is_token_expired(expires_at: Option<i64>) -> bool {
    expires_at.is_some_and(|expires_at| {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        now >= expires_at
    })
}

fn run_with_timeout(command: &mut Command, timeout: Duration) -> Option<std::process::Output> {
    let mut child = command.spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return None,
        }
    }
}

fn wait_for_refresh(child: &mut std::process::Child) {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > Duration::from_secs(30) => {
                let _ = child.kill();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(500)),
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::bundled_claude_version;
    use std::path::Path;

    #[test]
    fn bundled_claude_versions_sort_numerically() {
        let older = bundled_claude_version(Path::new("Claude/claude-code/2.1.9/claude.exe"));
        let newer = bundled_claude_version(Path::new("Claude/claude-code/2.1.10/claude.exe"));

        assert!(newer > older);
    }

    #[test]
    fn bundled_claude_versions_reject_non_numeric_directories() {
        let version = bundled_claude_version(Path::new("Claude/claude-code/current/claude.exe"));

        assert_eq!(version, None);
    }
}
