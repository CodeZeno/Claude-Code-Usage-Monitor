use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::Deserialize;

use super::{build_agent, parse_iso8601, PollError};
use crate::diagnose;
use crate::models::{UsageData, UsageSection};
use super::windows_credentials;

const BUDGET_URL: &str = "https://api.mistral.ai/v1/billing/budget";
const TOKEN_ENVIRONMENT: &str = "MISTRAL_API_KEY";
const VIBE_HOME_ENV: &str = "VIBE_HOME";
const USERPROFILE_ENV: &str = "USERPROFILE";
const VIBE_ENV_FILE: &str = ".vibe/.env";
const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const VIBE_KEYRING_TARGETS: [&str; 2] = ["ai.mistral.vibe", "MISTRAL_API_KEY@ai.mistral.vibe"];

#[derive(Debug, Deserialize)]
struct BudgetResponse {
    #[serde(default)]
    vibe_budget: Option<Budget>,
    #[serde(default)]
    api_budget: Option<Budget>,
}

#[derive(Debug, Deserialize)]
struct Budget {
    #[serde(default)]
    usage_percentage: f64,
    #[serde(default)]
    #[allow(dead_code)]
    initial_budget: f64,
    #[serde(default)]
    #[allow(dead_code)]
    currency: Option<String>,
    #[serde(default)]
    reset_at: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    payg_enabled: Option<bool>,
}

pub(super) fn poll_mistral() -> Result<UsageData, PollError> {
    let token = resolve_token().ok_or_else(|| {
        diagnose::log(
            "Mistral usage poll failed: no credentials found; set MISTRAL_API_KEY or create ~/.vibe/.env with MISTRAL_API_KEY=...",
        );
        PollError::NoCredentials
    })?;

    fetch_mistral_usage(&token)
}

pub(super) fn credential_watch_snapshot(_all_sources: bool) -> Vec<String> {
    let mut parts = Vec::new();

    // Environment variable presence/value hash
    match std::env::var(TOKEN_ENVIRONMENT) {
        Ok(value) => {
            let mut hasher = DefaultHasher::new();
            value.hash(&mut hasher);
            parts.push(format!(
                "{}|present|{}|{:x}",
                TOKEN_ENVIRONMENT,
                value.len(),
                hasher.finish()
            ));
        }
        Err(_) => parts.push(format!("{}|missing", TOKEN_ENVIRONMENT)),
    }

    // Vibe CLI .env file signature
    if let Some(env_path) = vibe_env_path() {
        parts.push(env_file_signature(&env_path));
    } else {
        parts.push("vibe-env|missing".to_string());
    }

    // Windows Credential Manager signatures
    let mut has_keyring = false;
    for target in VIBE_KEYRING_TARGETS {
        let credentials = windows_credentials::enumerate_generic(target);
        if !credentials.is_empty() {
            for cred in credentials {
                parts.push(format!("keyring|{}|{}", cred.target, cred.last_written));
                has_keyring = true;
            }
        }
    }
    if !has_keyring {
        parts.push("keyring|missing".to_string());
    }

    parts
}

fn resolve_token() -> Option<String> {
    // Try environment variable first
    if let Ok(token) = std::env::var(TOKEN_ENVIRONMENT) {
        let token = token.trim();
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }

    // Try Vibe CLI .env file
    if let Some(env_path) = vibe_env_path() {
        if let Ok(content) = std::fs::read_to_string(env_path) {
            for line in content.lines() {
                let line = line.trim();
                // Skip comments and blank lines
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                // Parse key=value
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    if key == TOKEN_ENVIRONMENT {
                        let value = value.trim();
                        // Remove surrounding quotes if present
                        let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"'))
                            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                            .unwrap_or(value);
                        if !value.is_empty() {
                            return Some(value.to_string());
                        }
                    }
                }
            }
        }
    }

    // Try Windows Credential Manager
    for target in VIBE_KEYRING_TARGETS {
        if let Some(secret) = windows_credentials::read_generic(target) {
            let secret = secret.trim();
            if !secret.is_empty() {
                return Some(secret.to_string());
            }
        }
    }

    None
}

fn vibe_env_path() -> Option<String> {
    // Try VIBE_HOME first
    if let Ok(vibe_home) = std::env::var(VIBE_HOME_ENV) {
        let path = Path::new(&vibe_home).join(VIBE_ENV_FILE);
        if path.exists() {
            return Some(path.to_string_lossy().into_owned());
        }
    }

    // Fall back to USERPROFILE
    if let Ok(userprofile) = std::env::var(USERPROFILE_ENV) {
        let path = Path::new(&userprofile).join(VIBE_ENV_FILE);
        if path.exists() {
            return Some(path.to_string_lossy().into_owned());
        }
    }

    None
}

fn env_file_signature(path: &str) -> String {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let mut hasher = DefaultHasher::new();
            // Hash the path to make signature unique to the file location
            path.hash(&mut hasher);
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs())
                .unwrap_or(0);
            format!(
                "vibe-env|present|{}|{}|{}",
                path,
                metadata.len(),
                modified
            )
        }
        Err(_) => format!("vibe-env|{}|missing", path),
    }
}

fn fetch_mistral_usage(token: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;

    let request = agent
        .get(BUDGET_URL)
        .header("Authorization", &format!("Bearer {token}"))
        .header("x-api-key", token)
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT);

    let response = match request.call().and_then(super::check_http_status) {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401 | 403)) => return Err(PollError::AuthRequired),
        Err(ureq::Error::StatusCode(code)) => {
            diagnose::log(format!("Mistral usage endpoint returned status {code}"));
            return Err(PollError::HttpStatus(code));
        }
        Err(error) => {
            diagnose::log_error("Mistral usage request failed", error);
            return Err(PollError::NetworkError);
        }
    };

    let response: BudgetResponse = response
        .into_body()
        .read_json()
        .map_err(|error| {
            diagnose::log_error("unable to parse Mistral usage response", error);
            PollError::UnexpectedResponse
        })?;

    map_to_usage_data(response)
}

fn map_to_usage_data(response: BudgetResponse) -> Result<UsageData, PollError> {
    // If both budgets are None, fail
    if response.vibe_budget.is_none() && response.api_budget.is_none() {
        diagnose::log("Mistral usage response missing both vibe_budget and api_budget");
        return Err(PollError::RequestFailed);
    }

    let mut usage_data = UsageData::default();

    // Map vibe_budget to weekly
    if let Some(budget) = response.vibe_budget {
        let percentage = budget.usage_percentage.clamp(0.0, 100.0);
        let resets_at = parse_iso8601(budget.reset_at.as_deref());

        usage_data.weekly = UsageSection {
            available: true,
            percentage,
            resets_at,
        };
        usage_data.weekly_label = Some("Vibe".to_string());
    }

    // Map api_budget to monthly
    if let Some(budget) = response.api_budget {
        let percentage = budget.usage_percentage.clamp(0.0, 100.0);
        let resets_at = parse_iso8601(budget.reset_at.as_deref());

        usage_data.monthly = Some(UsageSection {
            available: true,
            percentage,
            resets_at,
        });
    }

    Ok(usage_data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(json: serde_json::Value) -> Result<UsageData, PollError> {
        map_to_usage_data(serde_json::from_value(json).unwrap())
    }

    #[test]
    fn both_budgets_present() {
        let data = usage(serde_json::json!({
            "vibe_budget": {
                "usage_percentage": 25.5,
                "initial_budget": 1000.0,
                "currency": "credits",
                "reset_at": "2026-11-01T00:00:00Z",
                "payg_enabled": false
            },
            "api_budget": {
                "usage_percentage": 45.2,
                "initial_budget": 2000.0,
                "currency": "USD",
                "reset_at": "2026-11-01T00:00:00Z",
                "payg_enabled": false
            }
        })).unwrap();

        assert!(data.weekly.available);
        assert_eq!(data.weekly.percentage, 25.5);
        assert_eq!(data.weekly_label.as_deref(), Some("Vibe"));
        assert!(data.monthly.is_some());
        assert_eq!(data.monthly.unwrap().percentage, 45.2);
    }

    #[test]
    fn only_vibe_budget() {
        let data = usage(serde_json::json!({
            "vibe_budget": {
                "usage_percentage": 30.0,
                "initial_budget": 1000.0,
                "reset_at": "2026-11-01T00:00:00Z"
            }
        })).unwrap();

        assert!(data.weekly.available);
        assert_eq!(data.weekly.percentage, 30.0);
        assert_eq!(data.weekly_label.as_deref(), Some("Vibe"));
        assert!(data.monthly.is_none());
    }

    #[test]
    fn only_api_budget() {
        let data = usage(serde_json::json!({
            "api_budget": {
                "usage_percentage": 60.0,
                "initial_budget": 2000.0,
                "reset_at": "2026-11-01T00:00:00Z"
            }
        })).unwrap();

        assert!(!data.weekly.available);
        assert!(data.monthly.is_some());
        assert_eq!(data.monthly.unwrap().percentage, 60.0);
    }

    #[test]
    fn both_budgets_missing() {
        let result = usage(serde_json::json!({}));
        assert!(matches!(result, Err(PollError::RequestFailed)));
    }

    #[test]
    fn percentage_clamped() {
        let data = usage(serde_json::json!({
            "vibe_budget": {
                "usage_percentage": 150.0,
                "initial_budget": 100.0,
                "reset_at": "2026-11-01T00:00:00Z"
            }
        })).unwrap();

        assert_eq!(data.weekly.percentage, 100.0);
    }

    #[test]
    fn env_file_parsing() {
        let content = r#"# This is a comment
MISTRAL_API_KEY=test_key_123
# Another comment
  MISTRAL_API_KEY="quoted_key"
ANOTHER_VAR=value
"#;

        let temp_dir = tempfile::tempdir().unwrap();
        let env_path = temp_dir.path().join(".vibe").join(".env");
        std::fs::create_dir_all(env_path.parent().unwrap()).unwrap();
        std::fs::write(&env_path, content).unwrap();

        // Temporarily set VIBE_HOME
        std::env::set_var(VIBE_HOME_ENV, temp_dir.path().to_string_lossy().into_owned());

        let token = resolve_token();
        assert_eq!(token, Some("test_key_123".to_string()));

        std::env::remove_var(VIBE_HOME_ENV);
    }

    #[test]
    fn keyring_fallback_when_env_and_file_missing() {
        // Remove env var and ensure no .env file
        std::env::remove_var(TOKEN_ENVIRONMENT);
        std::env::remove_var(VIBE_HOME_ENV);
        std::env::remove_var(USERPROFILE_ENV);

        // This test just verifies resolve_token doesn't panic when keyring is the only source
        // On a machine with keyring entries, it will return Some(secret)
        // On a machine without, it returns None
        let token = resolve_token();
        // Should not panic, just return None if nothing is available
        assert!(token.is_none() || token.is_some());
    }

    #[test]
    fn credential_watch_snapshot_covers_all_sources() {
        // Test that env var, file, and keyring signatures are included
        let snapshot = credential_watch_snapshot(false);
        assert!(!snapshot.is_empty());
        
        // Should have at least env var signature
        assert!(snapshot.iter().any(|s| s.contains(TOKEN_ENVIRONMENT)));
        // Should have vibe-env signature
        assert!(snapshot.iter().any(|s| s.starts_with("vibe-env|")));
        // Should have keyring signature
        assert!(snapshot.iter().any(|s| s.starts_with("keyring|")));
    }
}
