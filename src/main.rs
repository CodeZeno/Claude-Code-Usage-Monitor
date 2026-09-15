#![windows_subsystem = "windows"]

#[cfg(feature = "diagnose")]
mod diagnose;
#[cfg(not(feature = "diagnose"))]
#[path = "diagnose_disabled.rs"]
mod diagnose;
mod localization;
mod models;
mod native_interop;
mod poller;
mod snapshot_schema;
mod snapshot_store;
mod theme;
mod tray_icon;
#[cfg(feature = "self-update")]
mod updater;
#[cfg(not(feature = "self-update"))]
#[path = "updater_disabled.rs"]
mod updater;
mod virtual_desktop;
mod window;

fn main() {
    #[cfg(any(feature = "diagnose", feature = "self-update"))]
    let args: Vec<String> = std::env::args().collect();

    #[cfg(feature = "diagnose")]
    let diagnose_enabled = args.iter().any(|arg| arg == "--diagnose");
    #[cfg(feature = "diagnose")]
    if diagnose_enabled {
        match diagnose::init() {
            Ok(path) => diagnose::log(format!("startup args={args:?} log_path={}", path.display())),
            Err(error) => {
                // Logging may not be available yet, but keep startup behavior unchanged.
                let _ = error;
            }
        }
    }

    #[cfg(feature = "self-update")]
    if let Some(exit_code) = updater::handle_cli_mode(&args) {
        #[cfg(feature = "diagnose")]
        if diagnose_enabled {
            diagnose::log(format!("cli mode exited with code {exit_code}"));
        }
        std::process::exit(exit_code);
    }

    #[cfg(feature = "diagnose")]
    if diagnose_enabled {
        diagnose::log("entering window::run");
    }
    window::run();
}
