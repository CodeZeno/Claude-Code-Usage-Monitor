use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::{build_agent, parse_iso8601, PollError};
use crate::diagnose;
use crate::models::{limit_slug, CreditsSection, UsageData, UsageLimit, UsageSection};

/// Grok Build reads its own `/usage` panel from the CLI chat proxy. The
/// monitor asks the same endpoint with the session the CLI already stored.
const GROK_BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
/// Static marker the proxy uses to route CLI user-token requests.
const GROK_TOKEN_AUTH: &str = "xai-grok-cli";
const GROK_CLIENT_MODE: &str = "cli";
/// Used only when the installed CLI cannot be asked for its own version.
const DEFAULT_GROK_CLIENT_VERSION: &str = "1.0.0";
const GROK_HOME_ENV: &str = "GROK_HOME";
const GROK_CLIENT_VERSION_ENV: &str = "GROK_CLIENT_VERSION";
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// One entry of `auth.json`, keyed by `"{issuer}::{client_id}"` or the
/// legacy `"https://accounts.x.ai/sign-in"` scope.
#[derive(Clone, Debug, Deserialize)]
struct GrokAuthEntry {
    #[serde(default)]
    key: String,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
}

/// Cents as a number or decimal string, optionally wrapped in a `val` field.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(untagged)]
enum GrokCents {
    Structured {
        #[serde(default, deserialize_with = "deserialize_cents_value")]
        val: f64,
    },
    Plain(#[serde(deserialize_with = "deserialize_cents_value")] f64),
}

fn deserialize_cents_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<f64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value {
        Number(f64),
        Text(String),
    }
    let value = match Value::deserialize(deserializer)? {
        Value::Number(value) => value,
        Value::Text(value) => value.parse::<f64>().map_err(serde::de::Error::custom)?,
    };
    if !value.is_finite() {
        return Err(serde::de::Error::custom("cents must be finite"));
    }
    Ok(value)
}

impl GrokCents {
    fn units(self) -> f64 {
        match self {
            Self::Structured { val } => val / 100.0,
            Self::Plain(value) => value / 100.0,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GrokBillingResponse {
    #[serde(default)]
    config: Option<GrokBillingConfig>,
    #[serde(default, rename = "onDemandEnabled")]
    on_demand_enabled: Option<bool>,
    #[serde(default, rename = "subscriptionTier")]
    subscription_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GrokBillingConfig {
    #[serde(default, rename = "creditUsagePercent")]
    credit_usage_percent: Option<f64>,
    #[serde(default, rename = "currentPeriod")]
    current_period: Option<GrokBillingPeriod>,
    #[serde(default, rename = "prepaidBalance")]
    prepaid_balance: Option<GrokCents>,
    #[serde(default, rename = "onDemandCap")]
    on_demand_cap: Option<GrokCents>,
    #[serde(default, rename = "onDemandUsed")]
    on_demand_used: Option<GrokCents>,
    /// Per-product split of the one pool, which Chat, Imagine, Voice, Build
    /// and the API all draw from.
    #[serde(default, rename = "productUsage")]
    product_usage: Vec<GrokProductUsage>,
}

#[derive(Debug, Deserialize)]
struct GrokProductUsage {
    #[serde(default)]
    product: Option<String>,
    #[serde(default, rename = "usagePercent")]
    usage_percent: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct GrokBillingPeriod {
    /// `USAGE_PERIOD_TYPE_WEEKLY` on the wire. Older responses spell the
    /// field `periodType`.
    #[serde(default, rename = "type", alias = "periodType")]
    period_type: Option<String>,
    #[serde(default)]
    end: Option<serde_json::Value>,
}

pub(super) fn poll_grok() -> Result<UsageData, PollError> {
    let path = grok_auth_path().ok_or(PollError::NoCredentials)?;
    let session = match read_grok_session(&path) {
        Some(session) => session,
        None => {
            diagnose::log(
                "Grok usage poll failed: no Grok session found (run 'grok login' to sign in)",
            );
            return Err(PollError::NoCredentials);
        }
    };

    match fetch_grok_usage(&session) {
        Err(PollError::AuthRequired) => {
            // The CLI owns this file and refreshes it silently whenever it
            // needs a token, so let it do the work rather than racing its
            // lock with a refresh of our own. Without a refresh token it
            // would fall back to an interactive browser sign-in, which a
            // background monitor must never trigger.
            if !session.can_refresh {
                return Err(PollError::TokenExpired);
            }
            cli_refresh_grok_token();
            let refreshed = read_grok_session(&path).ok_or(PollError::TokenExpired)?;
            if refreshed.access_token == session.access_token {
                return Err(PollError::TokenExpired);
            }
            fetch_grok_usage(&refreshed)
        }
        other => other,
    }
}

pub(super) fn credential_watch_snapshot(_all_sources: bool) -> Vec<String> {
    let Some(path) = grok_auth_path() else {
        return vec!["grok:auth-path-missing".into()];
    };
    vec![path_signature("grok", &path)]
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GrokSession {
    access_token: String,
    user_id: String,
    /// The CLI can renew this session silently, without a browser.
    can_refresh: bool,
}

fn grok_auth_path() -> Option<PathBuf> {
    if let Some(home) = non_empty_environment(GROK_HOME_ENV) {
        return crate::accounts::expand_path(Path::new(&home)).map(|home| home.join("auth.json"));
    }
    Some(
        crate::known_folders::home_dir()?
            .join(".grok")
            .join("auth.json"),
    )
}

fn read_grok_session(path: &Path) -> Option<GrokSession> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            diagnose::log_error(
                &format!("unable to read Grok credentials at {}", path.display()),
                error,
            );
            return None;
        }
    };
    select_grok_session(&content)
}

/// `auth.json` can also hold corporate OIDC tokens and plain API keys. Only
/// xAI sign-in scopes belong at the fixed production billing endpoint.
/// Prefer the latest-expiring usable xAI session.
fn select_grok_session(content: &str) -> Option<GrokSession> {
    let entries: std::collections::BTreeMap<String, GrokAuthEntry> =
        serde_json::from_str(content).ok()?;
    entries
        .into_iter()
        .filter(|(scope, _)| {
            scope == "https://accounts.x.ai/sign-in"
                || scope
                    .strip_prefix("https://auth.x.ai::")
                    .is_some_and(|client| !client.is_empty())
        })
        .map(|(_, entry)| entry)
        .filter(|entry| !entry.key.trim().is_empty())
        .max_by_key(|entry| {
            entry
                .expires_at
                .as_deref()
                .and_then(|value| parse_iso8601(Some(value)))
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs())
                .unwrap_or(0)
        })
        .map(|entry| GrokSession {
            access_token: entry.key.trim().to_string(),
            user_id: entry.user_id.unwrap_or_default().trim().to_string(),
            can_refresh: entry
                .refresh_token
                .is_some_and(|token| !token.trim().is_empty()),
        })
}

fn fetch_grok_usage(session: &GrokSession) -> Result<UsageData, PollError> {
    let mut request = build_agent()?
        .get(GROK_BILLING_URL)
        .header("Authorization", &format!("Bearer {}", session.access_token))
        .header("X-XAI-Token-Auth", GROK_TOKEN_AUTH)
        .header("x-grok-client-mode", GROK_CLIENT_MODE)
        .header("x-grok-client-version", &grok_client_version());
    if !session.user_id.is_empty() {
        request = request.header("x-userid", &session.user_id);
    }

    let mut response = match request.call().and_then(super::check_http_status) {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(code @ (401 | 403))) => {
            diagnose::log(format!(
                "Grok billing endpoint returned auth error status {code}; refresh required"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(ureq::Error::StatusCode(code)) => {
            diagnose::log(format!("Grok billing endpoint returned status {code}"));
            return Err(PollError::HttpStatus(code));
        }
        Err(error) => {
            diagnose::log_error("Grok billing endpoint request failed", error);
            return Err(PollError::NetworkError);
        }
    };

    let response: GrokBillingResponse = response.body_mut().read_json().map_err(|error| {
        diagnose::log_error("unable to parse Grok billing response", error);
        PollError::UnexpectedResponse
    })?;
    if let Some(tier) = response.subscription_tier.as_deref() {
        diagnose::log(format!("Grok subscription tier: {tier}"));
    }
    grok_usage_from_billing(response).ok_or_else(|| {
        diagnose::log("Grok billing response missing credit usage");
        PollError::UnexpectedResponse
    })
}

fn grok_usage_from_billing(response: GrokBillingResponse) -> Option<UsageData> {
    let config = response.config?;
    let percentage = config.credit_usage_percent?;
    let period = config.current_period.as_ref();
    let resets_at = period.and_then(|period| period_end(period.end.as_ref()));
    let monthly_period = period
        .and_then(|period| period.period_type.as_deref())
        .is_some_and(|period_type| period_type.to_ascii_lowercase().contains("monthly"));

    // Grok spends one pool per billing period and has no five-hour window, so
    // the pool lands in `weekly`, which is what tray badges read.
    let window = UsageSection {
        available: true,
        percentage: percentage.clamp(0.0, 100.0),
        resets_at,
    };
    Some(UsageData {
        limits: grok_limits(&config.product_usage, resets_at),
        session: UsageSection::default(),
        weekly: window.clone(),
        weekly_label: monthly_period.then(|| "30d".to_string()),
        monthly: monthly_period.then_some(window),
        credits: grok_credits(&config, response.on_demand_enabled),
        stale: false,
    })
}

/// The pool is shared, so the per-product split is the interesting part. It
/// stays out of the headline and reaches themes through `grok.limits.<key>`.
fn grok_limits(products: &[GrokProductUsage], resets_at: Option<SystemTime>) -> Vec<UsageLimit> {
    products
        .iter()
        .filter_map(|entry| {
            let product = entry
                .product
                .as_deref()
                .map(str::trim)
                .filter(|product| !product.is_empty())?;
            let percentage = entry.usage_percent?;
            Some(UsageLimit {
                key: limit_slug(product),
                kind: "product".into(),
                label: product.to_string(),
                model: None,
                model_id: None,
                scope: None,
                is_active: percentage > 0.0,
                usage: UsageSection {
                    available: true,
                    percentage: percentage.clamp(0.0, 100.0),
                    resets_at,
                },
            })
        })
        .collect()
}

/// On-demand spending only, and only once it is actually in play: themes read
/// credits as the headline figure, which would otherwise hide the pool.
fn grok_credits(
    config: &GrokBillingConfig,
    on_demand_enabled: Option<bool>,
) -> Option<CreditsSection> {
    if on_demand_enabled == Some(false) {
        return None;
    }
    let total = config.on_demand_cap?.units();
    let used = config.on_demand_used.map(GrokCents::units).unwrap_or(0.0);
    if !total.is_finite() || total <= 0.0 || used <= 0.0 {
        return None;
    }
    if let Some(prepaid) = config.prepaid_balance {
        diagnose::log(format!("Grok prepaid balance: {:.2}", prepaid.units()));
    }
    Some(CreditsSection {
        percentage: (used / total * 100.0).clamp(0.0, 100.0),
        remaining: (total - used).max(0.0),
        total,
    })
}

/// The proxy returns an RFC 3339 timestamp, but proto3 JSON also allows a
/// bare number of seconds.
fn period_end(value: Option<&serde_json::Value>) -> Option<SystemTime> {
    match value? {
        serde_json::Value::String(text) => parse_iso8601(Some(text)),
        serde_json::Value::Number(number) => {
            let seconds = number.as_f64()?;
            // Milliseconds once the value is far past any plausible epoch second.
            let seconds = if seconds > 100_000_000_000.0 {
                seconds / 1000.0
            } else {
                seconds
            };
            if seconds <= 0.0 {
                return None;
            }
            UNIX_EPOCH.checked_add(Duration::try_from_secs_f64(seconds).ok()?)
        }
        _ => None,
    }
}

/// Cache a detected CLI version, but retry after transient detection failures.
fn grok_client_version() -> String {
    static VERSION: OnceLock<String> = OnceLock::new();
    cached_cli_detection(&VERSION, || {
        non_empty_environment(GROK_CLIENT_VERSION_ENV).or_else(cli_grok_version)
    })
    .unwrap_or_else(|| DEFAULT_GROK_CLIENT_VERSION.to_string())
}

/// Only successful discovery is stable enough to cache for this process.
fn cached_cli_detection(
    cache: &OnceLock<String>,
    detect: impl FnOnce() -> Option<String>,
) -> Option<String> {
    cache.get().cloned().or_else(|| {
        let detected = detect()?;
        let _ = cache.set(detected.clone());
        Some(detected)
    })
}

fn cli_grok_version() -> Option<String> {
    let version = read_cli_version(&resolve_windows_grok_path(), Duration::from_secs(5))?;
    diagnose::log(format!("Grok CLI reports version {version}"));
    Some(version)
}

fn read_cli_version(path: &str, timeout: Duration) -> Option<String> {
    let start = std::time::Instant::now();
    let mut child = grok_command(path, &["version"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    // Drain while the process runs so a full pipe cannot block its exit.
    // Bound both the captured output and the wait, including inherited pipes.
    std::thread::spawn(move || {
        let mut output = String::new();
        let result = stdout.take(64 * 1024).read_to_string(&mut output);
        let _ = sender.send(result.ok().map(|_| output));
    });
    if !wait_for_command(&mut child, timeout) {
        return None;
    }
    let output = receiver
        .recv_timeout(timeout.saturating_sub(start.elapsed()))
        .ok()??;
    first_version(&output)
}

/// Pick the first `major.minor.patch` token out of arbitrary CLI output.
fn first_version(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map(|token| token.trim_matches('.'))
        .find(|token| {
            let mut parts = token.split('.');
            let numeric = |part: Option<&str>| {
                part.is_some_and(|part| {
                    !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())
                })
            };
            numeric(parts.next()) && numeric(parts.next()) && numeric(parts.next())
        })
        .map(str::to_string)
}

/// `grok models` needs a valid token but no model call, so it refreshes the
/// stored session without spending any of the allowance.
fn cli_refresh_grok_token() {
    let grok_path = resolve_windows_grok_path();
    diagnose::log(format!(
        "attempting Windows Grok token refresh via {grok_path}"
    ));

    let mut child = match grok_command(&grok_path, &["models"]).spawn() {
        Ok(child) => child,
        Err(error) => {
            diagnose::log_error("unable to spawn Windows Grok token refresh", error);
            return;
        }
    };
    wait_for_command(&mut child, Duration::from_secs(30));
}

fn grok_command(grok_path: &str, args: &[&str]) -> Command {
    let lowercase = grok_path.to_lowercase();
    let mut command = if lowercase.ends_with(".cmd") || lowercase.ends_with(".bat") {
        let mut command = Command::new("cmd.exe");
        command.arg("/d").arg("/c").arg(grok_path).args(args);
        command
    } else if lowercase.ends_with(".ps1") {
        let mut command = Command::new("powershell.exe");
        command
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(grok_path)
            .args(args);
        command
    } else {
        let mut command = Command::new(grok_path);
        command.args(args);
        command
    };
    command
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    command
}

fn resolve_windows_grok_path() -> String {
    static PATH: OnceLock<String> = OnceLock::new();
    cached_cli_detection(&PATH, || {
        // The official installer drops a real executable in ~/.grok/bin;
        // prefer it over an npm shim that would cost an extra cmd.exe hop.
        for name in ["grok.exe", "grok.cmd", "grok.ps1", "grok"] {
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
                        return Some(path.to_string());
                    }
                }
            }
        }
        None
    })
    .unwrap_or_else(|| "grok.cmd".to_string())
}

fn wait_for_command(child: &mut std::process::Child, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn non_empty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn path_signature(kind: &str, path: &Path) -> String {
    let key = format!("{kind}:{}", path.display());
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_nanos())
                .unwrap_or(0);
            format!("{key}|present|{}|{modified}", metadata.len())
        }
        Err(_) => format!("{key}|missing"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_expiring_last_wins_and_empty_tokens_are_skipped() {
        let content = r#"{
            "https://auth.x.ai::old": {
                "key": "older",
                "user_id": "u-old",
                "expires_at": "2026-01-01T00:00:00Z"
            },
            "https://auth.x.ai::new": {
                "key": "newer",
                "user_id": "u-new",
                "refresh_token": "rt-new",
                "expires_at": "2026-06-01T00:00:00Z"
            },
            "https://auth.x.ai::blank": {
                "key": "   ",
                "user_id": "u-blank",
                "expires_at": "2027-01-01T00:00:00Z"
            }
        }"#;

        let session = select_grok_session(content).unwrap();
        assert_eq!(session.access_token, "newer");
        assert_eq!(session.user_id, "u-new");
        assert!(session.can_refresh);
    }

    #[test]
    fn a_bearer_only_entry_is_still_usable() {
        let session = select_grok_session(
            r#"{"https://accounts.x.ai/sign-in":{"key":"token","user_id":"u-app"}}"#,
        )
        .unwrap();
        assert_eq!(session.access_token, "token");
        // Nothing to renew with, so the CLI must not be asked to sign in.
        assert!(!session.can_refresh);
        assert!(select_grok_session("{}").is_none());
        assert!(select_grok_session("not json").is_none());
    }

    #[test]
    fn unrelated_identity_providers_and_api_keys_are_not_billing_sessions() {
        let unrelated = r#"{
            "https://idp.example.com::client": {
                "key": "corporate-token", "expires_at": "2099-01-01T00:00:00Z"
            },
            "https://auth.x.ai.example.com::client": {"key": "lookalike-token"},
            "xai::api_key": {"key": "api-key"}
        }"#;
        assert!(select_grok_session(unrelated).is_none());
        let mut entries: serde_json::Value = serde_json::from_str(unrelated).unwrap();
        entries["https://auth.x.ai::grok-cli"] = serde_json::json!({
            "key": "session", "expires_at": "2026-01-01T00:00:00Z"
        });
        assert_eq!(
            select_grok_session(&entries.to_string())
                .unwrap()
                .access_token,
            "session"
        );
    }

    #[test]
    fn the_billing_pool_maps_to_the_weekly_window() {
        let response: GrokBillingResponse = serde_json::from_str(
            r#"{
                "config": {
                    "creditUsagePercent": 42.5,
                    "currentPeriod": {
                        "periodType": "weekly",
                        "end": "2026-09-28T19:27:24.000Z"
                    }
                },
                "subscriptionTier": "SuperGrok Heavy"
            }"#,
        )
        .unwrap();

        let data = grok_usage_from_billing(response).unwrap();
        assert_eq!(data.weekly.percentage, 42.5);
        assert!(data.weekly.available);
        assert!(data.weekly.resets_at.is_some());
        // Grok has no five-hour window, so nothing claims the session slot.
        assert!(!data.session.available);
        assert!(data.monthly.is_none());
        assert_eq!(data.weekly_label, None);
        assert!(data.credits.is_none());
    }

    #[test]
    fn a_live_weekly_response_is_read_end_to_end() {
        // Shape captured from cli-chat-proxy on a SuperGrok account: the
        // period type arrives as `type`, not `periodType`.
        let response: GrokBillingResponse = serde_json::from_str(
            r#"{
                "config": {
                    "currentPeriod": {
                        "type": "USAGE_PERIOD_TYPE_WEEKLY",
                        "start": "2026-09-20T13:10:00.412622+00:00",
                        "end": "2026-09-27T13:10:00.412622+00:00"
                    },
                    "creditUsagePercent": 48.0,
                    "onDemandCap": { "val": 0 },
                    "onDemandUsed": { "val": 0 },
                    "productUsage": [{ "product": "GrokBuild", "usagePercent": 48.0 }],
                    "isUnifiedBillingUser": true,
                    "prepaidBalance": { "val": 0 },
                    "topUpMethod": "TOP_UP_METHOD_SAVED_PAYMENT_METHOD",
                    "billingPeriodStart": "2026-09-20T13:10:00.412622+00:00",
                    "billingPeriodEnd": "2026-09-27T13:10:00.412622+00:00"
                }
            }"#,
        )
        .unwrap();

        let data = grok_usage_from_billing(response).unwrap();
        assert_eq!(data.weekly.percentage, 48.0);
        assert!(data.weekly.resets_at.is_some());
        // A weekly period keeps the localized label and leaves monthly unset.
        assert_eq!(data.weekly_label, None);
        assert!(data.monthly.is_none());
        // An empty on-demand cap is not credit spending.
        assert!(data.credits.is_none());
        let [product] = data.limits.as_slice() else {
            panic!("expected one product limit, got {:?}", data.limits);
        };
        assert_eq!(product.key, "grokbuild");
        assert_eq!(product.label, "GrokBuild");
        assert_eq!(product.kind, "product");
        assert_eq!(product.usage.percentage, 48.0);
        assert_eq!(product.usage.resets_at, data.weekly.resets_at);
    }

    #[test]
    fn a_monthly_period_also_fills_the_monthly_window() {
        let response: GrokBillingResponse = serde_json::from_str(
            r#"{"config":{"creditUsagePercent":10,"currentPeriod":{"periodType":"monthly","end":1790000000}}}"#,
        )
        .unwrap();

        let data = grok_usage_from_billing(response).unwrap();
        assert_eq!(data.weekly_label.as_deref(), Some("30d"));
        assert_eq!(data.monthly.unwrap().percentage, 10.0);
        assert_eq!(
            data.weekly.resets_at,
            Some(UNIX_EPOCH + Duration::from_secs(1_790_000_000))
        );
    }

    #[test]
    fn on_demand_credits_appear_only_once_they_are_spent() {
        let billing = |cents: &str| -> GrokBillingResponse {
            serde_json::from_str(&format!(
                r#"{{"config":{{"creditUsagePercent":100,"onDemandCap":{{"val":5000}},{cents}}},"onDemandEnabled":true}}"#
            ))
            .unwrap()
        };

        assert!(
            grok_usage_from_billing(billing(r#""onDemandUsed":{"val":0}"#))
                .unwrap()
                .credits
                .is_none()
        );

        let credits = grok_usage_from_billing(billing(r#""onDemandUsed":{"val":1250}"#))
            .unwrap()
            .credits
            .unwrap();
        assert_eq!(credits.total, 50.0);
        assert_eq!(credits.remaining, 37.5);
        assert_eq!(credits.percentage, 25.0);
    }

    #[test]
    fn cents_accept_wrapped_and_bare_numbers_and_strings() {
        for (cap, used) in [
            (serde_json::json!(2000), serde_json::json!(1000)),
            (serde_json::json!("2000"), serde_json::json!("1000")),
            (
                serde_json::json!({"val": 2000}),
                serde_json::json!({"val": 1000}),
            ),
            (
                serde_json::json!({"val": "2000"}),
                serde_json::json!({"val": "1000"}),
            ),
            (serde_json::json!("2e3"), serde_json::json!({"val": "1e3"})),
        ] {
            let response: GrokBillingResponse = serde_json::from_value(serde_json::json!({
                "config": {
                    "creditUsagePercent": 100,
                    "onDemandCap": cap,
                    "onDemandUsed": used,
                    "prepaidBalance": used
                }
            }))
            .unwrap();
            assert_eq!(
                response
                    .config
                    .as_ref()
                    .unwrap()
                    .prepaid_balance
                    .unwrap()
                    .units(),
                10.0
            );
            let data = grok_usage_from_billing(response).unwrap();
            assert_eq!(data.weekly.percentage, 100.0);
            let credits = data.credits.unwrap();
            assert_eq!(credits.total, 20.0);
            assert_eq!(credits.remaining, 10.0);
            assert_eq!(credits.percentage, 50.0);
        }
        assert_eq!(
            serde_json::from_str::<GrokCents>("{}").unwrap().units(),
            0.0
        );
    }

    #[test]
    fn invalid_cents_strings_are_rejected_instead_of_becoming_credit_values() {
        for value in ["", "invalid", "NaN", "Infinity", "-Infinity", "1e999"] {
            for input in [serde_json::json!(value), serde_json::json!({"val": value})] {
                assert!(
                    serde_json::from_value::<GrokCents>(input).is_err(),
                    "{value}"
                );
            }
        }
    }

    #[test]
    fn a_response_without_credit_usage_is_rejected() {
        let response: GrokBillingResponse =
            serde_json::from_str(r#"{"config":{"currentPeriod":{"periodType":"weekly"}}}"#)
                .unwrap();
        assert!(grok_usage_from_billing(response).is_none());
        let empty: GrokBillingResponse = serde_json::from_str("{}").unwrap();
        assert!(grok_usage_from_billing(empty).is_none());
    }

    #[test]
    fn a_cli_version_is_picked_out_of_arbitrary_output() {
        assert_eq!(
            first_version("grok 1.24.3 (abc1234)").as_deref(),
            Some("1.24.3")
        );
        assert_eq!(
            first_version("grok-build v2.0.11\n").as_deref(),
            Some("2.0.11")
        );
        assert_eq!(first_version("no version here"), None);
    }

    #[test]
    fn failed_cli_detection_is_retried_and_success_is_cached() {
        let cache = OnceLock::new();
        assert_eq!(cached_cli_detection(&cache, || None), None);
        assert_eq!(
            cached_cli_detection(&cache, || Some("1.2.3".into())).as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            cached_cli_detection(&cache, || panic!("successful detection must be cached"))
                .as_deref(),
            Some("1.2.3")
        );
    }

    #[test]
    fn numeric_period_ends_reject_overflow_without_losing_valid_timestamps() {
        for value in [serde_json::json!(1e300), serde_json::json!(1e25)] {
            assert_eq!(period_end(Some(&value)), None);
        }
        for value in [
            serde_json::json!(1_790_000_000),
            serde_json::json!(1_790_000_000_000u64),
        ] {
            assert_eq!(
                period_end(Some(&value)),
                Some(UNIX_EPOCH + Duration::from_secs(1_790_000_000))
            );
        }
    }

    #[test]
    fn version_detection_runs_windows_shims_with_spaces_in_the_path() {
        let root = crate::app_settings::app_data_directory().join("Grok CLI shims");
        std::fs::create_dir_all(&root).unwrap();
        for (extension, script) in [
            (
                "cmd",
                "@echo off\r\nif not \"%~1\"==\"version\" exit /b 1\r\necho grok 1.2.3\r\n",
            ),
            (
                "ps1",
                "if ($args[0] -ne 'version') { exit 1 }; Write-Output 'grok 1.2.3'",
            ),
        ] {
            let path = root.join(format!("grok.{extension}"));
            std::fs::write(&path, script).unwrap();
            assert_eq!(
                read_cli_version(path.to_str().unwrap(), Duration::from_secs(5)).as_deref(),
                Some("1.2.3"),
                "{}",
                path.display()
            );
        }
    }

    #[test]
    fn version_detection_rejects_failed_commands_and_times_out() {
        let root = crate::app_settings::app_data_directory();
        let failed = root.join("failed-version.cmd");
        std::fs::write(&failed, "@echo grok 1.2.3\r\n@exit /b 1\r\n").unwrap();
        assert_eq!(
            read_cli_version(failed.to_str().unwrap(), Duration::from_secs(5)),
            None
        );

        let slow = root.join("slow-version.ps1");
        std::fs::write(&slow, "Start-Sleep -Seconds 30; Write-Output 'grok 1.2.3'").unwrap();
        let start = std::time::Instant::now();
        assert_eq!(
            read_cli_version(slow.to_str().unwrap(), Duration::from_millis(200)),
            None
        );
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
