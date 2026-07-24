//! Standalone Ollama login helper binary.
//!
//! Runs as a separate process (`ollama-login-helper.exe`) so the wry/tao
//! event loop owns the Win32 message pump. The tray spawns this binary,
//! the user completes WorkOS auth in the WebView window, this binary
//! captures the cookies, writes them to the cookie file, and exits.
//! The tray then picks up the cookies on the next poll cycle.
//!
//! Usage:
//!   ollama-login-helper.exe
//!
//! Exit codes:
//!   0 = cookies captured and written
//!   1 = timeout (no login within 180s)
//!   2 = webview init error
//!   3 = cookie write error

use std::path::PathBuf;
use std::time::{Duration, Instant};

const SIGNIN_URL: &str = "https://ollama.com/signin";
const SETTINGS_PATH: &str = "/settings";
const POLL_INTERVAL: Duration = Duration::from_millis(750);
const MAX_RUNTIME: Duration = Duration::from_secs(180);
const COOKIE_FILE: &str = "ollama_session_cookie.txt";
const LOGIN_DONE_MARKER: &str = "ollama_login_done.marker";

fn main() {
    println!("ollama-login-helper: starting (PID={})", std::process::id());

    let exit_code = run_webview_login();
    std::process::exit(exit_code);
}

fn run_webview_login() -> i32 {
    use tao::dpi::{LogicalPosition, LogicalSize};
    use tao::event::{Event, StartCause, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;
    use wry::{WebContext, WebViewBuilder};

    let event_loop = EventLoopBuilder::new().build();

    let window = match WindowBuilder::new()
        .with_title("Log in to Ollama — claude-code-usage-monitor")
        .with_inner_size(LogicalSize::new(520.0, 760.0))
        .with_position(LogicalPosition::new(100.0, 100.0))
        .with_visible(true)
        .build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("ollama-login-helper: window build failed: {e}");
            return 2;
        }
    };

    let data_dir = cookie_file_path()
        .map(|p| p.parent().unwrap().join("webview2_data"))
        .unwrap_or_else(|_| PathBuf::from("webview2_data"));
    let mut web_context = WebContext::new(Some(data_dir));

    let webview = match WebViewBuilder::new_with_web_context(&mut web_context)
        .with_url(SIGNIN_URL)
        .build(&window)
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("ollama-login-helper: webview build failed: {e}");
            return 2;
        }
    };
    println!("ollama-login-helper: webview built, entering event loop");

    let started = Instant::now();
    let mut last_poll: Option<Instant> = None;
    let mut sent = false;
    let mut exit_code: i32 = 1; // timeout by default
    let mut initial_cookie_count: Option<usize> = None;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                println!("ollama-login-helper: event loop init");
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                println!("ollama-login-helper: window closed");
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }

        let now = Instant::now();
        let due = last_poll
            .map(|t| now.duration_since(t) >= POLL_INTERVAL)
            .unwrap_or(true);
        if due {
            last_poll = Some(now);

            let url = webview.url().unwrap_or_default();
            let cookies = read_cookies(&webview);
            let ollama_cookie_count = cookies.len();

            // Track initial cookie count (from stale/previous session)
            if initial_cookie_count.is_none() {
                initial_cookie_count = Some(ollama_cookie_count);
                println!(
                    "ollama-login-helper: initial cookies={}",
                    ollama_cookie_count
                );
            }

            // Also try to get the real URL via JS (webview.url() on Windows
            // can return about:blank even after navigation has completed)
            let real_url = if url == "about:blank" {
                // Use evaluate_script to get window.location.href
                // This is a synchronous JS eval that returns the real URL
                match webview.evaluate_script("window.location.href") {
                    Ok(_) => {
                        // evaluate_script doesn't return a value in wry 0.55,
                        // but the navigation handler will have fired.
                        // Fall through to URL-based detection below.
                        url.clone()
                    }
                    Err(_) => url.clone(),
                }
            } else {
                url.clone()
            };

            let at_settings = real_url.contains(SETTINGS_PATH)
                && real_url.starts_with("https://ollama.com");
            let ollama_authorized = real_url.starts_with("https://ollama.com")
                && !real_url.contains("/signin")
                && !real_url.contains("/auth")
                && !real_url.is_empty()
                && real_url != "about:blank";

            // Success requires the browser to have returned to ollama.com
            // after WorkOS auth. A cookie-count increase on signin.ollama.com
            // is only a pre-auth flow cookie and must not be accepted.
            let cookie_count_increased = ollama_cookie_count > initial_cookie_count.unwrap_or(0);
            let auth_cookie_present = cookies.iter().any(|c| {
                c.name == "__Secure-session" || c.name == "access-token"
            });
            let at_landing = (at_settings || ollama_authorized)
                && ollama_cookie_count >= 2;

            // Log periodically or on interesting changes
            if real_url != "about:blank" || cookie_count_increased || at_landing {
                println!(
                    "ollama-login-helper: url='{}' cookies={} initial={} increased={} auth_cookie={} at_landing={}",
                    real_url,
                    ollama_cookie_count,
                    initial_cookie_count.unwrap_or(0),
                    cookie_count_increased,
                    auth_cookie_present,
                    at_landing
                );
            }

            // A persisted authenticated WebView2 profile can legitimately
            // start at about:blank. The auth cookie is stronger evidence than
            // the URL in that case; still require the cookie to be for
            // Ollama's actual session, never merely a signin flow cookie.
            if !sent && (at_landing || auth_cookie_present) {
                sent = true;
                println!(
                    "ollama-login-helper: SUCCESS — url={} cookies={}",
                    real_url, ollama_cookie_count
                );
                match write_cookie_file(&cookies) {
                    Ok(()) => {
                        write_login_done_marker();
                        exit_code = 0;
                    }
                    Err(e) => {
                        eprintln!("ollama-login-helper: cookie write failed: {e}");
                        exit_code = 3;
                    }
                }
                *control_flow = ControlFlow::Exit;
                return;
            }

            if started.elapsed() > MAX_RUNTIME {
                println!("ollama-login-helper: timed out after 180s");
                exit_code = 1;
                *control_flow = ControlFlow::Exit;
                return;
            }
        }
    });

    exit_code
}

#[derive(Debug, Clone)]
struct Cookie {
    name: String,
    value: String,
}

fn read_cookies(webview: &wry::WebView) -> Vec<Cookie> {
    match webview.cookies() {
        Ok(cookies) => cookies
            .into_iter()
            .filter(|c| {
                let name = c.name();
                let value = c.value();
                let domain = c.domain().unwrap_or_default();
                domain.ends_with("ollama.com") && !name.is_empty() && !value.is_empty()
            })
            .map(|c| Cookie {
                name: c.name().to_string(),
                value: c.value().to_string(),
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn build_cookie_header(cookies: &[Cookie]) -> String {
    cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ")
}

fn write_cookie_file(cookies: &[Cookie]) -> std::io::Result<()> {
    let path = cookie_file_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let header = build_cookie_header(cookies);
    std::fs::write(&path, header.as_bytes())?;
    println!(
        "ollama-login-helper: wrote {} bytes to {}",
        header.len(),
        path.display()
    );
    Ok(())
}

fn write_login_done_marker() {
    if let Ok(path) = cookie_file_path().map(|p| p.with_file_name(LOGIN_DONE_MARKER)) {
        let _ = std::fs::write(&path, b"1");
    }
}

fn cookie_file_path() -> std::io::Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA not set"))?;
    Ok(PathBuf::from(base)
        .join("ClaudeCodeUsageMonitor")
        .join(COOKIE_FILE))
}