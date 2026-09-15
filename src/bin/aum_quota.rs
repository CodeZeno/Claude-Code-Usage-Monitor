//! Headless quota / routing CLI.
//!
//! Reuses the exact same polling code the GUI monitor uses
//! (`claude_code_usage_monitor::poller`, via the `quota_report` /
//! `quota_health` / `dispatcher` modules) — nothing here talks to a
//! provider directly, and nothing here requires the GUI monitor process to
//! be running.
//!
//! Subcommands:
//!   aum-quota quota                       Claude/Codex quota snapshot (JSON)
//!   aum-quota health                      snapshot + quota-health per provider (JSON)
//!   aum-quota dispatch --prompt "..."      routing decision for one task (JSON, dry-run by default)
//!       [--executor auto|claude|codex] [--complexity light|standard|hard]
//!       [--model-lock NAME] [--execute]
//!   aum-quota antigravity-statusline-bridge
//!       Reads one Antigravity CLI `/statusline <command>` JSON payload from
//!       stdin, sanitizes it, and atomically writes it to this app's
//!       Antigravity statusLine cache. Intended as the `command` target of
//!       an opt-in `/statusline` configuration in Antigravity — see
//!       `antigravity_statusline` module docs. Never talks to Google or
//!       Antigravity itself, never touches credentials, and always exits
//!       successfully so a bad tick never breaks the user's status line.
//!
//! `dispatch` never launches a provider unless `--execute` is passed
//! explicitly; by default it only reports what it *would* run.

use std::io::Read;
use std::process::ExitCode;

use claude_code_usage_monitor::antigravity_statusline::{self, BridgeOutcome};
use claude_code_usage_monitor::dispatcher::{
    self, Complexity, ExecutorSuitability, ModelMapping, PreferredExecutor, Suitability, Task,
};
use claude_code_usage_monitor::quota_health::provider_health;
use claude_code_usage_monitor::quota_report;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let compact = strip_flag(&mut args, "--compact");

    let mut args = args.into_iter();
    let command = args.next().unwrap_or_else(|| "quota".to_string());

    match command.as_str() {
        "quota" => {
            let snapshot = quota_report::collect();
            print_json(&snapshot, compact);
            ExitCode::SUCCESS
        }
        "health" => {
            let snapshot = quota_report::collect();
            let health: Vec<_> = snapshot
                .providers
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "provider": p.provider,
                        "status": p.status,
                        "health": provider_health(p, snapshot.generated_at),
                    })
                })
                .collect();
            let report = serde_json::json!({
                "schema_version": snapshot.schema_version,
                "generated_at": snapshot.generated_at,
                "providers": health,
            });
            print_json(&report, compact);
            ExitCode::SUCCESS
        }
        "dispatch" => run_dispatch(args, compact),
        "antigravity-statusline-bridge" => run_antigravity_statusline_bridge(),
        "--help" | "-h" | "help" => {
            print_usage();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown command: {other}");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

/// Reads exactly one Antigravity `/statusline <command>` JSON payload from
/// stdin and bridges it into this app's sanitized cache (see
/// `antigravity_statusline` module docs). Always exits successfully and
/// always prints *something* to stdout — Antigravity renders whatever this
/// prints as the status line text, so silence or a nonzero exit would show
/// up as a broken/blank status line in the user's own terminal. Never
/// prints the raw payload or any field from it; the short strings below are
/// the only possible outputs.
fn run_antigravity_statusline_bridge() -> ExitCode {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        println!("AUM");
        return ExitCode::SUCCESS;
    }

    let cache_path = antigravity_statusline::default_cache_path();
    match antigravity_statusline::run_bridge(&raw, &cache_path, std::time::SystemTime::now()) {
        BridgeOutcome::Written => println!("AUM"),
        BridgeOutcome::Rejected(_) | BridgeOutcome::WriteFailed(_) => println!("AUM"),
    }
    ExitCode::SUCCESS
}

fn run_dispatch(mut args: std::vec::IntoIter<String>, compact: bool) -> ExitCode {
    let mut prompt: Option<String> = None;
    let mut executor = PreferredExecutor::Auto;
    let mut complexity = Complexity::Standard;
    let mut model_lock: Option<String> = None;
    let mut suitability = ExecutorSuitability::default();
    let mut execute = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--prompt" => prompt = args.next(),
            "--executor" => match args.next().as_deref() {
                Some("auto") => executor = PreferredExecutor::Auto,
                Some("claude") => executor = PreferredExecutor::Claude,
                Some("codex") => executor = PreferredExecutor::Codex,
                other => {
                    eprintln!("invalid --executor value: {other:?} (expected auto|claude|codex)");
                    return ExitCode::FAILURE;
                }
            },
            "--complexity" => match args.next().as_deref() {
                Some("light") => complexity = Complexity::Light,
                Some("standard") => complexity = Complexity::Standard,
                Some("hard") => complexity = Complexity::Hard,
                other => {
                    eprintln!(
                        "invalid --complexity value: {other:?} (expected light|standard|hard)"
                    );
                    return ExitCode::FAILURE;
                }
            },
            "--model-lock" => model_lock = args.next(),
            "--claude-suitability" => match parse_suitability(args.next().as_deref()) {
                Some(value) => suitability.claude = value,
                None => return invalid_suitability("--claude-suitability"),
            },
            "--codex-suitability" => match parse_suitability(args.next().as_deref()) {
                Some(value) => suitability.codex = value,
                None => return invalid_suitability("--codex-suitability"),
            },
            "--execute" => execute = true,
            other => {
                eprintln!("unknown dispatch flag: {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let Some(prompt) = prompt else {
        eprintln!("dispatch requires --prompt \"...\"");
        return ExitCode::FAILURE;
    };

    let task = Task {
        prompt,
        preferred_executor: executor,
        complexity,
        model_lock,
    };

    let decision =
        match dispatcher::route_with_suitability(&task, &ModelMapping::default(), suitability) {
            Ok(decision) => decision,
            Err(error) => {
                print_json(&serde_json::json!({ "error": error }), compact);
                return ExitCode::FAILURE;
            }
        };
    print_json(&decision, compact);

    if execute {
        eprintln!(
            "executing via {} (model: {})",
            decision.provider.report_key(),
            decision.model.as_deref().unwrap_or("<provider default>")
        );
        return match dispatcher::execute(&decision, &task) {
            Ok(status) => {
                if status.success() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(status.code().unwrap_or(1) as u8)
                }
            }
            Err(err) => {
                eprintln!("failed to launch {}: {err}", decision.provider.report_key());
                ExitCode::FAILURE
            }
        };
    }

    ExitCode::SUCCESS
}

fn parse_suitability(value: Option<&str>) -> Option<Suitability> {
    match value {
        Some("best") => Some(Suitability::Best),
        Some("acceptable") => Some(Suitability::Acceptable),
        Some("unsuitable") => Some(Suitability::Unsuitable),
        _ => None,
    }
}

fn invalid_suitability(flag: &str) -> ExitCode {
    eprintln!("invalid {flag} value (expected best|acceptable|unsuitable)");
    ExitCode::FAILURE
}

fn strip_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|item| item != flag);
    args.len() != before
}

fn print_json(value: &impl serde::Serialize, compact: bool) {
    let json = if compact {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    }
    .expect("report types are always serializable");
    println!("{json}");
}

fn print_usage() {
    eprintln!(
        "usage:\n  \
         aum-quota quota\n  \
         aum-quota health\n  \
         aum-quota dispatch --prompt \"...\" [--executor auto|claude|codex] [--complexity light|standard|hard] [--claude-suitability best|acceptable|unsuitable] [--codex-suitability best|acceptable|unsuitable] [--model-lock NAME] [--execute]\n  \
         aum-quota antigravity-statusline-bridge   (reads one statusLine JSON payload from stdin)\n\n\
         Add --compact to any command for non-pretty-printed JSON."
    );
}
