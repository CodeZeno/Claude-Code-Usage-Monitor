//! Phase 3: minimal dispatcher.
//!
//! Takes one [`Task`] plus a freshly-collected [`QuotaSnapshot`]
//! (see `quota_report`), decides which provider and model tier to use, and
//! can build (or run) the matching native `claude` / `codex` CLI invocation.
//!
//! Routing priority, per the project brief:
//! 1. Executor affinity (the task's `preferred_executor`) wins first.
//! 2. Quota health (see `quota_health`) only overrides that when the
//!    preferred executor is in real trouble (`Critical`/`Unavailable`) *and*
//!    the alternate executor can actually take the task.
//! 3. Model tier follows task complexity through [`ModelMapping`] — never
//!    hardcoded here, so tier->model-name changes stay a config edit.
//! 4. When both executors are in trouble, the task's complexity (and so its
//!    model tier) is shrunk one notch rather than refusing outright.
//!
//! Model names are never invented here: Claude's defaults below are the
//! documented aliases from `claude --help` (`--model`); Codex's tiers are
//! left unset by default because no alias scheme or local config default
//! was found on this machine — `None` means "omit `-m`, let `codex exec`
//! use the account's own default" rather than guess an id that might not
//! exist.

use std::os::windows::process::CommandExt;
use std::process::{Command, ExitStatus};

use serde::Serialize;

use crate::poller;
use crate::quota_health::{self, provider_health, HealthCategory, ProviderHealth};
use crate::quota_report::QuotaSnapshot;

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    fn other(self) -> Provider {
        match self {
            Provider::Claude => Provider::Codex,
            Provider::Codex => Provider::Claude,
        }
    }

    /// Matches the `provider` field values used in `quota_report::QuotaSnapshot`.
    pub fn report_key(self) -> &'static str {
        match self {
            Provider::Claude => "claude_code",
            Provider::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferredExecutor {
    Auto,
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Complexity {
    Light,
    Standard,
    Hard,
}

impl Complexity {
    /// One notch cheaper; `Light` is already the floor.
    fn shrink(self) -> Complexity {
        match self {
            Complexity::Hard => Complexity::Standard,
            Complexity::Standard => Complexity::Light,
            Complexity::Light => Complexity::Light,
        }
    }
}

/// One task handed to the dispatcher. Hints are deliberately coarse — the
/// caller (e.g. a chat agent) picks a shape, not a model name.
#[derive(Debug, Clone)]
pub struct Task {
    pub prompt: String,
    pub preferred_executor: PreferredExecutor,
    pub complexity: Complexity,
    /// Escape hatch: force this exact model name, bypassing `ModelMapping`.
    /// Does not bypass executor/quota routing.
    pub model_lock: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TierModels {
    pub light: Option<String>,
    pub standard: Option<String>,
    pub hard: Option<String>,
}

impl TierModels {
    fn for_complexity(&self, complexity: Complexity) -> Option<&str> {
        match complexity {
            Complexity::Light => self.light.as_deref(),
            Complexity::Standard => self.standard.as_deref(),
            Complexity::Hard => self.hard.as_deref(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelMapping {
    pub claude: TierModels,
    pub codex: TierModels,
}

impl Default for ModelMapping {
    fn default() -> Self {
        Self {
            claude: TierModels {
                light: Some("haiku".to_string()),
                standard: Some("sonnet".to_string()),
                hard: Some("opus".to_string()),
            },
            codex: TierModels::default(),
        }
    }
}

impl ModelMapping {
    fn tiers(&self, provider: Provider) -> &TierModels {
        match provider {
            Provider::Claude => &self.claude,
            Provider::Codex => &self.codex,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RoutingDecision {
    pub provider: Provider,
    pub model: Option<String>,
    pub complexity_used: Complexity,
    pub claude_health: ProviderHealth,
    pub codex_health: ProviderHealth,
    pub reason: String,
}

/// Poll both providers fresh and route. Does not require the GUI monitor to
/// be running.
pub fn route(task: &Task, mapping: &ModelMapping) -> RoutingDecision {
    route_with_snapshot(task, mapping, crate::quota_report::collect_selected(true, true))
}

/// Route against an already-collected snapshot (e.g. one you also want to
/// print). Pure and deterministic given the snapshot, so this is what tests
/// exercise instead of hitting real providers.
pub fn route_with_snapshot(task: &Task, mapping: &ModelMapping, snapshot: QuotaSnapshot) -> RoutingDecision {
    let generated_at = snapshot.generated_at;
    let claude_health = snapshot
        .provider(Provider::Claude.report_key())
        .map(|p| provider_health(p, generated_at))
        .unwrap_or_else(unavailable_health);
    let codex_health = snapshot
        .provider(Provider::Codex.report_key())
        .map(|p| provider_health(p, generated_at))
        .unwrap_or_else(unavailable_health);

    let (provider, complexity_used, reason) = decide(task, claude_health, codex_health);
    let model = task
        .model_lock
        .clone()
        .or_else(|| mapping.tiers(provider).for_complexity(complexity_used).map(str::to_string));

    RoutingDecision {
        provider,
        model,
        complexity_used,
        claude_health,
        codex_health,
        reason,
    }
}

fn unavailable_health() -> ProviderHealth {
    ProviderHealth {
        category: HealthCategory::Unavailable,
        score: None,
        five_hour: None,
        seven_day: None,
    }
}

fn health_of(provider: Provider, claude: ProviderHealth, codex: ProviderHealth) -> ProviderHealth {
    match provider {
        Provider::Claude => claude,
        Provider::Codex => codex,
    }
}

fn is_usable(category: HealthCategory) -> bool {
    matches!(category, HealthCategory::Healthy | HealthCategory::Constrained)
}

fn decide(task: &Task, claude: ProviderHealth, codex: ProviderHealth) -> (Provider, Complexity, String) {
    let both_bad = !is_usable(claude.category) && !is_usable(codex.category);
    let complexity = if both_bad { task.complexity.shrink() } else { task.complexity };

    if let Some(preferred) = match task.preferred_executor {
        PreferredExecutor::Claude => Some(Provider::Claude),
        PreferredExecutor::Codex => Some(Provider::Codex),
        PreferredExecutor::Auto => None,
    } {
        let preferred_health = health_of(preferred, claude, codex);
        if is_usable(preferred_health.category) {
            return (
                preferred,
                complexity,
                format!(
                    "preferred executor {} kept: quota health is {:?}",
                    preferred.report_key(),
                    preferred_health.category
                ),
            );
        }

        let alternate = preferred.other();
        let alternate_health = health_of(alternate, claude, codex);
        if is_usable(alternate_health.category) {
            return (
                alternate,
                complexity,
                format!(
                    "offloaded from preferred {} ({:?}) to {} ({:?})",
                    preferred.report_key(),
                    preferred_health.category,
                    alternate.report_key(),
                    alternate_health.category
                ),
            );
        }

        return (
            preferred,
            complexity,
            format!(
                "both executors are constrained/unavailable ({}: {:?}, {}: {:?}); keeping preferred {} and shrinking task to {:?} instead of switching",
                preferred.report_key(),
                preferred_health.category,
                alternate.report_key(),
                alternate_health.category,
                preferred.report_key(),
                complexity
            ),
        );
    }

    // Auto: no fixed affinity, so let availability then quota health choose.
    match (claude.category, codex.category) {
        (HealthCategory::Unavailable, HealthCategory::Unavailable) => (
            Provider::Claude,
            complexity,
            "auto: both providers unavailable; defaulting to claude so the failure surfaces immediately".to_string(),
        ),
        (HealthCategory::Unavailable, _) => (
            Provider::Codex,
            complexity,
            "auto: claude unavailable, routing to codex".to_string(),
        ),
        (_, HealthCategory::Unavailable) => (
            Provider::Claude,
            complexity,
            "auto: codex unavailable, routing to claude".to_string(),
        ),
        _ => {
            let claude_score = claude.score.unwrap_or(0.0);
            let codex_score = codex.score.unwrap_or(0.0);
            if codex_score > claude_score + quota_health::AUTO_SWITCH_MARGIN {
                (
                    Provider::Codex,
                    complexity,
                    format!("auto: codex health {codex_score:.2} clearly ahead of claude {claude_score:.2}"),
                )
            } else {
                (
                    Provider::Claude,
                    complexity,
                    format!("auto: claude health {claude_score:.2} within margin of codex {codex_score:.2}; default to claude"),
                )
            }
        }
    }
}

/// Build (but do not run) the native CLI invocation for a routing decision.
/// Resolves the real installed executable on PATH.
pub fn build_command(decision: &RoutingDecision, task: &Task) -> Command {
    match decision.provider {
        Provider::Claude => build_claude_command(&resolve_claude_executable(), decision, task),
        Provider::Codex => {
            let executable = poller::resolve_windows_codex_path().unwrap_or_else(|| "codex.cmd".to_string());
            build_codex_command(&executable, decision, task)
        }
    }
}

fn build_claude_command(executable: &str, decision: &RoutingDecision, task: &Task) -> Command {
    let mut command = shim_command(executable);
    command.arg("-p").arg(&task.prompt);
    command.arg("--output-format").arg("json");
    if let Some(model) = &decision.model {
        command.arg("--model").arg(model);
    }
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn build_codex_command(executable: &str, decision: &RoutingDecision, task: &Task) -> Command {
    // `windows_codex_command` only branches on file extension
    // (.cmd/.bat/.ps1 vs a direct exe) — nothing codex-specific — so the
    // same helper doubles as the generic "invoke this Windows CLI shim"
    // builder for Claude in `shim_command` below.
    let mut command = poller::windows_codex_command(executable);
    command.arg("exec");
    if let Some(model) = &decision.model {
        command.arg("-m").arg(model);
    }
    command.arg(&task.prompt);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn shim_command(executable: &str) -> Command {
    poller::windows_codex_command(executable)
}

/// Actually launch the routed provider, inheriting stdio. Does not pass any
/// permission-bypass or sandbox-bypass flag — native prompts behave exactly
/// as they would if the user typed the command themselves.
pub fn execute(decision: &RoutingDecision, task: &Task) -> std::io::Result<ExitStatus> {
    build_command(decision, task).status()
}

fn resolve_claude_executable() -> String {
    for name in ["claude.cmd", "claude"] {
        let status = Command::new(name)
            .arg("--version")
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if status.is_ok() {
            return name.to_string();
        }
    }

    for name in ["claude.cmd", "claude"] {
        if let Ok(output) = Command::new("where.exe").arg(name).creation_flags(CREATE_NO_WINDOW).output() {
            if output.status.success() {
                if let Some(first_line) = String::from_utf8_lossy(&output.stdout).lines().next() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota_report::{ProviderQuota, ProviderStatus, QuotaWindow, QuotaWindows};

    fn healthy_windows() -> QuotaWindows {
        QuotaWindows {
            five_hour: Some(QuotaWindow { used_percent: 5.0, resets_at: Some(2_010_000), stale: false }),
            seven_day: Some(QuotaWindow { used_percent: 5.0, resets_at: Some(2_600_000), stale: false }),
        }
    }

    fn critical_windows() -> QuotaWindows {
        QuotaWindows {
            five_hour: Some(QuotaWindow { used_percent: 95.0, resets_at: Some(2_010_000), stale: false }),
            seven_day: Some(QuotaWindow { used_percent: 95.0, resets_at: Some(2_600_000), stale: false }),
        }
    }

    fn provider(key: &str, status: ProviderStatus, windows: QuotaWindows) -> ProviderQuota {
        ProviderQuota { provider: key.to_string(), status, error: None, windows }
    }

    fn snapshot(claude: ProviderQuota, codex: ProviderQuota) -> QuotaSnapshot {
        QuotaSnapshot {
            schema_version: 1,
            generated_at: 1_000_000,
            providers: vec![claude, codex],
        }
    }

    fn task(preferred: PreferredExecutor, complexity: Complexity) -> Task {
        Task { prompt: "do the thing".to_string(), preferred_executor: preferred, complexity, model_lock: None }
    }

    #[test]
    fn preferred_executor_kept_when_healthy() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, healthy_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        let decision = route_with_snapshot(&task(PreferredExecutor::Claude, Complexity::Standard), &ModelMapping::default(), snap);
        assert_eq!(decision.provider, Provider::Claude);
        assert_eq!(decision.model.as_deref(), Some("sonnet"));
        assert_eq!(decision.complexity_used, Complexity::Standard);
    }

    #[test]
    fn offloads_to_alternate_when_preferred_is_critical_and_alternate_is_healthy() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, critical_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        let decision = route_with_snapshot(&task(PreferredExecutor::Claude, Complexity::Standard), &ModelMapping::default(), snap);
        assert_eq!(decision.provider, Provider::Codex);
        assert!(decision.reason.contains("offloaded"));
    }

    #[test]
    fn keeps_preferred_and_shrinks_complexity_when_both_are_critical() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, critical_windows()),
            provider("codex", ProviderStatus::Ok, critical_windows()),
        );
        let decision = route_with_snapshot(&task(PreferredExecutor::Claude, Complexity::Hard), &ModelMapping::default(), snap);
        assert_eq!(decision.provider, Provider::Claude);
        assert_eq!(decision.complexity_used, Complexity::Standard);
        assert_eq!(decision.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn does_not_flip_provider_for_a_small_quota_difference_in_auto_mode() {
        // codex is a few points healthier on both windows, but well within
        // AUTO_SWITCH_MARGIN — should not cause a flip away from claude.
        let codex_windows = QuotaWindows {
            five_hour: Some(QuotaWindow { used_percent: 2.0, resets_at: Some(2_010_000), stale: false }),
            seven_day: Some(QuotaWindow { used_percent: 2.0, resets_at: Some(2_600_000), stale: false }),
        };
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, healthy_windows()),
            provider("codex", ProviderStatus::Ok, codex_windows),
        );
        let decision = route_with_snapshot(&task(PreferredExecutor::Auto, Complexity::Standard), &ModelMapping::default(), snap);
        assert_eq!(decision.provider, Provider::Claude);
    }

    #[test]
    fn auto_mode_switches_when_the_gap_is_large() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, critical_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        let decision = route_with_snapshot(&task(PreferredExecutor::Auto, Complexity::Standard), &ModelMapping::default(), snap);
        assert_eq!(decision.provider, Provider::Codex);
    }

    #[test]
    fn auto_mode_routes_around_an_unavailable_provider() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Unavailable, QuotaWindows::default()),
            provider("codex", ProviderStatus::Ok, critical_windows()),
        );
        let decision = route_with_snapshot(&task(PreferredExecutor::Auto, Complexity::Standard), &ModelMapping::default(), snap);
        assert_eq!(decision.provider, Provider::Codex);
    }

    #[test]
    fn model_lock_overrides_the_tier_mapping() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, healthy_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        let mut t = task(PreferredExecutor::Claude, Complexity::Light);
        t.model_lock = Some("claude-opus-4-6-custom".to_string());
        let decision = route_with_snapshot(&t, &ModelMapping::default(), snap);
        assert_eq!(decision.model.as_deref(), Some("claude-opus-4-6-custom"));
    }

    #[test]
    fn unset_codex_tier_omits_the_model_flag_instead_of_guessing() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, critical_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        let decision = route_with_snapshot(&task(PreferredExecutor::Codex, Complexity::Standard), &ModelMapping::default(), snap);
        assert_eq!(decision.provider, Provider::Codex);
        assert_eq!(decision.model, None);

        let task = Task { prompt: "hi".to_string(), preferred_executor: PreferredExecutor::Codex, complexity: Complexity::Standard, model_lock: None };
        let command = build_codex_command("codex.cmd", &decision, &task);
        let args: Vec<String> = command.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(args, vec!["/d", "/c", "codex.cmd", "exec", "hi"]);
        assert!(!args.iter().any(|a| a == "-m"));
    }

    #[test]
    fn build_claude_command_includes_prompt_and_model() {
        let decision = RoutingDecision {
            provider: Provider::Claude,
            model: Some("sonnet".to_string()),
            complexity_used: Complexity::Standard,
            claude_health: unavailable_health(),
            codex_health: unavailable_health(),
            reason: "test".to_string(),
        };
        let task = Task { prompt: "hello world".to_string(), preferred_executor: PreferredExecutor::Claude, complexity: Complexity::Standard, model_lock: None };
        let command = build_claude_command("claude.cmd", &decision, &task);
        assert_eq!(command.get_program().to_string_lossy(), "cmd.exe");
        let args: Vec<String> = command.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(args, vec!["/d", "/c", "claude.cmd", "-p", "hello world", "--output-format", "json", "--model", "sonnet"]);
    }

    #[test]
    fn build_codex_command_includes_prompt_and_model() {
        let decision = RoutingDecision {
            provider: Provider::Codex,
            model: Some("gpt-5.1-codex".to_string()),
            complexity_used: Complexity::Hard,
            claude_health: unavailable_health(),
            codex_health: unavailable_health(),
            reason: "test".to_string(),
        };
        let task = Task { prompt: "hello codex".to_string(), preferred_executor: PreferredExecutor::Codex, complexity: Complexity::Hard, model_lock: None };
        let command = build_codex_command("codex.cmd", &decision, &task);
        let args: Vec<String> = command.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(args, vec!["/d", "/c", "codex.cmd", "exec", "-m", "gpt-5.1-codex", "hello codex"]);
    }
}
