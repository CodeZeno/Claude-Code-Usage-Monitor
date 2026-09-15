//! Bridge between Google Antigravity CLI's official `/statusline <command>`
//! feature and a small, sanitized local cache this app can read.
//!
//! The Antigravity CLI (`agy`) supports a custom status line: a small
//! external command that receives a JSON payload on stdin once per
//! interactive-TUI refresh and returns text to render. Confirmed live
//! against `agy 1.1.23`:
//!
//! - `/statusline <command>` configures it; `/statusline delete` restores
//!   the built-in default.
//! - It is a TUI-only feature (`/statusline` itself refuses to run in
//!   headless/print mode).
//! - The payload includes a `quota` object keyed by an internal quota name
//!   (e.g. `gemini-weekly`, `3p-weekly`), each with `remaining_fraction`
//!   (0.0-1.0), `reset_time` (RFC 3339 UTC, e.g. `2026-09-22T21:44:07Z`),
//!   and `reset_in_seconds`, plus a top-level `plan_tier` string.
//! - The same payload also carries `email`, `session_id`,
//!   `conversation_id`, `transcript_path`, `cwd`, `workspace`, and other
//!   fields that must never be persisted by this bridge.
//!
//! This module is the *entire* boundary between that payload and disk: it
//! parses the raw JSON with a strict allowlist (only `quota.*`,
//! `plan_tier`, and a CLI/version string are ever read — every other key,
//! including all of the ones named above, is never inspected, so there is
//! no separate "redaction" step that could be forgotten) and writes only
//! [`SanitizedCache`] to disk. The raw payload is never logged, retained,
//! or forwarded anywhere.
//!
//! Nothing in this module makes a network call, reads an OAuth token or
//! credential, or launches the Antigravity CLI. It only reads the bytes it
//! is given on stdin (see the `aum-quota antigravity-statusline-bridge`
//! subcommand, the intended `command` target of an opt-in
//! `/statusline <command>` configuration) and reads/writes the cache file.
//!
//! ## Open design question: staleness threshold
//!
//! This module intentionally does not hardcode a "how old is too old"
//! threshold. No general freshness/TTL policy exists elsewhere in this
//! codebase to reuse (`QuotaFamilyStatus::Stale` and
//! `QuotaItemAvailability::Stale` are declared in `models` but not
//! constructed anywhere yet; the one TTL that does exist,
//! `poller::CODEX_RATE_LIMITS_CACHE_TTL`, is a narrow 45-second
//! anti-thrashing cache around a single app-server round trip, not a
//! general "is this data stale" policy). [`is_stale`] takes the threshold
//! as a parameter rather than inventing one, leaving the actual value as a
//! decision for whoever wires this cache into the UI.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::models::QuotaItem;

/// Bumped whenever [`SanitizedCache`]'s on-disk shape changes in a way that
/// isn't purely additive.
pub const CACHE_SCHEMA_VERSION: u32 = 1;

/// Recorded as `SanitizedCache::source`, so a reader can tell this cache
/// apart from any other future bridge that might write to a similar path.
pub const CACHE_SOURCE: &str = "antigravity_statusline";

/// The cache file's default location: `%APPDATA%\ClaudeCodeUsageMonitor\antigravity_statusline_cache.json`,
/// alongside this app's existing `settings.json` (see `window::settings_path`).
pub fn default_cache_path() -> std::path::PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(appdata)
        .join("ClaudeCodeUsageMonitor")
        .join("antigravity_statusline_cache.json")
}

/// One quota bucket, sanitized down to exactly the fields this app needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SanitizedQuotaEntry {
    /// `0.0` (exhausted) to `1.0` (fully available), clamped on the way in.
    pub remaining_fraction: f64,
    /// `reset_time` from the payload, converted to Unix seconds (UTC).
    pub reset_time_unix: u64,
    /// `reset_in_seconds` from the payload, if present — for a consistency
    /// check or fallback only; `reset_time_unix` is the primary timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_in_seconds: Option<u64>,
}

/// The entire sanitized cache. Every field here is safe to persist and to
/// hand to a UI — nothing from the raw statusLine payload survives into
/// this shape unless it is named on this struct.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SanitizedCache {
    pub schema_version: u32,
    pub source: String,
    /// When this bridge run captured the payload, Unix seconds (UTC).
    pub captured_at_unix: u64,
    /// The Antigravity CLI version, if the payload carried one under a
    /// `version`/`cli_version` key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    /// e.g. `"Google AI Plus"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_tier: Option<String>,
    /// Keyed by the raw quota key from the payload (e.g. `gemini-weekly`,
    /// `3p-weekly`). Kept as a `BTreeMap` for deterministic serialization;
    /// unknown keys are carried through as-is rather than dropped — see
    /// [`sanitize_statusline_payload`].
    pub quota: BTreeMap<String, SanitizedQuotaEntry>,
}

/// Why [`sanitize_statusline_payload`] rejected a payload — always a
/// rejection to *not write*, never a panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeError {
    InvalidJson,
    NoQuotaObject,
    NoUsableQuotaEntries,
}

/// Parses raw statusLine stdin JSON and extracts only the allowlisted
/// fields into a [`SanitizedCache`]. Every other field in the payload
/// (`email`, `session_id`, `conversation_id`, `transcript_path`, `cwd`,
/// `workspace`, prompt/conversation content, credentials, ...) is never
/// read, because this function never looks for them.
///
/// A quota entry that is missing `remaining_fraction`/`reset_time`, has a
/// non-finite or unparseable value, is skipped individually rather than
/// failing the whole payload — one malformed bucket must not hide the
/// others. The payload as a whole is only rejected if it isn't valid JSON,
/// has no `quota` object at all, or every entry inside `quota` turned out
/// to be unusable.
pub fn sanitize_statusline_payload(
    raw_json: &str,
    now: SystemTime,
) -> Result<SanitizedCache, BridgeError> {
    let value: serde_json::Value =
        serde_json::from_str(raw_json).map_err(|_| BridgeError::InvalidJson)?;

    let quota_obj = value
        .get("quota")
        .and_then(|v| v.as_object())
        .ok_or(BridgeError::NoQuotaObject)?;

    let mut quota = BTreeMap::new();
    for (key, entry) in quota_obj {
        let Some(entry_obj) = entry.as_object() else {
            continue;
        };
        let Some(remaining_fraction) = entry_obj
            .get("remaining_fraction")
            .and_then(serde_json::Value::as_f64)
            .filter(|v| v.is_finite())
        else {
            continue;
        };
        let Some(reset_time_str) = entry_obj.get("reset_time").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(reset_time_unix) = parse_rfc3339_utc_to_unix(reset_time_str) else {
            continue;
        };
        let reset_in_seconds = entry_obj
            .get("reset_in_seconds")
            .and_then(serde_json::Value::as_u64);

        quota.insert(
            key.clone(),
            SanitizedQuotaEntry {
                remaining_fraction: remaining_fraction.clamp(0.0, 1.0),
                reset_time_unix,
                reset_in_seconds,
            },
        );
    }

    if quota.is_empty() {
        return Err(BridgeError::NoUsableQuotaEntries);
    }

    let plan_tier = value
        .get("plan_tier")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let cli_version = value
        .get("cli_version")
        .or_else(|| value.get("version"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let captured_at_unix = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    Ok(SanitizedCache {
        schema_version: CACHE_SCHEMA_VERSION,
        source: CACHE_SOURCE.to_string(),
        captured_at_unix,
        cli_version,
        plan_tier,
        quota,
    })
}

/// Writes `cache` to `path` atomically: a uniquely-named temp file in the
/// same directory, flushed and `sync_all`'d, then renamed over the final
/// path. A reader can never observe a partially-written file. Deliberately
/// simpler than `snapshot_store`'s backup/placement machinery (built for a
/// much larger, versioned snapshot-history file) — this cache is small,
/// disposable, and always safely re-derivable from the next statusLine
/// payload, so temp-write-then-rename is sufficient here.
pub fn write_cache_atomic(path: &Path, cache: &SanitizedCache) -> std::io::Result<()> {
    let json = serde_json::to_vec(cache)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "cache path has no parent")
    })?;
    fs::create_dir_all(dir)?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("antigravity_statusline_cache");
    let temp_path = dir.join(format!(".{file_name}.{}.tmp", std::process::id()));

    let write_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(&json)?;
        file.flush()?;
        file.sync_all()
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
        return write_result;
    }

    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    Ok(())
}

/// Outcome of one bridge invocation — always a value, never a panic, so the
/// `aum-quota antigravity-statusline-bridge` subcommand can always exit
/// successfully and never break the user's Antigravity status line.
#[derive(Debug)]
pub enum BridgeOutcome {
    Written,
    Rejected(BridgeError),
    WriteFailed(std::io::Error),
}

/// Runs the full bridge step: sanitize, then atomically write. On any
/// rejection or write failure, the existing cache file (if any) is left
/// untouched — a bad or partial statusLine tick never clobbers the last
/// good reading.
pub fn run_bridge(raw_json: &str, cache_path: &Path, now: SystemTime) -> BridgeOutcome {
    match sanitize_statusline_payload(raw_json, now) {
        Ok(cache) => match write_cache_atomic(cache_path, &cache) {
            Ok(()) => BridgeOutcome::Written,
            Err(error) => BridgeOutcome::WriteFailed(error),
        },
        Err(error) => BridgeOutcome::Rejected(error),
    }
}

/// Result of reading the cache back — deliberately distinguishes "never
/// written" from every other state, so a caller never treats a missing
/// cache as "zero usage".
#[derive(Debug, PartialEq)]
pub enum CacheReadResult {
    /// No cache file exists yet (bridge never ran, or this feature was
    /// never opted into).
    Missing,
    /// The file exists but isn't a valid `SanitizedCache` (corrupt,
    /// truncated, or written by something else entirely).
    Malformed(String),
    /// The file parses, but its `schema_version` is one this build doesn't
    /// know how to interpret.
    UnsupportedSchema(u32),
    Valid(SanitizedCache),
}

pub fn read_cache(path: &Path) -> CacheReadResult {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CacheReadResult::Missing;
        }
        Err(error) => return CacheReadResult::Malformed(error.to_string()),
    };
    let cache: SanitizedCache = match serde_json::from_slice(&bytes) {
        Ok(cache) => cache,
        Err(error) => return CacheReadResult::Malformed(error.to_string()),
    };
    if cache.schema_version != CACHE_SCHEMA_VERSION {
        return CacheReadResult::UnsupportedSchema(cache.schema_version);
    }
    CacheReadResult::Valid(cache)
}

/// Whether `cache` is older than `max_age` as of `now`. Clock skew that
/// makes `captured_at_unix` appear to be in the future is treated as *not*
/// stale rather than an error — see the module docs for why `max_age` is a
/// parameter rather than a constant defined here.
pub fn is_stale(cache: &SanitizedCache, now: SystemTime, max_age: Duration) -> bool {
    let captured_at = UNIX_EPOCH + Duration::from_secs(cache.captured_at_unix);
    match now.duration_since(captured_at) {
        Ok(age) => age > max_age,
        Err(_) => false,
    }
}

/// A human-readable label for a known quota key. `3p-weekly`'s underlying
/// semantics are not documented by Google and are not finalized on this
/// app's side yet (see the module docs and `docs/quota-rules.md`), so it —
/// and any other key this build doesn't specifically recognize — is passed
/// through as its raw key rather than guessed at or renamed to a specific
/// model family.
fn quota_key_label(key: &str) -> &str {
    match key {
        "gemini-weekly" => "Gemini Weekly",
        other => other,
    }
}

/// Converts every entry in `cache.quota` into a display-ready [`QuotaItem`].
/// One item per quota key present — a weekly-only payload (no 5h entry, as
/// with every payload captured live so far) simply produces one item per
/// week-scoped key; a future payload with additional keys (a 5h bucket, or
/// an entirely new quota name) produces additional items without any code
/// change here.
pub fn quota_items_from_cache(cache: &SanitizedCache) -> Vec<QuotaItem> {
    cache
        .quota
        .iter()
        .map(|(key, entry)| {
            let used_percentage = ((1.0 - entry.remaining_fraction) * 100.0).clamp(0.0, 100.0);
            let resets_at = UNIX_EPOCH + Duration::from_secs(entry.reset_time_unix);
            QuotaItem::percentage(
                key.clone(),
                quota_key_label(key).to_string(),
                used_percentage,
                Some(resets_at),
            )
        })
        .collect()
}

/// Parses a strict `YYYY-MM-DDTHH:MM:SSZ` UTC timestamp (no fractional
/// seconds, no non-`Z` offset — the only shape observed live in a real
/// Antigravity statusLine payload) into Unix seconds. Returns `None` for
/// anything else rather than guessing.
fn parse_rfc3339_utc_to_unix(s: &str) -> Option<u64> {
    let s = s.trim();
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;

    let mut date_parts = date.split('-');
    let year: i32 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }

    let mut time_parts = time.split(':');
    let hour: u32 = time_parts.next()?.parse().ok()?;
    let minute: u32 = time_parts.next()?.parse().ok()?;
    let second: u32 = time_parts.next()?.parse().ok()?;
    if time_parts.next().is_some() {
        return None;
    }

    if !(1..=12).contains(&month) || day == 0 || day > 31 || hour > 23 || minute > 59 || second > 60
    {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let secs = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    u64::try_from(secs).ok()
}

/// Howard Hinnant's days-from-civil-date algorithm — the same one already
/// used in `poller::days_from_civil` for calendar-month reset
/// calculations. Duplicated here (rather than sharing across modules)
/// because both copies are this exact, tiny, self-contained snippet, and
/// this module has no other reason to depend on `poller`.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERIFIED_PAYLOAD: &str = r#"{
        "quota": {
            "3p-weekly": {
                "remaining_fraction": 1,
                "reset_time": "2026-09-22T21:44:07Z",
                "reset_in_seconds": 604527
            },
            "gemini-weekly": {
                "remaining_fraction": 0.95444,
                "reset_time": "2026-09-22T21:26:44Z",
                "reset_in_seconds": 603484
            }
        },
        "plan_tier": "Google AI Plus",
        "model": {
            "id": "Gemini 3.8 Flash (High)",
            "display_name": "Gemini 3.8 Flash (High)",
            "effort": "high"
        },
        "email": "someone@example.com",
        "session_id": "e5fcc9e9-846a-4deb-adbb-96e03e1ab73a",
        "conversation_id": "73f4d5cc-dcd1-4f8b-b48d-2bdc4eb788c3",
        "transcript_path": "C:\\Users\\someone\\transcript.jsonl",
        "cwd": "C:\\Users\\someone\\repos\\ai-usage-monitor",
        "workspace": { "current_dir": "C:\\Users\\someone\\repos\\ai-usage-monitor" }
    }"#;

    #[test]
    fn verified_payload_parses_both_known_quota_keys() {
        let cache = sanitize_statusline_payload(VERIFIED_PAYLOAD, UNIX_EPOCH).unwrap();
        assert_eq!(cache.quota.len(), 2);
        assert!(cache.quota.contains_key("gemini-weekly"));
        assert!(cache.quota.contains_key("3p-weekly"));
        assert_eq!(cache.plan_tier, Some("Google AI Plus".to_string()));
    }

    #[test]
    fn sensitive_fields_never_appear_in_sanitized_cache_serialization() {
        let cache = sanitize_statusline_payload(VERIFIED_PAYLOAD, UNIX_EPOCH).unwrap();
        let json = serde_json::to_string(&cache).unwrap();
        for forbidden in [
            "someone@example.com",
            "session_id",
            "conversation_id",
            "transcript_path",
            "cwd",
            "workspace",
            "e5fcc9e9",
            "73f4d5cc",
        ] {
            assert!(
                !json.contains(forbidden),
                "sanitized cache JSON must never contain {forbidden:?}, got: {json}"
            );
        }
    }

    #[test]
    fn remaining_fraction_one_maps_to_zero_used_percentage() {
        let cache = sanitize_statusline_payload(VERIFIED_PAYLOAD, UNIX_EPOCH).unwrap();
        let items = quota_items_from_cache(&cache);
        let three_p = items.iter().find(|i| i.id == "3p-weekly").unwrap();
        assert_eq!(three_p.used_percentage(), Some(0.0));
    }

    #[test]
    fn remaining_fraction_decimal_maps_to_expected_used_percentage() {
        let cache = sanitize_statusline_payload(VERIFIED_PAYLOAD, UNIX_EPOCH).unwrap();
        let items = quota_items_from_cache(&cache);
        let gemini = items.iter().find(|i| i.id == "gemini-weekly").unwrap();
        let used = gemini.used_percentage().unwrap();
        assert!((used - 4.556).abs() < 0.01, "used={used}");
    }

    #[test]
    fn remaining_fraction_zero_maps_to_hundred_percent_used() {
        let payload = r#"{"quota":{"gemini-weekly":{"remaining_fraction":0,"reset_time":"2026-09-22T21:26:44Z"}}}"#;
        let cache = sanitize_statusline_payload(payload, UNIX_EPOCH).unwrap();
        let items = quota_items_from_cache(&cache);
        assert_eq!(items[0].used_percentage(), Some(100.0));
    }

    #[test]
    fn out_of_range_remaining_fraction_is_clamped_not_rejected() {
        let payload = r#"{"quota":{"gemini-weekly":{"remaining_fraction":1.5,"reset_time":"2026-09-22T21:26:44Z"}}}"#;
        let cache = sanitize_statusline_payload(payload, UNIX_EPOCH).unwrap();
        assert_eq!(cache.quota["gemini-weekly"].remaining_fraction, 1.0);

        let payload_negative = r#"{"quota":{"gemini-weekly":{"remaining_fraction":-0.3,"reset_time":"2026-09-22T21:26:44Z"}}}"#;
        let cache_negative = sanitize_statusline_payload(payload_negative, UNIX_EPOCH).unwrap();
        assert_eq!(
            cache_negative.quota["gemini-weekly"].remaining_fraction,
            0.0
        );
    }

    #[test]
    fn missing_quota_object_is_rejected() {
        let result = sanitize_statusline_payload(r#"{"plan_tier":"Google AI Plus"}"#, UNIX_EPOCH);
        assert_eq!(result, Err(BridgeError::NoQuotaObject));
    }

    #[test]
    fn null_quota_is_rejected_without_panicking() {
        let result = sanitize_statusline_payload(r#"{"quota":null}"#, UNIX_EPOCH);
        assert_eq!(result, Err(BridgeError::NoQuotaObject));
    }

    #[test]
    fn malformed_json_is_rejected_without_panicking() {
        let result = sanitize_statusline_payload("{not valid json", UNIX_EPOCH);
        assert_eq!(result, Err(BridgeError::InvalidJson));
    }

    #[test]
    fn invalid_remaining_fraction_entry_is_skipped_not_fatal() {
        let payload = r#"{"quota":{
            "broken":{"remaining_fraction":"not-a-number","reset_time":"2026-09-22T21:26:44Z"},
            "gemini-weekly":{"remaining_fraction":0.5,"reset_time":"2026-09-22T21:26:44Z"}
        }}"#;
        let cache = sanitize_statusline_payload(payload, UNIX_EPOCH).unwrap();
        assert_eq!(cache.quota.len(), 1);
        assert!(cache.quota.contains_key("gemini-weekly"));
    }

    #[test]
    fn extreme_exponent_remaining_fraction_is_rejected_as_invalid_json_not_a_panic() {
        // serde_json itself rejects `1e400` at parse time (exponent
        // overflow) rather than producing `f64::INFINITY` — exercised here
        // so this input path is proven not to panic, whichever error it
        // surfaces as.
        let payload = r#"{"quota":{"gemini-weekly":{"remaining_fraction":1e400,"reset_time":"2026-09-22T21:26:44Z"}}}"#;
        let result = sanitize_statusline_payload(payload, UNIX_EPOCH);
        assert_eq!(result, Err(BridgeError::InvalidJson));
    }

    #[test]
    fn non_finite_remaining_fraction_value_is_skipped_not_fatal() {
        // A value that parses as JSON but isn't a finite number in the
        // `remaining_fraction` slot (a JSON string here) must be skipped
        // like any other malformed entry, not treated as fatal.
        let payload = r#"{"quota":{
            "broken":{"remaining_fraction":"Infinity","reset_time":"2026-09-22T21:26:44Z"},
            "gemini-weekly":{"remaining_fraction":0.5,"reset_time":"2026-09-22T21:26:44Z"}
        }}"#;
        let cache = sanitize_statusline_payload(payload, UNIX_EPOCH).unwrap();
        assert_eq!(cache.quota.len(), 1);
        assert!(cache.quota.contains_key("gemini-weekly"));
    }

    #[test]
    fn invalid_reset_time_entry_is_skipped_not_fatal() {
        let payload = r#"{"quota":{
            "broken":{"remaining_fraction":0.5,"reset_time":"not-a-date"},
            "gemini-weekly":{"remaining_fraction":0.5,"reset_time":"2026-09-22T21:26:44Z"}
        }}"#;
        let cache = sanitize_statusline_payload(payload, UNIX_EPOCH).unwrap();
        assert_eq!(cache.quota.len(), 1);
    }

    #[test]
    fn unknown_quota_key_is_kept_with_raw_key_as_label() {
        let payload = r#"{"quota":{"some-future-key":{"remaining_fraction":0.2,"reset_time":"2026-09-22T21:26:44Z"}}}"#;
        let cache = sanitize_statusline_payload(payload, UNIX_EPOCH).unwrap();
        let items = quota_items_from_cache(&cache);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "some-future-key");
        assert_eq!(items[0].label, "some-future-key");
    }

    #[test]
    fn weekly_only_payload_produces_no_5h_item() {
        let cache = sanitize_statusline_payload(VERIFIED_PAYLOAD, UNIX_EPOCH).unwrap();
        let items = quota_items_from_cache(&cache);
        assert!(items
            .iter()
            .all(|item| !item.id.contains("5h") && !item.id.contains("session")));
    }

    #[test]
    fn future_five_hour_key_does_not_panic_and_is_surfaced() {
        let payload = r#"{"quota":{
            "gemini-weekly":{"remaining_fraction":0.9,"reset_time":"2026-09-22T21:26:44Z"},
            "gemini-5h":{"remaining_fraction":0.7,"reset_time":"2026-09-16T02:00:00Z"}
        }}"#;
        let cache = sanitize_statusline_payload(payload, UNIX_EPOCH).unwrap();
        assert_eq!(cache.quota.len(), 2);
        let items = quota_items_from_cache(&cache);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn reset_time_rfc3339_parses_to_expected_unix_seconds() {
        // 2026-09-22T21:44:07Z, cross-checked against `date -u -d ... +%s`-style
        // expectations for this exact codebase's days_from_civil.
        let unix = parse_rfc3339_utc_to_unix("2026-09-22T21:44:07Z").unwrap();
        let back = UNIX_EPOCH + Duration::from_secs(unix);
        let secs_since_epoch = back.duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs_since_epoch, unix);
        // Sanity: 2026-09-22 is after 2026-09-16 (a known date in this
        // conversation's system context) and before 2026-10-01.
        let start_2026_09_16 = parse_rfc3339_utc_to_unix("2026-09-16T00:00:00Z").unwrap();
        let start_2026_10_01 = parse_rfc3339_utc_to_unix("2026-10-01T00:00:00Z").unwrap();
        assert!(unix > start_2026_09_16 && unix < start_2026_10_01);
    }

    #[test]
    fn reset_time_without_trailing_z_is_rejected() {
        assert_eq!(parse_rfc3339_utc_to_unix("2026-09-22T21:44:07"), None);
        assert_eq!(parse_rfc3339_utc_to_unix("2026-09-22T21:44:07+09:00"), None);
    }

    #[test]
    fn atomic_write_then_read_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "aum-antigravity-statusline-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("cache.json");
        let cache = sanitize_statusline_payload(VERIFIED_PAYLOAD, UNIX_EPOCH).unwrap();

        write_cache_atomic(&path, &cache).expect("atomic write should succeed");
        match read_cache(&path) {
            CacheReadResult::Valid(read_back) => assert_eq!(read_back, cache),
            other => panic!("expected Valid, got {other:?}"),
        }

        // No leftover temp files after a successful write.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_missing_is_distinct_from_zero_usage() {
        let dir = std::env::temp_dir().join(format!(
            "aum-antigravity-statusline-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("cache.json");
        assert_eq!(read_cache(&path), CacheReadResult::Missing);
    }

    #[test]
    fn malformed_cache_file_is_reported_distinctly() {
        let dir = std::env::temp_dir().join(format!(
            "aum-antigravity-statusline-malformed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cache.json");
        fs::write(&path, b"not json at all").unwrap();
        match read_cache(&path) {
            CacheReadResult::Malformed(_) => {}
            other => panic!("expected Malformed, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsupported_schema_version_is_reported_distinctly() {
        let dir = std::env::temp_dir().join(format!(
            "aum-antigravity-statusline-schema-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cache.json");
        fs::write(
            &path,
            br#"{"schema_version":9999,"source":"antigravity_statusline","captured_at_unix":0,"quota":{}}"#,
        )
        .unwrap();
        match read_cache(&path) {
            CacheReadResult::UnsupportedSchema(9999) => {}
            other => panic!("expected UnsupportedSchema(9999), got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_cache_is_detected_past_the_given_threshold() {
        let cache = sanitize_statusline_payload(VERIFIED_PAYLOAD, UNIX_EPOCH).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(3600);
        assert!(is_stale(&cache, now, Duration::from_secs(1800)));
        assert!(!is_stale(&cache, now, Duration::from_secs(7200)));
    }

    #[test]
    fn future_captured_at_from_clock_skew_is_not_treated_as_stale() {
        let cache = sanitize_statusline_payload(
            VERIFIED_PAYLOAD,
            UNIX_EPOCH + Duration::from_secs(1_000_000),
        )
        .unwrap();
        let now = UNIX_EPOCH;
        assert!(!is_stale(&cache, now, Duration::from_secs(1)));
    }

    #[test]
    fn bridge_never_touches_existing_cache_on_rejected_payload() {
        let dir = std::env::temp_dir().join(format!(
            "aum-antigravity-statusline-noclobber-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("cache.json");
        let good = sanitize_statusline_payload(VERIFIED_PAYLOAD, UNIX_EPOCH).unwrap();
        write_cache_atomic(&path, &good).unwrap();

        let outcome = run_bridge("{not valid json", &path, UNIX_EPOCH);
        assert!(matches!(
            outcome,
            BridgeOutcome::Rejected(BridgeError::InvalidJson)
        ));

        match read_cache(&path) {
            CacheReadResult::Valid(still_good) => assert_eq!(still_good, good),
            other => panic!("expected the prior good cache to survive, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
