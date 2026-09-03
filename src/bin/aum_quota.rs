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
//!
//! `dispatch` never launches a provider unless `--execute` is passed
//! explicitly; by default it only reports what it *would* run.

use std::process::ExitCode;

use claude_code_usage_monitor::dispatcher::{self, Complexity, ModelMapping, PreferredExecutor, Task};
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

fn run_dispatch(mut args: std::vec::IntoIter<String>, compact: bool) -> ExitCode {
    let mut prompt: Option<String> = None;
    let mut executor = PreferredExecutor::Auto;
    let mut complexity = Complexity::Standard;
    let mut model_lock: Option<String> = None;
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
                    eprintln!("invalid --complexity value: {other:?} (expected light|standard|hard)");
                    return ExitCode::FAILURE;
                }
            },
            "--model-lock" => model_lock = args.next(),
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

    let decision = dispatcher::route(&task, &ModelMapping::default());
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
         aum-quota dispatch --prompt \"...\" [--executor auto|claude|codex] [--complexity light|standard|hard] [--model-lock NAME] [--execute]\n\n\
         Add --compact to any command for non-pretty-printed JSON."
    );
}
