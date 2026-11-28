#![windows_subsystem = "windows"]

mod accounts;
mod app_settings;
mod context_menu;
mod dashboard;
mod desktop_compositor;
mod diagnose;
mod font_catalog;
#[cfg(test)]
mod https_test;
mod known_folders;
mod localization;
mod models;
mod native_interop;
#[cfg(test)]
mod policy_tests;
mod poller;
mod providers;
#[cfg(test)]
mod snapshot_test;
mod studio_app;
mod theme;
mod theme_engine;
mod theme_package;
mod theme_render;
mod tray_icon;
mod ui;
mod updater;
mod widget_font;
mod window;
mod winsqlite;

fn main() {
    diagnose::install_panic_hook();

    let args: Vec<String> = std::env::args().collect();
    let diagnose_enabled = args.iter().any(|arg| arg == "--diagnose");
    if diagnose_enabled {
        let init_result = if args.iter().any(|arg| arg == "--diagnose-append") {
            diagnose::init_append()
        } else {
            diagnose::init()
        };
        match init_result {
            Ok(path) => {
                diagnose::log(format!("startup args={args:?} log_path={}", path.display()));
                for provider in providers::ProviderId::ALL {
                    let descriptor = provider.descriptor();
                    let sources: Vec<String> = descriptor
                        .credentials
                        .iter()
                        .map(|source| source.describe())
                        .collect();
                    diagnose::log(format!(
                        "{} credentials: {}",
                        descriptor.key,
                        sources.join("; ")
                    ));
                }
            }
            Err(error) => {
                // Logging may not be available yet, but keep startup behavior unchanged.
                let _ = error;
            }
        }
    }

    if studio_app::handle_cli_mode(&args) {
        return;
    }

    if let Some(exit_code) = theme_render::handle_cli_mode(&args) {
        std::process::exit(exit_code);
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
