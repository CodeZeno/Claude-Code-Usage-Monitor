//! Phase 2: quota health — how much *room* a provider actually has, not
//! just how much percentage is left.
//!
//! A provider at "80% used, resets in 4 minutes" is healthy: the window is
//! about to roll over, so there's no reason to ration it. A provider at
//! "80% used, resets in 6 days" is not: it burned most of a week-long budget
//! almost immediately and should be protected. Plain remaining-percentage
//! can't tell these apart; this module folds in how much of the reset
//! window has actually elapsed.
//!
//! All tunable numbers (thresholds, window durations, the "unknown reset"
//! fallback) live in this one file so routing logic in `dispatcher.rs` never
//! hardcodes a magic number — it only ever compares against
//! [`HEALTHY_SCORE`] / [`CONSTRAINED_SCORE`] or matches on [`HealthCategory`].

use serde::Serialize;

use crate::quota_report::{ProviderQuota, ProviderStatus, QuotaWindow};

pub const FIVE_HOUR_SECONDS: u64 = 5 * 3600;
pub const SEVEN_DAY_SECONDS: u64 = 7 * 24 * 3600;

/// score >= HEALTHY_SCORE: consuming this provider is on-pace-or-better
/// relative to elapsed window time. No rationing needed.
pub const HEALTHY_SCORE: f64 = 1.0;

/// score >= CONSTRAINED_SCORE (but < HEALTHY_SCORE): running ahead of
/// budget for the time elapsed, but usable — a reasonable offload/fallback
/// target, just not the first choice over a healthy alternative.
pub const CONSTRAINED_SCORE: f64 = 0.4;

/// Below CONSTRAINED_SCORE: badly over-budget for how much window time has
/// elapsed. Avoid unless the alternative is equally bad or unavailable.
/// (Anything below CONSTRAINED_SCORE and not caught above is Critical.)

/// Auto-routing (no preferred executor) only switches away from the
/// higher-scoring provider when the gap exceeds this margin, so a few
/// points of quota noise doesn't flip the choice on every call.
pub const AUTO_SWITCH_MARGIN: f64 = 0.15;

/// Floor applied to `remaining_window_fraction` before dividing. Without it,
/// a reset that is imminent or already passed (fraction -> 0) would send the
/// score to +inf; flooring instead yields a large-but-finite score, which is
/// exactly the "don't ration near reset" behavior we want, just without the
/// infinity.
const MIN_REMAINING_WINDOW_FRACTION: f64 = 0.001;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthCategory {
    Healthy,
    Constrained,
    Critical,
    /// Provider wasn't polled successfully this run (no credentials, auth
    /// required, disabled, ...). Not a quota judgment — an availability one.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WindowHealth {
    pub score: f64,
    pub category: HealthCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ProviderHealth {
    /// The stricter of the two windows' categories, or `Unavailable`.
    pub category: HealthCategory,
    /// The stricter of the two windows' scores. `None` when `Unavailable`.
    pub score: Option<f64>,
    pub five_hour: Option<WindowHealth>,
    pub seven_day: Option<WindowHealth>,
}

pub fn provider_health(provider: &ProviderQuota, generated_at: u64) -> ProviderHealth {
    if provider.status != ProviderStatus::Ok {
        return ProviderHealth {
            category: HealthCategory::Unavailable,
            score: None,
            five_hour: None,
            seven_day: None,
        };
    }

    let five_hour = provider
        .windows
        .five_hour
        .as_ref()
        .map(|window| window_health(window, generated_at, FIVE_HOUR_SECONDS));
    let seven_day = provider
        .windows
        .seven_day
        .as_ref()
        .map(|window| window_health(window, generated_at, SEVEN_DAY_SECONDS));

    let stricter = [five_hour, seven_day]
        .into_iter()
        .flatten()
        .min_by(|a, b| a.score.total_cmp(&b.score));

    match stricter {
        Some(strictest) => ProviderHealth {
            category: strictest.category,
            score: Some(strictest.score),
            five_hour,
            seven_day,
        },
        // status is Ok but neither window reported data: nothing indicates
        // a constraint, so don't manufacture one.
        None => ProviderHealth {
            category: HealthCategory::Healthy,
            score: None,
            five_hour,
            seven_day,
        },
    }
}

pub fn window_health(window: &QuotaWindow, generated_at: u64, window_total_seconds: u64) -> WindowHealth {
    let remaining_quota_fraction = (1.0 - window.used_percent / 100.0).clamp(0.0, 1.0);

    let remaining_window_fraction = match window.resets_at {
        Some(resets_at) if resets_at > generated_at => {
            ((resets_at - generated_at) as f64 / window_total_seconds as f64).clamp(0.0, 1.0)
        }
        // Reset already passed (stale) but the provider hasn't rolled the
        // window over yet: treat as imminent, same leniency as "about to
        // reset" rather than panicking on a number that's about to change.
        Some(_) => 0.0,
        // Unknown reset time: no basis for leniency, so fall back to the
        // conservative plain remaining-quota reading (fraction = 1.0 means
        // the divide-by is a no-op).
        None => 1.0,
    };

    let score = remaining_quota_fraction / remaining_window_fraction.max(MIN_REMAINING_WINDOW_FRACTION);
    WindowHealth {
        score,
        category: category_for_score(score),
    }
}

fn category_for_score(score: f64) -> HealthCategory {
    if score >= HEALTHY_SCORE {
        HealthCategory::Healthy
    } else if score >= CONSTRAINED_SCORE {
        HealthCategory::Constrained
    } else {
        HealthCategory::Critical
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota_report::QuotaWindows;

    fn ok_provider(windows: QuotaWindows) -> ProviderQuota {
        ProviderQuota {
            provider: "test".to_string(),
            status: ProviderStatus::Ok,
            error: None,
            windows,
        }
    }

    fn window(used_percent: f64, resets_at: Option<u64>) -> QuotaWindow {
        QuotaWindow {
            used_percent,
            resets_at,
            stale: resets_at.is_some_and(|r| r <= 1_000_000),
        }
    }

    #[test]
    fn on_pace_usage_scores_exactly_healthy_boundary() {
        // Half the 5h window elapsed, half the quota used: exactly on pace.
        let generated_at = 1_000_000;
        let resets_at = generated_at + FIVE_HOUR_SECONDS / 2;
        let health = window_health(&window(50.0, Some(resets_at)), generated_at, FIVE_HOUR_SECONDS);
        assert!((health.score - 1.0).abs() < 1e-9);
        assert_eq!(health.category, HealthCategory::Healthy);
    }

    #[test]
    fn reset_imminent_with_low_remaining_quota_is_still_healthy() {
        let generated_at = 1_000_000;
        let resets_at = generated_at + 5; // 5 seconds left in a 5h window
        let health = window_health(&window(97.0, Some(resets_at)), generated_at, FIVE_HOUR_SECONDS);
        assert_eq!(health.category, HealthCategory::Healthy);
        assert!(health.score > HEALTHY_SCORE);
    }

    #[test]
    fn heavy_early_usage_in_a_long_window_is_critical() {
        // 7d window just reset (nearly all of it remains) but already 80% used.
        let generated_at = 1_000_000;
        let resets_at = generated_at + SEVEN_DAY_SECONDS - 60;
        let health = window_health(&window(80.0, Some(resets_at)), generated_at, SEVEN_DAY_SECONDS);
        assert_eq!(health.category, HealthCategory::Critical);
        assert!(health.score < CONSTRAINED_SCORE);
    }

    #[test]
    fn constrained_boundary_is_inclusive() {
        // remaining_quota_fraction / remaining_window_fraction == 0.4 exactly.
        let generated_at = 1_000_000;
        let resets_at = generated_at + FIVE_HOUR_SECONDS; // fraction = 1.0
        let health = window_health(&window(60.0, Some(resets_at)), generated_at, FIVE_HOUR_SECONDS);
        assert!((health.score - 0.4).abs() < 1e-9);
        assert_eq!(health.category, HealthCategory::Constrained);
    }

    #[test]
    fn just_below_constrained_boundary_is_critical() {
        let generated_at = 1_000_000;
        let resets_at = generated_at + FIVE_HOUR_SECONDS;
        let health = window_health(&window(60.01, Some(resets_at)), generated_at, FIVE_HOUR_SECONDS);
        assert_eq!(health.category, HealthCategory::Critical);
    }

    #[test]
    fn missing_reset_time_falls_back_to_plain_remaining_fraction() {
        let generated_at = 1_000_000;
        let health = window_health(&window(30.0, None), generated_at, FIVE_HOUR_SECONDS);
        assert!((health.score - 0.7).abs() < 1e-9);
    }

    #[test]
    fn stale_past_reset_is_treated_as_imminent_not_critical() {
        let generated_at = 1_000_000;
        let health = window_health(&window(95.0, Some(generated_at - 10)), generated_at, FIVE_HOUR_SECONDS);
        assert_eq!(health.category, HealthCategory::Healthy);
    }

    #[test]
    fn provider_health_takes_the_stricter_of_the_two_windows() {
        let generated_at = 1_000_000;
        // 5h: on-pace/healthy. 7d: heavy early usage/critical.
        let windows = QuotaWindows {
            five_hour: Some(window(10.0, Some(generated_at + FIVE_HOUR_SECONDS / 2))),
            seven_day: Some(window(80.0, Some(generated_at + SEVEN_DAY_SECONDS - 60))),
        };
        let health = provider_health(&ok_provider(windows), generated_at);
        assert_eq!(health.category, HealthCategory::Critical);
    }

    #[test]
    fn unavailable_status_short_circuits_to_unavailable_category() {
        let provider = ProviderQuota {
            provider: "test".to_string(),
            status: ProviderStatus::Unavailable,
            error: Some("no_credentials".to_string()),
            windows: QuotaWindows {
                five_hour: Some(window(10.0, None)),
                seven_day: None,
            },
        };
        let health = provider_health(&provider, 1_000_000);
        assert_eq!(health.category, HealthCategory::Unavailable);
        assert_eq!(health.score, None);
    }

    #[test]
    fn ok_status_with_no_window_data_defaults_to_healthy() {
        let health = provider_health(&ok_provider(QuotaWindows::default()), 1_000_000);
        assert_eq!(health.category, HealthCategory::Healthy);
        assert_eq!(health.score, None);
    }
}
