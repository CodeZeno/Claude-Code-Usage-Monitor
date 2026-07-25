//! Ollama Cloud login via embedded WebView2 (wry).
//!
//! # Why this exists
//!
//! ollama.com's auth is WorkOS-hosted. A raw `Cookie:` header from any
//! non-Chrome process fails the session-binding check → 303 to
//! `signin.ollama.com`. There is no public Ollama Cloud usage API. The
//! proven workaround on Windows is to log in inside a real browser context
//! and read the cookies that browser got from WorkOS.
//!
//! # How it runs
//!
//! The tray spawns a **separate helper process** (`ollama-login-helper.exe`)
//! that opens a WebView2 window pointing at `https://ollama.com/signin`.
//! The user completes WorkOS auth in that window. When the helper detects
//! the user has landed on `/settings` (or new cookies have been set), it
//! writes them to
//! `%LOCALAPPDATA%\ClaudeCodeUsageMonitor\ollama_session_cookie.txt` and
//! exits. The tray picks up the cookies on the next poll cycle.
//!
//! A separate process is required because tao's `EventLoop::run()` on
//! Windows MUST own the Win32 message pump. Running it inside the tray's
//! `DispatchMessageW` handler doesn't work — the event loop starves and
//! WebView2 never paints or navigates.
//!
//! # Feature gate
//!
//! Feature-gated behind `ollama-login-webview` so the default build doesn't
//! pull wry/tao. Enable with:
//!     `cargo build --release --features ollama-login-webview`

use crate::diagnose;

/// Spawn the standalone `ollama-login-helper.exe` as a separate process.
/// This is the ONLY way to make wry/tao work on Windows: the event loop
/// must own the Win32 message pump, which is impossible when running
/// inside the tray's `DispatchMessageW` handler.
///
/// The helper binary opens a WebView2 window, the user completes auth,
/// the helper captures cookies, writes them to the cookie file, and exits.
/// The tray picks up the cookies on the next poll cycle.
///
/// This function returns immediately (non-blocking). The tray stays
/// responsive while the helper runs.
pub fn run_login_blocking() {
    diagnose::log("ollama-login-webview: spawning ollama-login-helper.exe");

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    let helper_exe = exe_dir
        .as_ref()
        .map(|d| d.join("ollama-login-helper.exe"))
        .unwrap_or_else(|| std::path::PathBuf::from("ollama-login-helper.exe"));

    diagnose::log(format!(
        "ollama-login-webview: helper path = {}",
        helper_exe.display()
    ));

    if !helper_exe.exists() {
        diagnose::log(format!(
            "ollama-login-webview: helper exe NOT FOUND at {}",
            helper_exe.display()
        ));
        return;
    }

    match std::process::Command::new(&helper_exe)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => {
            diagnose::log(format!(
                "ollama-login-webview: helper spawned PID={}",
                child.id()
            ));
        }
        Err(e) => {
            diagnose::log_error("ollama-login-webview: failed to spawn helper", e);
        }
    }
}