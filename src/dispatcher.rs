//! Phase 3: minimal dispatcher.
//!
//! Takes one [`Task`] plus a freshly-collected [`QuotaSnapshot`]
//! (see `quota_report`), decides which provider and model tier to use, and
//! can build (or run) the matching native `claude` / `codex` CLI invocation.
//!
//! Routing priority, per the project brief:
//! 1. Executor suitability limits routing to the highest available tier.
//! 2. Model tier follows task complexity through [`ModelMapping`] — never
//!    hardcoded here, so tier->model-name changes stay a config edit.
//! 3. Explicit constraints such as model lock remain intact.
//! 4. Executor affinity and quota health choose only among equally suitable
//!    providers. Critical quota is pressure, not an availability veto.
//! 5. Quota pressure never changes task complexity.
//!
//! Model names are never invented here: Claude's defaults below are the
//! documented aliases from `claude --help` (`--model`); Codex's model ids
//! are advertised by the local app-server `model/list` method and kept here,
//! separate from the routing policy.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Suitability {
    Unsuitable,
    Acceptable,
    Best,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutorSuitability {
    pub claude: Suitability,
    pub codex: Suitability,
}

impl Default for ExecutorSuitability {
    fn default() -> Self {
        Self {
            claude: Suitability::Best,
            codex: Suitability::Best,
        }
    }
}

impl ExecutorSuitability {
    fn for_provider(self, provider: Provider) -> Suitability {
        match provider {
            Provider::Claude => self.claude,
            Provider::Codex => self.codex,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Complexity {
    Light,
    Standard,
    Hard,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchError {
    NoEligibleExecutor,
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
            codex: TierModels {
                light: Some("gpt-5.6-luna".to_string()),
                standard: Some("gpt-5.6-terra".to_string()),
                hard: Some("gpt-5.6-sol".to_string()),
            },
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
    route_with_snapshot(
        task,
        mapping,
        crate::quota_report::collect_selected(true, true),
    )
}

pub fn route_with_suitability(
    task: &Task,
    mapping: &ModelMapping,
    suitability: ExecutorSuitability,
) -> Result<RoutingDecision, DispatchError> {
    route_with_snapshot_and_suitability(
        task,
        mapping,
        crate::quota_report::collect_selected(true, true),
        suitability,
    )
}

/// Route against an already-collected snapshot (e.g. one you also want to
/// print). Pure and deterministic given the snapshot, so this is what tests
/// exercise instead of hitting real providers.
pub fn route_with_snapshot(
    task: &Task,
    mapping: &ModelMapping,
    snapshot: QuotaSnapshot,
) -> RoutingDecision {
    route_with_snapshot_and_suitability(task, mapping, snapshot, ExecutorSuitability::default())
        .expect("default suitability always leaves an executor candidate")
}

pub fn route_with_snapshot_and_suitability(
    task: &Task,
    mapping: &ModelMapping,
    snapshot: QuotaSnapshot,
    suitability: ExecutorSuitability,
) -> Result<RoutingDecision, DispatchError> {
    let generated_at = snapshot.generated_at;
    let claude_health = snapshot
        .provider(Provider::Claude.report_key())
        .map(|p| provider_health(p, generated_at))
        .unwrap_or_else(unavailable_health);
    let codex_health = snapshot
        .provider(Provider::Codex.report_key())
        .map(|p| provider_health(p, generated_at))
        .unwrap_or_else(unavailable_health);

    let (provider, complexity_used, reason) =
        decide(task, suitability, claude_health, codex_health)?;
    let model = task.model_lock.clone().or_else(|| {
        mapping
            .tiers(provider)
            .for_complexity(complexity_used)
            .map(str::to_string)
    });

    Ok(RoutingDecision {
        provider,
        model,
        complexity_used,
        claude_health,
        codex_health,
        reason,
    })
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

fn is_available(category: HealthCategory) -> bool {
    category != HealthCategory::Unavailable
}

fn decide(
    task: &Task,
    suitability: ExecutorSuitability,
    claude: ProviderHealth,
    codex: ProviderHealth,
) -> Result<(Provider, Complexity, String), DispatchError> {
    let complexity = task.complexity;

    let candidates: Vec<Provider> = [Provider::Claude, Provider::Codex]
        .into_iter()
        .filter(|provider| {
            suitability.for_provider(*provider) != Suitability::Unsuitable
                && is_available(health_of(*provider, claude, codex).category)
        })
        .collect();
    let candidates = if candidates.is_empty() {
        [Provider::Claude, Provider::Codex]
            .into_iter()
            .filter(|provider| suitability.for_provider(*provider) != Suitability::Unsuitable)
            .collect()
    } else {
        candidates
    };
    let highest_tier = candidates
        .iter()
        .map(|provider| suitability.for_provider(*provider))
        .max()
        .ok_or(DispatchError::NoEligibleExecutor)?;
    let candidates: Vec<Provider> = candidates
        .into_iter()
        .filter(|provider| suitability.for_provider(*provider) == highest_tier)
        .collect();

    if candidates.len() == 1 {
        let provider = candidates[0];
        return Ok((
            provider,
            complexity,
            format!(
                "{} is the only available executor in the highest suitability tier {:?}",
                provider.report_key(),
                highest_tier
            ),
        ));
    }

    if let Some(preferred) = match task.preferred_executor {
        PreferredExecutor::Claude => Some(Provider::Claude),
        PreferredExecutor::Codex => Some(Provider::Codex),
        PreferredExecutor::Auto => None,
    } {
        if candidates.contains(&preferred) {
            let preferred_health = health_of(preferred, claude, codex);
            let alternate = preferred.other();
            let alternate_health = health_of(alternate, claude, codex);
            if preferred_health.category == HealthCategory::Critical
                && alternate_health.category != HealthCategory::Critical
            {
                return Ok((
                    alternate,
                    complexity,
                    format!(
                        "offloaded within suitability tier {:?} from preferred {} ({:?}) to {} ({:?})",
                        highest_tier,
                        preferred.report_key(),
                        preferred_health.category,
                        alternate.report_key(),
                        alternate_health.category
                    ),
                ));
            }
            return Ok((
                preferred,
                complexity,
                format!(
                    "preferred executor {} kept within suitability tier {:?}: quota health is {:?}",
                    preferred.report_key(),
                    highest_tier,
                    preferred_health.category
                ),
            ));
        }
    }

    let claude_score = claude.score.unwrap_or(0.0);
    let codex_score = codex.score.unwrap_or(0.0);
    if codex_score > claude_score + quota_health::AUTO_SWITCH_MARGIN {
        Ok((
            Provider::Codex,
            complexity,
            format!("auto: equally suitable codex health {codex_score:.2} clearly ahead of claude {claude_score:.2}"),
        ))
    } else {
        Ok((
            Provider::Claude,
            complexity,
            format!("auto: equally suitable claude health {claude_score:.2} within margin of codex {codex_score:.2}; default to claude"),
        ))
    }
}

/// Build (but do not run) the native CLI invocation for a routing decision.
/// Resolves the real installed executable on PATH.
pub fn build_command(decision: &RoutingDecision, task: &Task) -> Command {
    match decision.provider {
        Provider::Claude => build_claude_command(&resolve_claude_executable(), decision, task),
        Provider::Codex => {
            let executable =
                poller::resolve_windows_codex_path().unwrap_or_else(|| "codex.cmd".to_string());
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
        if let Ok(output) = Command::new("where.exe")
            .arg(name)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
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
            five_hour: Some(QuotaWindow {
                used_percent: 5.0,
                resets_at: Some(2_010_000),
                stale: false,
            }),
            seven_day: Some(QuotaWindow {
                used_percent: 5.0,
                resets_at: Some(2_600_000),
                stale: false,
            }),
        }
    }

    fn critical_windows() -> QuotaWindows {
        QuotaWindows {
            five_hour: Some(QuotaWindow {
                used_percent: 95.0,
                resets_at: Some(2_010_000),
                stale: false,
            }),
            seven_day: Some(QuotaWindow {
                used_percent: 95.0,
                resets_at: Some(2_600_000),
                stale: false,
            }),
        }
    }

    fn provider(key: &str, status: ProviderStatus, windows: QuotaWindows) -> ProviderQuota {
        ProviderQuota {
            provider: key.to_string(),
            status,
            error: None,
            windows,
        }
    }

    fn snapshot(claude: ProviderQuota, codex: ProviderQuota) -> QuotaSnapshot {
        QuotaSnapshot {
            schema_version: 1,
            generated_at: 1_000_000,
            providers: vec![claude, codex],
        }
    }

    fn task(preferred: PreferredExecutor, complexity: Complexity) -> Task {
        Task {
            prompt: "do the thing".to_string(),
            preferred_executor: preferred,
            complexity,
            model_lock: None,
        }
    }

    fn route_ok(task: &Task, mapping: &ModelMapping, snapshot: QuotaSnapshot) -> RoutingDecision {
        route_with_snapshot(task, mapping, snapshot)
    }

    fn route_suitable(
        task: &Task,
        mapping: &ModelMapping,
        snapshot: QuotaSnapshot,
        claude: Suitability,
        codex: Suitability,
    ) -> Result<RoutingDecision, DispatchError> {
        route_with_snapshot_and_suitability(
            task,
            mapping,
            snapshot,
            ExecutorSuitability { claude, codex },
        )
    }

    #[test]
    fn best_claude_is_not_offloaded_to_acceptable_codex_for_quota() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, critical_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        let task = task(PreferredExecutor::Auto, Complexity::Standard);
        let decision = route_suitable(
            &task,
            &ModelMapping::default(),
            snap,
            Suitability::Best,
            Suitability::Acceptable,
        )
        .unwrap();
        assert_eq!(decision.provider, Provider::Claude);
    }

    #[test]
    fn equally_best_codex_can_receive_offload_for_quota() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, critical_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        assert_eq!(
            route_ok(
                &task(PreferredExecutor::Auto, Complexity::Standard),
                &ModelMapping::default(),
                snap,
            )
            .provider,
            Provider::Codex
        );
    }

    #[test]
    fn unsuitable_claude_routes_to_best_codex() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, healthy_windows()),
            provider("codex", ProviderStatus::Ok, critical_windows()),
        );
        let task = task(PreferredExecutor::Claude, Complexity::Standard);
        let decision = route_suitable(
            &task,
            &ModelMapping::default(),
            snap,
            Suitability::Unsuitable,
            Suitability::Best,
        )
        .unwrap();
        assert_eq!(decision.provider, Provider::Codex);
    }

    #[test]
    fn unsuitable_codex_routes_to_best_claude() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, critical_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        let task = task(PreferredExecutor::Codex, Complexity::Standard);
        let decision = route_suitable(
            &task,
            &ModelMapping::default(),
            snap,
            Suitability::Best,
            Suitability::Unsuitable,
        )
        .unwrap();
        assert_eq!(decision.provider, Provider::Claude);
    }

    #[test]
    fn both_unsuitable_returns_no_eligible_executor() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, healthy_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        let task = task(PreferredExecutor::Auto, Complexity::Standard);
        assert!(matches!(
            route_suitable(
                &task,
                &ModelMapping::default(),
                snap,
                Suitability::Unsuitable,
                Suitability::Unsuitable,
            ),
            Err(DispatchError::NoEligibleExecutor)
        ));
    }

    #[test]
    fn unavailable_best_provider_falls_back_to_available_acceptable_tier() {
        let snap = snapshot(
            provider(
                "claude_code",
                ProviderStatus::Unavailable,
                QuotaWindows::default(),
            ),
            provider("codex", ProviderStatus::Ok, critical_windows()),
        );
        let task = task(PreferredExecutor::Claude, Complexity::Hard);
        let decision = route_suitable(
            &task,
            &ModelMapping::default(),
            snap,
            Suitability::Best,
            Suitability::Acceptable,
        )
        .unwrap();
        assert_eq!(decision.provider, Provider::Codex);
        assert_eq!(decision.complexity_used, Complexity::Hard);
    }

    #[test]
    fn preferred_executor_kept_when_healthy() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, healthy_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        let decision = route_ok(
            &task(PreferredExecutor::Claude, Complexity::Standard),
            &ModelMapping::default(),
            snap,
        );
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
        let decision = route_ok(
            &task(PreferredExecutor::Claude, Complexity::Standard),
            &ModelMapping::default(),
            snap,
        );
        assert_eq!(decision.provider, Provider::Codex);
        assert!(decision.reason.contains("offloaded"));
    }

    #[test]
    fn keeps_hard_complexity_when_both_are_critical() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, critical_windows()),
            provider("codex", ProviderStatus::Ok, critical_windows()),
        );
        let decision = route_ok(
            &task(PreferredExecutor::Claude, Complexity::Hard),
            &ModelMapping::default(),
            snap,
        );
        assert_eq!(decision.provider, Provider::Claude);
        assert_eq!(decision.complexity_used, Complexity::Hard);
        assert_eq!(decision.model.as_deref(), Some("opus"));
    }

    #[test]
    fn auto_keeps_hard_complexity_when_both_are_critical() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, critical_windows()),
            provider("codex", ProviderStatus::Ok, critical_windows()),
        );
        let decision = route_ok(
            &task(PreferredExecutor::Auto, Complexity::Hard),
            &ModelMapping::default(),
            snap,
        );
        assert_eq!(decision.complexity_used, Complexity::Hard);
        assert_eq!(decision.model.as_deref(), Some("opus"));
    }

    #[test]
    fn does_not_flip_provider_for_a_small_quota_difference_in_auto_mode() {
        // codex is a few points healthier on both windows, but well within
        // AUTO_SWITCH_MARGIN — should not cause a flip away from claude.
        let codex_windows = QuotaWindows {
            five_hour: Some(QuotaWindow {
                used_percent: 2.0,
                resets_at: Some(2_010_000),
                stale: false,
            }),
            seven_day: Some(QuotaWindow {
                used_percent: 2.0,
                resets_at: Some(2_600_000),
                stale: false,
            }),
        };
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, healthy_windows()),
            provider("codex", ProviderStatus::Ok, codex_windows),
        );
        let decision = route_ok(
            &task(PreferredExecutor::Auto, Complexity::Standard),
            &ModelMapping::default(),
            snap,
        );
        assert_eq!(decision.provider, Provider::Claude);
    }

    #[test]
    fn auto_mode_switches_when_the_gap_is_large() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, critical_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        let decision = route_ok(
            &task(PreferredExecutor::Auto, Complexity::Standard),
            &ModelMapping::default(),
            snap,
        );
        assert_eq!(decision.provider, Provider::Codex);
    }

    #[test]
    fn auto_mode_routes_around_an_unavailable_provider() {
        let snap = snapshot(
            provider(
                "claude_code",
                ProviderStatus::Unavailable,
                QuotaWindows::default(),
            ),
            provider("codex", ProviderStatus::Ok, critical_windows()),
        );
        let decision = route_ok(
            &task(PreferredExecutor::Auto, Complexity::Standard),
            &ModelMapping::default(),
            snap,
        );
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
        let decision = route_ok(&t, &ModelMapping::default(), snap);
        assert_eq!(decision.model.as_deref(), Some("claude-opus-4-6-custom"));
    }

    #[test]
    fn codex_model_mapping_covers_all_complexities() {
        let snap = snapshot(
            provider("claude_code", ProviderStatus::Ok, critical_windows()),
            provider("codex", ProviderStatus::Ok, healthy_windows()),
        );
        for (complexity, expected) in [
            (Complexity::Light, "gpt-5.6-luna"),
            (Complexity::Standard, "gpt-5.6-terra"),
            (Complexity::Hard, "gpt-5.6-sol"),
        ] {
            let decision = route_ok(
                &task(PreferredExecutor::Codex, complexity),
                &ModelMapping::default(),
                snap.clone(),
            );
            assert_eq!(decision.provider, Provider::Codex);
            assert_eq!(decision.complexity_used, complexity);
            assert_eq!(decision.model.as_deref(), Some(expected));
        }
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
        let task = Task {
            prompt: "hello world".to_string(),
            preferred_executor: PreferredExecutor::Claude,
            complexity: Complexity::Standard,
            model_lock: None,
        };
        let command = build_claude_command("claude.cmd", &decision, &task);
        assert_eq!(command.get_program().to_string_lossy(), "cmd.exe");
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            args,
            vec![
                "/d",
                "/c",
                "claude.cmd",
                "-p",
                "hello world",
                "--output-format",
                "json",
                "--model",
                "sonnet"
            ]
        );
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
        let task = Task {
            prompt: "hello codex".to_string(),
            preferred_executor: PreferredExecutor::Codex,
            complexity: Complexity::Hard,
            model_lock: None,
        };
        let command = build_codex_command("codex.cmd", &decision, &task);
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            args,
            vec![
                "/d",
                "/c",
                "codex.cmd",
                "exec",
                "-m",
                "gpt-5.1-codex",
                "hello codex"
            ]
        );
    }
}
