use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::System::Console::{
    AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS,
};

struct DiagnoseState {
    file: Mutex<File>,
}

static DIAGNOSE_STATE: OnceLock<DiagnoseState> = OnceLock::new();

pub fn init() -> Result<PathBuf, String> {
    // Attach to the parent terminal's console (e.g. the terminal that ran cargo run).
    // If no parent console exists (double-clicked the exe), allocate a new one.
    // This makes eprintln! work despite #![windows_subsystem = "windows"].
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            let _ = AllocConsole();
        }
    }

    let path = std::env::temp_dir().join("claude-code-usage-monitor.log");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|e| format!("Unable to open diagnostic log file {}: {e}", path.display()))?;

    let _ = DIAGNOSE_STATE.set(DiagnoseState {
        file: Mutex::new(file),
    });

    log("diagnostic logging enabled");
    Ok(path)
}

pub fn is_enabled() -> bool {
    DIAGNOSE_STATE.get().is_some()
}

pub fn log(message: impl AsRef<str>) {
    let Some(state) = DIAGNOSE_STATE.get() else {
        return;
    };

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    let line = format!("[{timestamp}] {}", message.as_ref());

    if let Ok(mut file) = state.file.lock() {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }

    // Mirror to stderr when a console is attached (debugger, cargo run, etc.)
    let _ = eprintln!("{line}");
}

pub fn log_error(context: &str, error: impl std::fmt::Display) {
    log(format!("{context}: {error}"));
}
