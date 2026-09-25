use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::diagnose;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const REFRESH_ARGS: &[&str] = &["-p", "."];

pub(super) fn windows_command(path: &str, directory: &Path, args: &[&str]) -> Command {
    let mut command = if path.to_ascii_lowercase().ends_with(".cmd") {
        let mut command = Command::new("cmd.exe");
        command.args(["/d", "/c", path]);
        command
    } else {
        Command::new(path)
    };
    command.args(args).env("CLAUDE_CONFIG_DIR", directory);
    configure_command(&mut command);
    command
}

pub(super) fn wsl_command(distro: &str, args: &[&str]) -> Command {
    let mut command = Command::new("wsl.exe");
    command.args([
        "-d",
        distro,
        "--exec",
        "bash",
        "-lic",
        // Resolve and invoke the CLI inside this distribution for both the
        // version probe and refresh. Positional arguments preserve quoting.
        r#"export CLAUDE_CONFIG_DIR="$HOME/.claude"; if command -v claude >/dev/null 2>&1; then claude "$@"; elif [ -x "$HOME/.local/bin/claude" ]; then "$HOME/.local/bin/claude" "$@"; else exit 127; fi"#,
        "claude-code-usage-monitor",
    ]);
    command.args(args);
    configure_command(&mut command);
    command
}

fn configure_command(command: &mut Command) {
    command
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
}

pub(super) fn refresh(mut version_command: Command, mut refresh_command: Command) -> bool {
    let Some(version) = read_version(&mut version_command, VERSION_TIMEOUT) else {
        diagnose::log("Skipping Claude token refresh: unable to determine the CLI version. Check the Claude CLI installation or sign in manually.");
        return false;
    };
    if version >= semver::Version::new(2, 0, 63) {
        refresh_command.arg("--no-session-persistence");
        diagnose::log(format!(
            "Refreshing Claude token with CLI version {version} without session persistence"
        ));
    } else {
        diagnose::log(format!("Refreshing Claude token with older CLI version {version}; background sessions will be saved. Update to 2.0.63 or later to disable session persistence."));
    }
    let mut child = match refresh_command.spawn() {
        Ok(child) => child,
        Err(error) => {
            diagnose::log_error("unable to spawn Claude token refresh", error);
            return false;
        }
    };
    let success =
        wait_for_command(&mut child, REFRESH_TIMEOUT).is_some_and(|status| status.success());
    if !success {
        diagnose::log("Claude token refresh failed or timed out; re-reading credentials");
    }
    success
}

fn parse_version(output: &str) -> Option<semver::Version> {
    // Login shells can print a banner. Only accept a complete version line,
    // with the optional suffix emitted by the official Claude CLI.
    output.lines().rev().find_map(|line| {
        let line = line.trim();
        semver::Version::parse(line.strip_suffix(" (Claude Code)").unwrap_or(line)).ok()
    })
}

fn read_version(command: &mut Command, timeout: Duration) -> Option<semver::Version> {
    let start = Instant::now();
    let mut child = command.stdout(Stdio::piped()).spawn().ok()?;
    let stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    // Drain while the command runs, bounding both memory and the wait for
    // output. A noisy shell or inherited pipe must not block the poller.
    std::thread::spawn(move || {
        let mut output = String::new();
        let result = stdout.take(64 * 1024).read_to_string(&mut output);
        let _ = sender.send(result.ok().map(|_| output));
    });
    if !wait_for_command(&mut child, timeout)?.success() {
        return None;
    }
    let output = receiver
        .recv_timeout(timeout.saturating_sub(start.elapsed()))
        .ok()??;
    parse_version(&output)
}

fn wait_for_command(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parser_accepts_cli_output_and_ignores_shell_banners() {
        for output in [
            "2.0.63 (Claude Code)\r\n",
            "2.0.63\n",
            "Welcome to Ubuntu 24.04\n2.0.63 (Claude Code)\n",
        ] {
            assert_eq!(parse_version(output), Some(semver::Version::new(2, 0, 63)));
        }
        for output in ["", "Claude not found", "2.0", "node 24.14.0", "v2.0.63"] {
            assert_eq!(parse_version(output), None, "{output}");
        }
    }

    #[test]
    fn refresh_checks_versions_before_running_a_prompt_through_a_windows_shim() {
        let root = tempfile::Builder::new()
            .prefix("Claude CLI with spaces ")
            .tempdir()
            .unwrap();
        let path = root.path().join("claude.cmd");
        let marker = root.path().join("refreshed");
        for (output, exit_code, expected_args) in [
            ("1.0.0", 0, Some("-p .")),
            ("2.0.57", 0, Some("-p .")),
            ("2.0.62", 0, Some("-p .")),
            ("2.0.63-beta.1", 0, Some("-p .")),
            ("2.0.63", 0, Some("-p . --no-session-persistence")),
            ("2.0.77", 0, Some("-p . --no-session-persistence")),
            ("2.1.0", 0, Some("-p . --no-session-persistence")),
            ("2.1.282", 0, Some("-p . --no-session-persistence")),
            ("3.0.0", 0, Some("-p . --no-session-persistence")),
            ("unknown", 0, None),
            ("2.1.282", 1, None),
        ] {
            std::fs::write(
                &path,
                format!(
                    "@echo off\r\n\
                     if \"%~1\"==\"--version\" (\r\n\
                     echo {output}\r\nexit /b {exit_code}\r\n)\r\n\
                     if not \"%~1\"==\"-p\" exit /b 2\r\n\
                     if not \"%~2\"==\".\" exit /b 2\r\n\
                     echo %*>\"%CLAUDE_CONFIG_DIR%\\refreshed\"\r\n"
                ),
            )
            .unwrap();
            let command =
                |args: &[&str]| windows_command(path.to_str().unwrap(), root.path(), args);
            assert_eq!(
                refresh(command(&["--version"]), command(REFRESH_ARGS)),
                expected_args.is_some(),
                "{output}, exit {exit_code}"
            );
            assert_eq!(
                marker.exists(),
                expected_args.is_some(),
                "{output}, exit {exit_code}"
            );
            if let Some(expected_args) = expected_args {
                assert_eq!(
                    std::fs::read_to_string(&marker).unwrap().trim(),
                    expected_args
                );
                std::fs::remove_file(&marker).unwrap();
            }
        }
    }

    #[test]
    fn version_probe_rejects_missing_and_hung_commands() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("missing.exe");
        assert_eq!(
            read_version(
                &mut windows_command(path.to_str().unwrap(), root.path(), &["--version"]),
                Duration::from_millis(200)
            ),
            None
        );
        let path = root.path().join("hung.cmd");
        std::fs::write(&path, "@echo off\r\n:loop\r\ngoto loop\r\n").unwrap();
        let start = Instant::now();
        assert_eq!(
            read_version(
                &mut windows_command(path.to_str().unwrap(), root.path(), &["--version"]),
                Duration::from_millis(200)
            ),
            None
        );
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
