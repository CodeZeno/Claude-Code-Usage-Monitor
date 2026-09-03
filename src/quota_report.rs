//! Phase 1: headless, machine-readable quota reporting.
//!
//! Builds a JSON-serializable snapshot of Claude Code / Codex quota by
//! calling straight into [`crate::poller`] — the exact same polling code
//! path the GUI monitor uses (`poller::poll_report`). No provider request is
//! reimplemented here; this module only reshapes [`poller::PollReport`] into
//! a small, stable, secret-free schema.
//!
//! Deliberately excluded from the schema: OAuth/access tokens, account ids,
//! and raw provider response bodies. Only aggregate percentages, reset
//! timestamps (unix seconds, matching this repo's existing snapshot
//! convention — see `snapshot_schema.rs`), and coarse status/error tags are
//! ever serialized.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::models::UsageData;
use crate::poller::{self, PollError, ProviderPollOutcome};

/// Bump when the JSON shape changes in a way a consumer must know about.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    /// Polled successfully this run.
    Ok,
    /// Polling was attempted and failed (no credentials, expired token,
    /// auth required, or the request itself failed). See `error`.
    Unavailable,
    /// Not requested for this report.
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct QuotaWindow {
    pub used_percent: f64,
    /// Unix seconds, when known.
    pub resets_at: Option<u64>,
    /// True when `resets_at` is already in the past relative to
    /// `generated_at`. The provider hasn't rolled the window over yet in its
    /// own bookkeeping, so `used_percent` may not reflect the fresh window —
    /// treat it as informative but not yet authoritative for that window.
    pub stale: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct QuotaWindows {
    pub five_hour: Option<QuotaWindow>,
    pub seven_day: Option<QuotaWindow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProviderQuota {
    pub provider: String,
    pub status: ProviderStatus,
    /// Coarse error tag (e.g. "no_credentials"), never a token or raw
    /// response. Present only when `status` is `unavailable`.
    pub error: Option<String>,
    pub windows: QuotaWindows,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuotaSnapshot {
    pub schema_version: u32,
    /// Unix seconds this snapshot was generated.
    pub generated_at: u64,
    pub providers: Vec<ProviderQuota>,
}

impl QuotaSnapshot {
    pub fn provider(&self, name: &str) -> Option<&ProviderQuota> {
        self.providers.iter().find(|p| p.provider == name)
    }
}

/// Poll both Claude Code and Codex and build a snapshot. This is the normal
/// entry point for the headless CLI; it does not require the GUI monitor to
/// be running and does not touch any monitor-only state.
pub fn collect() -> QuotaSnapshot {
    collect_selected(true, true)
}

pub fn collect_selected(show_claude_code: bool, show_codex: bool) -> QuotaSnapshot {
    let report = poller::poll_report(show_claude_code, show_codex, false);
    build_snapshot(report, unix_now())
}

fn build_snapshot(report: poller::PollReport, generated_at: u64) -> QuotaSnapshot {
    QuotaSnapshot {
        schema_version: SCHEMA_VERSION,
        generated_at,
        providers: vec![
            provider_quota("claude_code", report.claude_code, generated_at),
            provider_quota("codex", report.codex, generated_at),
        ],
    }
}

fn provider_quota(name: &str, outcome: ProviderPollOutcome, generated_at: u64) -> ProviderQuota {
    match outcome {
        ProviderPollOutcome::Disabled => ProviderQuota {
            provider: name.to_string(),
            status: ProviderStatus::Disabled,
            error: None,
            windows: QuotaWindows::default(),
        },
        ProviderPollOutcome::Error { error, .. } => ProviderQuota {
            provider: name.to_string(),
            status: ProviderStatus::Unavailable,
            error: Some(poll_error_tag(error).to_string()),
            windows: QuotaWindows::default(),
        },
        ProviderPollOutcome::Success { usage, .. } => ProviderQuota {
            provider: name.to_string(),
            status: ProviderStatus::Ok,
            error: None,
            windows: windows_from_usage(&usage, generated_at),
        },
    }
}

fn windows_from_usage(usage: &UsageData, generated_at: u64) -> QuotaWindows {
    QuotaWindows {
        five_hour: usage
            .session_available()
            .then(|| quota_window(usage.session.percentage, usage.session.resets_at, generated_at)),
        seven_day: usage
            .weekly_available()
            .then(|| quota_window(usage.weekly.percentage, usage.weekly.resets_at, generated_at)),
    }
}

fn quota_window(used_percent: f64, resets_at: Option<SystemTime>, generated_at: u64) -> QuotaWindow {
    let resets_at = resets_at.and_then(system_time_to_unix);
    let stale = resets_at.is_some_and(|resets_at| resets_at <= generated_at);
    QuotaWindow {
        used_percent,
        resets_at,
        stale,
    }
}

fn poll_error_tag(error: PollError) -> &'static str {
    match error {
        PollError::AuthRequired => "auth_required",
        PollError::NoCredentials => "no_credentials",
        PollError::TokenExpired => "token_expired",
        PollError::RequestFailed => "request_failed",
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn system_time_to_unix(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UsageSection;
    use std::time::Duration;

    fn usage_with(session: UsageSection, weekly: UsageSection) -> UsageData {
        let mut usage = UsageData::default();
        usage.set_session(session);
        usage.set_weekly(weekly);
        usage
    }

    #[test]
    fn success_outcome_carries_both_windows_with_unix_timestamps() {
        let generated_at = 1_000_000_u64;
        let resets_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_050_000);
        let usage = usage_with(
            UsageSection {
                percentage: 42.0,
                resets_at: Some(resets_at),
            },
            UsageSection {
                percentage: 10.0,
                resets_at: None,
            },
        );

        let outcome = ProviderPollOutcome::Success {
            source: poller_test_source(),
            attempted_at: SystemTime::now(),
            acquired_at: SystemTime::now(),
            usage,
        };

        let quota = provider_quota("claude_code", outcome, generated_at);
        assert_eq!(quota.status, ProviderStatus::Ok);
        assert!(quota.error.is_none());

        let five_hour = quota.windows.five_hour.expect("session window present");
        assert_eq!(five_hour.used_percent, 42.0);
        assert_eq!(five_hour.resets_at, Some(1_050_000));
        assert!(!five_hour.stale);

        let seven_day = quota.windows.seven_day.expect("weekly window present");
        assert_eq!(seven_day.used_percent, 10.0);
        assert_eq!(seven_day.resets_at, None);
        assert!(!seven_day.stale);
    }

    #[test]
    fn reset_time_in_the_past_is_flagged_stale() {
        let generated_at = 1_000_000_u64;
        let resets_at = SystemTime::UNIX_EPOCH + Duration::from_secs(900_000);
        let window = quota_window(75.0, Some(resets_at), generated_at);
        assert!(window.stale);
    }

    #[test]
    fn error_outcome_maps_to_unavailable_with_a_safe_tag_no_secrets() {
        let outcome = ProviderPollOutcome::Error {
            source: poller_test_source(),
            attempted_at: SystemTime::now(),
            error: PollError::NoCredentials,
        };
        let quota = provider_quota("codex", outcome, 1_000_000);
        assert_eq!(quota.status, ProviderStatus::Unavailable);
        assert_eq!(quota.error.as_deref(), Some("no_credentials"));
        assert_eq!(quota.windows, QuotaWindows::default());
    }

    #[test]
    fn disabled_outcome_has_no_error_and_no_windows() {
        let quota = provider_quota("codex", ProviderPollOutcome::Disabled, 1_000_000);
        assert_eq!(quota.status, ProviderStatus::Disabled);
        assert!(quota.error.is_none());
        assert_eq!(quota.windows, QuotaWindows::default());
    }

    #[test]
    fn json_never_contains_token_or_credential_fields() {
        let generated_at = 1_000_000_u64;
        let usage = usage_with(
            UsageSection {
                percentage: 5.0,
                resets_at: None,
            },
            UsageSection {
                percentage: 5.0,
                resets_at: None,
            },
        );
        let report = poller::PollReport {
            claude_code: ProviderPollOutcome::Success {
                source: poller_test_source(),
                attempted_at: SystemTime::now(),
                acquired_at: SystemTime::now(),
                usage,
            },
            codex: ProviderPollOutcome::Error {
                source: poller_test_source(),
                attempted_at: SystemTime::now(),
                error: PollError::AuthRequired,
            },
            antigravity: ProviderPollOutcome::Disabled,
            github_copilot: ProviderPollOutcome::Disabled,
        };

        let snapshot = build_snapshot(report, generated_at);
        let json = serde_json::to_string(&snapshot).unwrap();

        assert_eq!(snapshot.schema_version, SCHEMA_VERSION);
        assert_eq!(snapshot.generated_at, generated_at);
        for banned in [
            "token", "access_token", "credential", "authorization", "Bearer",
        ] {
            assert!(
                !json.to_ascii_lowercase().contains(&banned.to_ascii_lowercase()),
                "json unexpectedly contained {banned:?}: {json}"
            );
        }
    }

    fn poller_test_source() -> poller::ProviderPollSource {
        poller::ProviderPollSource::AnthropicOauthUsage
    }
}
