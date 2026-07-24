use std::time::SystemTime;

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
    /// Ollama Cloud plan-tier usage. Polled from ollama.com/settings
    /// using the user's browser-captured session cookie. `None` when the
    /// cookie is missing or the poll fails (e.g. session expired).
    pub ollama: Option<UsageData>,
}
