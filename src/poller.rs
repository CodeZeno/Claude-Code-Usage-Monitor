use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::diagnose;
use crate::models::{AppUsageData, UsageData, UsageSection};
use crate::providers::{ProviderId, ProviderSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollError {
    AuthRequired,
    NoCredentials,
    TokenExpired,
    RequestFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialWatchMode {
    ActiveSource(ProviderId),
    AllSources(ProviderId),
}

pub type CredentialWatchSnapshot = Vec<String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PollFailure {
    pub provider: ProviderId,
    pub error: PollError,
}

pub fn poll(enabled_providers: ProviderSet) -> Result<AppUsageData, PollFailure> {
    poll_with(enabled_providers, poll_provider)
}

fn poll_with(
    enabled_providers: ProviderSet,
    mut poll_provider: impl FnMut(ProviderId) -> Result<UsageData, PollError>,
) -> Result<AppUsageData, PollFailure> {
    let mut data = AppUsageData::default();
    let mut first_error = None;
    for provider in enabled_providers.iter() {
        match poll_provider(provider) {
            Ok(usage) => {
                data.insert(provider, usage);
            }
            Err(error) => {
                if enabled_providers.len() > 1 {
                    diagnose::log(format!(
                        "{} usage poll failed: {error:?}",
                        provider.descriptor().display_name
                    ));
                }
                first_error.get_or_insert(PollFailure { provider, error });
            }
        }
    }

    if data.is_empty() {
        Err(first_error.unwrap_or(PollFailure {
            provider: enabled_providers.first().unwrap_or_default(),
            error: PollError::RequestFailed,
        }))
    } else {
        Ok(data)
    }
}

mod antigravity;
mod claude;
mod codex;
mod opencode;

struct ProviderPoller {
    id: ProviderId,
    poll: fn() -> Result<UsageData, PollError>,
    credential_watch: fn(bool) -> CredentialWatchSnapshot,
}

const PROVIDER_POLLERS: [ProviderPoller; 4] = [
    ProviderPoller {
        id: ProviderId::Claude,
        poll: claude::poll_claude_code,
        credential_watch: claude::credential_watch_snapshot,
    },
    ProviderPoller {
        id: ProviderId::Codex,
        poll: codex::poll_codex,
        credential_watch: codex_credential_watch_snapshot,
    },
    ProviderPoller {
        id: ProviderId::Antigravity,
        poll: antigravity::poll_antigravity,
        credential_watch: antigravity_credential_watch_snapshot,
    },
    ProviderPoller {
        id: ProviderId::OpenCode,
        poll: opencode::poll_opencode,
        credential_watch: opencode::credential_watch_snapshot,
    },
];

fn provider_poller(provider: ProviderId) -> Option<&'static ProviderPoller> {
    PROVIDER_POLLERS.iter().find(|poller| poller.id == provider)
}

fn poll_provider(provider: ProviderId) -> Result<UsageData, PollError> {
    provider_poller(provider)
        .ok_or(PollError::RequestFailed)
        .and_then(|poller| (poller.poll)())
}

pub fn credential_watch_snapshot(mode: CredentialWatchMode) -> CredentialWatchSnapshot {
    let (provider, all_sources) = match mode {
        CredentialWatchMode::ActiveSource(provider) => (provider, false),
        CredentialWatchMode::AllSources(provider) => (provider, true),
    };
    provider_poller(provider)
        .map(|poller| (poller.credential_watch)(all_sources))
        .unwrap_or_default()
}

fn codex_credential_watch_snapshot(_all_sources: bool) -> CredentialWatchSnapshot {
    codex::credential_watch_snapshot()
}

fn antigravity_credential_watch_snapshot(_all_sources: bool) -> CredentialWatchSnapshot {
    vec![antigravity::antigravity_credential_watch_signature()]
}

fn build_agent() -> Result<ureq::Agent, PollError> {
    let tls = native_tls::TlsConnector::new().map_err(|_| PollError::RequestFailed)?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .tls_connector(std::sync::Arc::new(tls))
        .build())
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
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

/// Calculate how long until the display text would change
pub fn time_until_display_change(resets_at: Option<SystemTime>) -> Option<Duration> {
    let reset = resets_at?;
    let remaining = reset.duration_since(SystemTime::now()).ok()?;
    Some(time_until_display_change_from_secs(remaining.as_secs()))
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

pub fn app_is_past_reset(data: &AppUsageData) -> bool {
    data.iter().any(|(_, usage)| is_past_reset(usage))
}

#[cfg(test)]
mod tests;
