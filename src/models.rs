use std::sync::atomic::{AtomicU8, Ordering};
use std::time::SystemTime;

/// Whether percentages are shown as consumed quota or as what is left.
///
/// The value only changes how numbers are rendered. Every stored percentage
/// stays "used", so colors and warning thresholds keep their meaning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageDisplayMode {
    #[default]
    Used,
    Remaining,
}

impl UsageDisplayMode {
    pub fn display_percentage(self, used_percentage: f64) -> f64 {
        let used_percentage = used_percentage.clamp(0.0, 100.0);
        match self {
            Self::Used => used_percentage,
            Self::Remaining => 100.0 - used_percentage,
        }
    }
}

static USAGE_DISPLAY: AtomicU8 = AtomicU8::new(0);

pub fn set_usage_display(mode: UsageDisplayMode) {
    USAGE_DISPLAY.store(
        match mode {
            UsageDisplayMode::Used => 0,
            UsageDisplayMode::Remaining => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn usage_display() -> UsageDisplayMode {
    match USAGE_DISPLAY.load(Ordering::Relaxed) {
        1 => UsageDisplayMode::Remaining,
        _ => UsageDisplayMode::Used,
    }
}

/// Convert a stored (used) percentage into the number the user should see.
pub fn display_percentage(used_percentage: f64) -> f64 {
    usage_display().display_percentage(used_percentage)
}

#[derive(Clone, Debug, Default)]
pub struct UsageSection {
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
}

#[derive(Clone, Debug, Default)]
pub struct UsageData {
    pub session: UsageSection,
    pub weekly: UsageSection,
}

#[derive(Clone, Debug, Default)]
pub struct AppUsageData {
    pub claude_code: Option<UsageData>,
    pub codex: Option<UsageData>,
    pub antigravity: Option<UsageData>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_percentage_inverts_only_in_remaining_mode() {
        for (used, expected_used, expected_remaining) in [
            (0.0, 0.0, 100.0),
            (37.0, 37.0, 63.0),
            (100.0, 100.0, 0.0),
            // Out-of-range input is clamped before inverting.
            (-5.0, 0.0, 100.0),
            (105.0, 100.0, 0.0),
        ] {
            assert_eq!(UsageDisplayMode::Used.display_percentage(used), expected_used);
            assert_eq!(
                UsageDisplayMode::Remaining.display_percentage(used),
                expected_remaining
            );
        }
    }

    #[test]
    fn usage_display_default_is_used() {
        assert_eq!(UsageDisplayMode::default(), UsageDisplayMode::Used);
    }
}
