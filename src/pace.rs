//! Pure pace / burn-rate math. No Win32, no rendering — fully unit tested.
use std::time::{Duration, SystemTime};

/// Which provider a usage cell belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provider {
    ClaudeCode,
    Codex,
    Antigravity,
}

/// Which rolling window a cell represents.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Session, // "5h"
    Weekly,  // "7d"
}

/// Fixed, known window length. `None` => unknown => omit all pace UI/alerts.
pub fn window_length(provider: Provider, section: Section) -> Option<Duration> {
    match provider {
        Provider::ClaudeCode => Some(match section {
            Section::Session => Duration::from_secs(5 * 3600),
            Section::Weekly => Duration::from_secs(7 * 24 * 3600),
        }),
        // Real windows differ and are not reliably known; omit rather than fake.
        Provider::Codex | Provider::Antigravity => None,
    }
}

/// Fraction of the window elapsed, clamped to 0.0..=1.0.
/// `None` if inputs are unusable (no reset, no length, or reset already passed).
pub fn elapsed_fraction(
    resets_at: Option<SystemTime>,
    window_len: Option<Duration>,
    now: SystemTime,
) -> Option<f64> {
    let resets_at = resets_at?;
    let window_len = window_len?;
    let remaining = resets_at.duration_since(now).ok()?; // Err => already past reset
    let remaining_s = remaining.as_secs_f64();
    let window_s = window_len.as_secs_f64();
    if window_s <= 0.0 {
        return None;
    }
    let frac = 1.0 - (remaining_s / window_s);
    Some(frac.clamp(0.0, 1.0))
}

/// Seconds until projected exhaustion at the current burn rate.
/// `None` when not projected to run out this window (rate <= 100%) or no usage yet.
pub fn eta_to_empty_secs(
    used_percent: f64,
    elapsed_fraction: f64,
    window_len: Duration,
) -> Option<u64> {
    if used_percent <= 0.0 || elapsed_fraction <= 0.0 {
        return None;
    }
    // Projected end-of-window usage if the current rate holds.
    let projected = used_percent / elapsed_fraction;
    if projected <= 100.0 {
        return None;
    }
    // Total time from window start to reach 100% at this rate.
    let window_s = window_len.as_secs_f64();
    let elapsed_s = elapsed_fraction * window_s;
    let time_to_100 = elapsed_s * (100.0 / used_percent);
    let eta = time_to_100 - elapsed_s;
    if eta <= 0.0 {
        Some(0)
    } else {
        Some(eta as u64)
    }
}

/// At-a-glance pace verdict, used to color the tick and (single-provider) the % text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaceStatus {
    /// Comfortably under pace.
    Ahead,
    /// Near the pace line (within the near-band on either side).
    OnPace,
    /// Over pace — projected to run out early.
    Behind,
}

/// Compare used% against the elapsed-fraction tick.
/// `near_band` is the +/- percentage-point tolerance treated as "on pace" (e.g. 5.0).
pub fn pace_status(used_percent: f64, elapsed_fraction: f64, near_band: f64) -> PaceStatus {
    let expected = 100.0 * elapsed_fraction;
    let delta = used_percent - expected;
    if delta > near_band {
        PaceStatus::Behind
    } else if delta < -near_band {
        PaceStatus::Ahead
    } else {
        PaceStatus::OnPace
    }
}

/// User-selectable alert sensitivity.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlertPreset {
    Off,
    At90,
    At80And90,
    Pace,
}

impl AlertPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertPreset::Off => "off",
            AlertPreset::At90 => "at90",
            AlertPreset::At80And90 => "at80and90",
            AlertPreset::Pace => "pace",
        }
    }
    pub fn from_str(s: &str) -> AlertPreset {
        match s {
            "at90" => AlertPreset::At90,
            "at80and90" => AlertPreset::At80And90,
            "pace" => AlertPreset::Pace,
            _ => AlertPreset::Off,
        }
    }
}

/// Result of evaluating one cell against the active preset.
/// `active` = the cell currently meets the alert condition (drives danger badge).
/// `fire` = we should raise a balloon NOW (edge: became active this evaluation).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AlertDecision {
    pub active: bool,
    pub fire: bool,
}

/// Pure, edge-triggered evaluation. `was_active` is the previous stored state for the cell.
/// `over_pace` should be `eta_to_empty_secs(...).is_some()` for the cell.
pub fn evaluate_alert(
    preset: AlertPreset,
    used_percent: f64,
    over_pace: bool,
    was_active: bool,
) -> AlertDecision {
    let active = match preset {
        AlertPreset::Off => false,
        AlertPreset::At90 => used_percent >= 90.0,
        AlertPreset::At80And90 => used_percent >= 80.0,
        AlertPreset::Pace => over_pace,
    };
    AlertDecision {
        active,
        fire: active && !was_active, // rising edge only
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs_from_now: i64, now: SystemTime) -> Option<SystemTime> {
        if secs_from_now >= 0 {
            Some(now + Duration::from_secs(secs_from_now as u64))
        } else {
            Some(now - Duration::from_secs((-secs_from_now) as u64))
        }
    }

    #[test]
    fn window_length_known_for_claude_only() {
        assert_eq!(
            window_length(Provider::ClaudeCode, Section::Session),
            Some(Duration::from_secs(5 * 3600))
        );
        assert_eq!(
            window_length(Provider::ClaudeCode, Section::Weekly),
            Some(Duration::from_secs(7 * 24 * 3600))
        );
        assert_eq!(window_length(Provider::Codex, Section::Session), None);
        assert_eq!(window_length(Provider::Antigravity, Section::Weekly), None);
    }

    #[test]
    fn elapsed_fraction_halfway() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        // 5h window, 2.5h remaining => 50% elapsed.
        let f = elapsed_fraction(
            t(2 * 3600 + 1800, now),
            Some(Duration::from_secs(5 * 3600)),
            now,
        );
        assert!((f.unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn elapsed_fraction_none_when_past_or_missing() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(
            elapsed_fraction(t(-10, now), Some(Duration::from_secs(3600)), now),
            None
        );
        assert_eq!(elapsed_fraction(None, Some(Duration::from_secs(3600)), now), None);
        assert_eq!(elapsed_fraction(t(10, now), None, now), None);
    }

    #[test]
    fn eta_none_when_on_or_under_pace() {
        // 50% used at 50% elapsed => projected 100% exactly => not "runs out early".
        assert_eq!(
            eta_to_empty_secs(50.0, 0.5, Duration::from_secs(5 * 3600)),
            None
        );
        // Under pace.
        assert_eq!(
            eta_to_empty_secs(20.0, 0.5, Duration::from_secs(5 * 3600)),
            None
        );
        // No usage yet.
        assert_eq!(eta_to_empty_secs(0.0, 0.5, Duration::from_secs(5 * 3600)), None);
    }

    #[test]
    fn eta_positive_when_burning_fast() {
        // 80% used at 50% elapsed of a 5h window.
        // elapsed_s = 9000s; time_to_100 = 9000 * (100/80) = 11250s; eta = 2250s.
        let eta = eta_to_empty_secs(80.0, 0.5, Duration::from_secs(5 * 3600)).unwrap();
        assert_eq!(eta, 2250);
    }

    #[test]
    fn pace_status_bands() {
        // 50% elapsed => expected 50%.
        assert_eq!(pace_status(30.0, 0.5, 5.0), PaceStatus::Ahead);
        assert_eq!(pace_status(52.0, 0.5, 5.0), PaceStatus::OnPace);
        assert_eq!(pace_status(70.0, 0.5, 5.0), PaceStatus::Behind);
    }

    #[test]
    fn preset_str_roundtrip() {
        for p in [
            AlertPreset::Off,
            AlertPreset::At90,
            AlertPreset::At80And90,
            AlertPreset::Pace,
        ] {
            assert_eq!(AlertPreset::from_str(p.as_str()), p);
        }
        assert_eq!(AlertPreset::from_str("garbage"), AlertPreset::Off);
    }

    #[test]
    fn alert_edge_triggers_once() {
        // Rising edge fires.
        let d = evaluate_alert(AlertPreset::At90, 91.0, false, false);
        assert_eq!(d, AlertDecision { active: true, fire: true });
        // Still active, already fired => no re-fire.
        let d = evaluate_alert(AlertPreset::At90, 95.0, false, true);
        assert_eq!(d, AlertDecision { active: true, fire: false });
        // Dropped below => inactive, re-arms.
        let d = evaluate_alert(AlertPreset::At90, 50.0, false, true);
        assert_eq!(d, AlertDecision { active: false, fire: false });
    }

    #[test]
    fn alert_off_never_fires() {
        assert_eq!(
            evaluate_alert(AlertPreset::Off, 99.0, true, false),
            AlertDecision { active: false, fire: false }
        );
    }

    #[test]
    fn alert_pace_uses_over_pace_flag() {
        assert_eq!(
            evaluate_alert(AlertPreset::Pace, 60.0, true, false),
            AlertDecision { active: true, fire: true }
        );
        assert_eq!(
            evaluate_alert(AlertPreset::Pace, 60.0, false, false),
            AlertDecision { active: false, fire: false }
        );
    }
}
