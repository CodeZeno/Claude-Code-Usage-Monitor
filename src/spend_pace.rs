use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::models::{AccountUsage, SpendPaceSlots, SpendPaceView};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaceLevel {
    Ok,
    High,
    Critical,
}

impl PaceLevel {
    pub fn as_bar_level(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::High => 1,
            Self::Critical => 2,
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct SpendAnchorsDisk {
    day_key: String,
    day_spend_start: f64,
    week_key: String,
    week_spend_start: f64,
    last_spend: f64,
}

fn anchors_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata)
        .join("ClaudeCodeUsageMonitor")
        .join("spend_anchors.json")
}

fn load_anchors() -> SpendAnchorsDisk {
    std::fs::read_to_string(anchors_path())
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_anchors(anchors: &SpendAnchorsDisk) {
    let path = anchors_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(anchors) {
        let _ = std::fs::write(path, json);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn local_day_key(secs: u64) -> String {
    let (year, month, day, _) = unix_to_ymd_hms(secs);
    format!("{year:04}-{month:02}-{day:02}")
}

fn local_week_key(secs: u64) -> String {
    let (year, month, day, _) = unix_to_ymd_hms(secs);
    let weekday = unix_weekday_monday_zero(secs);
    let day = day.saturating_sub(weekday as u32);
    let (year, month, day) = normalize_ymd(year, month, day);
    format!("{year:04}-{month:02}-{day:02}")
}

fn unix_weekday_monday_zero(secs: u64) -> u64 {
    let days = secs / 86_400;
    (days + 3) % 7
}

fn normalize_ymd(mut year: u32, mut month: u32, mut day: u32) -> (u32, u32, u32) {
    while day == 0 {
        month = month.saturating_sub(1);
        if month == 0 {
            year -= 1;
            month = 12;
        }
        day += days_in_month(year, month);
    }
    (year, month, day)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        _ => 28,
    }
}

fn is_leap_year(year: u32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn unix_to_ymd_hms(secs: u64) -> (u32, u32, u32, u32) {
    let days = secs / 86_400;
    let time = secs % 86_400;
    let hour = (time / 3600) as u32;

    let mut remaining = days;
    let mut year = 1970u32;
    loop {
        let year_days = if is_leap_year(year) { 366 } else { 365 };
        if remaining < year_days {
            break;
        }
        remaining -= year_days;
        year += 1;
    }

    let month_lengths = [
        31,
        if is_leap_year(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u32;
    for &len in &month_lengths {
        if remaining < len {
            break;
        }
        remaining -= len;
        month += 1;
    }
    (year, month, (remaining + 1) as u32, hour)
}

fn cycle_bounds(account: &AccountUsage, now: SystemTime) -> (SystemTime, SystemTime) {
    if let Some(end) = account.credit_expiry {
        let start = end
            .checked_sub(Duration::from_secs(30 * 86_400))
            .unwrap_or(UNIX_EPOCH);
        return (start, end);
    }

    let secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (year, month, _, _) = unix_to_ymd_hms(secs);
    let start_secs = month_start_unix(year, month);
    let next_month = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    let end_secs = month_start_unix(next_month.0, next_month.1);
    (
        UNIX_EPOCH + Duration::from_secs(start_secs),
        UNIX_EPOCH + Duration::from_secs(end_secs),
    )
}

fn month_start_unix(year: u32, month: u32) -> u64 {
    let mut days = 0u64;
    for y in 1970..year {
        days += if is_leap_year(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += days_in_month(year, m) as u64;
    }
    days * 86_400
}

pub fn evaluate_pace(actual: f64, expected: f64) -> PaceLevel {
    if expected <= 0.0 || actual <= 0.0 {
        return PaceLevel::Ok;
    }
    let ratio = actual / expected;
    if ratio <= 1.0 {
        PaceLevel::Ok
    } else if ratio <= 1.15 {
        PaceLevel::High
    } else {
        PaceLevel::Critical
    }
}

fn elapsed_fraction(now: SystemTime, start: SystemTime, end: SystemTime) -> f64 {
    let total = end.duration_since(start).map(|d| d.as_secs_f64()).unwrap_or(1.0);
    if total <= 0.0 {
        return 1.0;
    }
    let elapsed = now
        .duration_since(start)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
        .clamp(0.0, total);
    (elapsed / total).clamp(0.0, 1.0)
}

fn day_fraction(secs: u64) -> f64 {
    let seconds_into_day = secs % 86_400;
    let fraction = seconds_into_day as f64 / 86_400.0;
    fraction.clamp(1.0 / 24.0, 1.0)
}

fn week_fraction(secs: u64) -> f64 {
    let weekday = unix_weekday_monday_zero(secs);
    let seconds_into_day = secs % 86_400;
    let elapsed = weekday as f64 * 86_400.0 + seconds_into_day as f64;
    let fraction = elapsed / (7.0 * 86_400.0);
    fraction.clamp(1.0 / (7.0 * 24.0), 1.0)
}

pub fn pace_accent(level: u8) -> crate::native_interop::Color {
    match level {
        2 => crate::native_interop::Color::from_hex("#ef4444"),
        1 => crate::native_interop::Color::from_hex("#eab308"),
        _ => crate::native_interop::Color::from_hex("#22c55e"),
    }
}

pub fn bar_fill_percent(actual: f64, cap: f64) -> f64 {
    if cap <= 0.0 {
        return 0.0;
    }
    (actual / cap * 100.0).clamp(0.0, 100.0)
}

pub fn format_pace_fraction(spent: f64, cap: f64) -> String {
    if cap <= 0.0 {
        return format_usd(spent);
    }
    format!("{}/{}", format_usd(spent), format_usd(cap))
}

fn format_usd(amount: f64) -> String {
    if amount >= 100.0 {
        format!("${:.0}", amount.round())
    } else if amount >= 10.0 {
        format!("${:.0}", amount)
    } else if amount >= 1.0 {
        format!("${:.1}", amount)
    } else {
        format!("${:.2}", amount)
    }
}

fn spend_close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

/// Week/day pace uses cumulative billing spend minus anchors at period start.
/// If both anchors are pinned to the current total, week/day incorrectly read $0.
fn repair_stuck_anchors(anchors: &mut SpendAnchorsDisk, spend_used: f64) {
    if spend_used <= 0.0 {
        return;
    }
    if anchors.day_spend_start <= 0.01 && anchors.week_spend_start <= 0.01 {
        return;
    }
    // Both period baselines pinned to the live total => zero delta for week and day.
    // Legitimate day rollover only pins day_spend_start, so leave that case alone.
    if spend_close(anchors.week_spend_start, spend_used)
        && spend_close(anchors.day_spend_start, spend_used)
    {
        anchors.week_spend_start = 0.0;
        anchors.day_spend_start = 0.0;
    }
}

/// Fresh install or anchor reset with zero baselines must not attribute existing
/// billing-cycle spend to today/week.
fn repair_inflated_period_anchors(anchors: &mut SpendAnchorsDisk, spend_used: f64) {
    if spend_used <= 0.01 {
        return;
    }
    if anchors.day_spend_start > 0.01 || anchors.week_spend_start > 0.01 {
        return;
    }
    anchors.day_spend_start = spend_used;
    anchors.week_spend_start = spend_used;
}

fn update_anchors(spend_used: f64) -> SpendAnchorsDisk {
    let secs = now_secs();
    let day_key = local_day_key(secs);
    let week_key = local_week_key(secs);
    let mut anchors = load_anchors();

    repair_inflated_period_anchors(&mut anchors, spend_used);
    if !spend_close(anchors.day_spend_start, spend_used) {
        repair_stuck_anchors(&mut anchors, spend_used);
    }

    // Billing cycle reset: spend dropped — re-anchor from zero, not from the new total.
    if spend_used + 0.01 < anchors.last_spend {
        anchors = SpendAnchorsDisk {
            day_key: day_key.clone(),
            day_spend_start: 0.0,
            week_key: week_key.clone(),
            week_spend_start: 0.0,
            last_spend: spend_used,
        };
        save_anchors(&anchors);
        return anchors;
    }

    if anchors.day_key.is_empty() {
        anchors.day_key = day_key.clone();
        anchors.day_spend_start = spend_used;
    } else if anchors.day_key != day_key {
        anchors.day_key = day_key;
        anchors.day_spend_start = anchors.last_spend;
    }

    if anchors.week_key.is_empty() {
        anchors.week_key = week_key.clone();
        anchors.week_spend_start = spend_used;
    } else if anchors.week_key != week_key {
        anchors.week_key = week_key;
        anchors.week_spend_start = anchors.last_spend;
    }

    anchors.last_spend = spend_used;
    save_anchors(&anchors);
    anchors
}

pub fn compute_spend_pace(account: &AccountUsage) -> Option<SpendPaceView> {
    if account.spend_limit <= 0.0 {
        return None;
    }

    let now = SystemTime::now();
    let secs = now_secs();
    let anchors = update_anchors(account.spend_used);

    let month_actual = account.spend_used;
    let week_actual = (account.spend_used - anchors.week_spend_start).max(0.0);
    let day_actual = (account.spend_used - anchors.day_spend_start).max(0.0);

    let (cycle_start, cycle_end) = cycle_bounds(account, now);
    let cycle_days = cycle_end
        .duration_since(cycle_start)
        .map(|d| d.as_secs_f64() / 86_400.0)
        .unwrap_or(30.0)
        .max(1.0);

    let linear_daily = account.spend_limit / cycle_days;
    let linear_week = linear_daily * 7.0;
    let month_fraction = elapsed_fraction(now, cycle_start, cycle_end);
    let month_expected = account.spend_limit * month_fraction;
    let week_expected = linear_week * week_fraction(secs);
    let day_expected = linear_daily * day_fraction(secs);

    let month_level = evaluate_pace(month_actual, month_expected);
    let week_level = evaluate_pace(week_actual, week_expected);
    let day_level = evaluate_pace(day_actual, day_expected);

    Some(SpendPaceView {
        credit_pct: account.credit_pct,
        credit_expiry: account.credit_expiry,
        slots: SpendPaceSlots {
            month_actual,
            month_cap: account.spend_limit,
            month_expected,
            month_level: month_level.as_bar_level(),
            week_actual,
            week_cap: linear_week,
            week_expected,
            week_level: week_level.as_bar_level(),
            day_actual,
            day_cap: linear_daily,
            day_expected,
            day_level: day_level.as_bar_level(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_pace_thresholds() {
        assert_eq!(evaluate_pace(90.0, 100.0), PaceLevel::Ok);
        assert_eq!(evaluate_pace(110.0, 100.0), PaceLevel::High);
        assert_eq!(evaluate_pace(120.0, 100.0), PaceLevel::Critical);
    }

    #[test]
    fn format_pace_fraction_rounds_dollars() {
        assert_eq!(format_pace_fraction(57.2, 400.0), "$57/$400");
        assert_eq!(format_pace_fraction(13.4, 13.3), "$13/$13");
    }

    #[test]
    fn repair_stuck_anchors_unpins_week_and_day() {
        let mut anchors = SpendAnchorsDisk {
            day_key: "2026-07-01".to_string(),
            day_spend_start: 27.41,
            week_key: "2026-06-30".to_string(),
            week_spend_start: 27.41,
            last_spend: 27.41,
        };
        repair_stuck_anchors(&mut anchors, 27.41);
        assert_eq!(anchors.week_spend_start, 0.0);
        assert_eq!(anchors.day_spend_start, 0.0);
    }

    #[test]
    fn repair_stuck_anchors_leaves_day_rollover_alone() {
        let mut anchors = SpendAnchorsDisk {
            day_key: "2026-07-01".to_string(),
            day_spend_start: 27.41,
            week_key: "2026-06-30".to_string(),
            week_spend_start: 0.0,
            last_spend: 27.41,
        };
        repair_stuck_anchors(&mut anchors, 27.41);
        assert_eq!(anchors.week_spend_start, 0.0);
        assert_eq!(anchors.day_spend_start, 27.41);
    }

    #[test]
    fn repair_inflated_anchors_pins_unknown_history() {
        let mut anchors = SpendAnchorsDisk {
            day_key: "2026-07-20".to_string(),
            day_spend_start: 0.0,
            week_key: "2026-07-20".to_string(),
            week_spend_start: 0.0,
            last_spend: 317.0,
        };
        repair_inflated_period_anchors(&mut anchors, 317.42);
        assert_eq!(anchors.day_spend_start, 317.42);
        assert_eq!(anchors.week_spend_start, 317.42);
    }

    #[test]
    fn repair_inflated_anchors_skips_when_baseline_exists() {
        let mut anchors = SpendAnchorsDisk {
            day_key: "2026-07-20".to_string(),
            day_spend_start: 12.0,
            week_key: "2026-07-14".to_string(),
            week_spend_start: 5.0,
            last_spend: 20.0,
        };
        repair_inflated_period_anchors(&mut anchors, 20.0);
        assert_eq!(anchors.day_spend_start, 12.0);
        assert_eq!(anchors.week_spend_start, 5.0);
    }

    #[test]
    fn repair_stuck_anchors_without_last_spend_match() {
        let mut anchors = SpendAnchorsDisk {
            day_key: "2026-07-01".to_string(),
            day_spend_start: 27.41,
            week_key: "2026-06-30".to_string(),
            week_spend_start: 27.41,
            last_spend: 26.0,
        };
        repair_stuck_anchors(&mut anchors, 27.41);
        assert_eq!(anchors.week_spend_start, 0.0);
        assert_eq!(anchors.day_spend_start, 0.0);
    }
}
