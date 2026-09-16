use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use std::os::windows::process::CommandExt;

use crate::diagnose;
use crate::localization::Strings;
use crate::models::{
    AppUsageData, BankedResetCount, QuotaFamily, QuotaFamilyId, QuotaFamilyStatus, QuotaItem,
    QuotaItemAvailability, QuotaMetric, QuotaUnit, UsageData, UsageSection,
    GITHUB_COPILOT_MONTHLY_ITEM_ID,
};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
#[cfg(feature = "claude-messages-fallback")]
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const CODEX_APP_SERVER_TIMEOUT: Duration = Duration::from_secs(10);
/// Codex quota (5h/7d usage and banked reset) comes from a single
/// `codex app-server` `account/rateLimits/read` call per
/// `CODEX-OFFICIAL-PATH-IMPLEMENT-01`. This TTL bounds how often that
/// subprocess is spawned across the app's ~5s poll cycle without standing up
/// a resident daemon.
const CODEX_RATE_LIMITS_CACHE_TTL: Duration = Duration::from_secs(45);
const GITHUB_API_VERSION: &str = "2026-03-10";
const GITHUB_COPILOT_USAGE_ENDPOINT_SUFFIX: &str = "/settings/billing/ai_credit/usage";
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn resolve_user_home() -> Option<PathBuf> {
    resolve_user_home_from(
        dirs::home_dir(),
        std::env::var_os("USERPROFILE"),
        std::env::var_os("HOMEDRIVE"),
        std::env::var_os("HOMEPATH"),
    )
}

fn resolve_user_home_from(
    dirs_home: Option<PathBuf>,
    user_profile: Option<std::ffi::OsString>,
    home_drive: Option<std::ffi::OsString>,
    home_path: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    dirs_home
        .or_else(|| {
            user_profile
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            let mut home = PathBuf::from(home_drive.filter(|value| !value.is_empty())?);
            home.push(home_path.filter(|value| !value.is_empty())?);
            Some(home)
        })
}

#[cfg(feature = "claude-messages-fallback")]
const MODEL_FALLBACK_CHAIN: &[&str] = &["claude-3-haiku-20240307", "claude-haiku-4-5-20251001"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollError {
    AuthRequired,
    NoCredentials,
    TokenExpired,
    RequestFailed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GithubCopilotPlan {
    #[default]
    Unknown,
    Pro,
    ProPlus,
    Max,
}

impl GithubCopilotPlan {
    pub(crate) const fn allowance(self) -> Option<f64> {
        match self {
            Self::Unknown => None,
            Self::Pro => Some(1_500.0),
            Self::ProPlus => Some(7_000.0),
            Self::Max => Some(20_000.0),
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Pro => "Pro",
            Self::ProPlus => "Pro+",
            Self::Max => "Max",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderPollSource {
    AnthropicOauthUsage,
    ChatgptWhamUsage,
    AntigravityQuotaUsage,
    GithubBillingApi,
}

#[derive(Clone, Debug)]
pub(crate) enum ProviderPollOutcome {
    Disabled,
    Success {
        source: ProviderPollSource,
        attempted_at: SystemTime,
        acquired_at: SystemTime,
        usage: UsageData,
    },
    Error {
        source: ProviderPollSource,
        attempted_at: SystemTime,
        error: PollError,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct PollReport {
    pub(crate) claude_code: ProviderPollOutcome,
    pub(crate) codex: ProviderPollOutcome,
    pub(crate) antigravity: ProviderPollOutcome,
    pub(crate) github_copilot: ProviderPollOutcome,
}

impl PollReport {
    pub(crate) fn into_app_usage_data(self) -> Result<AppUsageData, PollError> {
        let mut data = AppUsageData::default();
        let mut first_error = None;
        let mut any_success = false;

        for (id, outcome) in [
            (QuotaFamilyId::Claude, self.claude_code),
            (QuotaFamilyId::Codex, self.codex),
            (QuotaFamilyId::Antigravity, self.antigravity),
            (QuotaFamilyId::GithubCopilot, self.github_copilot),
        ] {
            match outcome {
                ProviderPollOutcome::Success { usage, .. } => {
                    any_success = true;
                    let mut family = usage.into_quota_family(id);
                    // Antigravity's official-cache items are judged for
                    // staleness per reset boundary, independently of each
                    // other (see `antigravity_statusline::
                    // quota_items_from_cache`). If every item that came
                    // back is past its own reset, the family as a whole has
                    // no current reading either — reflect that at the
                    // family level too, rather than showing "Available"
                    // over a family with nothing but stale items.
                    if id == QuotaFamilyId::Antigravity
                        && !family.items.is_empty()
                        && family
                            .items
                            .iter()
                            .all(|item| item.availability != QuotaItemAvailability::Available)
                    {
                        family.status = QuotaFamilyStatus::Stale;
                    }
                    data.upsert(family);
                }
                ProviderPollOutcome::Error { error, .. } => {
                    first_error.get_or_insert(error);
                    data.upsert(QuotaFamily::with_status(id, QuotaFamilyStatus::Unavailable));
                }
                ProviderPollOutcome::Disabled => {
                    data.upsert(QuotaFamily::with_status(id, QuotaFamilyStatus::Disabled));
                }
            }
        }

        if !any_success {
            Err(first_error.unwrap_or(PollError::RequestFailed))
        } else {
            Ok(data)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialWatchMode {
    ActiveSource,
    AllSources,
}

pub type CredentialWatchSnapshot = Vec<String>;

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageBucket>,
    seven_day: Option<UsageBucket>,
}

#[derive(Deserialize)]
struct UsageBucket {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubAiCreditUsageResponse {
    #[serde(default)]
    usage_items: Vec<GithubAiCreditUsageItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GithubAiCreditUsageItem {
    product: String,
    sku: String,
    unit_type: String,
    gross_quantity: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexRateLimitsReadResult {
    rate_limits: Option<CodexNativeRateLimitSnapshot>,
    rate_limit_reset_credits: Option<CodexResetCreditsSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexNativeRateLimitSnapshot {
    primary: Option<CodexNativeRateLimitWindow>,
    secondary: Option<CodexNativeRateLimitWindow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexNativeRateLimitWindow {
    used_percent: f64,
    resets_at: Option<i64>,
    window_duration_mins: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexResetCreditsSummary {
    available_count: u64,
}

#[derive(Deserialize)]
struct CodexAppServerResponse {
    id: Option<serde_json::Value>,
    result: Option<CodexRateLimitsReadResult>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CodexAppServerError {
    CliUnavailable,
    StartFailed,
    InitializeFailed,
    Timeout,
    Protocol,
}

enum CodexAppServerMessage {
    Response(CodexAppServerResponse),
    ProtocolError,
    EndOfStream,
}

#[derive(Default)]
struct CodexUsageCache {
    fetched_at: Option<Instant>,
    usage: Option<Result<UsageData, CodexAppServerError>>,
}

pub fn poll(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
) -> Result<AppUsageData, PollError> {
    poll_report(show_claude_code, show_codex, show_antigravity).into_app_usage_data()
}

pub(crate) fn poll_report(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
) -> PollReport {
    #[cfg(feature = "antigravity")]
    {
        return poll_report_with(
            show_claude_code,
            show_codex,
            show_antigravity,
            poll_claude_code,
            poll_codex,
            poll_antigravity,
        );
    }

    #[cfg(not(feature = "antigravity"))]
    {
        let _ = show_antigravity;
        poll_report_with(
            show_claude_code,
            show_codex,
            false,
            poll_claude_code,
            poll_codex,
            || unreachable!("Antigravity is unavailable in this build"),
        )
    }
}

/// Headless polling shares the same `poll_codex` (app-server-only) path as
/// the GUI's `poll_report` — Codex quota is fetched identically either way.
pub(crate) fn poll_report_headless(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
) -> PollReport {
    #[cfg(feature = "antigravity")]
    {
        return poll_report_with(
            show_claude_code,
            show_codex,
            show_antigravity,
            poll_claude_code,
            poll_codex,
            poll_antigravity,
        );
    }

    #[cfg(not(feature = "antigravity"))]
    {
        let _ = show_antigravity;
        poll_report_with(
            show_claude_code,
            show_codex,
            false,
            poll_claude_code,
            poll_codex,
            || unreachable!("Antigravity is unavailable in this build"),
        )
    }
}

pub(crate) fn poll_report_with_github_copilot_updates(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    show_github_copilot: bool,
    github_copilot_plan: GithubCopilotPlan,
    mut on_provider_complete: impl FnMut(QuotaFamilyId, &ProviderPollOutcome),
) -> PollReport {
    #[cfg(feature = "antigravity")]
    let mut report = poll_report_with_updates(
        show_claude_code,
        show_codex,
        show_antigravity,
        poll_claude_code,
        poll_codex,
        poll_antigravity,
        &mut on_provider_complete,
    );

    #[cfg(not(feature = "antigravity"))]
    let mut report = {
        let _ = show_antigravity;
        poll_report_with_updates(
            show_claude_code,
            show_codex,
            false,
            poll_claude_code,
            poll_codex,
            || unreachable!("Antigravity is unavailable in this build"),
            &mut on_provider_complete,
        )
    };

    report.github_copilot = poll_provider(
        show_github_copilot,
        ProviderPollSource::GithubBillingApi,
        &mut || poll_github_copilot(github_copilot_plan),
        &mut SystemTime::now,
    );
    on_provider_complete(QuotaFamilyId::GithubCopilot, &report.github_copilot);
    report
}

fn poll_with(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    mut poll_claude_code: impl FnMut() -> Result<UsageData, PollError>,
    mut poll_codex: impl FnMut() -> Result<UsageData, PollError>,
    mut poll_antigravity: impl FnMut() -> Result<UsageData, PollError>,
) -> Result<AppUsageData, PollError> {
    poll_report_with(
        show_claude_code,
        show_codex,
        show_antigravity,
        &mut poll_claude_code,
        &mut poll_codex,
        &mut poll_antigravity,
    )
    .into_app_usage_data()
}

fn poll_report_with(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    poll_claude_code: impl FnMut() -> Result<UsageData, PollError>,
    poll_codex: impl FnMut() -> Result<UsageData, PollError>,
    poll_antigravity: impl FnMut() -> Result<UsageData, PollError>,
) -> PollReport {
    let mut ignore_update = |_: QuotaFamilyId, _: &ProviderPollOutcome| {};
    poll_report_with_updates(
        show_claude_code,
        show_codex,
        show_antigravity,
        poll_claude_code,
        poll_codex,
        poll_antigravity,
        &mut ignore_update,
    )
}

fn poll_report_with_updates(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    poll_claude_code: impl FnMut() -> Result<UsageData, PollError>,
    poll_codex: impl FnMut() -> Result<UsageData, PollError>,
    poll_antigravity: impl FnMut() -> Result<UsageData, PollError>,
    on_provider_complete: &mut impl FnMut(QuotaFamilyId, &ProviderPollOutcome),
) -> PollReport {
    poll_report_with_clock_and_updates(
        show_claude_code,
        show_codex,
        show_antigravity,
        poll_claude_code,
        poll_codex,
        poll_antigravity,
        SystemTime::now,
        on_provider_complete,
    )
}

fn poll_report_with_clock(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    mut poll_claude_code: impl FnMut() -> Result<UsageData, PollError>,
    mut poll_codex: impl FnMut() -> Result<UsageData, PollError>,
    mut poll_antigravity: impl FnMut() -> Result<UsageData, PollError>,
    mut now: impl FnMut() -> SystemTime,
) -> PollReport {
    let mut ignore_update = |_: QuotaFamilyId, _: &ProviderPollOutcome| {};
    poll_report_with_clock_and_updates(
        show_claude_code,
        show_codex,
        show_antigravity,
        poll_claude_code,
        poll_codex,
        poll_antigravity,
        &mut now,
        &mut ignore_update,
    )
}

fn poll_report_with_clock_and_updates(
    show_claude_code: bool,
    show_codex: bool,
    show_antigravity: bool,
    mut poll_claude_code: impl FnMut() -> Result<UsageData, PollError>,
    mut poll_codex: impl FnMut() -> Result<UsageData, PollError>,
    mut poll_antigravity: impl FnMut() -> Result<UsageData, PollError>,
    mut now: impl FnMut() -> SystemTime,
    on_provider_complete: &mut impl FnMut(QuotaFamilyId, &ProviderPollOutcome),
) -> PollReport {
    let active_provider_count = show_claude_code as u8 + show_codex as u8 + show_antigravity as u8;

    let claude_code = poll_provider(
        show_claude_code,
        ProviderPollSource::AnthropicOauthUsage,
        &mut poll_claude_code,
        &mut now,
    );
    log_partial_failure("Claude Code", &claude_code, active_provider_count);
    on_provider_complete(QuotaFamilyId::Claude, &claude_code);

    let codex = poll_provider(
        show_codex,
        ProviderPollSource::ChatgptWhamUsage,
        &mut poll_codex,
        &mut now,
    );
    log_partial_failure("Codex", &codex, active_provider_count);
    on_provider_complete(QuotaFamilyId::Codex, &codex);

    let antigravity_source = ProviderPollSource::AntigravityQuotaUsage;

    let antigravity = poll_provider(
        show_antigravity,
        antigravity_source,
        &mut poll_antigravity,
        &mut now,
    );
    log_partial_failure("Antigravity", &antigravity, active_provider_count);
    on_provider_complete(QuotaFamilyId::Antigravity, &antigravity);

    PollReport {
        claude_code,
        codex,
        antigravity,
        github_copilot: ProviderPollOutcome::Disabled,
    }
}

fn poll_provider(
    requested: bool,
    source: ProviderPollSource,
    poll_provider: &mut impl FnMut() -> Result<UsageData, PollError>,
    now: &mut impl FnMut() -> SystemTime,
) -> ProviderPollOutcome {
    if !requested {
        return ProviderPollOutcome::Disabled;
    }

    let attempted_at = now();
    match poll_provider() {
        Ok(usage) => ProviderPollOutcome::Success {
            source,
            attempted_at,
            acquired_at: now(),
            usage,
        },
        Err(error) => ProviderPollOutcome::Error {
            source,
            attempted_at,
            error,
        },
    }
}

fn log_partial_failure(
    provider_name: &str,
    outcome: &ProviderPollOutcome,
    active_provider_count: u8,
) {
    if active_provider_count > 1 {
        if let ProviderPollOutcome::Error { error, .. } = outcome {
            diagnose::log(format!("{provider_name} usage poll failed: {error:?}"));
        }
    }
}

fn poll_claude_code() -> Result<UsageData, PollError> {
    let creds = match read_first_credentials() {
        Some(c) => c,
        None => {
            diagnose::log("poll failed: no Claude credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    #[cfg(feature = "legacy-auto-refresh")]
    let creds = refresh_or_fallback(creds)?;

    #[cfg(not(feature = "legacy-auto-refresh"))]
    if is_token_expired(creds.expires_at) {
        diagnose::log("Claude credentials are expired; automatic CLI refresh is disabled");
        return Err(PollError::TokenExpired);
    }

    fetch_usage_with_fallback(&creds.access_token)
}

/// Codex quota is fetched exclusively through OpenAI's own `codex app-server`
/// (`account/rateLimits/read`) per `CODEX-OFFICIAL-PATH-IMPLEMENT-01`. The app
/// never reads `~/.codex/auth.json`, extracts or refreshes a Codex OAuth
/// token, or calls a ChatGPT/Codex backend endpoint directly — authentication
/// is entirely the app-server's (and thus the Codex CLI login's)
/// responsibility. There is deliberately no fallback to a legacy credential
/// or HTTP path on any app-server failure.
fn poll_codex() -> Result<UsageData, PollError> {
    cached_codex_usage().map_err(codex_app_server_error_to_poll_error)
}

/// Reads the Antigravity CLI's official `/statusline <command>` cache
/// (`antigravity_statusline`) rather than talking to Google or Antigravity
/// directly — see `ANTIGRAVITY-STATUSLINE-BRIDGE-01` /
/// `ANTIGRAVITY-ROUTING-SWITCH-01`. This path makes no network call, reads
/// no OAuth token, and touches Windows Credential Manager for nothing;
/// `PollError::RequestFailed` covers a missing, malformed, or
/// unsupported-schema cache alike (all "no usable current reading", never
/// "zero usage") since there is no legacy path left to fall back to.
#[cfg(feature = "antigravity")]
fn poll_antigravity() -> Result<UsageData, PollError> {
    use crate::antigravity_statusline::{self, CacheReadResult};

    let cache_path = antigravity_statusline::default_cache_path();
    let cache = match antigravity_statusline::read_cache(&cache_path) {
        CacheReadResult::Missing => {
            diagnose::log("Antigravity statusline cache is missing (bridge not configured yet)");
            return Err(PollError::RequestFailed);
        }
        CacheReadResult::Malformed(error) => {
            diagnose::log(format!(
                "Antigravity statusline cache is malformed: {error}"
            ));
            return Err(PollError::RequestFailed);
        }
        CacheReadResult::UnsupportedSchema(version) => {
            diagnose::log(format!(
                "Antigravity statusline cache has unsupported schema_version {version}"
            ));
            return Err(PollError::RequestFailed);
        }
        CacheReadResult::Valid(cache) => cache,
    };

    let items = antigravity_statusline::quota_items_from_cache(&cache, SystemTime::now());
    if items.is_empty() {
        return Err(PollError::RequestFailed);
    }
    Ok(UsageData::from_quota_items(items))
}

fn poll_github_copilot(plan: GithubCopilotPlan) -> Result<UsageData, PollError> {
    let username = run_gh_api(&["user", "--jq", ".login"])?;
    let username = username.trim();
    if username.is_empty() || username.contains(['/', '\\', '?', '#']) {
        return Err(PollError::RequestFailed);
    }

    let endpoint = format!("/users/{username}{GITHUB_COPILOT_USAGE_ENDPOINT_SUFFIX}");
    let json = run_gh_api(&[
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        &format!("X-GitHub-Api-Version: {GITHUB_API_VERSION}"),
        &endpoint,
    ])?;
    let response: GithubAiCreditUsageResponse =
        serde_json::from_str(&json).map_err(|_| PollError::RequestFailed)?;
    github_copilot_usage_from_response(response, plan, SystemTime::now())
}

fn run_gh_api(args: &[&str]) -> Result<String, PollError> {
    let mut command = Command::new(resolve_github_cli_executable());
    command
        .arg("api")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let output = run_with_captured_stdout(&mut command, Duration::from_secs(20))
        .ok_or(PollError::NoCredentials)?;
    if !output.status.success() {
        return Err(PollError::AuthRequired);
    }
    String::from_utf8(output.stdout).map_err(|_| PollError::RequestFailed)
}

fn run_with_captured_stdout(
    command: &mut Command,
    timeout: Duration,
) -> Option<std::process::Output> {
    command.stdout(Stdio::piped());
    run_with_timeout(command, timeout)
}

fn resolve_github_cli_executable() -> PathBuf {
    let path = std::env::var_os("PATH");
    let program_files = std::env::var_os("ProgramFiles");
    resolve_github_cli_executable_with(path.as_deref(), program_files.as_deref(), |candidate| {
        candidate.is_file()
    })
}

fn resolve_github_cli_executable_with<F>(
    path: Option<&OsStr>,
    program_files: Option<&OsStr>,
    is_file: F,
) -> PathBuf
where
    F: Fn(&Path) -> bool,
{
    if let Some(path) = path {
        for directory in std::env::split_paths(path) {
            let candidate = directory.join("gh.exe");
            if is_file(&candidate) {
                return candidate;
            }
        }
    }

    if let Some(program_files) = program_files {
        let candidate = PathBuf::from(program_files)
            .join("GitHub CLI")
            .join("gh.exe");
        if is_file(&candidate) {
            return candidate;
        }
    }

    PathBuf::from("gh.exe")
}

fn github_copilot_usage_from_response(
    response: GithubAiCreditUsageResponse,
    plan: GithubCopilotPlan,
    now: SystemTime,
) -> Result<UsageData, PollError> {
    let had_items = !response.usage_items.is_empty();
    let mut matched = false;
    let mut gross_usage = 0.0;
    for item in response.usage_items {
        let product = item.product.to_ascii_lowercase();
        let sku = item.sku.to_ascii_lowercase();
        let unit = item.unit_type.to_ascii_lowercase();
        let is_copilot_credit =
            product.contains("copilot") && (sku.contains("ai credit") || unit.contains("credit"));
        if !is_copilot_credit {
            continue;
        }
        if !item.gross_quantity.is_finite() || item.gross_quantity < 0.0 {
            return Err(PollError::RequestFailed);
        }
        matched = true;
        gross_usage += item.gross_quantity;
    }
    if had_items && !matched {
        return Err(PollError::RequestFailed);
    }

    let metric = QuotaMetric::Used {
        used: gross_usage,
        limit: plan.allowance(),
    };
    Ok(UsageData::from_quota_items(vec![QuotaItem {
        id: GITHUB_COPILOT_MONTHLY_ITEM_ID.to_string(),
        label: "Monthly AI credits".to_string(),
        availability: QuotaItemAvailability::Available,
        metric: Some(metric),
        unit: QuotaUnit::AiCredits,
        resets_at: next_calendar_month_utc(now),
    }]))
}

fn next_calendar_month_utc(now: SystemTime) -> Option<SystemTime> {
    let seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let days = i64::try_from(seconds / 86_400).ok()?;
    let (year, month, _) = civil_from_days(days);
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next_days = days_from_civil(next_year, next_month, 1);
    let next_seconds = u64::try_from(next_days).ok()?.checked_mul(86_400)?;
    Some(UNIX_EPOCH + Duration::from_secs(next_seconds))
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(feature = "legacy-auto-refresh")]
fn refresh_or_fallback(mut creds: Credentials) -> Result<Credentials, PollError> {
    loop {
        if !is_token_expired(creds.expires_at) {
            return Ok(creds);
        }

        let source = creds.source.clone();
        cli_refresh_token(&source);

        match read_credentials_from_source(&source) {
            Some(refreshed) if !is_token_expired(refreshed.expires_at) => return Ok(refreshed),
            Some(_) => diagnose::log(format!(
                "credentials from {source:?} still expired after refresh attempt"
            )),
            None => diagnose::log(format!(
                "credentials from {source:?} unavailable after refresh attempt"
            )),
        }

        match read_next_credentials_after(&source) {
            Some(next) => creds = next,
            None => return Err(PollError::TokenExpired),
        }
    }
}

/// Invoke the Claude CLI with a minimal prompt to force its internal
/// OAuth token refresh.
#[cfg(feature = "legacy-auto-refresh")]
fn cli_refresh_token(source: &CredentialSource) {
    match source {
        CredentialSource::Windows(_) => cli_refresh_windows_token(),
        CredentialSource::Wsl { distro } => cli_refresh_wsl_token(distro),
    }
}

#[cfg(feature = "legacy-auto-refresh")]
fn cli_refresh_windows_token() {
    let claude_path = resolve_windows_claude_path();
    let is_cmd = claude_path.to_lowercase().ends_with(".cmd");
    diagnose::log(format!(
        "attempting Windows Claude token refresh via {claude_path}"
    ));

    let args: &[&str] = &["-p", "."];

    let mut cmd = if is_cmd {
        let mut c = Command::new("cmd.exe");
        c.arg("/c").arg(&claude_path).args(args);
        c
    } else {
        let mut c = Command::new(&claude_path);
        c.args(args);
        c
    };
    cmd.env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("unable to spawn Windows Claude token refresh", error);
            return;
        }
    };

    // Wait up to 30 seconds — don't block the poll thread forever
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(30) {
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(_) => break,
        }
    }
}

#[cfg(feature = "legacy-auto-refresh")]
fn cli_refresh_wsl_token(distro: &str) {
    diagnose::log(format!(
        "attempting WSL Claude token refresh in distro {distro}"
    ));
    let mut cmd = Command::new("wsl.exe");
    cmd.arg("-d")
        .arg(distro)
        .arg("--")
        .arg("bash")
        .arg("-lic")
        .arg("if command -v claude >/dev/null 2>&1; then claude -p .; elif [ -x \"$HOME/.local/bin/claude\" ]; then \"$HOME/.local/bin/claude\" -p .; else exit 127; fi")
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("unable to spawn WSL Claude token refresh", error);
            return;
        }
    };

    wait_for_refresh(&mut child);
}

/// Spawn a command and wait up to `timeout` for it to finish.
/// Returns None if the process fails to start or exceeds the deadline.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<std::process::Output> {
    let mut child = cmd.spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
}

#[cfg(feature = "legacy-auto-refresh")]
fn wait_for_refresh(child: &mut std::process::Child) {
    // Wait up to 30 seconds; don't block the poll thread forever.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > Duration::from_secs(30) {
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(_) => break,
        }
    }
}

/// Resolve the full path to the `claude` CLI executable.
#[cfg(feature = "legacy-auto-refresh")]
fn resolve_windows_claude_path() -> String {
    for name in &["claude.cmd", "claude"] {
        if Command::new(name)
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return name.to_string();
        }
    }

    for name in &["claude.cmd", "claude"] {
        if let Ok(output) = Command::new("where.exe")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(first_line) = stdout.lines().next() {
                    let path = first_line.trim().to_string();
                    if !path.is_empty() {
                        return path;
                    }
                }
            }
        }
    }

    "claude.cmd".to_string()
}

pub(crate) fn resolve_windows_codex_path() -> Option<String> {
    for name in &["codex.cmd", "codex.ps1", "codex.exe", "codex"] {
        if Command::new(name)
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return Some(name.to_string());
        }
    }

    for name in &["codex.cmd", "codex.ps1", "codex.exe", "codex"] {
        if let Ok(output) = Command::new("where.exe")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(first_line) = stdout.lines().next() {
                    let path = first_line.trim().to_string();
                    if !path.is_empty() {
                        return Some(path);
                    }
                }
            }
        }
    }

    None
}

pub(crate) fn windows_codex_command(codex_path: &str) -> Command {
    let lower = codex_path.to_ascii_lowercase();
    if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        let mut command = Command::new("cmd.exe");
        command.arg("/d").arg("/c").arg(codex_path);
        command
    } else if lower.ends_with(".ps1") {
        let mut command = Command::new("powershell.exe");
        command
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(codex_path);
        command
    } else {
        Command::new(codex_path)
    }
}

struct CodexAppServer {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<CodexAppServerMessage>,
    reader: Option<JoinHandle<()>>,
}

impl CodexAppServer {
    fn start() -> Result<Self, CodexAppServerError> {
        let codex_path = resolve_windows_codex_path().ok_or(CodexAppServerError::CliUnavailable)?;
        let mut command = windows_codex_command(&codex_path);
        configure_codex_home(
            &mut command,
            std::env::var_os("CODEX_HOME"),
            resolve_user_home(),
            |path| path.is_dir(),
        );
        command
            .arg("app-server")
            .arg("--stdio")
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = command
            .spawn()
            .map_err(|_| CodexAppServerError::StartFailed)?;
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CodexAppServerError::StartFailed);
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CodexAppServerError::StartFailed);
        };
        let (sender, messages) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    let _ = sender.send(CodexAppServerMessage::ProtocolError);
                    return;
                };
                match serde_json::from_str::<CodexAppServerResponse>(&line) {
                    Ok(response) if response.id.is_some() => {
                        if sender
                            .send(CodexAppServerMessage::Response(response))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => {
                        let _ = sender.send(CodexAppServerMessage::ProtocolError);
                        return;
                    }
                }
            }
            let _ = sender.send(CodexAppServerMessage::EndOfStream);
        });

        Ok(Self {
            child,
            stdin: Some(stdin),
            messages,
            reader: Some(reader),
        })
    }

    fn send(&mut self, message: &serde_json::Value) -> Result<(), CodexAppServerError> {
        let stdin = self.stdin.as_mut().ok_or(CodexAppServerError::Protocol)?;
        serde_json::to_writer(&mut *stdin, message).map_err(|_| CodexAppServerError::Protocol)?;
        stdin
            .write_all(b"\n")
            .and_then(|_| stdin.flush())
            .map_err(|_| CodexAppServerError::Protocol)
    }

    fn wait_for_response(
        &self,
        expected_id: u64,
        deadline: Instant,
    ) -> Result<CodexAppServerResponse, CodexAppServerError> {
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(CodexAppServerError::Timeout)?;
            match self.messages.recv_timeout(remaining) {
                Ok(CodexAppServerMessage::Response(response)) => {
                    if response.id.as_ref().and_then(serde_json::Value::as_u64) == Some(expected_id)
                    {
                        return Ok(response);
                    }
                }
                Ok(CodexAppServerMessage::ProtocolError)
                | Ok(CodexAppServerMessage::EndOfStream) => {
                    return Err(CodexAppServerError::Protocol);
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(CodexAppServerError::Timeout);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(CodexAppServerError::Protocol);
                }
            }
        }
    }
}

impl Drop for CodexAppServer {
    fn drop(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn configure_codex_home(
    command: &mut Command,
    existing_codex_home: Option<std::ffi::OsString>,
    user_home: Option<PathBuf>,
    is_dir: impl FnOnce(&Path) -> bool,
) {
    if existing_codex_home.is_some() {
        return;
    }
    let Some(codex_home) = user_home.map(|home| home.join(".codex")) else {
        return;
    };
    if is_dir(&codex_home) {
        command.env("CODEX_HOME", codex_home);
    }
}

fn fetch_codex_rate_limits_result() -> Result<CodexRateLimitsReadResult, CodexAppServerError> {
    let deadline = Instant::now() + CODEX_APP_SERVER_TIMEOUT;
    let mut server = CodexAppServer::start()?;
    server.send(&serde_json::json!({
        "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": "ai-usage-monitor",
                "version": env!("CARGO_PKG_VERSION")
            }
        }
    }))?;
    let initialized = server.wait_for_response(1, deadline)?;
    if initialized.result.is_none() {
        return Err(CodexAppServerError::InitializeFailed);
    }

    server.send(&serde_json::json!({ "method": "initialized" }))?;
    server.send(&serde_json::json!({
        "id": 2,
        "method": "account/rateLimits/read"
    }))?;
    let response = server.wait_for_response(2, deadline)?;
    response.result.ok_or(CodexAppServerError::Protocol)
}

fn fetch_codex_app_server_usage() -> Result<UsageData, CodexAppServerError> {
    codex_native_usage_from_result(fetch_codex_rate_limits_result()?)
}

fn codex_native_usage_from_result(
    result: CodexRateLimitsReadResult,
) -> Result<UsageData, CodexAppServerError> {
    let limits = result
        .rate_limits
        .as_ref()
        .ok_or(CodexAppServerError::Protocol)?;
    let mut usage = UsageData::default();
    for window in [&limits.primary, &limits.secondary].into_iter().flatten() {
        let Some(duration_mins) = window.window_duration_mins else {
            continue;
        };
        let section = UsageSection {
            percentage: window.used_percent,
            resets_at: unix_to_system_time(window.resets_at),
            ..UsageSection::default()
        };
        match duration_mins {
            300 if !usage.session_available() => usage.set_session(section),
            10_080 if !usage.weekly_available() => usage.set_weekly(section),
            _ => {}
        }
    }
    if !usage.session_available() && !usage.weekly_available() {
        return Err(CodexAppServerError::Protocol);
    }
    usage.banked_reset_count = result
        .rate_limit_reset_credits
        .map(|credits| BankedResetCount::Available(credits.available_count))
        .unwrap_or(BankedResetCount::Unavailable);
    Ok(usage)
}

fn log_codex_app_server_error(error: CodexAppServerError) {
    let category = match error {
        CodexAppServerError::CliUnavailable => "Codex CLI unavailable",
        CodexAppServerError::StartFailed => "app-server start failed",
        CodexAppServerError::InitializeFailed => "app-server initialize failed",
        CodexAppServerError::Timeout => "app-server timeout",
        CodexAppServerError::Protocol => "app-server protocol error",
    };
    diagnose::log(format!("Codex quota unavailable: {category}"));
}

/// Maps an app-server failure onto an existing `PollError` variant rather
/// than growing a new one (`CODEX-OFFICIAL-PATH-IMPLEMENT-01` calls for no
/// new variants when an existing one already fits): `CliUnavailable` reads
/// the same as "no way to obtain Codex credentials/quota" to the UI
/// (`CellState::CredentialsUnavailable`), while every other app-server
/// failure (start/initialize/timeout/protocol) degrades to the generic
/// `RequestFailed` fetch-failure state. There is no `AuthRequired` mapping
/// here: this app never inspects Codex auth state itself, so it cannot claim
/// to know that re-authentication specifically is what's needed.
fn codex_app_server_error_to_poll_error(error: CodexAppServerError) -> PollError {
    log_codex_app_server_error(error);
    match error {
        CodexAppServerError::CliUnavailable => PollError::NoCredentials,
        CodexAppServerError::StartFailed
        | CodexAppServerError::InitializeFailed
        | CodexAppServerError::Timeout
        | CodexAppServerError::Protocol => PollError::RequestFailed,
    }
}

/// Short-TTL cache around one `account/rateLimits/read` app-server round
/// trip. Both 5h/7d usage and the banked reset count come from this single
/// cached result (`codex_native_usage_from_result`), so a poll cycle never
/// spawns `codex app-server` more than once per `CODEX_RATE_LIMITS_CACHE_TTL`
/// regardless of how many quota rows are on screen.
fn cached_codex_usage() -> Result<UsageData, CodexAppServerError> {
    static CACHE: OnceLock<Mutex<CodexUsageCache>> = OnceLock::new();

    let now = Instant::now();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(CodexUsageCache::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(usage) = &cache.usage {
        if cache
            .fetched_at
            .is_some_and(|fetched_at| now.duration_since(fetched_at) < CODEX_RATE_LIMITS_CACHE_TTL)
        {
            return usage.clone();
        }
    }

    let usage = fetch_codex_app_server_usage();
    cache.usage = Some(usage.clone());
    cache.fetched_at = Some(Instant::now());
    usage
}

fn build_agent() -> Result<ureq::Agent, PollError> {
    let tls = native_tls::TlsConnector::new().map_err(|_| PollError::RequestFailed)?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .tls_connector(std::sync::Arc::new(tls))
        .build())
}

pub fn credential_watch_snapshot(mode: CredentialWatchMode) -> CredentialWatchSnapshot {
    let sources = match mode {
        CredentialWatchMode::ActiveSource => read_first_credentials()
            .map(|creds| vec![creds.source])
            .unwrap_or_else(all_known_credential_sources),
        CredentialWatchMode::AllSources => all_known_credential_sources(),
    };

    let mut snapshot: CredentialWatchSnapshot = sources
        .into_iter()
        .filter_map(|source| credential_watch_signature(&source))
        .collect();
    snapshot.sort();
    snapshot.dedup();
    snapshot
}

fn all_known_credential_sources() -> Vec<CredentialSource> {
    let mut sources = Vec::new();
    if let Some(source) = windows_credential_source() {
        sources.push(source);
    }
    for distro in list_wsl_distros() {
        sources.push(CredentialSource::Wsl { distro });
    }
    sources
}

fn windows_credential_source() -> Option<CredentialSource> {
    windows_credential_source_from_home(resolve_user_home())
}

fn windows_credential_source_from_home(home: Option<PathBuf>) -> Option<CredentialSource> {
    Some(CredentialSource::Windows(
        home?.join(".claude").join(".credentials.json"),
    ))
}

fn credential_watch_signature(source: &CredentialSource) -> Option<String> {
    match source {
        CredentialSource::Windows(path) => Some(windows_credential_watch_signature(path)),
        CredentialSource::Wsl { distro } => wsl_credential_watch_signature(distro),
    }
}

fn windows_credential_watch_signature(path: &PathBuf) -> String {
    let key = format!("win:{}", path.display());
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .map(|value| value.as_secs())
                .unwrap_or(0);
            format!("{key}|present|{}|{modified}", metadata.len())
        }
        Err(_) => format!("{key}|missing"),
    }
}

fn wsl_credential_watch_signature(distro: &str) -> Option<String> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg(
                "if [ -f ~/.claude/.credentials.json ]; then \
                 stat -c 'present|%s|%Y' ~/.claude/.credentials.json; \
                 else echo missing; fi",
            )
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    )?;

    let state = if output.status.success() {
        decode_wsl_text(&output.stdout).trim().to_string()
    } else {
        format!("status-{}", output.status)
    };

    Some(format!("wsl:{distro}|{state}"))
}

fn fetch_usage_with_fallback(token: &str) -> Result<UsageData, PollError> {
    #[cfg(not(feature = "claude-messages-fallback"))]
    {
        return try_usage_endpoint(token)?.ok_or(PollError::RequestFailed);
    }

    #[cfg(feature = "claude-messages-fallback")]
    {
        // Try the dedicated usage endpoint first
        match try_usage_endpoint(token)? {
            Some(data) => {
                // If reset timers are missing, fill them in from the Messages API
                if data.session.resets_at.is_none() || data.weekly.resets_at.is_none() {
                    if let Ok(fallback) = fetch_usage_via_messages(token) {
                        let mut merged = data;
                        if merged.session.resets_at.is_none() {
                            merged.session.resets_at = fallback.session.resets_at;
                        }
                        if merged.weekly.resets_at.is_none() {
                            merged.weekly.resets_at = fallback.weekly.resets_at;
                        }
                        return Ok(merged);
                    }
                }
                return Ok(data);
            }
            None => {}
        }

        // Fall back to Messages API with rate limit headers
        let result = fetch_usage_via_messages(token);
        if result.is_err() {
            diagnose::log("usage endpoint and Messages API fallback both failed");
        }
        result
    }
}

fn try_usage_endpoint(token: &str) -> Result<Option<UsageData>, PollError> {
    let agent = build_agent()?;

    let resp = match agent
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call()
    {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "usage endpoint returned auth error status {code}; re-login required"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(_) => return Ok(None),
    };

    let response: UsageResponse = match resp.into_json() {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    Ok(Some(claude_usage_from_response(response)))
}

fn claude_usage_from_response(response: UsageResponse) -> UsageData {
    let mut data = UsageData::default();

    if let Some(bucket) = response.five_hour {
        data.set_session(UsageSection {
            percentage: bucket.utilization,
            resets_at: parse_iso8601(bucket.resets_at.as_deref()),
        });
    }

    if let Some(bucket) = response.seven_day {
        data.set_weekly(UsageSection {
            percentage: bucket.utilization,
            resets_at: parse_iso8601(bucket.resets_at.as_deref()),
        });
    }

    data
}

#[cfg(feature = "claude-messages-fallback")]
fn fetch_usage_via_messages(token: &str) -> Result<UsageData, PollError> {
    let agent = build_agent()?;

    for model in MODEL_FALLBACK_CHAIN {
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "."}]
        });

        let response = match agent
            .post(MESSAGES_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("anthropic-version", "2023-06-01")
            .set("anthropic-beta", "oauth-2025-04-20")
            .send_json(&body)
        {
            Ok(resp) => resp,
            Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => {
                diagnose::log(format!(
                    "messages endpoint returned auth error status {code}; re-login required"
                ));
                return Err(PollError::AuthRequired);
            }
            Err(ureq::Error::Status(_code, resp)) => resp,
            Err(_) => continue,
        };

        let h5 = response.header("anthropic-ratelimit-unified-5h-utilization");
        let h7 = response.header("anthropic-ratelimit-unified-7d-utilization");
        let hs = response.header("anthropic-ratelimit-unified-status");

        if h5.is_some() || h7.is_some() || hs.is_some() {
            return Ok(parse_rate_limit_headers(&response));
        }
    }

    Err(PollError::RequestFailed)
}

#[cfg(feature = "claude-messages-fallback")]
fn parse_rate_limit_headers(response: &ureq::Response) -> UsageData {
    let mut data = UsageData::default();

    let session_resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-5h-reset",
    ));
    if let Some(utilization) =
        get_header_f64(response, "anthropic-ratelimit-unified-5h-utilization")
    {
        data.set_session(UsageSection {
            percentage: utilization * 100.0,
            resets_at: session_resets_at,
        });
    } else {
        data.session.resets_at = session_resets_at;
    }

    let weekly_resets_at = unix_to_system_time(get_header_i64(
        response,
        "anthropic-ratelimit-unified-7d-reset",
    ));
    if let Some(utilization) =
        get_header_f64(response, "anthropic-ratelimit-unified-7d-utilization")
    {
        data.set_weekly(UsageSection {
            percentage: utilization * 100.0,
            resets_at: weekly_resets_at,
        });
    } else {
        data.weekly.resets_at = weekly_resets_at;
    }

    let overall_reset = get_header_i64(response, "anthropic-ratelimit-unified-reset");

    if data.session.percentage == 0.0 && data.weekly.percentage == 0.0 {
        let status = response.header("anthropic-ratelimit-unified-status");
        if status == Some("rejected") {
            let claim = response.header("anthropic-ratelimit-unified-representative-claim");
            match claim {
                Some("five_hour") => {
                    let resets_at = data.session.resets_at;
                    data.set_session(UsageSection {
                        percentage: 100.0,
                        resets_at,
                    });
                }
                Some("seven_day") => {
                    let resets_at = data.weekly.resets_at;
                    data.set_weekly(UsageSection {
                        percentage: 100.0,
                        resets_at,
                    });
                }
                _ => {}
            }
        }

        if data.session.resets_at.is_none() && overall_reset.is_some() {
            data.session.resets_at = unix_to_system_time(overall_reset);
        }
    }

    data
}

#[cfg(feature = "claude-messages-fallback")]
fn get_header_f64(response: &ureq::Response, name: &str) -> Option<f64> {
    response.header(name).and_then(|s| s.parse::<f64>().ok())
}

#[cfg(feature = "claude-messages-fallback")]
fn get_header_i64(response: &ureq::Response, name: &str) -> Option<i64> {
    response.header(name).and_then(|s| s.parse::<i64>().ok())
}

fn unix_to_system_time(unix_secs: Option<i64>) -> Option<SystemTime> {
    let secs = unix_secs?;
    if secs < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(secs as u64))
}

struct Credentials {
    access_token: String,
    expires_at: Option<i64>,
    source: CredentialSource,
}

#[derive(Clone, Debug)]
enum CredentialSource {
    Windows(PathBuf),
    Wsl { distro: String },
}

fn read_first_credentials() -> Option<Credentials> {
    if let Some(creds) = read_windows_credentials() {
        return Some(creds);
    }

    for distro in list_wsl_distros() {
        if let Some(creds) = read_wsl_credentials(&distro) {
            return Some(creds);
        }
    }

    None
}

fn read_windows_credentials() -> Option<Credentials> {
    let CredentialSource::Windows(cred_path) = windows_credential_source()? else {
        return None;
    };
    let content = match std::fs::read_to_string(&cred_path) {
        Ok(content) => content,
        Err(error) => {
            if diagnose::is_enabled() {
                diagnose::log_error(
                    &format!(
                        "unable to read Windows credentials at {}",
                        cred_path.display()
                    ),
                    error,
                );
            }
            return None;
        }
    };
    parse_credentials(&content, CredentialSource::Windows(cred_path))
}

fn read_credentials_from_source(source: &CredentialSource) -> Option<Credentials> {
    match source {
        CredentialSource::Windows(path) => {
            let content = std::fs::read_to_string(path).ok()?;
            parse_credentials(&content, source.clone())
        }
        CredentialSource::Wsl { distro } => read_wsl_credentials(distro),
    }
}

fn read_wsl_credentials(distro: &str) -> Option<Credentials> {
    let output = run_with_timeout(
        Command::new("wsl.exe")
            .arg("-d")
            .arg(distro)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg("cat ~/.claude/.credentials.json")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    )?;

    if !output.status.success() {
        diagnose::log(format!(
            "WSL credentials probe failed for distro {distro} with status {}",
            output.status
        ));
        return None;
    }

    let content = String::from_utf8(output.stdout).ok()?;
    parse_credentials(
        &content,
        CredentialSource::Wsl {
            distro: distro.to_string(),
        },
    )
}

fn parse_credentials(content: &str, source: CredentialSource) -> Option<Credentials> {
    let json: serde_json::Value = serde_json::from_str(content).ok()?;

    let oauth = json.get("claudeAiOauth")?;
    let access_token = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())?
        .to_string();
    let expires_at = oauth.get("expiresAt").and_then(|v| v.as_i64());

    Some(Credentials {
        access_token,
        expires_at,
        source,
    })
}

#[cfg(feature = "legacy-auto-refresh")]
fn read_next_credentials_after(source: &CredentialSource) -> Option<Credentials> {
    match source {
        CredentialSource::Windows(_) => {
            for distro in list_wsl_distros() {
                if let Some(creds) = read_wsl_credentials(&distro) {
                    return Some(creds);
                }
            }
        }
        CredentialSource::Wsl { distro } => {
            let mut past_current = false;
            for candidate_distro in list_wsl_distros() {
                if !past_current {
                    past_current = candidate_distro == *distro;
                    continue;
                }
                if let Some(creds) = read_wsl_credentials(&candidate_distro) {
                    return Some(creds);
                }
            }
        }
    }

    None
}

fn list_wsl_distros() -> Vec<String> {
    let output = match run_with_timeout(
        Command::new("wsl.exe")
            .args(["-l", "-q"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null()),
        Duration::from_secs(5),
    ) {
        Some(output) if output.status.success() => output,
        _ => {
            diagnose::log("unable to enumerate WSL distros");
            return Vec::new();
        }
    };

    let stdout = decode_wsl_text(&output.stdout);
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn decode_wsl_text(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    if let Some(decoded) = decode_utf16le(bytes) {
        return decoded;
    }

    String::from_utf8_lossy(bytes).into_owned()
}

fn decode_utf16le(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 2 || bytes.len() % 2 != 0 {
        return None;
    }

    let body = if bytes.starts_with(&[0xFF, 0xFE]) {
        &bytes[2..]
    } else if looks_like_utf16le(bytes) {
        bytes
    } else {
        return None;
    };

    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    Some(String::from_utf16_lossy(&units))
}

fn looks_like_utf16le(bytes: &[u8]) -> bool {
    let sample_len = bytes.len().min(128);
    let units = sample_len / 2;
    if units == 0 {
        return false;
    }

    let nul_high_bytes = bytes[..sample_len]
        .chunks_exact(2)
        .filter(|chunk| chunk[1] == 0)
        .count();

    nul_high_bytes * 2 >= units
}

fn is_token_expired(expires_at: Option<i64>) -> bool {
    let Some(exp) = expires_at else { return false };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    now >= exp
}

/// Parse an ISO 8601 timestamp string into a SystemTime.
fn parse_iso8601(s: Option<&str>) -> Option<SystemTime> {
    let s = s?;
    // Strip timezone offset to get "YYYY-MM-DDTHH:MM:SS" or with fractional seconds
    // The API returns formats like "2026-03-05T08:00:00.321598+00:00"
    let datetime_part = s.split('+').next().unwrap_or(s);
    let datetime_part = datetime_part.split('Z').next().unwrap_or(datetime_part);

    // Try parsing with and without fractional seconds
    let formats = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S"];
    for fmt in &formats {
        if let Ok(secs) = parse_datetime_to_unix(datetime_part, fmt) {
            return Some(UNIX_EPOCH + Duration::from_secs(secs));
        }
    }
    None
}

/// Minimal datetime parser — avoids pulling in chrono/time crates.
fn parse_datetime_to_unix(s: &str, _fmt: &str) -> Result<u64, ()> {
    // Extract date and time parts from "YYYY-MM-DDTHH:MM:SS[.frac]"
    let (date_str, time_str) = s.split_once('T').ok_or(())?;
    let date_parts: Vec<&str> = date_str.split('-').collect();
    if date_parts.len() != 3 {
        return Err(());
    }

    let year: u64 = date_parts[0].parse().map_err(|_| ())?;
    let month: u64 = date_parts[1].parse().map_err(|_| ())?;
    let day: u64 = date_parts[2].parse().map_err(|_| ())?;

    // Strip fractional seconds
    let time_base = time_str.split('.').next().unwrap_or(time_str);
    let time_parts: Vec<&str> = time_base.split(':').collect();
    if time_parts.len() != 3 {
        return Err(());
    }

    let hour: u64 = time_parts[0].parse().map_err(|_| ())?;
    let min: u64 = time_parts[1].parse().map_err(|_| ())?;
    let sec: u64 = time_parts[2].parse().map_err(|_| ())?;

    // Days from year (using a simplified calculation for dates after 1970)
    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }

    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += month_days[m as usize];
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }
    days += day - 1;

    Ok(days * 86400 + hour * 3600 + min * 60 + sec)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Format a usage section as "X% · Yh" style text
pub fn format_line(section: &UsageSection, strings: Strings) -> String {
    let pct = format!("{:.0}%", section.percentage);
    let cd = format_countdown(section.resets_at, strings);
    if cd.is_empty() {
        pct
    } else {
        format!("{pct} \u{00b7} {cd}")
    }
}

fn format_countdown(resets_at: Option<SystemTime>, strings: Strings) -> String {
    let reset = match resets_at {
        Some(t) => t,
        None => return String::new(),
    };

    let remaining = match reset.duration_since(SystemTime::now()) {
        Ok(d) => d,
        Err(_) => return strings.now.to_string(),
    };

    format_countdown_from_secs(remaining.as_secs(), strings)
}

/// Calculate how long until the display text would change
pub fn time_until_display_change(resets_at: Option<SystemTime>) -> Option<Duration> {
    let reset = resets_at?;
    let remaining = reset.duration_since(SystemTime::now()).ok()?;
    Some(time_until_display_change_from_secs(remaining.as_secs()))
}

fn format_countdown_from_secs(total_secs: u64, strings: Strings) -> String {
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3600;
    let total_days = total_secs / 86400;

    if total_days >= 1 {
        format!("{total_days}{}", strings.day_suffix)
    } else if total_hours >= 1 {
        format!("{total_hours}{}", strings.hour_suffix)
    } else if total_mins >= 1 {
        format!("{total_mins}{}", strings.minute_suffix)
    } else {
        format!("{total_secs}{}", strings.second_suffix)
    }
}

fn time_until_display_change_from_secs(total_secs: u64) -> Duration {
    let total_mins = total_secs / 60;
    let total_hours = total_secs / 3600;
    let total_days = total_secs / 86400;

    let current_bucket_start = if total_days >= 1 {
        total_days * 86400
    } else if total_hours >= 1 {
        total_hours * 3600
    } else if total_mins >= 1 {
        total_mins * 60
    } else {
        total_secs
    };

    Duration::from_secs(total_secs.saturating_sub(current_bucket_start) + 1)
}

/// Returns true if either section has reached "now" (reset time has passed).
pub fn is_past_reset(data: &UsageData) -> bool {
    let now = SystemTime::now();
    let past = |s: &UsageSection| matches!(s.resets_at, Some(t) if now.duration_since(t).is_ok());
    past(&data.session) || past(&data.weekly)
}

pub fn app_is_past_reset(data: &AppUsageData) -> bool {
    let now = SystemTime::now();
    data.families.iter().any(|family| {
        family
            .items
            .iter()
            .any(|item| matches!(item.resets_at, Some(reset) if now.duration_since(reset).is_ok()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_home_prefers_dirs_value() {
        let resolved = resolve_user_home_from(
            Some(PathBuf::from(r"C:\known-folder")),
            Some(r"C:\profile-fallback".into()),
            Some("D:".into()),
            Some(r"\home-fallback".into()),
        );
        assert_eq!(resolved, Some(PathBuf::from(r"C:\known-folder")));
    }

    #[test]
    fn user_home_falls_back_to_userprofile() {
        let resolved = resolve_user_home_from(None, Some(r"C:\Users\sandbox".into()), None, None);
        assert_eq!(resolved, Some(PathBuf::from(r"C:\Users\sandbox")));
    }

    #[test]
    fn claude_credential_path_uses_fallback_home() {
        let source = windows_credential_source_from_home(Some(PathBuf::from(r"C:\Users\sandbox")));
        let Some(CredentialSource::Windows(path)) = source else {
            panic!("Windows credential source should be available");
        };
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\sandbox\.claude\.credentials.json")
        );
    }

    #[test]
    fn codex_child_gets_resolved_codex_home_when_directory_exists() {
        let mut command = Command::new("codex.exe");
        configure_codex_home(
            &mut command,
            None,
            Some(PathBuf::from(r"C:\Users\sandbox")),
            |_| true,
        );
        let value = command
            .get_envs()
            .find_map(|(key, value)| (key == "CODEX_HOME").then_some(value).flatten());
        assert_eq!(value, Some(OsStr::new(r"C:\Users\sandbox\.codex")));
    }

    #[test]
    fn codex_child_does_not_override_external_codex_home() {
        let mut command = Command::new("codex.exe");
        configure_codex_home(
            &mut command,
            Some(r"D:\external-codex".into()),
            Some(PathBuf::from(r"C:\Users\sandbox")),
            |_| true,
        );
        assert!(!command.get_envs().any(|(key, _)| key == "CODEX_HOME"));
    }

    fn usage_with_session_percent(percentage: f64) -> UsageData {
        let mut usage = UsageData::default();
        usage.set_session(UsageSection {
            percentage,
            resets_at: None,
        });
        usage
    }

    #[test]
    fn claude_missing_windows_remain_unavailable() {
        let usage = claude_usage_from_response(UsageResponse {
            five_hour: None,
            seven_day: None,
        });

        assert!(!usage.session_available());
        assert!(!usage.weekly_available());
        assert_eq!(usage.session.percentage, 0.0);
        assert_eq!(usage.weekly.percentage, 0.0);
    }

    #[test]
    fn claude_actual_zero_is_available() {
        let usage = claude_usage_from_response(UsageResponse {
            five_hour: Some(UsageBucket {
                utilization: 0.0,
                resets_at: None,
            }),
            seven_day: None,
        });

        assert!(usage.session_available());
        assert_eq!(usage.session.percentage, 0.0);
        assert!(!usage.weekly_available());
    }

    #[test]
    fn claude_window_availability_is_independent() {
        let usage = claude_usage_from_response(UsageResponse {
            five_hour: None,
            seven_day: Some(UsageBucket {
                utilization: 42.0,
                resets_at: None,
            }),
        });

        assert!(!usage.session_available());
        assert!(usage.weekly_available());
        assert_eq!(usage.session.percentage, 0.0);
        assert_eq!(usage.weekly.percentage, 42.0);
    }

    // ── CODEX-OFFICIAL-PATH-IMPLEMENT-01: Codex quota comes exclusively from
    // `codex app-server`'s `account/rateLimits/read`, classified by
    // `windowDurationMins` (never by `primary`/`secondary` position). ──────

    fn codex_rate_limits_result(json: &str) -> CodexRateLimitsReadResult {
        serde_json::from_str::<CodexAppServerResponse>(json)
            .expect("valid app-server response envelope should parse")
            .result
            .expect("response should carry a result")
    }

    /// A: normal round trip — both windows present, `usedPercent`/`resetsAt`
    /// survive unchanged, and the banked reset count comes from the same
    /// response's `rateLimitResetCredits.availableCount`.
    #[test]
    fn codex_native_rate_limits_map_to_session_and_weekly_windows() {
        let result = codex_rate_limits_result(
            r#"{"id":2,"result":{
                "rateLimits": {
                    "primary": {"usedPercent": 12, "resetsAt": 1234, "windowDurationMins": 300},
                    "secondary": {"usedPercent": 34, "resetsAt": 5678, "windowDurationMins": 10080}
                },
                "rateLimitResetCredits": {"availableCount": 2}
            }}"#,
        );

        let usage = codex_native_usage_from_result(result).expect("known native windows");
        assert!(usage.session_available());
        assert!(usage.weekly_available());
        assert_eq!(usage.session.percentage, 12.0);
        assert_eq!(usage.weekly.percentage, 34.0);
        assert_eq!(usage.session.resets_at, unix_to_system_time(Some(1_234)));
        assert_eq!(usage.weekly.resets_at, unix_to_system_time(Some(5_678)));
        assert_eq!(usage.banked_reset_count, BankedResetCount::Available(2));
    }

    /// B: `primary`/`secondary` reversed relative to case A — classification
    /// must still follow `windowDurationMins`, not slot position.
    #[test]
    fn codex_native_classifies_correctly_regardless_of_primary_secondary_order() {
        let result = codex_rate_limits_result(
            r#"{"id":2,"result":{
                "rateLimits": {
                    "primary": {"usedPercent": 60, "resetsAt": 500, "windowDurationMins": 10080},
                    "secondary": {"usedPercent": 15, "resetsAt": 600, "windowDurationMins": 300}
                }
            }}"#,
        );

        let usage = codex_native_usage_from_result(result).expect("known native windows");
        assert!(usage.session_available());
        assert!(usage.weekly_available());
        assert_eq!(usage.session.percentage, 15.0);
        assert_eq!(usage.weekly.percentage, 60.0);
    }

    /// C: only the 5h window is present — 5h is available, 7d stays
    /// unavailable rather than showing a fabricated 0%.
    #[test]
    fn codex_native_five_hour_only_leaves_weekly_unavailable() {
        let result = codex_rate_limits_result(
            r#"{"id":2,"result":{
                "rateLimits": {
                    "primary": {"usedPercent": 25, "resetsAt": 100, "windowDurationMins": 300},
                    "secondary": null
                }
            }}"#,
        );

        let usage = codex_native_usage_from_result(result).expect("known native windows");
        assert!(usage.session_available());
        assert!(!usage.weekly_available());
        assert_eq!(usage.session.percentage, 25.0);
        assert_eq!(usage.weekly.percentage, 0.0);
    }

    /// D: only the 7d/weekly window is present — mirror of case C.
    #[test]
    fn codex_native_weekly_only_leaves_session_unavailable() {
        let result = codex_rate_limits_result(
            r#"{"id":2,"result":{
                "rateLimits": {
                    "primary": null,
                    "secondary": {"usedPercent": 70, "resetsAt": 200, "windowDurationMins": 10080}
                }
            }}"#,
        );

        let usage = codex_native_usage_from_result(result).expect("known native windows");
        assert!(!usage.session_available());
        assert!(usage.weekly_available());
        assert_eq!(usage.weekly.percentage, 70.0);
        assert_eq!(usage.session.percentage, 0.0);
    }

    /// E: a null window must not be treated as a fabricated 0% row.
    #[test]
    fn codex_native_null_window_is_not_fabricated_as_zero() {
        let result = codex_rate_limits_result(
            r#"{"id":2,"result":{"rateLimits": {"primary": null, "secondary": null}}}"#,
        );

        let error = codex_native_usage_from_result(result)
            .expect_err("no usable window should be a protocol error, not a fabricated 0%");
        assert_eq!(error, CodexAppServerError::Protocol);
    }

    /// F: `windowDurationMins` missing entirely on an otherwise valid window
    /// — dropped rather than guessed into either slot.
    #[test]
    fn codex_native_window_with_missing_duration_is_not_classified() {
        let result = codex_rate_limits_result(
            r#"{"id":2,"result":{
                "rateLimits": {
                    "primary": {"usedPercent": 70, "resetsAt": 700},
                    "secondary": null
                }
            }}"#,
        );

        let error = codex_native_usage_from_result(result)
            .expect_err("a window with no duration should not be classified");
        assert_eq!(error, CodexAppServerError::Protocol);
    }

    /// F: `resetsAt` missing on an otherwise valid window — the window is
    /// still available, just with no reset time.
    #[test]
    fn codex_native_window_with_missing_resets_at_still_available() {
        let result = codex_rate_limits_result(
            r#"{"id":2,"result":{
                "rateLimits": {
                    "primary": {"usedPercent": 40, "windowDurationMins": 300},
                    "secondary": null
                }
            }}"#,
        );

        let usage = codex_native_usage_from_result(result).expect("known native windows");
        assert!(usage.session_available());
        assert_eq!(usage.session.percentage, 40.0);
        assert_eq!(usage.session.resets_at, None);
    }

    /// F: `usedPercent` missing is a required field on the wire shape —
    /// the whole response fails to parse rather than defaulting to 0 or
    /// panicking.
    #[test]
    fn codex_native_window_with_missing_used_percent_fails_to_parse() {
        let response = serde_json::from_str::<CodexAppServerResponse>(
            r#"{"id":2,"result":{
                "rateLimits": {"primary": {"resetsAt": 100, "windowDurationMins": 300}}
            }}"#,
        );

        assert!(
            response.is_err(),
            "a window missing the required usedPercent field must not silently parse"
        );
    }

    /// F: `rateLimitResetCredits`/`availableCount` missing — banked reset is
    /// unavailable, not fabricated as zero, while usage windows remain valid.
    #[test]
    fn codex_native_missing_banked_reset_is_unavailable_not_zero() {
        let result = codex_rate_limits_result(
            r#"{"id":2,"result":{
                "rateLimits": {"primary": {"usedPercent": 5, "windowDurationMins": 300}}
            }}"#,
        );

        let usage = codex_native_usage_from_result(result).expect("known native windows");
        assert_eq!(usage.banked_reset_count, BankedResetCount::Unavailable);
    }

    /// F: `rateLimitResetCredits` present but malformed (missing the
    /// required `availableCount`) — the whole response fails to parse.
    #[test]
    fn codex_native_malformed_banked_reset_summary_fails_to_parse() {
        let response = serde_json::from_str::<CodexAppServerResponse>(
            r#"{"id":2,"result":{"rateLimits":{},"rateLimitResetCredits":{}}}"#,
        );

        assert!(
            response.is_err(),
            "a malformed rateLimitResetCredits summary must not silently parse"
        );
    }

    /// G: a duration matching neither 300 nor 10080 minutes must not be
    /// guessed into either slot.
    #[test]
    fn codex_native_window_with_unknown_duration_is_not_classified() {
        let result = codex_rate_limits_result(
            r#"{"id":2,"result":{
                "rateLimits": {
                    "primary": {"usedPercent": 70, "resetsAt": 700, "windowDurationMins": 60},
                    "secondary": null
                }
            }}"#,
        );

        let error = codex_native_usage_from_result(result)
            .expect_err("an unknown window duration should not be classified");
        assert_eq!(error, CodexAppServerError::Protocol);
    }

    /// H: a malformed top-level response (a JSON-RPC error envelope with no
    /// `result`) degrades to a `Protocol` error rather than panicking.
    #[test]
    fn codex_error_envelope_response_has_no_result() {
        let response: CodexAppServerResponse =
            serde_json::from_str(r#"{"id":2,"error":{"code":-32603,"message":"request failed"}}"#)
                .expect("a JSON-RPC error envelope should still parse as a response");

        assert!(response.result.is_none());
    }

    /// `codex_app_server_error_to_poll_error` must resolve every
    /// `CodexAppServerError` to an existing `PollError` variant (no new
    /// variant is added for the app-server path), and must never claim
    /// `AuthRequired` — this app does not inspect Codex auth state itself.
    #[test]
    fn codex_app_server_error_maps_to_existing_poll_error_variants_only() {
        assert_eq!(
            codex_app_server_error_to_poll_error(CodexAppServerError::CliUnavailable),
            PollError::NoCredentials
        );
        for error in [
            CodexAppServerError::StartFailed,
            CodexAppServerError::InitializeFailed,
            CodexAppServerError::Timeout,
            CodexAppServerError::Protocol,
        ] {
            assert_eq!(
                codex_app_server_error_to_poll_error(error),
                PollError::RequestFailed
            );
        }
    }

    #[test]
    fn partial_success_report_keeps_provider_error() {
        let report = poll_report_with(
            true,
            true,
            false,
            || Err(PollError::AuthRequired),
            || Ok(usage_with_session_percent(42.0)),
            || unreachable!("antigravity is disabled"),
        );

        match report.claude_code {
            ProviderPollOutcome::Error {
                source,
                attempted_at: _,
                error,
            } => {
                assert_eq!(source, ProviderPollSource::AnthropicOauthUsage);
                assert_eq!(error, PollError::AuthRequired);
            }
            _ => panic!("Claude Code error must remain in the report"),
        }
        assert!(matches!(
            report.codex,
            ProviderPollOutcome::Success {
                source: ProviderPollSource::ChatgptWhamUsage,
                ..
            }
        ));
    }

    #[test]
    fn partial_success_report_converts_to_existing_ui_result() {
        let data = poll_report_with(
            true,
            true,
            false,
            || Err(PollError::AuthRequired),
            || Ok(usage_with_session_percent(42.0)),
            || unreachable!("antigravity is disabled"),
        )
        .into_app_usage_data()
        .expect("Codex data should keep the poll successful");

        assert_eq!(
            data.family(QuotaFamilyId::Claude).unwrap().status,
            QuotaFamilyStatus::Unavailable
        );
        let codex = data.family(QuotaFamilyId::Codex).unwrap();
        assert_eq!(codex.item("session").unwrap().used_percentage(), Some(42.0));
        assert!(codex.item("weekly").is_none());
    }

    #[test]
    fn report_conversion_preserves_first_error_priority() {
        let error = poll_report_with(
            true,
            true,
            true,
            || Err(PollError::AuthRequired),
            || Err(PollError::RequestFailed),
            || Err(PollError::NoCredentials),
        )
        .into_app_usage_data()
        .expect_err("all-provider failure should return an error");

        assert_eq!(error, PollError::AuthRequired);
    }

    #[test]
    fn unrequested_providers_are_explicitly_disabled() {
        let report = poll_report_with(
            false,
            false,
            false,
            || unreachable!("Claude Code is disabled"),
            || unreachable!("Codex is disabled"),
            || unreachable!("Antigravity is disabled"),
        );

        assert!(matches!(report.claude_code, ProviderPollOutcome::Disabled));
        assert!(matches!(report.codex, ProviderPollOutcome::Disabled));
        assert!(matches!(report.antigravity, ProviderPollOutcome::Disabled));
    }

    #[test]
    fn provider_updates_are_emitted_in_serial_poll_order() {
        use std::cell::RefCell;

        let events = RefCell::new(Vec::new());
        let mut on_provider_complete = |provider: QuotaFamilyId, _: &ProviderPollOutcome| {
            events.borrow_mut().push(match provider {
                QuotaFamilyId::Claude => "claude_update",
                QuotaFamilyId::Codex => "codex_update",
                QuotaFamilyId::Antigravity => "antigravity_update",
                QuotaFamilyId::GithubCopilot => "github_copilot_update",
            });
        };

        let report = poll_report_with_clock_and_updates(
            true,
            true,
            true,
            || {
                events.borrow_mut().push("claude_poll");
                Ok(usage_with_session_percent(10.0))
            },
            || {
                events.borrow_mut().push("codex_poll");
                Err(PollError::RequestFailed)
            },
            || {
                events.borrow_mut().push("antigravity_poll");
                Ok(usage_with_session_percent(30.0))
            },
            SystemTime::now,
            &mut on_provider_complete,
        );

        assert!(matches!(
            report.claude_code,
            ProviderPollOutcome::Success { .. }
        ));
        assert!(matches!(report.codex, ProviderPollOutcome::Error { .. }));
        assert!(matches!(
            report.antigravity,
            ProviderPollOutcome::Success { .. }
        ));
        assert_eq!(
            events.into_inner(),
            [
                "claude_poll",
                "claude_update",
                "codex_poll",
                "codex_update",
                "antigravity_poll",
                "antigravity_update",
            ]
        );
    }

    #[test]
    fn success_outcome_keeps_usage_source_and_timestamps() {
        let attempted_at = UNIX_EPOCH + Duration::from_secs(10);
        let acquired_at = UNIX_EPOCH + Duration::from_secs(11);
        let mut times = [attempted_at, acquired_at].into_iter();
        let report = poll_report_with_clock(
            true,
            false,
            false,
            || Ok(usage_with_session_percent(42.0)),
            || unreachable!("Codex is disabled"),
            || unreachable!("Antigravity is disabled"),
            || times.next().expect("fixed clock should have enough values"),
        );

        match report.claude_code {
            ProviderPollOutcome::Success {
                source,
                attempted_at: actual_attempted_at,
                acquired_at: actual_acquired_at,
                usage,
            } => {
                assert_eq!(source, ProviderPollSource::AnthropicOauthUsage);
                assert_eq!(actual_attempted_at, attempted_at);
                assert_eq!(actual_acquired_at, acquired_at);
                assert_eq!(usage.session.percentage, 42.0);
                assert!(usage.session_available());
                assert!(!usage.weekly_available());
            }
            _ => panic!("Claude Code should have a successful outcome"),
        }
    }

    #[test]
    fn error_outcome_keeps_error_source_and_attempt_timestamp_without_usage() {
        let attempted_at = UNIX_EPOCH + Duration::from_secs(10);
        let mut times = [attempted_at].into_iter();
        let report = poll_report_with_clock(
            false,
            true,
            false,
            || unreachable!("Claude Code is disabled"),
            || Err(PollError::RequestFailed),
            || unreachable!("Antigravity is disabled"),
            || times.next().expect("fixed clock should have enough values"),
        );

        match report.codex {
            ProviderPollOutcome::Error {
                source,
                attempted_at: actual_attempted_at,
                error,
            } => {
                assert_eq!(source, ProviderPollSource::ChatgptWhamUsage);
                assert_eq!(actual_attempted_at, attempted_at);
                assert_eq!(error, PollError::RequestFailed);
            }
            _ => panic!("Codex should have an error outcome without usage"),
        }
    }

    #[cfg(not(feature = "antigravity"))]
    #[test]
    fn antigravity_report_is_disabled_without_feature() {
        let report = poll_report(false, false, true);

        assert!(matches!(report.antigravity, ProviderPollOutcome::Disabled));
    }

    #[test]
    fn claude_failure_does_not_block_codex_when_both_are_enabled() {
        let data = poll_with(
            true,
            true,
            false,
            || Err(PollError::AuthRequired),
            || Ok(usage_with_session_percent(42.0)),
            || unreachable!("antigravity is disabled"),
        )
        .expect("codex data should keep the poll successful");

        assert_eq!(
            data.family(QuotaFamilyId::Claude).unwrap().status,
            QuotaFamilyStatus::Unavailable
        );
        assert_eq!(
            data.family(QuotaFamilyId::Codex)
                .unwrap()
                .item("session")
                .unwrap()
                .used_percentage(),
            Some(42.0)
        );
    }

    #[test]
    fn codex_failure_does_not_block_claude_when_both_are_enabled() {
        let data = poll_with(
            true,
            true,
            false,
            || Ok(usage_with_session_percent(64.0)),
            || Err(PollError::RequestFailed),
            || unreachable!("antigravity is disabled"),
        )
        .expect("claude data should keep the poll successful");

        assert_eq!(
            data.family(QuotaFamilyId::Claude)
                .unwrap()
                .item("session")
                .unwrap()
                .used_percentage(),
            Some(64.0)
        );
        assert_eq!(
            data.family(QuotaFamilyId::Codex).unwrap().status,
            QuotaFamilyStatus::Unavailable
        );
    }

    #[test]
    fn returns_first_error_when_no_enabled_provider_succeeds() {
        let error = poll_with(
            true,
            true,
            true,
            || Err(PollError::AuthRequired),
            || Err(PollError::RequestFailed),
            || Err(PollError::NoCredentials),
        )
        .expect_err("all-provider failure should return an error");

        assert_eq!(error, PollError::AuthRequired);
    }

    #[test]
    fn antigravity_failure_does_not_block_codex_when_both_are_enabled() {
        let data = poll_with(
            false,
            true,
            true,
            || unreachable!("claude code is disabled"),
            || Ok(usage_with_session_percent(42.0)),
            || Err(PollError::NoCredentials),
        )
        .expect("codex data should keep the poll successful");

        assert_eq!(
            data.family(QuotaFamilyId::Antigravity).unwrap().status,
            QuotaFamilyStatus::Unavailable
        );
        assert_eq!(
            data.family(QuotaFamilyId::Codex)
                .unwrap()
                .item("session")
                .unwrap()
                .used_percentage(),
            Some(42.0)
        );
    }

    #[test]
    fn antigravity_family_status_is_stale_when_every_item_is_past_reset() {
        let stale_items = vec![
            QuotaItem {
                id: "gemini-weekly".to_string(),
                label: "Gemini Weekly".to_string(),
                availability: QuotaItemAvailability::Stale,
                metric: None,
                unit: QuotaUnit::Percent,
                resets_at: None,
            },
            QuotaItem {
                id: "3p-weekly".to_string(),
                label: "3p-weekly".to_string(),
                availability: QuotaItemAvailability::Stale,
                metric: None,
                unit: QuotaUnit::Percent,
                resets_at: None,
            },
        ];
        let data = poll_with(
            false,
            false,
            true,
            || unreachable!("claude code is disabled"),
            || unreachable!("codex is disabled"),
            || Ok(UsageData::from_quota_items(stale_items.clone())),
        )
        .expect("an all-stale Antigravity family is still a successful poll, not an error");

        let family = data.family(QuotaFamilyId::Antigravity).unwrap();
        assert_eq!(family.status, QuotaFamilyStatus::Stale);
        assert!(family
            .items
            .iter()
            .all(|item| item.availability == QuotaItemAvailability::Stale));
    }

    #[test]
    fn antigravity_family_stays_available_when_at_least_one_item_is_current() {
        let items = vec![
            QuotaItem::percentage("gemini-weekly", "Gemini Weekly", 12.0, None),
            QuotaItem {
                id: "3p-weekly".to_string(),
                label: "3p-weekly".to_string(),
                availability: QuotaItemAvailability::Stale,
                metric: None,
                unit: QuotaUnit::Percent,
                resets_at: None,
            },
        ];
        let data = poll_with(
            false,
            false,
            true,
            || unreachable!("claude code is disabled"),
            || unreachable!("codex is disabled"),
            || Ok(UsageData::from_quota_items(items.clone())),
        )
        .expect("a partially-current Antigravity family is a successful poll");

        let family = data.family(QuotaFamilyId::Antigravity).unwrap();
        assert_eq!(family.status, QuotaFamilyStatus::Available);
        assert_eq!(
            family.item("gemini-weekly").unwrap().used_percentage(),
            Some(12.0)
        );
    }

    #[test]
    fn github_copilot_uses_gross_quantity_and_ignores_net_quantity() {
        let response: GithubAiCreditUsageResponse = serde_json::from_str(
            r#"{
                "usageItems": [
                    {
                        "product": "Copilot",
                        "sku": "Copilot AI Credits",
                        "unitType": "credits",
                        "grossQuantity": 0.681855,
                        "netQuantity": 0
                    },
                    {
                        "product": "Copilot",
                        "sku": "Copilot AI Credits",
                        "unitType": "credits",
                        "grossQuantity": 1.25,
                        "netQuantity": 999
                    }
                ]
            }"#,
        )
        .expect("GitHub response should deserialize without retaining netQuantity");

        let usage =
            github_copilot_usage_from_response(response, GithubCopilotPlan::Pro, UNIX_EPOCH)
                .expect("Copilot credit rows should aggregate");
        let item = usage.quota_items().pop().unwrap();
        assert_eq!(item.id, GITHUB_COPILOT_MONTHLY_ITEM_ID);
        assert_eq!(
            item.metric,
            Some(QuotaMetric::Used {
                used: 1.931855,
                limit: Some(1_500.0),
            })
        );
    }

    #[test]
    fn github_cli_resolution_prefers_path_then_standard_program_files_install() {
        let path = std::ffi::OsString::from(r"C:\custom-gh;C:\other");
        let program_files = std::ffi::OsString::from(r"C:\Program Files");
        let path_candidate = PathBuf::from(r"C:\custom-gh\gh.exe");
        let standard_candidate = PathBuf::from(r"C:\Program Files\GitHub CLI\gh.exe");

        let resolved = resolve_github_cli_executable_with(
            Some(path.as_os_str()),
            Some(program_files.as_os_str()),
            |candidate| candidate == path_candidate || candidate == standard_candidate,
        );
        assert_eq!(resolved, path_candidate);

        let resolved = resolve_github_cli_executable_with(
            Some(path.as_os_str()),
            Some(program_files.as_os_str()),
            |candidate| candidate == standard_candidate,
        );
        assert_eq!(resolved, standard_candidate);
    }

    #[test]
    fn github_cli_runner_captures_child_stdout() {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/C", "echo safe-output"]);

        let output = run_with_captured_stdout(&mut command, Duration::from_secs(5))
            .expect("test child should finish");

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            "safe-output"
        );
    }

    #[test]
    fn github_copilot_plan_allowances_are_manual_and_unknown_is_usage_only() {
        assert_eq!(GithubCopilotPlan::Unknown.allowance(), None);
        assert_eq!(GithubCopilotPlan::Pro.allowance(), Some(1_500.0));
        assert_eq!(GithubCopilotPlan::ProPlus.allowance(), Some(7_000.0));
        assert_eq!(GithubCopilotPlan::Max.allowance(), Some(20_000.0));

        let usage = github_copilot_usage_from_response(
            GithubAiCreditUsageResponse {
                usage_items: Vec::new(),
            },
            GithubCopilotPlan::Unknown,
            UNIX_EPOCH,
        )
        .expect("an empty successful response is valid zero usage");
        assert_eq!(
            usage.quota_items()[0].metric,
            Some(QuotaMetric::Used {
                used: 0.0,
                limit: None,
            })
        );
    }

    #[test]
    fn github_copilot_unrecognized_nonempty_response_is_not_zero() {
        let error = github_copilot_usage_from_response(
            GithubAiCreditUsageResponse {
                usage_items: vec![GithubAiCreditUsageItem {
                    product: "Actions".to_string(),
                    sku: "Linux minutes".to_string(),
                    unit_type: "minutes".to_string(),
                    gross_quantity: 12.0,
                }],
            },
            GithubCopilotPlan::Pro,
            UNIX_EPOCH,
        )
        .expect_err("an unexpected API shape must remain unavailable, never zero");
        assert_eq!(error, PollError::RequestFailed);
    }

    #[test]
    fn github_copilot_reset_is_first_day_of_next_calendar_month_utc() {
        let now = UNIX_EPOCH
            + Duration::from_secs(days_from_civil(2026, 12, 31) as u64 * 86_400 + 86_399);
        let expected =
            UNIX_EPOCH + Duration::from_secs(days_from_civil(2027, 1, 1) as u64 * 86_400);
        assert_eq!(next_calendar_month_utc(now), Some(expected));

        let february =
            UNIX_EPOCH + Duration::from_secs(days_from_civil(2028, 2, 29) as u64 * 86_400 + 1);
        let march = UNIX_EPOCH + Duration::from_secs(days_from_civil(2028, 3, 1) as u64 * 86_400);
        assert_eq!(next_calendar_month_utc(february), Some(march));
    }

    #[cfg(not(feature = "antigravity"))]
    #[test]
    fn antigravity_requests_are_inert_without_feature() {
        assert!(
            matches!(poll(false, false, true), Err(PollError::RequestFailed)),
            "the disabled provider must not enter a poll path"
        );
    }
}
