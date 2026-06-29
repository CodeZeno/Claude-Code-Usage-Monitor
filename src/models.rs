use std::time::SystemTime;

#[derive(Clone, Debug, Default)]
pub struct UsageSection {
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
    pub has_bucket: bool,
}

#[derive(Clone, Debug, Default)]
pub struct UsageData {
    pub session: UsageSection,
    pub weekly: UsageSection,
}

#[derive(Clone, Debug, Default)]
pub struct AccountUsage {
    pub credit_pct: f64,
    pub credit_expiry: Option<SystemTime>,
    pub spend_used: f64,
    pub spend_limit: f64,
}

#[derive(Clone, Debug, Default)]
pub struct SpendPaceSlots {
    pub month_actual: f64,
    pub month_cap: f64,
    pub month_expected: f64,
    pub month_level: u8,
    pub week_actual: f64,
    pub week_cap: f64,
    pub week_expected: f64,
    pub week_level: u8,
    pub day_actual: f64,
    pub day_cap: f64,
    pub day_expected: f64,
    pub day_level: u8,
}

#[derive(Clone, Debug)]
pub struct SpendPaceView {
    pub credit_pct: f64,
    pub credit_expiry: Option<SystemTime>,
    pub slots: SpendPaceSlots,
}

#[derive(Clone, Debug, Default)]
pub struct AppUsageData {
    pub claude_code: Option<UsageData>,
    pub codex: Option<UsageData>,
    pub antigravity: Option<UsageData>,
    pub account: Option<AccountUsage>,
    pub spend_pace: Option<SpendPaceView>,
}
