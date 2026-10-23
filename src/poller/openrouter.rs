use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::{build_agent, PollError};
use crate::app_settings::BudgetPeriod;
use crate::diagnose;
use crate::models::{CreditsSection, UsageData, UsageLimit, UsageSection};

/// OpenRouter management endpoints. A management key reads account-level data;
/// an optional API key additionally exposes the key's own usage via `/key`.
const BASE_URL: &str = "https://openrouter.ai/api/v1";
const MANAGEMENT_KEY_ENV: &str = "OPENROUTER_MANAGEMENT_KEY";
const API_KEY_ENV: &str = "OPENROUTER_API_KEY";

/// Windows Credential Manager target for the OpenRouter management key.
pub const MANAGEMENT_KEY_TARGET: &str = "ClaudeCodeUsageMonitor/OpenRouter/management-key";
const SECONDS_PER_DAY: i64 = 86_400;

/// `GET /credits` -> account balance and lifetime usage.
#[derive(Deserialize)]
struct CreditsResponse {
    data: CreditsData,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CreditsData {
    #[serde(default)]
    total_credits: f64,
    #[serde(default)]
    total_usage: f64,
}

/// `GET /activity` -> one entry per completed UTC day (today excluded).
#[derive(Deserialize)]
struct ActivityResponse {
    #[serde(default)]
    data: Vec<ActivityEntry>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ActivityEntry {
    #[serde(default)]
    date: String,
    #[serde(default)]
    usage: f64,
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    reasoning_tokens: u64,
    #[serde(default)]
    cached_tokens: u64,
    #[serde(default)]
    requests: u64,
}

/// `GET /key` -> per-window spend for the key that made the request.
#[derive(Deserialize)]
struct KeyResponse {
    data: KeyData,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct KeyData {
    /// Only today's per-key spend is used now: `/activity` (management key)
    /// already reports the account-wide weekly/monthly windows.
    #[serde(default)]
    usage_daily: f64,
}

struct Credentials {
    management_key: String,
    api_key: Option<String>,
}

/// Cheap connectivity probe for the settings UI: validates a management key and
/// returns `(total_credits, total_usage)`.
pub fn probe_management_key(key: &str) -> Result<(f64, f64), PollError> {
    let response = fetch_credits(key)?;
    Ok((response.data.total_credits, response.data.total_usage))
}

pub(super) fn poll_openrouter() -> Result<UsageData, PollError> {
    let credentials = read_credentials().ok_or_else(|| {
        diagnose::log("OpenRouter usage poll failed: no management key found");
        PollError::NoCredentials
    })?;
    poll_account(&credentials)
}

/// Fingerprints only: secrets are hashed so a change can be detected without
/// ever logging the key itself.
pub(super) fn credential_watch_snapshot(_all_sources: bool) -> Vec<String> {
    vec![
        secret_signature(MANAGEMENT_KEY_ENV),
        secret_signature(API_KEY_ENV),
    ]
}

fn poll_account(credentials: &Credentials) -> Result<UsageData, PollError> {
    let credits = fetch_credits(&credentials.management_key)?;
    let activity = fetch_activity(&credentials.management_key)?;
    let key = match &credentials.api_key {
        Some(api_key) => match fetch_key(api_key) {
            Ok(key) => Some(key),
            Err(error) => {
                // The management key already answered; a key without the
                // right scope (or a revoked key) must not blank the account.
                diagnose::log(format!(
                    "OpenRouter /key poll failed; continuing without daily spend: {error:?}"
                ));
                None
            }
        },
        None => None,
    };
    let now = SystemTime::now();
    let days = today_days(now);
    // Analytics reports live spend for every window; /activity only has
    // completed days and /key's usage_daily lags. A single day-granularity query
    // spanning the wider of the week/month windows feeds all three.
    let (week_start, _) = week_bounds(days);
    let (month_start, _) = month_bounds(days);
    let analytics = fetch_daily_usage(
        &credentials.management_key,
        week_start.min(month_start),
        days,
    );
    Ok(usage_from_account(
        &credits.data,
        &activity.data,
        key.as_ref().map(|key| &key.data),
        budget_config(),
        days,
        analytics.as_deref(),
    ))
}

/// OpenRouter budget configuration from the app's settings file.
#[derive(Clone, Copy, Debug)]
struct BudgetConfig {
    amount: Option<f64>,
    budget_period: BudgetPeriod,
    monitor_period: BudgetPeriod,
    show_cap: bool,
    month_deposit: Option<f64>,
    show_deposit: bool,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            amount: None,
            budget_period: BudgetPeriod::Monthly,
            monitor_period: BudgetPeriod::Monthly,
            show_cap: false,
            month_deposit: None,
            show_deposit: false,
        }
    }
}

/// Cache of the `/credits` total captured on the first poll of a UTC day, used
/// to derive live "today" spend.
fn day_baseline_path() -> std::path::PathBuf {
    crate::app_settings::settings_path().with_file_name("openrouter-day.json")
}

fn live_today_spend(total_usage: f64, today_days: i64) -> f64 {
    let path = day_baseline_path();
    let stored = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let same_day = stored
        .as_ref()
        .and_then(|value| value.get("day"))
        .and_then(serde_json::Value::as_i64)
        == Some(today_days);
    if same_day {
        let base = stored
            .and_then(|value| value.get("usage").and_then(serde_json::Value::as_f64))
            .unwrap_or(total_usage);
        (total_usage - base).max(0.0)
    } else {
        let snapshot = serde_json::json!({ "day": today_days, "usage": total_usage });
        let _ = std::fs::write(&path, snapshot.to_string());
        0.0
    }
}

fn budget_config() -> BudgetConfig {
    let Ok(text) = std::fs::read_to_string(crate::app_settings::settings_path()) else {
        return BudgetConfig::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return BudgetConfig::default();
    };
    BudgetConfig {
        amount: value
            .get("openrouter_budget_amount")
            .and_then(serde_json::Value::as_f64)
            .filter(|amount| amount.is_finite() && *amount > 0.0),
        budget_period: value
            .get("openrouter_budget_period")
            .and_then(|raw| serde_json::from_value(raw.clone()).ok())
            .unwrap_or_default(),
        monitor_period: value
            .get("openrouter_monitor_period")
            .and_then(|raw| serde_json::from_value(raw.clone()).ok())
            .unwrap_or_default(),
        show_cap: value
            .get("openrouter_show_cap")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
        month_deposit: value
            .get("openrouter_monthly_deposit")
            .and_then(serde_json::Value::as_f64)
            .filter(|deposit| deposit.is_finite() && *deposit > 0.0),
        show_deposit: value
            .get("openrouter_show_deposit")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    }
}

fn fetch_credits(management_key: &str) -> Result<CreditsResponse, PollError> {
    get_json(&format!("{BASE_URL}/credits"), management_key, "credits")
}

fn fetch_activity(management_key: &str) -> Result<ActivityResponse, PollError> {
    get_json(&format!("{BASE_URL}/activity"), management_key, "activity")
}

fn fetch_key(api_key: &str) -> Result<KeyResponse, PollError> {
    get_json(&format!("{BASE_URL}/key"), api_key, "key")
}

/// Live per-day spend (UTC day index -> USD) from the analytics API.
///
/// One day-granularity query spanning the wider of the week/month windows feeds
/// today, the current week and the current month, all in real time. Failures
/// degrade to `None` so a degraded analytics endpoint falls back to /activity.
fn fetch_daily_usage(management_key: &str, start_days: i64, today: i64) -> Option<Vec<(i64, f64)>> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let seconds = now % SECONDS_PER_DAY as u64;
    let (start_year, start_month, start_day) = civil_from_days(start_days);
    let (end_year, end_month, end_day) = civil_from_days(today);
    let body = serde_json::json!({
        "metrics": ["total_usage"],
        "granularity": "day",
        "time_range": {
            "start": format!("{start_year:04}-{start_month:02}-{start_day:02}T00:00:00Z"),
            "end": format!(
                "{end_year:04}-{end_month:02}-{end_day:02}T{:02}:{:02}:{:02}Z",
                seconds / 3600,
                (seconds % 3600) / 60,
                seconds % 60
            ),
        },
    });
    let response: serde_json::Value = post_json(
        &format!("{BASE_URL}/analytics/query"),
        management_key,
        &body,
        "/analytics/query",
    )
    .ok()?;
    let rows = response.get("data")?.get("data")?.as_array()?;
    Some(
        rows.iter()
            .filter_map(|row| {
                let day = date_days(row.get("date__day")?.as_str()?)?;
                let usage = row.get("total_usage").and_then(serde_json::Value::as_f64)?;
                Some((day, usage))
            })
            .collect(),
    )
}

fn post_json<T: for<'de> Deserialize<'de>>(
    url: &str,
    token: &str,
    body: &serde_json::Value,
    endpoint: &str,
) -> Result<T, PollError> {
    let mut response = match build_agent()?
        .post(url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/json")
        .send_json(body)
        .and_then(super::check_http_status)
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401 | 403)) => return Err(PollError::AuthRequired),
        Err(error) => {
            diagnose::log_error(&format!("OpenRouter {endpoint} request failed"), error);
            return Err(PollError::RequestFailed);
        }
    };
    response.body_mut().read_json::<T>().map_err(|error| {
        diagnose::log_error(
            &format!("OpenRouter {endpoint} response is not valid JSON"),
            error,
        );
        PollError::RequestFailed
    })
}

fn get_json<T: for<'de> Deserialize<'de>>(
    url: &str,
    token: &str,
    endpoint: &str,
) -> Result<T, PollError> {
    let mut response = match build_agent()?
        .get(url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/json")
        .call()
        .and_then(super::check_http_status)
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401 | 403)) => return Err(PollError::AuthRequired),
        Err(error) => {
            diagnose::log_error(&format!("OpenRouter /{endpoint} request failed"), error);
            return Err(PollError::RequestFailed);
        }
    };

    response.body_mut().read_json::<T>().map_err(|error| {
        diagnose::log_error(
            &format!("OpenRouter /{endpoint} response is not valid JSON"),
            error,
        );
        PollError::RequestFailed
    })
}

/// Map OpenRouter account data into the shared usage model.
///
/// OpenRouter has no five-hour window, so nothing claims the `session` slot.
/// The balance lands in `credits` (currency), while calendar-week and
/// calendar-month spend become the `weekly`/`monthly` gauges, normalised
/// against the account's total credits so the percentages stay meaningful.
/// Token counts and today's spend reach themes through opt-in `limits`.
fn usage_from_account(
    credits: &CreditsData,
    activity: &[ActivityEntry],
    key: Option<&KeyData>,
    budget: BudgetConfig,
    today_days: i64,
    analytics: Option<&[(i64, f64)]>,
) -> UsageData {
    let total_credits = credits.total_credits.max(0.0);
    let balance = (credits.total_credits - credits.total_usage).max(0.0);

    let (week_start, next_week) = week_bounds(today_days);
    let (month_start, next_month) = month_bounds(today_days);
    let month_days = (next_month - month_start).max(1) as u32;

    // Analytics gives live spend for every window. Without it, fall back to the
    // completed-day activity sums plus a live today (credits delta or /key).
    let (today_spend, week_spend, month_spend) = match analytics {
        Some(rows) => (
            sum_days(rows, today_days, today_days + 1),
            sum_days(rows, week_start, next_week),
            sum_days(rows, month_start, next_month),
        ),
        None => {
            let live_today = live_today_spend(credits.total_usage, today_days);
            let today = live_today.max(key.map(|key| key.usage_daily.max(0.0)).unwrap_or(0.0));
            (
                today,
                sum_usage_between(activity, week_start, next_week) + today,
                sum_usage_between(activity, month_start, next_month) + today,
            )
        }
    };

    // The budget amount is a rate per `budget_period`; convert it to whichever
    // window is monitored, so a monthly cap can be watched day by day.
    let daily_rate = budget
        .amount
        .filter(|amount| *amount > 0.0)
        .map(|amount| amount / budget.budget_period.days(month_days));
    let cap_for = |period: BudgetPeriod| match daily_rate {
        Some(rate) => rate * period.days(month_days),
        None => total_credits,
    };
    let spend_for = |period: BudgetPeriod| match period {
        BudgetPeriod::Daily => today_spend,
        BudgetPeriod::Weekly => week_spend,
        BudgetPeriod::Monthly => month_spend,
    };

    let (monthly_percentage, weekly_percentage, credits_section) = match daily_rate {
        Some(_) => {
            let monitor_spend = spend_for(budget.monitor_period);
            let monitor_cap = cap_for(budget.monitor_period);
            (
                share_of_total(month_spend, cap_for(BudgetPeriod::Monthly)),
                share_of_total(week_spend, cap_for(BudgetPeriod::Weekly)),
                (monitor_cap > 0.0).then_some(CreditsSection {
                    percentage: share_of_total(monitor_spend, monitor_cap),
                    // Not clamped: themes derive `spent` from `total - remaining`,
                    // so this keeps the real spend visible when over the cap (the
                    // gauge percentage still clamps at 100).
                    remaining: monitor_cap - monitor_spend,
                    total: monitor_cap,
                }),
            )
        }
        None => (
            share_of_total(month_spend, total_credits),
            share_of_total(week_spend, total_credits),
            (total_credits > 0.0).then_some(CreditsSection {
                percentage: share_of_total(credits.total_usage, credits.total_credits),
                remaining: balance,
                total: total_credits,
            }),
        ),
    };

    let tokens = token_totals(activity);

    let mut limits = vec![UsageLimit {
        key: "tokens".into(),
        kind: "tokens".into(),
        label: "Tokens".into(),
        model: None,
        model_id: None,
        scope: Some(serde_json::json!({
            "prompt_tokens": tokens.prompt,
            "completion_tokens": tokens.completion,
            "reasoning_tokens": tokens.reasoning,
            "cached_tokens": tokens.cached,
            "requests": tokens.requests,
        })),
        is_active: tokens.prompt + tokens.completion + tokens.reasoning + tokens.cached > 0,
        usage: UsageSection {
            available: true,
            percentage: 0.0,
            resets_at: days_to_system_time(next_month),
        },
    }];

    // Line 1 (daily) and line 2 (weekly or monthly) for the two-line OpenRouter
    // display. Dollar amounts ride in the scope so themes can print them.
    let long_period = budget.monitor_period;
    let long_spend = spend_for(long_period);
    let long_cap = cap_for(long_period);
    let day_cap = cap_for(BudgetPeriod::Daily);
    // A zero/absent amount turns the cap off: the gauge still falls back to the
    // credit total, but the printed cap is hidden and shows 0.
    let display_cap_for = |period: BudgetPeriod| {
        if daily_rate.is_some() {
            cap_for(period)
        } else {
            0.0
        }
    };
    let cap_visible = daily_rate.is_some() && budget.show_cap;
    let long_label = match long_period {
        BudgetPeriod::Daily => "1d",
        BudgetPeriod::Monthly => "30d",
        BudgetPeriod::Weekly => "7d",
    };
    let long_reset = match long_period {
        BudgetPeriod::Daily => today_days + 1,
        BudgetPeriod::Monthly => next_month,
        BudgetPeriod::Weekly => next_week,
    };
    limits.push(UsageLimit {
        key: "daily".into(),
        kind: "daily".into(),
        label: "1d".into(),
        model: None,
        model_id: None,
        scope: Some(serde_json::json!({
            "usage_usd": today_spend,
            "cap_usd": display_cap_for(BudgetPeriod::Daily),
        })),
        is_active: true,
        usage: UsageSection {
            available: true,
            percentage: share_of_total(today_spend, day_cap),
            resets_at: days_to_system_time(today_days + 1),
        },
    });
    limits.push(UsageLimit {
        key: "long".into(),
        kind: "window".into(),
        label: long_label.into(),
        model: None,
        model_id: None,
        scope: Some(serde_json::json!({
            "usage_usd": long_spend,
            "cap_usd": display_cap_for(long_period),
            "show_cap": if cap_visible { 1.0 } else { 0.0 },
        })),
        is_active: true,
        usage: UsageSection {
            available: true,
            percentage: share_of_total(long_spend, long_cap),
            resets_at: days_to_system_time(long_reset),
        },
    });

    // Line 1: account balance. The bar uses the fraction still available, so it
    // shrinks as spend grows (a fuel-gauge read of the remaining credits).
    // The printed number is always the real account balance
    // (total_credits - total_usage). A manually entered monthly deposit only
    // re-scales the bar denominator; when unset the lifetime total is used.
    let (acct_balance, acct_total) = (balance, budget.month_deposit.unwrap_or(total_credits));
    // With the toggle on, the balance line prints the deposit next to the
    // balance; when no deposit is set it falls back to the accumulated account
    // total (acct_total below).
    let deposit_visible = budget.show_deposit;
    let remaining_pct = if acct_total > 0.0 {
        (acct_balance / acct_total * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    limits.push(UsageLimit {
        key: "account".into(),
        kind: "account".into(),
        label: "Balance".into(),
        model: None,
        model_id: None,
        scope: Some(serde_json::json!({
            "balance_usd": acct_balance,
            "total_usd": acct_total,
            "remaining_pct": remaining_pct,
            "show_deposit": if deposit_visible { 1.0 } else { 0.0 },
        })),
        is_active: acct_total > 0.0,
        usage: UsageSection {
            available: acct_total > 0.0,
            percentage: remaining_pct,
            resets_at: None,
        },
    });

    if key.is_some() {
        // `/key`'s `usage_daily` lags (often 0 while spending), so the "today"
        // limit reuses the live value derived from the credits delta instead of
        // the per-key counter. The limit only exists when a key is configured.
        let daily = today_spend;
        limits.push(UsageLimit {
            key: "today".into(),
            kind: "daily".into(),
            label: "Today".into(),
            model: None,
            model_id: None,
            scope: Some(serde_json::json!({ "usage_usd": daily })),
            is_active: daily > 0.0,
            usage: UsageSection {
                available: true,
                percentage: share_of_total(daily, total_credits),
                resets_at: days_to_system_time(today_days + 1),
            },
        });
    }

    UsageData {
        limits,
        // No five-hour window: the short slot stays empty.
        session: UsageSection::default(),
        weekly: UsageSection {
            available: true,
            percentage: weekly_percentage,
            resets_at: days_to_system_time(next_week),
        },
        weekly_label: Some("7d".into()),
        monthly: Some(UsageSection {
            available: true,
            percentage: monthly_percentage,
            resets_at: days_to_system_time(next_month),
        }),
        credits: credits_section,
        stale: false,
        ..Default::default()
    }
}

fn share_of_total(amount: f64, total: f64) -> f64 {
    if !amount.is_finite() || total <= 0.0 || !total.is_finite() {
        return 0.0;
    }
    (amount / total * 100.0).clamp(0.0, 100.0)
}

#[derive(Default)]
struct TokenTotals {
    prompt: u64,
    completion: u64,
    reasoning: u64,
    cached: u64,
    requests: u64,
}

fn token_totals(activity: &[ActivityEntry]) -> TokenTotals {
    activity
        .iter()
        .fold(TokenTotals::default(), |mut totals, entry| {
            totals.prompt = totals.prompt.saturating_add(entry.prompt_tokens);
            totals.completion = totals.completion.saturating_add(entry.completion_tokens);
            totals.reasoning = totals.reasoning.saturating_add(entry.reasoning_tokens);
            totals.cached = totals.cached.saturating_add(entry.cached_tokens);
            totals.requests = totals.requests.saturating_add(entry.requests);
            totals
        })
}

fn sum_days(rows: &[(i64, f64)], start: i64, end: i64) -> f64 {
    rows.iter()
        .filter(|(day, usage)| *day >= start && *day < end && usage.is_finite() && *usage >= 0.0)
        .map(|(_, usage)| *usage)
        .sum()
}

fn sum_usage_between(activity: &[ActivityEntry], start: i64, end: i64) -> f64 {
    activity
        .iter()
        .filter_map(|entry| {
            let days = date_days(&entry.date)?;
            (days >= start && days < end && entry.usage.is_finite() && entry.usage >= 0.0)
                .then_some(entry.usage)
        })
        .sum()
}

/// Monday-based week boundaries in days since 1970-01-01 (a Thursday).
fn week_bounds(today_days: i64) -> (i64, i64) {
    let weekday = (today_days + 3).rem_euclid(7);
    let start = today_days - weekday;
    (start, start + 7)
}

/// Calendar-month boundaries: first day of this month and of the next.
fn month_bounds(today_days: i64) -> (i64, i64) {
    let (year, month, _) = civil_from_days(today_days);
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    (
        days_from_civil(year, month, 1).unwrap_or(today_days),
        days_from_civil(next_year, next_month, 1).unwrap_or(today_days + 31),
    )
}

fn days_to_system_time(days: i64) -> Option<SystemTime> {
    let seconds = u64::try_from(days.max(0))
        .ok()?
        .checked_mul(SECONDS_PER_DAY as u64)?;
    UNIX_EPOCH.checked_add(std::time::Duration::from_secs(seconds))
}

fn today_days(now: SystemTime) -> i64 {
    now.duration_since(UNIX_EPOCH)
        .map(|duration| (duration.as_secs() / SECONDS_PER_DAY as u64) as i64)
        .unwrap_or(0)
}

/// Days since 1970-01-01 for a UTC date string.
///
/// OpenRouter's `/activity` returns timestamps like `2026-10-21 00:00:00`;
/// earlier payloads used a bare `YYYY-MM-DD`. Only the leading date matters.
fn date_days(value: &str) -> Option<i64> {
    let head = value.get(..10)?;
    let bytes = head.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = parse_digits(&bytes[0..4])? as i64;
    let month = parse_digits(&bytes[5..7])?;
    let day = parse_digits(&bytes[8..10])?;
    days_from_civil(year, month, day)
}

fn parse_digits(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    bytes.iter().try_fold(0_u32, |value, byte| {
        value.checked_mul(10)?.checked_add(u32::from(byte - b'0'))
    })
}

fn days_from_civil(year: i64, month: u32, day: u32) -> Option<i64> {
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
    {
        return None;
    }
    let mut days: i64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += i64::from(month_days[m as usize]);
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }
    Some(days + i64::from(day) - 1)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Howard Hinnant's civil-from-days algorithm, inverted for `days_from_civil`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (year + i64::from(month <= 2), month, day)
}

fn read_credentials() -> Option<Credentials> {
    let management_key = resolve_management_key()?;
    let api_key = non_empty_environment(API_KEY_ENV).filter(|key| is_header_safe_token(key));
    Some(Credentials {
        management_key,
        api_key,
    })
}

/// Resolve the management key by trying the credential store first, then the
/// environment variable as a fallback.  A value that fails token validation is
/// logged (without the value) and the next source is tried.
fn resolve_management_key() -> Option<String> {
    // Source 1: Windows Credential Manager.
    match crate::secret_store::read_secret(MANAGEMENT_KEY_TARGET) {
        Ok(Some(value)) => {
            if is_header_safe_token(&value) {
                return Some(value);
            }
            diagnose::log(
                "OpenRouter management key from credential store contains characters \
                 unsafe for a header; falling back to environment variable",
            );
        }
        Ok(None) => {}
        Err(err) => {
            diagnose::log(format!(
                "OpenRouter credential store read failed; \
                 falling back to environment variable: {err}"
            ));
        }
    }

    // Source 2: environment variable.
    let value = non_empty_environment(MANAGEMENT_KEY_ENV)?;
    if is_header_safe_token(&value) {
        return Some(value);
    }
    diagnose::log(
        "OpenRouter management key from environment variable contains characters \
         unsafe for a header",
    );
    None
}

/// Tokens go into a header, so anything that could split it is unusable.
fn is_header_safe_token(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_graphic())
}

fn non_empty_environment(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn secret_signature(name: &str) -> String {
    match non_empty_environment(name) {
        Some(value) => {
            let mut hasher = DefaultHasher::new();
            value.hash(&mut hasher);
            format!("{name}|present|{}|{:x}", value.len(), hasher.finish())
        }
        None => format!("{name}|missing"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY_2026_10_22: i64 = 20_748; // 2026-10-22 is a Thursday.

    fn credits(total_credits: f64, total_usage: f64) -> CreditsData {
        CreditsData {
            total_credits,
            total_usage,
        }
    }

    #[test]
    fn balance_maps_to_credits_and_week_month_spend_to_gauges() {
        // 2026-10-19 (Mon) and 2026-10-20 fall inside this calendar week;
        // 2026-09-30 is last month.
        let activity = [
            ActivityEntry {
                date: "2026-10-19".into(),
                usage: 1.5,
                prompt_tokens: 100,
                completion_tokens: 200,
                requests: 3,
                ..Default::default()
            },
            ActivityEntry {
                date: "2026-10-20".into(),
                usage: 2.5,
                prompt_tokens: 50,
                completion_tokens: 60,
                reasoning_tokens: 10,
                cached_tokens: 5,
                requests: 1,
                ..Default::default()
            },
            ActivityEntry {
                date: "2026-09-30".into(),
                usage: 10.0,
                prompt_tokens: 1,
                completion_tokens: 1,
                requests: 1,
                ..Default::default()
            },
        ];

        let data = usage_from_account(
            &credits(100.0, 20.0),
            &activity,
            None,
            BudgetConfig::default(),
            TODAY_2026_10_22,
            None,
        );

        let credits = data.credits.expect("credits present");
        assert_eq!(credits.remaining, 80.0);
        assert_eq!(credits.total, 100.0);
        assert_eq!(credits.percentage, 20.0);

        // This week spends 4.0 of 100 credits; the month includes 2026-10-19/20
        // but not 2026-09-30.
        assert_eq!(data.weekly.percentage, 4.0);
        assert_eq!(data.monthly.as_ref().unwrap().percentage, 4.0);
        assert!(data.weekly.available);
        // No five-hour window.
        assert!(!data.session.available);
        assert_eq!(data.weekly_label.as_deref(), Some("7d"));

        // Tokens are aggregated across the whole activity window.
        let tokens = data
            .limits
            .iter()
            .find(|limit| limit.key == "tokens")
            .unwrap();
        let scope = tokens.scope.as_ref().unwrap();
        assert_eq!(scope["prompt_tokens"], 151);
        assert_eq!(scope["completion_tokens"], 261);
        assert_eq!(scope["reasoning_tokens"], 10);
        assert_eq!(scope["cached_tokens"], 5);
        assert_eq!(scope["requests"], 5);
        assert!(tokens.is_active);
    }

    #[test]
    fn monthly_budget_overrides_the_gauge_denominator() {
        let activity = [ActivityEntry {
            date: "2026-10-20 00:00:00".into(),
            usage: 5.0,
            ..Default::default()
        }];
        let data = usage_from_account(
            &credits(140.0, 0.0),
            &activity,
            None,
            BudgetConfig {
                amount: Some(20.0),
                budget_period: BudgetPeriod::Monthly,
                monitor_period: BudgetPeriod::Monthly,
                show_cap: true,
                month_deposit: None,
                show_deposit: true,
            },
            TODAY_2026_10_22,
            None,
        );
        // Monthly gauge: 5 / 20 = 25% instead of 5 / 140.
        assert!((data.monthly.as_ref().unwrap().percentage - 25.0).abs() < 1e-9);
        let credits = data.credits.expect("budget credits present");
        assert!((credits.percentage - 25.0).abs() < 1e-9);
        assert!((credits.total - 20.0).abs() < 1e-9);
        assert!((credits.remaining - 15.0).abs() < 1e-9);
    }

    #[test]
    fn key_endpoint_adds_today_spend_without_overriding_activity_windows() {
        let key = KeyData {
            usage_daily: 0.5,
            ..Default::default()
        };
        // Window spend is account-wide, from /activity, not from /key.
        let activity = [ActivityEntry {
            date: "2026-10-20 00:00:00".into(), // same week and month
            usage: 4.0,
            ..Default::default()
        }];
        let data = usage_from_account(
            &credits(200.0, 0.0),
            &activity,
            Some(&key),
            BudgetConfig::default(),
            TODAY_2026_10_22,
            None,
        );

        // Window spend includes /activity (4.0) plus today's /key spend (0.5).
        assert!((data.weekly.percentage - 2.25).abs() < 1e-9); // 4.5 / 200 * 100
        assert!((data.monthly.as_ref().unwrap().percentage - 2.25).abs() < 1e-9);

        let today = data
            .limits
            .iter()
            .find(|limit| limit.key == "today")
            .expect("today limit present");
        assert_eq!(today.scope.as_ref().unwrap()["usage_usd"], 0.5);
        assert!((today.usage.percentage - 0.25).abs() < 1e-9); // 0.5 / 200 * 100
        assert!(today.is_active);
    }

    #[test]
    fn zero_credits_suppress_the_balance_and_clamp_percentages() {
        let data = usage_from_account(
            &credits(0.0, 0.0),
            &[],
            None,
            BudgetConfig::default(),
            TODAY_2026_10_22,
            None,
        );
        assert!(data.credits.is_none());
        assert_eq!(data.weekly.percentage, 0.0);
        assert_eq!(data.monthly.as_ref().unwrap().percentage, 0.0);
        assert!(!data.session.available);
    }

    #[test]
    fn weekly_and_monthly_resets_land_on_utc_boundaries() {
        let data = usage_from_account(
            &credits(10.0, 0.0),
            &[],
            None,
            BudgetConfig::default(),
            TODAY_2026_10_22,
            None,
        );
        // 2026-10-22 is Thursday; the next Monday is 2026-10-26.
        let expected_monday = days_to_system_time(date_days("2026-10-26").unwrap());
        assert_eq!(data.weekly.resets_at, expected_monday);
        let expected_month = days_to_system_time(date_days("2026-11-01").unwrap());
        assert_eq!(data.monthly.as_ref().unwrap().resets_at, expected_month);
    }

    #[test]
    fn activity_dates_outside_the_windows_are_ignored_for_spend() {
        let activity = [
            ActivityEntry {
                date: "2026-10-12".into(), // last week (Monday 2026-10-12)
                usage: 9.0,
                ..Default::default()
            },
            ActivityEntry {
                date: "2026-10-21".into(), // this week
                usage: 1.0,
                ..Default::default()
            },
        ];
        let data = usage_from_account(
            &credits(10.0, 0.0),
            &activity,
            None,
            BudgetConfig::default(),
            TODAY_2026_10_22,
            None,
        );
        assert_eq!(data.weekly.percentage, 10.0); // only 1.0 / 10
    }

    #[test]
    fn civil_date_helpers_round_trip() {
        for (days, civil) in [
            (0_i64, (1970_i64, 1_u32, 1_u32)),
            (date_days("2000-03-01").unwrap(), (2000, 3, 1)),
            (date_days("2024-02-29").unwrap(), (2024, 2, 29)),
            (date_days("2026-10-22").unwrap(), (2026, 10, 22)),
            (date_days("2026-12-31").unwrap(), (2026, 12, 31)),
        ] {
            assert_eq!(civil_from_days(days), civil);
            assert_eq!(days_from_civil(civil.0, civil.1, civil.2), Some(days));
        }
    }

    #[test]
    fn week_and_month_bounds_are_monday_and_first_of_month() {
        assert_eq!(week_bounds(TODAY_2026_10_22), (20_745, 20_752)); // Mon 10-19 .. Mon 10-26
        assert_eq!(
            month_bounds(TODAY_2026_10_22),
            (
                date_days("2026-10-01").unwrap(),
                date_days("2026-11-01").unwrap()
            )
        );
        // December rolls into the next year.
        let dec31 = date_days("2026-12-31").unwrap();
        assert_eq!(
            month_bounds(dec31),
            (
                date_days("2026-12-01").unwrap(),
                date_days("2027-01-01").unwrap()
            )
        );
    }

    #[test]
    fn malformed_activity_dates_are_skipped() {
        assert_eq!(date_days("2026-10-22"), Some(TODAY_2026_10_22));
        // Real /activity timestamps carry a time; only the date prefix counts.
        assert_eq!(date_days("2026-10-21 00:00:00"), date_days("2026-10-21"));
        for invalid in [
            "",
            "2026-13-01",
            "2026-00-10",
            "2026-02-30",
            "not-a-date",
            "2026/10/22",
        ] {
            assert_eq!(date_days(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn header_unsafe_tokens_are_refused() {
        assert!(is_header_safe_token("sk-or-v1-fixture"));
        assert!(!is_header_safe_token(""));
        assert!(!is_header_safe_token("sk-or\r\nX-Test: injected"));
        assert!(!is_header_safe_token("two words"));
    }
}
