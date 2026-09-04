use std::sync::OnceLock;
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
    poll_concurrently_with(enabled_providers, poll_provider)
}

/// Keep the previous reading for any enabled provider that failed this cycle.
///
/// A poll succeeds as long as one provider answers, so without this a single
/// provider's outage blanks its row on every refresh while the others carry
/// on updating. The carried figures are marked stale rather than passed off as
/// current.
pub fn carry_forward_failures(
    fresh: AppUsageData,
    previous: &AppUsageData,
    enabled: ProviderSet,
) -> AppUsageData {
    let mut merged = fresh;
    for provider in enabled.iter() {
        if merged.get(provider).is_some() {
            continue;
        }
        if let Some(last) = previous.get(provider) {
            let mut carried = last.clone();
            carried.stale = true;
            merged.insert(provider, carried);
        }
    }
    merged
}

fn poll_with(
    enabled_providers: ProviderSet,
    mut poll_provider: impl FnMut(ProviderId) -> Result<UsageData, PollError>,
) -> Result<AppUsageData, PollFailure> {
    let results = enabled_providers
        .iter()
        .map(|provider| (provider, poll_provider(provider)))
        .collect::<Vec<_>>();
    merge_poll_results(enabled_providers, results)
}

const MAX_CONCURRENT_PROVIDER_POLLS: usize = 3;

fn poll_concurrently_with<F>(
    enabled_providers: ProviderSet,
    poll_provider: F,
) -> Result<AppUsageData, PollFailure>
where
    F: Fn(ProviderId) -> Result<UsageData, PollError> + Sync,
{
    let providers = enabled_providers.iter().collect::<Vec<_>>();
    if providers.len() <= 1 {
        return poll_with(enabled_providers, poll_provider);
    }

    let worker_count = providers.len().min(MAX_CONCURRENT_PROVIDER_POLLS);
    let next_provider = std::sync::atomic::AtomicUsize::new(0);
    let mut results = std::thread::scope(|scope| {
        let (sender, receiver) = std::sync::mpsc::channel();
        for _ in 0..worker_count {
            let sender = sender.clone();
            let providers = &providers;
            let poll_provider = &poll_provider;
            let next_provider = &next_provider;
            scope.spawn(move || loop {
                let index = next_provider.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(provider) = providers.get(index).copied() else {
                    break;
                };
                if sender.send((provider, poll_provider(provider))).is_err() {
                    break;
                }
            });
        }
        drop(sender);
        receiver.into_iter().collect::<Vec<_>>()
    });
    results.sort_by_key(|(provider, _)| *provider);
    merge_poll_results(enabled_providers, results)
}

fn merge_poll_results(
    enabled_providers: ProviderSet,
    results: impl IntoIterator<Item = (ProviderId, Result<UsageData, PollError>)>,
) -> Result<AppUsageData, PollFailure> {
    let mut data = AppUsageData::default();
    let mut first_error = None;
    for (provider, result) in results {
        match result {
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
mod claude_desktop;
mod codex;
mod cursor;
mod opencode;

struct ProviderPoller {
    id: ProviderId,
    poll: fn() -> Result<UsageData, PollError>,
    credential_watch: fn(bool) -> CredentialWatchSnapshot,
}

const PROVIDER_POLLERS: [ProviderPoller; 5] = [
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
    ProviderPoller {
        id: ProviderId::Cursor,
        poll: cursor::poll_cursor,
        credential_watch: cursor::credential_watch_snapshot,
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
    static AGENT: OnceLock<Result<ureq::Agent, PollError>> = OnceLock::new();
    // Agent clones share their connection pool, cookies, and TLS configuration.
    AGENT
        .get_or_init(|| {
            let tls = ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build();
            Ok(ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .tls_config(tls)
                .build()
                .into())
        })
        .clone()
}

type HttpResponse = ureq::http::Response<ureq::Body>;

fn get_header_f64(response: &HttpResponse, name: &str) -> f64 {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn get_header_i64(response: &HttpResponse, name: &str) -> Option<i64> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok())
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
    let unix_secs = parse_datetime_to_unix(s?)?;
    UNIX_EPOCH.checked_add(Duration::from_secs(unix_secs))
}

/// Minimal datetime parser — avoids pulling in chrono/time crates.
fn parse_datetime_to_unix(s: &str) -> Option<u64> {
    let (datetime, offset_seconds) = split_timezone(s)?;
    let datetime = match datetime.split_once('.') {
        Some((base, fraction))
            if !fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit()) =>
        {
            base
        }
        Some(_) => return None,
        None => datetime,
    };
    let bytes = datetime.as_bytes();
    if bytes.len() != 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }

    let year = parse_digits(&bytes[0..4])?;
    let month = parse_digits(&bytes[5..7])?;
    let day = parse_digits(&bytes[8..10])?;
    let hour = parse_digits(&bytes[11..13])?;
    let minute = parse_digits(&bytes[14..16])?;
    let second = parse_digits(&bytes[17..19])?;
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }

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

    let local_seconds = days
        .checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)?;
    u64::try_from(
        i64::try_from(local_seconds)
            .ok()?
            .checked_sub(offset_seconds)?,
    )
    .ok()
}

fn split_timezone(s: &str) -> Option<(&str, i64)> {
    if let Some(datetime) = s.strip_suffix('Z') {
        return Some((datetime, 0));
    }

    if s.len() >= 25 {
        let offset_start = s.len() - 6;
        let offset = &s.as_bytes()[offset_start..];
        if matches!(offset[0], b'+' | b'-') && offset[3] == b':' {
            let hours = parse_digits(&offset[1..3])?;
            let minutes = parse_digits(&offset[4..6])?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let seconds = i64::try_from(hours * 3_600 + minutes * 60).ok()?;
            return Some((
                &s[..offset_start],
                if offset[0] == b'+' { seconds } else { -seconds },
            ));
        }
    }

    Some((s, 0))
}

fn parse_digits(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    bytes.iter().try_fold(0_u64, |value, byte| {
        value.checked_mul(10)?.checked_add(u64::from(byte - b'0'))
    })
}

fn days_in_month(year: u64, month: u64) -> u64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    }
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
    if data.stale {
        return false;
    }
    let now = SystemTime::now();
    let past = |s: &UsageSection| matches!(s.resets_at, Some(t) if now.duration_since(t).is_ok());
    past(&data.session) || past(&data.weekly) || data.fable.as_ref().is_some_and(past)
}

pub fn app_is_past_reset(data: &AppUsageData) -> bool {
    data.iter().any(|(_, usage)| is_past_reset(usage))
}

#[cfg(test)]
mod tests;
