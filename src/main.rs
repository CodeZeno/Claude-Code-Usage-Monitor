#![windows_subsystem = "windows"]

mod diagnose;
mod highlight;
mod localization;
mod models;
mod native_interop;
mod poller;
mod theme;
mod tray_icon;
mod updater;
mod window;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let diagnose_enabled = args.iter().any(|arg| arg == "--diagnose");
    let debug_render_enabled = args.iter().any(|arg| arg == "--debug-render");
    if diagnose_enabled {
        match diagnose::init() {
            Ok(path) => diagnose::log(format!(
                "startup args={args:?} log_path={}",
                path.display()
            )),
            Err(error) => {
                // Logging may not be available yet, but keep startup behavior unchanged.
                let _ = error;
            }
        }
    }
    if debug_render_enabled {
        highlight::DEBUG_RENDER_ENABLED
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    if let Some(exit_code) = updater::handle_cli_mode(&args) {
        if diagnose_enabled {
            diagnose::log(format!("cli mode exited with code {exit_code}"));
        }
        std::process::exit(exit_code);
    }

    if diagnose_enabled {
        diagnose::log("entering window::run");
    }
    window::run();
}
