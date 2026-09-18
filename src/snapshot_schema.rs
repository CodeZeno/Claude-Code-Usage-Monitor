use std::{
    collections::BTreeMap,
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::models::{
    QuotaFamilyId, QuotaItem, QuotaItemAvailability, QuotaMetric, UsageData, UsageSection,
};
use crate::poller::{PollError, PollReport, ProviderPollOutcome, ProviderPollSource};
use crate::snapshot_store::MachineId;

const SCHEMA_VERSION: u8 = 1;
const MAX_PROVIDER_ERROR_MESSAGE_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapshotConversionError {
    TimestampBeforeUnixEpoch,
    TimestampMillisOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapshotValidationError {
    InvalidMachineId,
    InvalidProviderState,
    InvalidProviderSource,
    InvalidUsageWindow,
    ErrorMessageTooLong,
    InvalidErrorMessage,
    InvalidErrorRetryability,
}

impl fmt::Display for SnapshotValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidMachineId => "invalid snapshot machine ID",
            Self::InvalidProviderState => "invalid snapshot provider state",
            Self::InvalidProviderSource => "invalid snapshot provider source",
            Self::InvalidUsageWindow => "invalid snapshot usage window",
            Self::ErrorMessageTooLong => "snapshot error message is too long",
            Self::InvalidErrorMessage => "invalid snapshot error message",
            Self::InvalidErrorRetryability => "invalid snapshot error retryability",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for SnapshotValidationError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapshotDeserializeError {
    InvalidJson,
    Validation(SnapshotValidationError),
}

impl fmt::Display for SnapshotDeserializeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson => formatter.write_str("invalid snapshot JSON"),
            Self::Validation(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for SnapshotDeserializeError {}

pub(crate) fn deserialize_validated_snapshot(
    json: &str,
) -> Result<SnapshotV1, SnapshotDeserializeError> {
    let snapshot: SnapshotV1 =
        serde_json::from_str(json).map_err(|_| SnapshotDeserializeError::InvalidJson)?;
    snapshot
        .validate()
        .map_err(SnapshotDeserializeError::Validation)?;
    Ok(snapshot)
}

pub(crate) fn snapshot_from_poll_report(
    machine_id: &MachineId,
    report: &PollReport,
    captured_at: SystemTime,
) -> Result<SnapshotV1, SnapshotConversionError> {
    let captured_at = system_time_to_unix_millis(captured_at)?;
    let poll_started_at = earliest_attempted_at(report)
        .map(system_time_to_unix_millis)
        .transpose()?
        .unwrap_or(captured_at);
    let providers = Providers::new(
        provider_snapshot_from_outcome(&report.claude_code)?,
        provider_snapshot_from_outcome(&report.codex)?,
        provider_snapshot_from_outcome(&report.antigravity)?,
    );
    let mut providers = providers;
    providers.insert(
        QuotaFamilyId::GithubCopilot.stable_id(),
        provider_snapshot_from_outcome(&report.github_copilot)?,
    );
    providers.insert(
        "vercel_ai_gateway",
        provider_snapshot_from_outcome(&report.vercel_ai_gateway)?,
    );

    Ok(SnapshotV1::new(
        machine_id.as_str().to_string(),
        captured_at,
        poll_started_at,
        captured_at,
        providers,
    ))
}

fn provider_snapshot_from_outcome(
    outcome: &ProviderPollOutcome,
) -> Result<ProviderSnapshot, SnapshotConversionError> {
    match outcome {
        ProviderPollOutcome::Disabled => Ok(ProviderSnapshot::disabled()),
        ProviderPollOutcome::Success {
            source,
            attempted_at,
            acquired_at,
            usage,
        } => Ok(ProviderSnapshot::success(
            provider_source(*source),
            system_time_to_unix_millis(*attempted_at)?,
            system_time_to_unix_millis(*acquired_at)?,
            provider_usage(usage)?,
        )),
        ProviderPollOutcome::Error {
            source,
            attempted_at,
            error,
        } => Ok(ProviderSnapshot::error(
            Some(provider_source(*source)),
            system_time_to_unix_millis(*attempted_at)?,
            provider_error(*error),
        )),
    }
}

fn provider_source(source: ProviderPollSource) -> ProviderSource {
    match source {
        ProviderPollSource::AnthropicOauthUsage => ProviderSource::AnthropicOauthUsage,
        ProviderPollSource::ChatgptWhamUsage => ProviderSource::ChatgptWhamUsage,
        ProviderPollSource::AntigravityQuotaUsage => ProviderSource::AntigravityQuotaUsage,
        ProviderPollSource::GithubBillingApi => ProviderSource::GithubBillingApi,
        ProviderPollSource::VercelAiGatewayQuotasApi => ProviderSource::VercelAiGatewayQuotasApi,
    }
}

fn provider_error(error: PollError) -> ProviderError {
    let code = match error {
        PollError::AuthRequired => ProviderErrorCode::AuthRequired,
        PollError::NoCredentials => ProviderErrorCode::NoCredentials,
        PollError::TokenExpired => ProviderErrorCode::TokenExpired,
        PollError::RequestFailed => ProviderErrorCode::RequestFailed,
    };

    ProviderError::from_code(code)
}

fn provider_usage(usage: &UsageData) -> Result<ProviderUsage, SnapshotConversionError> {
    let session = usage
        .session_available()
        .then(|| usage_window(&usage.session))
        .transpose()?;
    let weekly = usage
        .weekly_available()
        .then(|| usage_window(&usage.weekly))
        .transpose()?;

    let quota_items = usage
        .quota_items()
        .iter()
        .map(SnapshotQuotaItem::from_runtime)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ProviderUsage::with_quota_items(
        session,
        weekly,
        quota_items,
    ))
}

fn usage_window(section: &UsageSection) -> Result<UsageWindow, SnapshotConversionError> {
    let resets_at = section
        .resets_at
        .map(system_time_to_unix_millis)
        .transpose()?;

    Ok(UsageWindow::new(Some(section.percentage), resets_at))
}

fn earliest_attempted_at(report: &PollReport) -> Option<SystemTime> {
    [
        attempted_at(&report.claude_code),
        attempted_at(&report.codex),
        attempted_at(&report.antigravity),
        attempted_at(&report.github_copilot),
        attempted_at(&report.vercel_ai_gateway),
    ]
    .into_iter()
    .flatten()
    .min()
}

fn attempted_at(outcome: &ProviderPollOutcome) -> Option<SystemTime> {
    match outcome {
        ProviderPollOutcome::Disabled => None,
        ProviderPollOutcome::Success { attempted_at, .. }
        | ProviderPollOutcome::Error { attempted_at, .. } => Some(*attempted_at),
    }
}

fn system_time_to_unix_millis(time: SystemTime) -> Result<u64, SnapshotConversionError> {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SnapshotConversionError::TimestampBeforeUnixEpoch)?;
    duration_to_unix_millis(duration)
}

fn duration_to_unix_millis(duration: Duration) -> Result<u64, SnapshotConversionError> {
    u64::try_from(duration.as_millis())
        .map_err(|_| SnapshotConversionError::TimestampMillisOverflow)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotV1 {
    schema_version: SchemaVersion,
    timestamp_unit: TimestampUnit,
    machine_id: String,
    generated_at: u64,
    poll_started_at: u64,
    poll_finished_at: u64,
    providers: Providers,
}

impl SnapshotV1 {
    pub fn new(
        machine_id: String,
        generated_at: u64,
        poll_started_at: u64,
        poll_finished_at: u64,
        providers: Providers,
    ) -> Self {
        Self {
            schema_version: SchemaVersion,
            timestamp_unit: TimestampUnit::UnixMs,
            machine_id,
            generated_at,
            poll_started_at,
            poll_finished_at,
            providers,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), SnapshotValidationError> {
        MachineId::parse(&self.machine_id)
            .map_err(|_| SnapshotValidationError::InvalidMachineId)?;
        self.providers.validate()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SchemaVersion;

impl Serialize for SchemaVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(SCHEMA_VERSION)
    }
}

impl<'de> Deserialize<'de> for SchemaVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u8::deserialize(deserializer)?;
        if value == SCHEMA_VERSION {
            Ok(Self)
        } else {
            Err(de::Error::custom("unsupported snapshot schema version"))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TimestampUnit {
    UnixMs,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Providers(BTreeMap<String, ProviderSnapshot>);

impl Providers {
    pub fn new(
        claude_code: ProviderSnapshot,
        codex: ProviderSnapshot,
        antigravity: ProviderSnapshot,
    ) -> Self {
        Self(BTreeMap::from([
            ("claude_code".to_string(), claude_code),
            ("codex".to_string(), codex),
            ("antigravity".to_string(), antigravity),
        ]))
    }

    pub fn insert(&mut self, family_id: impl Into<String>, snapshot: ProviderSnapshot) {
        self.0.insert(family_id.into(), snapshot);
    }

    pub fn get(&self, family_id: &str) -> Option<&ProviderSnapshot> {
        self.0.get(family_id)
    }

    fn validate(&self) -> Result<(), SnapshotValidationError> {
        for (family_id, provider) in &self.0 {
            let expected_source = match family_id.as_str() {
                "claude_code" | "claude" => Some(ProviderSource::AnthropicOauthUsage),
                "codex" => Some(ProviderSource::ChatgptWhamUsage),
                "antigravity" => Some(ProviderSource::AntigravityQuotaUsage),
                "github_copilot" => Some(ProviderSource::GithubBillingApi),
                "vercel_ai_gateway" => Some(ProviderSource::VercelAiGatewayQuotasApi),
                _ => provider.source,
            };
            provider.validate(expected_source)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    requested: bool,
    status: ProviderStatus,
    source: Option<ProviderSource>,
    attempted_at: Option<u64>,
    acquired_at: Option<u64>,
    last_success_at: Option<u64>,
    stale: bool,
    usage: Option<ProviderUsage>,
    error: Option<ProviderError>,
}

impl ProviderSnapshot {
    pub fn success(
        source: ProviderSource,
        attempted_at: u64,
        acquired_at: u64,
        usage: ProviderUsage,
    ) -> Self {
        Self {
            requested: true,
            status: ProviderStatus::Success,
            source: Some(source),
            attempted_at: Some(attempted_at),
            acquired_at: Some(acquired_at),
            last_success_at: Some(acquired_at),
            stale: false,
            usage: Some(usage),
            error: None,
        }
    }

    pub fn error(source: Option<ProviderSource>, attempted_at: u64, error: ProviderError) -> Self {
        Self {
            requested: true,
            status: ProviderStatus::Error,
            source,
            attempted_at: Some(attempted_at),
            acquired_at: None,
            last_success_at: None,
            stale: false,
            usage: None,
            error: Some(error),
        }
    }

    pub fn stale(
        source: Option<ProviderSource>,
        attempted_at: u64,
        last_success_at: u64,
        usage: ProviderUsage,
        error: ProviderError,
    ) -> Self {
        Self {
            requested: true,
            status: ProviderStatus::Stale,
            source,
            attempted_at: Some(attempted_at),
            acquired_at: Some(last_success_at),
            last_success_at: Some(last_success_at),
            stale: true,
            usage: Some(usage),
            error: Some(error),
        }
    }

    pub fn disabled() -> Self {
        Self {
            requested: false,
            status: ProviderStatus::Disabled,
            source: None,
            attempted_at: None,
            acquired_at: None,
            last_success_at: None,
            stale: false,
            usage: None,
            error: None,
        }
    }

    fn validate(
        &self,
        expected_source: Option<ProviderSource>,
    ) -> Result<(), SnapshotValidationError> {
        let valid_state = match self.status {
            ProviderStatus::Success => {
                self.requested
                    && self.source.is_some()
                    && self.attempted_at.is_some()
                    && self.acquired_at.is_some()
                    && self.last_success_at == self.acquired_at
                    && !self.stale
                    && self.usage.is_some()
                    && self.error.is_none()
            }
            ProviderStatus::Error => {
                self.requested
                    && self.attempted_at.is_some()
                    && self.acquired_at.is_none()
                    && self.last_success_at.is_none()
                    && !self.stale
                    && self.usage.is_none()
                    && self.error.is_some()
            }
            ProviderStatus::Stale => {
                self.requested
                    && self.attempted_at.is_some()
                    && self.acquired_at.is_some()
                    && self.last_success_at == self.acquired_at
                    && self.stale
                    && self.usage.is_some()
                    && self.error.is_some()
            }
            ProviderStatus::Disabled => {
                !self.requested
                    && self.source.is_none()
                    && self.attempted_at.is_none()
                    && self.acquired_at.is_none()
                    && self.last_success_at.is_none()
                    && !self.stale
                    && self.usage.is_none()
                    && self.error.is_none()
            }
        };
        if !valid_state {
            return Err(SnapshotValidationError::InvalidProviderState);
        }

        match expected_source {
            Some(expected) if self.source.is_some_and(|source| source != expected) => {
                return Err(SnapshotValidationError::InvalidProviderSource);
            }
            None if self.status != ProviderStatus::Disabled => {
                return Err(SnapshotValidationError::InvalidProviderSource);
            }
            _ => {}
        }

        if let Some(usage) = &self.usage {
            usage.validate()?;
        }
        if let Some(error) = &self.error {
            error.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Success,
    Error,
    Stale,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSource {
    AnthropicOauthUsage,
    ChatgptWhamUsage,
    AntigravityQuotaUsage,
    GithubBillingApi,
    VercelAiGatewayQuotasApi,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderUsage {
    session: Option<UsageWindow>,
    weekly: Option<UsageWindow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    quota_items: Vec<SnapshotQuotaItem>,
}

impl ProviderUsage {
    pub fn new(session: Option<UsageWindow>, weekly: Option<UsageWindow>) -> Self {
        Self {
            session,
            weekly,
            quota_items: Vec::new(),
        }
    }

    pub fn with_quota_items(
        session: Option<UsageWindow>,
        weekly: Option<UsageWindow>,
        quota_items: Vec<SnapshotQuotaItem>,
    ) -> Self {
        Self {
            session,
            weekly,
            quota_items,
        }
    }

    fn validate(&self) -> Result<(), SnapshotValidationError> {
        if let Some(window) = &self.session {
            window.validate()?;
        }
        if let Some(window) = &self.weekly {
            window.validate()?;
        }
        for item in &self.quota_items {
            item.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotQuotaItem {
    id: String,
    label: String,
    available: bool,
    metric: Option<SnapshotQuotaMetric>,
    unit: String,
    resets_at: Option<u64>,
}

impl SnapshotQuotaItem {
    fn from_runtime(item: &QuotaItem) -> Result<Self, SnapshotConversionError> {
        let metric = item.metric.as_ref().map(|metric| match metric {
            QuotaMetric::Percentage(value) => SnapshotQuotaMetric::Percentage { value: *value },
            QuotaMetric::Used { used, limit } => SnapshotQuotaMetric::Used {
                used: *used,
                limit: *limit,
            },
            QuotaMetric::Remaining { remaining, limit } => SnapshotQuotaMetric::Remaining {
                remaining: *remaining,
                limit: *limit,
            },
        });
        Ok(Self {
            id: item.id.clone(),
            label: item.label.clone(),
            available: item.availability != QuotaItemAvailability::Unavailable,
            metric,
            unit: item.unit.as_str().to_string(),
            resets_at: item.resets_at.map(system_time_to_unix_millis).transpose()?,
        })
    }

    fn from_percentage(
        id: &str,
        label: &str,
        section: &UsageSection,
    ) -> Result<Self, SnapshotConversionError> {
        Ok(Self {
            id: id.to_string(),
            label: label.to_string(),
            available: true,
            metric: Some(SnapshotQuotaMetric::Percentage {
                value: section.percentage,
            }),
            unit: "percent".to_string(),
            resets_at: section
                .resets_at
                .map(system_time_to_unix_millis)
                .transpose()?,
        })
    }

    fn validate(&self) -> Result<(), SnapshotValidationError> {
        if self.id.is_empty() || self.label.is_empty() || self.unit.is_empty() {
            return Err(SnapshotValidationError::InvalidUsageWindow);
        }
        if let Some(metric) = &self.metric {
            metric.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SnapshotQuotaMetric {
    Percentage { value: f64 },
    Used { used: f64, limit: Option<f64> },
    Remaining { remaining: f64, limit: Option<f64> },
}

impl SnapshotQuotaMetric {
    fn validate(&self) -> Result<(), SnapshotValidationError> {
        let valid = match self {
            Self::Percentage { value } => value.is_finite() && *value >= 0.0,
            Self::Used { used, limit } => {
                used.is_finite()
                    && *used >= 0.0
                    && limit.is_none_or(|value| value.is_finite() && value >= 0.0)
            }
            Self::Remaining { remaining, limit } => {
                remaining.is_finite()
                    && *remaining >= 0.0
                    && limit.is_none_or(|value| value.is_finite() && value >= 0.0)
            }
        };
        valid
            .then_some(())
            .ok_or(SnapshotValidationError::InvalidUsageWindow)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    used_percent: Option<f64>,
    resets_at: Option<u64>,
}

impl UsageWindow {
    pub fn new(used_percent: Option<f64>, resets_at: Option<u64>) -> Self {
        Self {
            used_percent,
            resets_at,
        }
    }

    fn validate(&self) -> Result<(), SnapshotValidationError> {
        match self.used_percent {
            Some(value) if value.is_finite() && value >= 0.0 => Ok(()),
            _ => Err(SnapshotValidationError::InvalidUsageWindow),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderError {
    code: ProviderErrorCode,
    message: String,
    retryable: bool,
}

impl ProviderError {
    pub fn from_code(code: ProviderErrorCode) -> Self {
        let (message, retryable) = match code {
            ProviderErrorCode::AuthRequired => ("Authentication required", false),
            ProviderErrorCode::NoCredentials => ("Credentials not found", false),
            ProviderErrorCode::TokenExpired => ("Token expired", false),
            ProviderErrorCode::RequestFailed => ("Provider request failed", true),
        };

        Self {
            code,
            message: message.to_string(),
            retryable,
        }
    }

    fn validate(&self) -> Result<(), SnapshotValidationError> {
        if self.message.len() > MAX_PROVIDER_ERROR_MESSAGE_LEN {
            return Err(SnapshotValidationError::ErrorMessageTooLong);
        }

        let expected = Self::from_code(self.code);
        if self.message != expected.message {
            return Err(SnapshotValidationError::InvalidErrorMessage);
        }
        if self.retryable != expected.retryable {
            return Err(SnapshotValidationError::InvalidErrorRetryability);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorCode {
    AuthRequired,
    NoCredentials,
    TokenExpired,
    RequestFailed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{QuotaFamilyStatus, QuotaUnit};
    use serde_json::{json, Value};

    fn usage() -> ProviderUsage {
        ProviderUsage::new(
            Some(UsageWindow::new(Some(42.5), Some(1_725_000_000_000))),
            Some(UsageWindow::new(Some(63.0), None)),
        )
    }

    fn snapshot_with(claude_code: ProviderSnapshot) -> SnapshotV1 {
        SnapshotV1::new(
            "home".to_string(),
            1_725_000_000_200,
            1_725_000_000_000,
            1_725_000_000_100,
            Providers::new(
                claude_code,
                ProviderSnapshot::disabled(),
                ProviderSnapshot::disabled(),
            ),
        )
    }

    fn at_millis(milliseconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(milliseconds)
    }

    fn machine_id() -> MachineId {
        MachineId::parse("home").expect("test machine ID should be valid")
    }

    fn poll_usage(
        session: Option<(f64, Option<SystemTime>)>,
        weekly: Option<(f64, Option<SystemTime>)>,
    ) -> UsageData {
        let mut usage = UsageData::default();
        if let Some((percentage, resets_at)) = session {
            usage.set_session(UsageSection {
                percentage,
                resets_at,
            });
        }
        if let Some((percentage, resets_at)) = weekly {
            usage.set_weekly(UsageSection {
                percentage,
                resets_at,
            });
        }
        usage
    }

    fn success_outcome(source: ProviderPollSource, usage: UsageData) -> ProviderPollOutcome {
        ProviderPollOutcome::Success {
            source,
            attempted_at: at_millis(1_725_000_000_000),
            acquired_at: at_millis(1_725_000_000_100),
            usage,
        }
    }

    fn error_outcome(source: ProviderPollSource, error: PollError) -> ProviderPollOutcome {
        ProviderPollOutcome::Error {
            source,
            attempted_at: at_millis(1_725_000_000_000),
            error,
        }
    }

    fn poll_report(claude_code: ProviderPollOutcome, codex: ProviderPollOutcome) -> PollReport {
        PollReport {
            claude_code,
            codex,
            antigravity: ProviderPollOutcome::Disabled,
            github_copilot: ProviderPollOutcome::Disabled,
        }
    }

    fn converted_value(report: &PollReport) -> Value {
        let snapshot =
            snapshot_from_poll_report(&machine_id(), report, at_millis(1_725_000_000_200))
                .expect("report should convert");
        serde_json::to_value(snapshot).expect("snapshot should serialize")
    }

    fn validated_from_value(value: Value) -> Result<SnapshotV1, SnapshotDeserializeError> {
        let json = serde_json::to_string(&value).expect("snapshot value should serialize");
        deserialize_validated_snapshot(&json)
    }

    #[test]
    fn schema_constants_are_fixed() {
        let snapshot = snapshot_with(ProviderSnapshot::disabled());
        let mut value = serde_json::to_value(snapshot).expect("snapshot should serialize");

        assert_eq!(value["schema_version"], json!(1));
        assert_eq!(value["timestamp_unit"], json!("unix_ms"));

        value["schema_version"] = json!(2);
        assert!(serde_json::from_value::<SnapshotV1>(value).is_err());
    }

    #[test]
    fn success_serialization_has_usage_without_error() {
        let provider = ProviderSnapshot::success(
            ProviderSource::AnthropicOauthUsage,
            1_725_000_000_000,
            1_725_000_000_100,
            usage(),
        );
        let value =
            serde_json::to_value(snapshot_with(provider)).expect("snapshot should serialize");
        let provider = &value["providers"]["claude_code"];

        assert_eq!(provider["requested"], json!(true));
        assert_eq!(provider["status"], json!("success"));
        assert_eq!(provider["stale"], json!(false));
        assert!(provider["usage"].is_object());
        assert_eq!(provider["error"], Value::Null);
    }

    #[test]
    fn error_serialization_has_error_without_usage() {
        let provider = ProviderSnapshot::error(
            Some(ProviderSource::ChatgptWhamUsage),
            1_725_000_000_000,
            ProviderError::from_code(ProviderErrorCode::RequestFailed),
        );
        let value =
            serde_json::to_value(snapshot_with(provider)).expect("snapshot should serialize");
        let provider = &value["providers"]["claude_code"];

        assert_eq!(provider["status"], json!("error"));
        assert_eq!(provider["stale"], json!(false));
        assert_eq!(provider["usage"], Value::Null);
        assert_eq!(provider["error"]["code"], json!("request_failed"));
    }

    #[test]
    fn stale_serialization_keeps_usage_and_error() {
        let provider = ProviderSnapshot::stale(
            Some(ProviderSource::AnthropicOauthUsage),
            1_725_000_000_000,
            1_724_999_000_000,
            usage(),
            ProviderError::from_code(ProviderErrorCode::AuthRequired),
        );
        let value =
            serde_json::to_value(snapshot_with(provider)).expect("snapshot should serialize");
        let provider = &value["providers"]["claude_code"];

        assert_eq!(provider["status"], json!("stale"));
        assert_eq!(provider["stale"], json!(true));
        assert!(provider["usage"].is_object());
        assert!(provider["error"].is_object());
    }

    #[test]
    fn disabled_serialization_has_no_attempt_or_result() {
        let value = serde_json::to_value(snapshot_with(ProviderSnapshot::disabled()))
            .expect("snapshot should serialize");
        let provider = &value["providers"]["claude_code"];

        assert_eq!(provider["requested"], json!(false));
        assert_eq!(provider["status"], json!("disabled"));
        assert_eq!(provider["source"], Value::Null);
        assert_eq!(provider["attempted_at"], Value::Null);
        assert_eq!(provider["acquired_at"], Value::Null);
        assert_eq!(provider["last_success_at"], Value::Null);
        assert_eq!(provider["stale"], json!(false));
        assert_eq!(provider["usage"], Value::Null);
        assert_eq!(provider["error"], Value::Null);
    }

    #[test]
    fn enums_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_value(ProviderStatus::Success).unwrap(),
            json!("success")
        );
        assert_eq!(
            serde_json::to_value(ProviderStatus::Error).unwrap(),
            json!("error")
        );
        assert_eq!(
            serde_json::to_value(ProviderStatus::Stale).unwrap(),
            json!("stale")
        );
        assert_eq!(
            serde_json::to_value(ProviderStatus::Disabled).unwrap(),
            json!("disabled")
        );
        assert_eq!(
            serde_json::to_value(ProviderSource::AnthropicOauthUsage).unwrap(),
            json!("anthropic_oauth_usage")
        );
        assert_eq!(
            serde_json::to_value(ProviderSource::ChatgptWhamUsage).unwrap(),
            json!("chatgpt_wham_usage")
        );
        assert_eq!(
            serde_json::to_value(ProviderSource::AntigravityQuotaUsage).unwrap(),
            json!("antigravity_quota_usage")
        );
        assert_eq!(
            serde_json::to_value(ProviderErrorCode::AuthRequired).unwrap(),
            json!("auth_required")
        );
        assert_eq!(
            serde_json::to_value(ProviderErrorCode::NoCredentials).unwrap(),
            json!("no_credentials")
        );
        assert_eq!(
            serde_json::to_value(ProviderErrorCode::TokenExpired).unwrap(),
            json!("token_expired")
        );
        assert_eq!(
            serde_json::to_value(ProviderErrorCode::RequestFailed).unwrap(),
            json!("request_failed")
        );
    }

    #[test]
    fn missing_windows_and_values_serialize_as_null() {
        let usage = ProviderUsage::new(Some(UsageWindow::new(None, None)), None);
        let provider = ProviderSnapshot::success(
            ProviderSource::AnthropicOauthUsage,
            1_725_000_000_000,
            1_725_000_000_100,
            usage,
        );
        let value =
            serde_json::to_value(snapshot_with(provider)).expect("snapshot should serialize");
        let usage = &value["providers"]["claude_code"]["usage"];

        assert_eq!(usage["session"]["used_percent"], Value::Null);
        assert_eq!(usage["session"]["resets_at"], Value::Null);
        assert_eq!(usage["weekly"], Value::Null);
    }

    #[test]
    fn snapshot_round_trips_through_validated_json() {
        let snapshot = snapshot_with(ProviderSnapshot::success(
            ProviderSource::AnthropicOauthUsage,
            1_725_000_000_000,
            1_725_000_000_100,
            usage(),
        ));
        let json = serde_json::to_string(&snapshot).expect("snapshot should serialize");
        let decoded =
            deserialize_validated_snapshot(&json).expect("snapshot should validate after decoding");

        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn poll_report_snapshot_passes_validation() {
        let report = poll_report(
            success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(Some((0.0, None)), Some((42.0, None))),
            ),
            error_outcome(
                ProviderPollSource::ChatgptWhamUsage,
                PollError::RequestFailed,
            ),
        );
        let snapshot =
            snapshot_from_poll_report(&machine_id(), &report, at_millis(1_725_000_000_200))
                .expect("report should convert");

        assert_eq!(snapshot.validate(), Ok(()));
    }

    #[test]
    fn validated_deserialize_rejects_unsupported_schema_version() {
        let mut value = serde_json::to_value(snapshot_with(ProviderSnapshot::disabled())).unwrap();
        value["schema_version"] = json!(2);

        assert_eq!(
            validated_from_value(value),
            Err(SnapshotDeserializeError::InvalidJson)
        );
    }

    #[test]
    fn validation_rejects_invalid_machine_id() {
        let mut value = serde_json::to_value(snapshot_with(ProviderSnapshot::disabled())).unwrap();
        value["machine_id"] = json!("Home/other");

        assert_eq!(
            validated_from_value(value),
            Err(SnapshotDeserializeError::Validation(
                SnapshotValidationError::InvalidMachineId
            ))
        );
    }

    #[test]
    fn validation_rejects_status_and_error_contradiction() {
        let mut value = serde_json::to_value(snapshot_with(ProviderSnapshot::success(
            ProviderSource::AnthropicOauthUsage,
            1_725_000_000_000,
            1_725_000_000_100,
            usage(),
        )))
        .unwrap();
        value["providers"]["claude_code"]["error"] = json!({
            "code": "request_failed",
            "message": "Provider request failed",
            "retryable": true
        });

        assert_eq!(
            validated_from_value(value),
            Err(SnapshotDeserializeError::Validation(
                SnapshotValidationError::InvalidProviderState
            ))
        );
    }

    #[test]
    fn validation_rejects_provider_source_mismatch() {
        let mut value = serde_json::to_value(snapshot_with(ProviderSnapshot::success(
            ProviderSource::AnthropicOauthUsage,
            1_725_000_000_000,
            1_725_000_000_100,
            usage(),
        )))
        .unwrap();
        value["providers"]["claude_code"]["source"] = json!("chatgpt_wham_usage");

        assert_eq!(
            validated_from_value(value),
            Err(SnapshotDeserializeError::Validation(
                SnapshotValidationError::InvalidProviderSource
            ))
        );
    }

    #[test]
    fn validation_rejects_available_window_without_value() {
        let mut value = serde_json::to_value(snapshot_with(ProviderSnapshot::success(
            ProviderSource::AnthropicOauthUsage,
            1_725_000_000_000,
            1_725_000_000_100,
            usage(),
        )))
        .unwrap();
        value["providers"]["claude_code"]["usage"]["session"]["used_percent"] = Value::Null;

        assert_eq!(
            validated_from_value(value),
            Err(SnapshotDeserializeError::Validation(
                SnapshotValidationError::InvalidUsageWindow
            ))
        );
    }

    #[test]
    fn validation_rejects_non_finite_window_value() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let snapshot = snapshot_with(ProviderSnapshot::success(
                ProviderSource::AnthropicOauthUsage,
                1_725_000_000_000,
                1_725_000_000_100,
                ProviderUsage::new(Some(UsageWindow::new(Some(value), None)), None),
            ));

            assert_eq!(
                snapshot.validate(),
                Err(SnapshotValidationError::InvalidUsageWindow)
            );
        }
    }

    #[test]
    fn validation_rejects_negative_finite_window_value() {
        let mut value = serde_json::to_value(snapshot_with(ProviderSnapshot::success(
            ProviderSource::AnthropicOauthUsage,
            1_725_000_000_000,
            1_725_000_000_100,
            usage(),
        )))
        .unwrap();
        value["providers"]["claude_code"]["usage"]["session"]["used_percent"] = json!(-0.5);

        assert_eq!(
            validated_from_value(value),
            Err(SnapshotDeserializeError::Validation(
                SnapshotValidationError::InvalidUsageWindow
            ))
        );
    }

    #[test]
    fn validated_deserialize_accepts_one_hundred_and_preserves_value_above_it() {
        for expected in [100.0, 125.5] {
            let report = poll_report(
                success_outcome(
                    ProviderPollSource::AnthropicOauthUsage,
                    poll_usage(Some((expected, None)), None),
                ),
                ProviderPollOutcome::Disabled,
            );
            let value = converted_value(&report);
            let decoded = validated_from_value(value)
                .expect("finite value at or above 100 must remain valid");
            let decoded = serde_json::to_value(decoded).unwrap();

            assert_eq!(
                decoded["providers"]["claude_code"]["usage"]["session"]["used_percent"],
                json!(expected)
            );
        }
    }

    #[test]
    fn validated_deserialize_accepts_actual_zero_percent() {
        let report = poll_report(
            success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(Some((0.0, None)), None),
            ),
            ProviderPollOutcome::Disabled,
        );
        let value = converted_value(&report);
        let decoded = validated_from_value(value).expect("actual zero must remain valid");
        let decoded = serde_json::to_value(decoded).unwrap();

        assert_eq!(
            decoded["providers"]["claude_code"]["usage"]["session"]["used_percent"],
            json!(0.0)
        );
    }

    #[test]
    fn validated_deserialize_keeps_missing_window_null() {
        let report = poll_report(
            success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(None, Some((42.0, None))),
            ),
            ProviderPollOutcome::Disabled,
        );
        let value = converted_value(&report);
        let decoded = validated_from_value(value).expect("missing window must remain valid");
        let decoded = serde_json::to_value(decoded).unwrap();

        assert_eq!(
            decoded["providers"]["claude_code"]["usage"]["session"],
            Value::Null
        );
    }

    #[test]
    fn validation_accepts_disabled_without_error() {
        let snapshot = snapshot_with(ProviderSnapshot::disabled());

        assert_eq!(snapshot.validate(), Ok(()));
    }

    #[test]
    fn validation_accepts_error_without_source() {
        let snapshot = snapshot_with(ProviderSnapshot::error(
            None,
            1_725_000_000_000,
            ProviderError::from_code(ProviderErrorCode::RequestFailed),
        ));

        assert_eq!(snapshot.validate(), Ok(()));
    }

    #[test]
    fn validation_accepts_existing_stale_contract() {
        let snapshot = snapshot_with(ProviderSnapshot::stale(
            None,
            1_725_000_000_000,
            1_724_999_000_000,
            usage(),
            ProviderError::from_code(ProviderErrorCode::RequestFailed),
        ));

        assert_eq!(snapshot.validate(), Ok(()));
    }

    #[test]
    fn validation_accepts_non_disabled_antigravity_with_expected_source() {
        let snapshot = SnapshotV1::new(
            "home".to_string(),
            1_725_000_000_200,
            1_725_000_000_000,
            1_725_000_000_100,
            Providers::new(
                ProviderSnapshot::disabled(),
                ProviderSnapshot::disabled(),
                ProviderSnapshot::error(
                    Some(ProviderSource::AntigravityQuotaUsage),
                    1_725_000_000_000,
                    ProviderError::from_code(ProviderErrorCode::RequestFailed),
                ),
            ),
        );

        assert_eq!(snapshot.validate(), Ok(()));
    }

    #[test]
    fn validation_rejects_overlong_error_message() {
        let mut value = serde_json::to_value(snapshot_with(ProviderSnapshot::error(
            Some(ProviderSource::AnthropicOauthUsage),
            1_725_000_000_000,
            ProviderError::from_code(ProviderErrorCode::RequestFailed),
        )))
        .unwrap();
        value["providers"]["claude_code"]["error"]["message"] =
            json!("x".repeat(MAX_PROVIDER_ERROR_MESSAGE_LEN + 1));

        assert_eq!(
            validated_from_value(value),
            Err(SnapshotDeserializeError::Validation(
                SnapshotValidationError::ErrorMessageTooLong
            ))
        );
    }

    #[test]
    fn validation_rejects_noncanonical_error_message_and_retryability() {
        let base = serde_json::to_value(snapshot_with(ProviderSnapshot::error(
            Some(ProviderSource::AnthropicOauthUsage),
            1_725_000_000_000,
            ProviderError::from_code(ProviderErrorCode::RequestFailed),
        )))
        .unwrap();
        let mut message_changed = base.clone();
        message_changed["providers"]["claude_code"]["error"]["message"] = json!("Request failed");
        let mut retryability_changed = base;
        retryability_changed["providers"]["claude_code"]["error"]["retryable"] = json!(false);

        assert_eq!(
            validated_from_value(message_changed),
            Err(SnapshotDeserializeError::Validation(
                SnapshotValidationError::InvalidErrorMessage
            ))
        );
        assert_eq!(
            validated_from_value(retryability_changed),
            Err(SnapshotDeserializeError::Validation(
                SnapshotValidationError::InvalidErrorRetryability
            ))
        );
    }

    #[test]
    fn validated_deserialize_errors_do_not_echo_input_values() {
        const SECRET_SENTINEL: &str = "TEST_SECRET_CREDENTIAL_7f3a";
        const RAW_SOURCE_SENTINEL: &str = "TEST_RAW_SOURCE_RESPONSE_9b21";

        let mut secret_value = serde_json::to_value(snapshot_with(ProviderSnapshot::error(
            Some(ProviderSource::AnthropicOauthUsage),
            1_725_000_000_000,
            ProviderError::from_code(ProviderErrorCode::RequestFailed),
        )))
        .unwrap();
        secret_value["providers"]["claude_code"]["error"]["message"] = json!(SECRET_SENTINEL);
        let secret_error = validated_from_value(secret_value).unwrap_err();

        let mut source_value = serde_json::to_value(snapshot_with(ProviderSnapshot::success(
            ProviderSource::AnthropicOauthUsage,
            1_725_000_000_000,
            1_725_000_000_100,
            usage(),
        )))
        .unwrap();
        source_value["providers"]["claude_code"]["source"] = json!(RAW_SOURCE_SENTINEL);
        let source_error = validated_from_value(source_value).unwrap_err();

        for (error, sentinel) in [
            (secret_error, SECRET_SENTINEL),
            (source_error, RAW_SOURCE_SENTINEL),
        ] {
            let rendered = format!("{error:?}: {error}");
            assert!(!rendered.contains(sentinel));
        }
    }

    #[test]
    fn serialized_error_has_only_safe_fields() {
        let error = ProviderError::from_code(ProviderErrorCode::RequestFailed);
        let value = serde_json::to_value(error).expect("error should serialize");

        assert_eq!(
            value,
            json!({
                "code": "request_failed",
                "message": "Provider request failed",
                "retryable": true
            })
        );
    }

    #[test]
    fn claude_success_maps_both_windows() {
        let report = poll_report(
            success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(
                    Some((42.5, Some(at_millis(1_725_000_100_000)))),
                    Some((63.0, None)),
                ),
            ),
            ProviderPollOutcome::Disabled,
        );
        let value = converted_value(&report);
        let provider = &value["providers"]["claude_code"];

        assert_eq!(provider["status"], json!("success"));
        assert_eq!(provider["source"], json!("anthropic_oauth_usage"));
        assert_eq!(provider["attempted_at"], json!(1_725_000_000_000_u64));
        assert_eq!(provider["acquired_at"], json!(1_725_000_000_100_u64));
        assert_eq!(provider["usage"]["session"]["used_percent"], json!(42.5));
        assert_eq!(
            provider["usage"]["session"]["resets_at"],
            json!(1_725_000_100_000_u64)
        );
        assert_eq!(provider["usage"]["weekly"]["used_percent"], json!(63.0));
        assert_eq!(provider["usage"]["weekly"]["resets_at"], Value::Null);
        assert_eq!(provider["error"], Value::Null);
    }

    #[test]
    fn claude_missing_session_maps_to_null() {
        let report = poll_report(
            success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(None, Some((20.0, None))),
            ),
            ProviderPollOutcome::Disabled,
        );
        let value = converted_value(&report);
        let usage = &value["providers"]["claude_code"]["usage"];

        assert_eq!(usage["session"], Value::Null);
        assert_eq!(usage["weekly"]["used_percent"], json!(20.0));
    }

    #[test]
    fn claude_missing_weekly_maps_to_null() {
        let report = poll_report(
            success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(Some((20.0, None)), None),
            ),
            ProviderPollOutcome::Disabled,
        );
        let value = converted_value(&report);
        let usage = &value["providers"]["claude_code"]["usage"];

        assert_eq!(usage["session"]["used_percent"], json!(20.0));
        assert_eq!(usage["weekly"], Value::Null);
    }

    #[test]
    fn claude_actual_zero_remains_an_available_window() {
        let report = poll_report(
            success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(Some((0.0, None)), None),
            ),
            ProviderPollOutcome::Disabled,
        );
        let value = converted_value(&report);
        let session = &value["providers"]["claude_code"]["usage"]["session"];

        assert!(session.is_object());
        assert_eq!(session["used_percent"], json!(0.0));
    }

    #[test]
    fn claude_error_maps_to_safe_schema_error() {
        let report = poll_report(
            error_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                PollError::AuthRequired,
            ),
            ProviderPollOutcome::Disabled,
        );
        let value = converted_value(&report);
        let provider = &value["providers"]["claude_code"];

        assert_eq!(provider["status"], json!("error"));
        assert_eq!(provider["usage"], Value::Null);
        assert_eq!(
            provider["error"],
            json!({
                "code": "auth_required",
                "message": "Authentication required",
                "retryable": false
            })
        );
    }

    #[test]
    fn claude_disabled_maps_to_disabled() {
        let report = poll_report(ProviderPollOutcome::Disabled, ProviderPollOutcome::Disabled);
        let value = converted_value(&report);
        let provider = &value["providers"]["claude_code"];

        assert_eq!(provider["requested"], json!(false));
        assert_eq!(provider["status"], json!("disabled"));
        assert_eq!(provider["attempted_at"], Value::Null);
        assert_eq!(provider["usage"], Value::Null);
        assert_eq!(provider["error"], Value::Null);
    }

    #[test]
    fn codex_success_maps_both_windows() {
        let report = poll_report(
            ProviderPollOutcome::Disabled,
            success_outcome(
                ProviderPollSource::ChatgptWhamUsage,
                poll_usage(
                    Some((12.0, Some(at_millis(1_725_000_200_000)))),
                    Some((34.0, Some(at_millis(1_725_000_300_000)))),
                ),
            ),
        );
        let value = converted_value(&report);
        let provider = &value["providers"]["codex"];

        assert_eq!(provider["status"], json!("success"));
        assert_eq!(provider["source"], json!("chatgpt_wham_usage"));
        assert_eq!(provider["usage"]["session"]["used_percent"], json!(12.0));
        assert_eq!(provider["usage"]["weekly"]["used_percent"], json!(34.0));
    }

    #[test]
    fn codex_missing_window_maps_to_null() {
        let report = poll_report(
            ProviderPollOutcome::Disabled,
            success_outcome(
                ProviderPollSource::ChatgptWhamUsage,
                poll_usage(None, Some((34.0, None))),
            ),
        );
        let value = converted_value(&report);
        let usage = &value["providers"]["codex"]["usage"];

        assert_eq!(usage["session"], Value::Null);
        assert_eq!(usage["weekly"]["used_percent"], json!(34.0));
    }

    #[test]
    fn codex_actual_zero_remains_an_available_window() {
        let report = poll_report(
            ProviderPollOutcome::Disabled,
            success_outcome(
                ProviderPollSource::ChatgptWhamUsage,
                poll_usage(None, Some((0.0, None))),
            ),
        );
        let value = converted_value(&report);
        let weekly = &value["providers"]["codex"]["usage"]["weekly"];

        assert!(weekly.is_object());
        assert_eq!(weekly["used_percent"], json!(0.0));
    }

    #[test]
    fn codex_error_maps_to_safe_schema_error() {
        let report = poll_report(
            ProviderPollOutcome::Disabled,
            error_outcome(
                ProviderPollSource::ChatgptWhamUsage,
                PollError::RequestFailed,
            ),
        );
        let value = converted_value(&report);
        let provider = &value["providers"]["codex"];

        assert_eq!(provider["status"], json!("error"));
        assert_eq!(provider["usage"], Value::Null);
        assert_eq!(
            provider["error"],
            json!({
                "code": "request_failed",
                "message": "Provider request failed",
                "retryable": true
            })
        );
    }

    #[test]
    fn codex_disabled_maps_to_disabled() {
        let report = poll_report(ProviderPollOutcome::Disabled, ProviderPollOutcome::Disabled);
        let value = converted_value(&report);
        let provider = &value["providers"]["codex"];

        assert_eq!(provider["requested"], json!(false));
        assert_eq!(provider["status"], json!("disabled"));
        assert_eq!(provider["source"], Value::Null);
    }

    #[test]
    fn machine_id_and_supplied_capture_time_are_preserved() {
        let report = poll_report(ProviderPollOutcome::Disabled, ProviderPollOutcome::Disabled);
        let machine_id = MachineId::parse("central").expect("machine ID should be valid");
        let snapshot =
            snapshot_from_poll_report(&machine_id, &report, at_millis(1_800_000_000_123))
                .expect("report should convert");
        let value = serde_json::to_value(snapshot).expect("snapshot should serialize");

        assert_eq!(value["machine_id"], json!("central"));
        assert_eq!(value["generated_at"], json!(1_800_000_000_123_u64));
        assert_eq!(value["poll_started_at"], json!(1_800_000_000_123_u64));
        assert_eq!(value["poll_finished_at"], json!(1_800_000_000_123_u64));
    }

    #[test]
    fn normal_and_missing_reset_times_remain_distinct() {
        let report = poll_report(
            success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(
                    Some((1.0, Some(at_millis(1_725_000_100_000)))),
                    Some((2.0, None)),
                ),
            ),
            ProviderPollOutcome::Disabled,
        );
        let value = converted_value(&report);
        let usage = &value["providers"]["claude_code"]["usage"];

        assert_eq!(usage["session"]["resets_at"], json!(1_725_000_100_000_u64));
        assert_eq!(usage["weekly"]["resets_at"], Value::Null);
    }

    #[test]
    fn captured_time_before_unix_epoch_is_rejected() {
        let report = poll_report(ProviderPollOutcome::Disabled, ProviderPollOutcome::Disabled);
        let before_epoch = UNIX_EPOCH
            .checked_sub(Duration::from_millis(1))
            .expect("one millisecond before epoch should be representable");

        assert_eq!(
            snapshot_from_poll_report(&machine_id(), &report, before_epoch),
            Err(SnapshotConversionError::TimestampBeforeUnixEpoch)
        );
    }

    #[test]
    fn reset_time_before_unix_epoch_is_rejected() {
        let before_epoch = UNIX_EPOCH
            .checked_sub(Duration::from_millis(1))
            .expect("one millisecond before epoch should be representable");
        let report = poll_report(
            success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(Some((1.0, Some(before_epoch))), None),
            ),
            ProviderPollOutcome::Disabled,
        );

        assert_eq!(
            snapshot_from_poll_report(&machine_id(), &report, at_millis(1_725_000_000_200)),
            Err(SnapshotConversionError::TimestampBeforeUnixEpoch)
        );
    }

    #[test]
    fn millisecond_overflow_is_rejected_without_panicking() {
        assert_eq!(
            duration_to_unix_millis(Duration::from_secs(u64::MAX)),
            Err(SnapshotConversionError::TimestampMillisOverflow)
        );
    }

    #[test]
    fn conversion_borrows_report_so_ui_conversion_remains_available() {
        let report = poll_report(
            success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(Some((7.0, None)), None),
            ),
            error_outcome(
                ProviderPollSource::ChatgptWhamUsage,
                PollError::RequestFailed,
            ),
        );

        snapshot_from_poll_report(&machine_id(), &report, at_millis(1_725_000_000_200))
            .expect("report should convert by reference");
        let app_usage = report
            .into_app_usage_data()
            .expect("partial success should remain successful for the UI");

        assert_eq!(
            app_usage
                .family(QuotaFamilyId::Claude)
                .expect("Claude usage should remain")
                .item("session")
                .expect("session item should remain")
                .used_percentage(),
            Some(7.0)
        );
        assert_eq!(
            app_usage.family(QuotaFamilyId::Codex).unwrap().status,
            QuotaFamilyStatus::Unavailable
        );
    }

    #[test]
    fn every_poll_error_maps_to_a_stable_safe_error() {
        const SECRET_SENTINELS: [&str; 4] = [
            "TEST_SECRET_CREDENTIAL_7f3a",
            "TEST_RAW_RESPONSE_BODY_9b21",
            "Bearer TEST_TOKEN_a14c",
            "TEST_AUTHORIZATION_HEADER_d4e2",
        ];
        let cases = [
            (
                "auth-required",
                PollError::AuthRequired,
                "auth_required",
                "Authentication required",
                false,
            ),
            (
                "no-credentials",
                PollError::NoCredentials,
                "no_credentials",
                "Credentials not found",
                false,
            ),
            (
                "token-expired",
                PollError::TokenExpired,
                "token_expired",
                "Token expired",
                false,
            ),
            (
                "request-failed",
                PollError::RequestFailed,
                "request_failed",
                "Provider request failed",
                true,
            ),
        ];

        for (case_id, poll_error, code, message, retryable) in cases {
            let report = poll_report(
                error_outcome(ProviderPollSource::AnthropicOauthUsage, poll_error),
                ProviderPollOutcome::Disabled,
            );
            let value = converted_value(&report);
            let error = &value["providers"]["claude_code"]["error"];

            let expected = json!({
                "code": code,
                "message": message,
                "retryable": retryable
            });
            assert!(
                error == &expected,
                "{case_id}: fixed error contract changed"
            );
            assert!(
                error["message"]
                    .as_str()
                    .expect("fixed error message must be a string")
                    .len()
                    <= MAX_PROVIDER_ERROR_MESSAGE_LEN,
                "{case_id}: fixed error message is unexpectedly long"
            );

            let serialized = error.to_string();
            for sentinel in SECRET_SENTINELS {
                assert!(
                    !serialized.contains(sentinel),
                    "{case_id}: secret sentinel leaked"
                );
            }
            assert!(
                !serialized.contains(&format!("{poll_error:?}")),
                "{case_id}: raw PollError Debug representation leaked"
            );
        }
    }

    #[test]
    fn non_disabled_antigravity_builds_and_round_trips_in_schema_v1() {
        let report = PollReport {
            claude_code: ProviderPollOutcome::Disabled,
            codex: ProviderPollOutcome::Disabled,
            antigravity: success_outcome(
                ProviderPollSource::AntigravityQuotaUsage,
                poll_usage(Some((1.0, None)), None),
            ),
            github_copilot: ProviderPollOutcome::Disabled,
            vercel_ai_gateway: ProviderPollOutcome::Disabled,
        };

        let snapshot =
            snapshot_from_poll_report(&machine_id(), &report, at_millis(1_725_000_000_200))
                .expect("known Antigravity source should not reject the whole snapshot");
        assert_eq!(snapshot.validate(), Ok(()));

        let json = serde_json::to_string(&snapshot).expect("snapshot should serialize");
        let value: Value = serde_json::from_str(&json).expect("snapshot JSON should parse");
        let provider = &value["providers"]["antigravity"];
        assert_eq!(provider["requested"], json!(true));
        assert_eq!(provider["status"], json!("success"));
        assert_eq!(provider["source"], json!("antigravity_quota_usage"));
        assert_eq!(provider["usage"]["session"]["used_percent"], json!(1.0));
        assert_eq!(provider["usage"]["weekly"], Value::Null);
        assert_eq!(
            deserialize_validated_snapshot(&json).expect("snapshot should deserialize"),
            snapshot
        );
    }

    #[test]
    fn antigravity_error_and_disabled_remain_distinct() {
        let error_report = PollReport {
            claude_code: success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(Some((10.0, None)), None),
            ),
            codex: ProviderPollOutcome::Disabled,
            antigravity: error_outcome(
                ProviderPollSource::AntigravityQuotaUsage,
                PollError::RequestFailed,
            ),
            github_copilot: ProviderPollOutcome::Disabled,
            vercel_ai_gateway: ProviderPollOutcome::Disabled,
        };
        let error_value = converted_value(&error_report);
        let unavailable = &error_value["providers"]["antigravity"];
        assert_eq!(unavailable["requested"], json!(true));
        assert_eq!(unavailable["status"], json!("error"));
        assert_eq!(unavailable["source"], json!("antigravity_quota_usage"));
        assert_eq!(unavailable["usage"], Value::Null);
        assert_eq!(unavailable["error"]["code"], json!("request_failed"));

        let disabled_report = PollReport {
            claude_code: ProviderPollOutcome::Disabled,
            codex: ProviderPollOutcome::Disabled,
            antigravity: ProviderPollOutcome::Disabled,
            github_copilot: ProviderPollOutcome::Disabled,
            vercel_ai_gateway: ProviderPollOutcome::Disabled,
        };
        let disabled_value = converted_value(&disabled_report);
        let disabled = &disabled_value["providers"]["antigravity"];
        assert_eq!(disabled["requested"], json!(false));
        assert_eq!(disabled["status"], json!("disabled"));
        assert_eq!(disabled["source"], Value::Null);
        assert_eq!(disabled["error"], Value::Null);
    }

    #[test]
    fn mixed_provider_snapshot_with_antigravity_validates() {
        let report = PollReport {
            claude_code: success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(Some((10.0, None)), Some((20.0, None))),
            ),
            codex: success_outcome(
                ProviderPollSource::ChatgptWhamUsage,
                poll_usage(Some((30.0, None)), Some((40.0, None))),
            ),
            antigravity: success_outcome(
                ProviderPollSource::AntigravityQuotaUsage,
                poll_usage(Some((50.0, None)), Some((60.0, None))),
            ),
            github_copilot: ProviderPollOutcome::Disabled,
            vercel_ai_gateway: ProviderPollOutcome::Disabled,
        };

        let snapshot =
            snapshot_from_poll_report(&machine_id(), &report, at_millis(1_725_000_000_200))
                .expect("all known providers should convert together");
        assert_eq!(snapshot.validate(), Ok(()));
    }

    #[test]
    fn actual_pre_antigravity_json_without_new_provider_keys_still_deserializes() {
        let json = r#"{
            "schema_version": 1,
            "timestamp_unit": "unix_ms",
            "machine_id": "home",
            "generated_at": 1725000000200,
            "poll_started_at": 1725000000000,
            "poll_finished_at": 1725000000100,
            "providers": {
                "claude_code": {
                    "requested": false,
                    "status": "disabled",
                    "source": null,
                    "attempted_at": null,
                    "acquired_at": null,
                    "last_success_at": null,
                    "stale": false,
                    "usage": null,
                    "error": null
                },
                "codex": {
                    "requested": false,
                    "status": "disabled",
                    "source": null,
                    "attempted_at": null,
                    "acquired_at": null,
                    "last_success_at": null,
                    "stale": false,
                    "usage": null,
                    "error": null
                }
            }
        }"#;

        let snapshot = deserialize_validated_snapshot(json)
            .expect("an actual legacy object must remain readable without migration");
        assert!(snapshot.providers.get("claude_code").is_some());
        assert!(snapshot.providers.get("antigravity").is_none());
        assert!(snapshot.providers.get("github_copilot").is_none());
        assert_eq!(
            snapshot.providers.get("codex").unwrap().status,
            ProviderStatus::Disabled
        );
    }

    #[test]
    fn copilot_snapshot_uses_generic_quota_item_and_keeps_disabled_distinct() {
        let usage = UsageData::from_quota_items(vec![QuotaItem {
            id: "monthly_ai_credits".to_string(),
            label: "Monthly AI credits".to_string(),
            availability: QuotaItemAvailability::Available,
            metric: Some(QuotaMetric::Used {
                used: 375.0,
                limit: Some(1_500.0),
            }),
            unit: QuotaUnit::AiCredits,
            resets_at: Some(at_millis(1_725_000_000_000)),
        }]);
        let report = PollReport {
            claude_code: ProviderPollOutcome::Disabled,
            codex: ProviderPollOutcome::Disabled,
            antigravity: ProviderPollOutcome::Disabled,
            github_copilot: success_outcome(ProviderPollSource::GithubBillingApi, usage),
            vercel_ai_gateway: ProviderPollOutcome::Disabled,
        };
        let value = converted_value(&report);
        let copilot = &value["providers"]["github_copilot"];
        assert_eq!(copilot["status"], json!("success"));
        assert_eq!(copilot["source"], json!("github_billing_api"));
        assert_eq!(copilot["usage"]["session"], Value::Null);
        assert_eq!(
            copilot["usage"]["quota_items"][0]["metric"],
            json!({"kind": "used", "used": 375.0, "limit": 1500.0})
        );
        assert_eq!(
            value["providers"]["claude_code"]["status"],
            json!("disabled")
        );
    }

    #[test]
    fn legacy_fixed_reader_ignores_new_provider_and_quota_item_fields() {
        #[derive(Deserialize)]
        struct LegacyUsage {
            session: Option<Value>,
            weekly: Option<Value>,
        }
        #[derive(Deserialize)]
        struct LegacyProvider {
            usage: Option<LegacyUsage>,
        }
        #[derive(Deserialize)]
        struct LegacyProviders {
            claude_code: LegacyProvider,
            codex: LegacyProvider,
            antigravity: LegacyProvider,
        }
        #[derive(Deserialize)]
        struct LegacySnapshot {
            providers: LegacyProviders,
        }

        let report = PollReport {
            claude_code: success_outcome(
                ProviderPollSource::AnthropicOauthUsage,
                poll_usage(Some((10.0, None)), Some((20.0, None))),
            ),
            codex: ProviderPollOutcome::Disabled,
            antigravity: ProviderPollOutcome::Disabled,
            github_copilot: success_outcome(
                ProviderPollSource::GithubBillingApi,
                UsageData::from_quota_items(vec![QuotaItem {
                    id: "monthly_ai_credits".to_string(),
                    label: "Monthly AI credits".to_string(),
                    availability: QuotaItemAvailability::Available,
                    metric: Some(QuotaMetric::Used {
                        used: 1.0,
                        limit: None,
                    }),
                    unit: QuotaUnit::AiCredits,
                    resets_at: None,
                }]),
            ),
            vercel_ai_gateway: ProviderPollOutcome::Disabled,
        };
        let new_json = serde_json::to_string(
            &snapshot_from_poll_report(&machine_id(), &report, at_millis(1_725_000_000_200))
                .unwrap(),
        )
        .unwrap();
        let legacy: LegacySnapshot = serde_json::from_str(&new_json)
            .expect("serde's old fixed reader must ignore additive fields");
        let claude_usage = legacy.providers.claude_code.usage.unwrap();
        assert!(claude_usage.session.is_some());
        assert!(claude_usage.weekly.is_some());
        assert!(legacy.providers.codex.usage.is_none());
        assert!(legacy.providers.antigravity.usage.is_none());
    }
}
