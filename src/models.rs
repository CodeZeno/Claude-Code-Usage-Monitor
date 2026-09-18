use std::time::SystemTime;

pub const GITHUB_COPILOT_MONTHLY_ITEM_ID: &str = "monthly_ai_credits";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuotaFamilyId {
    Claude,
    Codex,
    Antigravity,
    GithubCopilot,
    VercelAiGateway,
}

impl QuotaFamilyId {
    pub const fn stable_id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
            Self::GithubCopilot => "github_copilot",
            Self::VercelAiGateway => "vercel_ai_gateway",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Antigravity => "Antigravity",
            Self::GithubCopilot => "GitHub Copilot",
            Self::VercelAiGateway => "Vercel AI Gateway",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaFamilyStatus {
    Available,
    Unavailable,
    Disabled,
    Stale,
}

#[derive(Clone, Debug, PartialEq)]
pub enum QuotaMetric {
    Percentage(f64),
    Used { used: f64, limit: Option<f64> },
    Remaining { remaining: f64, limit: Option<f64> },
}

impl QuotaMetric {
    pub fn used_percentage(&self) -> Option<f64> {
        match self {
            Self::Percentage(value) => Some(*value),
            Self::Used {
                used,
                limit: Some(limit),
            } if *limit > 0.0 => Some(*used / *limit * 100.0),
            Self::Remaining {
                remaining,
                limit: Some(limit),
            } if *limit > 0.0 => Some((1.0 - *remaining / *limit) * 100.0),
            _ => None,
        }
    }

    pub fn used(&self) -> Option<f64> {
        match self {
            Self::Used { used, .. } => Some(*used),
            Self::Remaining {
                remaining,
                limit: Some(limit),
            } => Some((*limit - *remaining).max(0.0)),
            Self::Percentage(_) | Self::Remaining { limit: None, .. } => None,
        }
    }

    pub fn remaining(&self) -> Option<f64> {
        match self {
            Self::Remaining { remaining, .. } => Some(*remaining),
            Self::Used {
                used,
                limit: Some(limit),
            } => Some((*limit - *used).max(0.0)),
            Self::Percentage(_) | Self::Used { limit: None, .. } => None,
        }
    }

    pub fn limit(&self) -> Option<f64> {
        match self {
            Self::Percentage(_) => None,
            Self::Used { limit, .. } | Self::Remaining { limit, .. } => *limit,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum QuotaUnit {
    Percent,
    AiCredits,
    Other(String),
}

impl QuotaUnit {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Percent => "percent",
            Self::AiCredits => "ai-credits",
            Self::Other(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaItemAvailability {
    Available,
    Unavailable,
    Stale,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuotaItem {
    pub id: String,
    pub label: String,
    pub availability: QuotaItemAvailability,
    pub metric: Option<QuotaMetric>,
    pub unit: QuotaUnit,
    pub resets_at: Option<SystemTime>,
}

impl QuotaItem {
    pub fn percentage(
        id: impl Into<String>,
        label: impl Into<String>,
        percentage: f64,
        resets_at: Option<SystemTime>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            availability: QuotaItemAvailability::Available,
            metric: Some(QuotaMetric::Percentage(percentage)),
            unit: QuotaUnit::Percent,
            resets_at,
        }
    }

    pub fn unavailable(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            availability: QuotaItemAvailability::Unavailable,
            metric: None,
            unit: QuotaUnit::Percent,
            resets_at: None,
        }
    }

    pub fn used_percentage(&self) -> Option<f64> {
        (self.availability == QuotaItemAvailability::Available)
            .then(|| self.metric.as_ref().and_then(QuotaMetric::used_percentage))
            .flatten()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuotaFamily {
    pub id: QuotaFamilyId,
    pub display_name: String,
    pub status: QuotaFamilyStatus,
    pub items: Vec<QuotaItem>,
    pub(crate) banked_reset_count: BankedResetCount,
}

impl QuotaFamily {
    pub fn available(id: QuotaFamilyId, items: Vec<QuotaItem>) -> Self {
        Self {
            id,
            display_name: id.display_name().to_string(),
            status: QuotaFamilyStatus::Available,
            items,
            banked_reset_count: BankedResetCount::Unavailable,
        }
    }

    pub fn with_status(id: QuotaFamilyId, status: QuotaFamilyStatus) -> Self {
        Self {
            id,
            display_name: id.display_name().to_string(),
            status,
            items: Vec::new(),
            banked_reset_count: BankedResetCount::Unavailable,
        }
    }

    pub fn item(&self, id: &str) -> Option<&QuotaItem> {
        self.items.iter().find(|item| item.id == id)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum BankedResetCount {
    Available(u64),
    #[default]
    Unavailable,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageSection {
    pub percentage: f64,
    pub resets_at: Option<SystemTime>,
}

#[derive(Clone, Debug, Default)]
pub struct UsageData {
    pub session: UsageSection,
    pub weekly: UsageSection,
    pub(crate) banked_reset_count: BankedResetCount,
    session_available: bool,
    weekly_available: bool,
    custom_items: Vec<QuotaItem>,
}

impl UsageData {
    pub(crate) fn set_session(&mut self, section: UsageSection) {
        self.session = section;
        self.session_available = true;
    }

    pub(crate) fn set_weekly(&mut self, section: UsageSection) {
        self.weekly = section;
        self.weekly_available = true;
    }

    pub(crate) fn session_available(&self) -> bool {
        self.session_available
    }

    pub(crate) fn weekly_available(&self) -> bool {
        self.weekly_available
    }

    pub(crate) fn from_quota_items(items: Vec<QuotaItem>) -> Self {
        Self {
            custom_items: items,
            ..Self::default()
        }
    }

    pub(crate) fn quota_items(&self) -> Vec<QuotaItem> {
        if !self.custom_items.is_empty() {
            return self.custom_items.clone();
        }
        let mut items = Vec::with_capacity(2);
        if self.weekly_available {
            items.push(QuotaItem::percentage(
                "weekly",
                "7d",
                self.weekly.percentage,
                self.weekly.resets_at,
            ));
        }
        if self.session_available {
            items.push(QuotaItem::percentage(
                "session",
                "5h",
                self.session.percentage,
                self.session.resets_at,
            ));
        }
        items
    }

    pub(crate) fn into_quota_family(self, id: QuotaFamilyId) -> QuotaFamily {
        let items = self.quota_items();
        let mut family = QuotaFamily::available(id, items);
        family.banked_reset_count = self.banked_reset_count;
        family
    }
}

#[derive(Clone, Debug, Default)]
pub struct AppUsageData {
    pub families: Vec<QuotaFamily>,
}

impl AppUsageData {
    pub fn family(&self, id: QuotaFamilyId) -> Option<&QuotaFamily> {
        self.families.iter().find(|family| family.id == id)
    }

    pub fn upsert(&mut self, family: QuotaFamily) {
        if let Some(existing) = self
            .families
            .iter_mut()
            .find(|existing| existing.id == family.id)
        {
            *existing = family;
        } else {
            self.families.push(family);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_usage_has_no_available_windows() {
        let usage = UsageData::default();

        assert!(!usage.session_available());
        assert!(!usage.weekly_available());
        assert_eq!(usage.session.percentage, 0.0);
        assert_eq!(usage.weekly.percentage, 0.0);
        assert_eq!(usage.banked_reset_count, BankedResetCount::Unavailable);
    }

    #[test]
    fn setters_replace_sections_and_mark_them_available() {
        let mut usage = UsageData::default();
        usage.set_session(UsageSection {
            percentage: 0.0,
            resets_at: None,
        });
        usage.set_weekly(UsageSection {
            percentage: 42.0,
            resets_at: None,
        });

        assert!(usage.session_available());
        assert!(usage.weekly_available());
        assert_eq!(usage.session.percentage, 0.0);
        assert_eq!(usage.weekly.percentage, 42.0);
    }

    #[test]
    fn quota_family_supports_one_or_many_items_without_fixed_windows() {
        let one = QuotaFamily::available(
            QuotaFamilyId::GithubCopilot,
            vec![QuotaItem::percentage("monthly", "Monthly", 25.0, None)],
        );
        assert_eq!(one.items.len(), 1);

        let many = QuotaFamily::available(
            QuotaFamilyId::Claude,
            vec![
                QuotaItem::percentage("session", "5h", 10.0, None),
                QuotaItem::percentage("weekly", "7d", 20.0, None),
            ],
        );
        assert_eq!(many.items.len(), 2);
        assert_eq!(many.item("weekly").unwrap().used_percentage(), Some(20.0));
    }

    #[test]
    fn quota_metrics_cover_percentage_counts_and_remaining_forms() {
        let percentage = QuotaMetric::Percentage(12.5);
        assert_eq!(percentage.used_percentage(), Some(12.5));

        let used_with_limit = QuotaMetric::Used {
            used: 375.0,
            limit: Some(1_500.0),
        };
        assert_eq!(used_with_limit.used_percentage(), Some(25.0));
        assert_eq!(used_with_limit.remaining(), Some(1_125.0));
        assert_eq!(used_with_limit.limit(), Some(1_500.0));

        let used_only = QuotaMetric::Used {
            used: 17.0,
            limit: None,
        };
        assert_eq!(used_only.used(), Some(17.0));
        assert_eq!(used_only.used_percentage(), None);

        let remaining = QuotaMetric::Remaining {
            remaining: 300.0,
            limit: Some(1_500.0),
        };
        assert_eq!(remaining.used(), Some(1_200.0));
        assert_eq!(remaining.used_percentage(), Some(80.0));
    }

    #[test]
    fn reset_only_unavailable_zero_absent_and_disabled_are_distinct() {
        let reset_at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400);
        let reset_only = QuotaItem {
            id: "reset-only".to_string(),
            label: "Reset only".to_string(),
            availability: QuotaItemAvailability::Available,
            metric: None,
            unit: QuotaUnit::Other("requests".to_string()),
            resets_at: Some(reset_at),
        };
        assert_eq!(reset_only.used_percentage(), None);
        assert_eq!(reset_only.resets_at, Some(reset_at));

        let unavailable = QuotaItem::unavailable("unavailable", "Unavailable");
        let zero = QuotaItem::percentage("zero", "Zero", 0.0, None);
        assert_eq!(unavailable.used_percentage(), None);
        assert_eq!(zero.used_percentage(), Some(0.0));

        let mut data = AppUsageData::default();
        assert!(data.family(QuotaFamilyId::Codex).is_none());
        data.upsert(QuotaFamily {
            id: QuotaFamilyId::Codex,
            display_name: "Codex".to_string(),
            status: QuotaFamilyStatus::Disabled,
            items: Vec::new(),
            banked_reset_count: BankedResetCount::Unavailable,
        });
        assert_eq!(
            data.family(QuotaFamilyId::Codex).unwrap().status,
            QuotaFamilyStatus::Disabled
        );
    }
}
