//! Read-only Vercel AI Gateway quota acquisition.
//!
//! The API key is read from AI_GATEWAY_API_KEY only for the duration of a
//! poll. It is never persisted, logged, or serialized by this module.
//!
//! Vercel AI Gateway budgets are user-configured spend caps, not native model
//! rate limits. They are therefore represented as USD used / limit values.
//! The Quotas API currently does not expose an exact reset timestamp, so this
//! module does not invent one.

use std::ffi::OsStr;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::models::{QuotaItem, QuotaItemAvailability, QuotaMetric, QuotaUnit, UsageData};
use crate::poller::PollError;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const QUOTAS_URL: &str = "https://ai-gateway.vercel.sh/v1/quotas";

pub(crate) const VERCEL_AI_GATEWAY_SPEND_ITEM_ID: &str = "spend_budget";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiKeyListResponse {
    #[serde(default)]
    api_keys: Vec<ApiKeyMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiKeyMetadata {
    id: String,
    partial_key: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaResponse {
    limit_amount: f64,
    current_spend: f64,
    refresh_period: String,
    active: bool,
    #[serde(default)]
    archived: bool,
}

pub(crate) fn poll() -> Result<UsageData, PollError> {
    let secret = std::env::var("AI_GATEWAY_API_KEY").map_err(|_| PollError::NoCredentials)?;

    let secret = secret.trim();
    if secret.is_empty() {
        return Err(PollError::NoCredentials);
    }

    let keys = list_ai_gateway_keys()?;
    let key_id = matching_key_id(&keys, secret).ok_or(PollError::AuthRequired)?;
    let quota = fetch_quota(secret, key_id)?;

    usage_from_quota(quota)
}

fn list_ai_gateway_keys() -> Result<ApiKeyListResponse, PollError> {
    let executable = resolve_vercel_cli_executable().ok_or(PollError::RequestFailed)?;

    let mut command = vercel_command(&executable);
    command
        .arg("api")
        .arg("/v1/api-keys?purpose=ai-gateway")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let output =
        run_with_timeout(&mut command, Duration::from_secs(20)).ok_or(PollError::RequestFailed)?;

    if !output.status.success() {
        return Err(PollError::AuthRequired);
    }

    serde_json::from_slice(&output.stdout).map_err(|_| PollError::RequestFailed)
}

fn matching_key_id<'a>(keys: &'a ApiKeyListResponse, secret: &str) -> Option<&'a str> {
    let mut matches = keys.api_keys.iter().filter(|key| {
        let suffix = key.partial_key.trim().trim_start_matches('…');

        !suffix.is_empty() && secret.ends_with(suffix)
    });

    let first = matches.next()?;

    // A partial-key collision must never silently select an arbitrary key.
    if matches.next().is_some() {
        return None;
    }

    Some(first.id.as_str())
}

fn fetch_quota(secret: &str, key_id: &str) -> Result<QuotaResponse, PollError> {
    let tls = native_tls::TlsConnector::new().map_err(|_| PollError::RequestFailed)?;

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .tls_connector(std::sync::Arc::new(tls))
        .build();

    let url = format!("{QUOTAS_URL}?quotaEntityId=api_key_id_{key_id}");

    let response = match agent
        .get(&url)
        .set("Authorization", &format!("Bearer {secret}"))
        .call()
    {
        Ok(response) => response,

        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            return Err(PollError::AuthRequired);
        }

        Err(_) => return Err(PollError::RequestFailed),
    };

    response.into_json().map_err(|_| PollError::RequestFailed)
}

fn usage_from_quota(quota: QuotaResponse) -> Result<UsageData, PollError> {
    if !quota.active
        || quota.archived
        || !quota.limit_amount.is_finite()
        || quota.limit_amount <= 0.0
        || !quota.current_spend.is_finite()
        || quota.current_spend < 0.0
    {
        return Err(PollError::RequestFailed);
    }

    let label = match quota.refresh_period.as_str() {
        "daily" => "Daily spend",
        "weekly" => "Weekly spend",
        "monthly" => "Monthly spend",
        "none" => "Spend",
        _ => return Err(PollError::RequestFailed),
    };

    Ok(UsageData::from_quota_items(vec![QuotaItem {
        id: VERCEL_AI_GATEWAY_SPEND_ITEM_ID.to_string(),
        label: label.to_string(),
        availability: QuotaItemAvailability::Available,
        metric: Some(QuotaMetric::Used {
            used: quota.current_spend,
            limit: Some(quota.limit_amount),
        }),
        unit: QuotaUnit::Other("USD".to_string()),
        // Vercel tells us the refresh period, but not an exact reset time.
        resets_at: None,
    }]))
}

fn resolve_vercel_cli_executable() -> Option<PathBuf> {
    let path = std::env::var_os("PATH");
    let app_data = std::env::var_os("APPDATA");

    resolve_vercel_cli_executable_with(path.as_deref(), app_data.as_deref(), |candidate| {
        candidate.is_file()
    })
}

fn resolve_vercel_cli_executable_with<F>(
    path: Option<&OsStr>,
    app_data: Option<&OsStr>,
    is_file: F,
) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool,
{
    const NAMES: &[&str] = &["vercel.exe", "vercel.cmd", "vercel.bat", "vercel.ps1"];

    if let Some(path) = path {
        for directory in std::env::split_paths(path) {
            for name in NAMES {
                let candidate = directory.join(name);
                if is_file(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    if let Some(app_data) = app_data {
        for name in NAMES {
            let candidate = PathBuf::from(app_data).join("npm").join(name);

            if is_file(&candidate) {
                return Some(candidate);
            }
        }
    }

    None
}

fn vercel_command(executable: &Path) -> Command {
    let lower = executable.to_string_lossy().to_ascii_lowercase();

    if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        let mut command = Command::new("cmd.exe");
        command.arg("/d").arg("/c").arg(executable);
        command
    } else if lower.ends_with(".ps1") {
        let mut command = Command::new("powershell.exe");
        command
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(executable);
        command
    } else {
        Command::new(executable)
    }
}

fn run_with_timeout(command: &mut Command, timeout: Duration) -> Option<std::process::Output> {
    let mut child = command.spawn().ok()?;
    let started = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child.wait_with_output().ok();
            }

            Ok(None) if started.elapsed() <= timeout => {
                std::thread::sleep(Duration::from_millis(100));
            }

            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }

            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str, partial_key: &str) -> ApiKeyMetadata {
        ApiKeyMetadata {
            id: id.to_string(),
            partial_key: partial_key.to_string(),
        }
    }

    #[test]
    fn matches_key_by_non_secret_suffix() {
        let keys = ApiKeyListResponse {
            api_keys: vec![key("other", "xxxxxx"), key("wanted", "3WTVby")],
        };

        assert_eq!(
            matching_key_id(&keys, "secret-prefix-3WTVby"),
            Some("wanted")
        );
    }

    #[test]
    fn accepts_display_ellipsis_in_partial_key() {
        let keys = ApiKeyListResponse {
            api_keys: vec![key("wanted", "…3WTVby")],
        };

        assert_eq!(
            matching_key_id(&keys, "secret-prefix-3WTVby"),
            Some("wanted")
        );
    }

    #[test]
    fn refuses_ambiguous_partial_key_match() {
        let keys = ApiKeyListResponse {
            api_keys: vec![key("one", "3WTVby"), key("two", "3WTVby")],
        };

        assert_eq!(matching_key_id(&keys, "secret-prefix-3WTVby"), None);
    }

    #[test]
    fn monthly_quota_maps_to_usd_used_limit() {
        let usage = usage_from_quota(QuotaResponse {
            limit_amount: 1.0,
            current_spend: 0.000_016_38,
            refresh_period: "monthly".to_string(),
            active: true,
            archived: false,
        })
        .unwrap();

        let items = usage.quota_items();

        assert_eq!(items.len(), 1);

        let item = &items[0];

        assert_eq!(item.id, VERCEL_AI_GATEWAY_SPEND_ITEM_ID);

        assert_eq!(item.label, "Monthly spend");

        assert_eq!(item.unit.as_str(), "USD");

        assert_eq!(item.resets_at, None);

        assert_eq!(
            item.metric,
            Some(QuotaMetric::Used {
                used: 0.000_016_38,
                limit: Some(1.0),
            })
        );
    }

    #[test]
    fn rejects_inactive_quota() {
        let result = usage_from_quota(QuotaResponse {
            limit_amount: 1.0,
            current_spend: 0.1,
            refresh_period: "monthly".to_string(),
            active: false,
            archived: false,
        });

        assert!(matches!(result, Err(PollError::RequestFailed)));
    }

    #[test]
    fn rejects_archived_quota() {
        let result = usage_from_quota(QuotaResponse {
            limit_amount: 1.0,
            current_spend: 0.1,
            refresh_period: "monthly".to_string(),
            active: true,
            archived: true,
        });

        assert!(matches!(result, Err(PollError::RequestFailed)));
    }

    #[test]
    fn rejects_zero_limit() {
        let result = usage_from_quota(QuotaResponse {
            limit_amount: 0.0,
            current_spend: 0.0,
            refresh_period: "monthly".to_string(),
            active: true,
            archived: false,
        });

        assert!(matches!(result, Err(PollError::RequestFailed)));
    }
}
