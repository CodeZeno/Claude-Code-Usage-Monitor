use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

struct DiagnoseState {
    file: Mutex<File>,
}

static DIAGNOSE_STATE: OnceLock<DiagnoseState> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());

pub fn log_path() -> PathBuf {
    std::env::temp_dir().join("claude-code-usage-monitor.log")
}

/// Only load the tail: long diagnostic sessions must not freeze the dashboard.
pub fn read_tail(path: &std::path::Path, max_bytes: u64) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(max_bytes).read_to_end(&mut bytes)?;
    let start = if start > 0 {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |index| index + 1)
    } else {
        0
    };
    Ok(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

pub fn init() -> Result<PathBuf, String> {
    init_file(false)
}

pub fn init_append() -> Result<PathBuf, String> {
    init_file(true)
}

fn init_file(append: bool) -> Result<PathBuf, String> {
    let _guard = INIT_LOCK.lock().map_err(|error| error.to_string())?;
    let path = log_path();
    if is_enabled() {
        return Ok(path);
    }
    let file = open_log(&path, append)
        .map_err(|e| format!("Unable to open diagnostic log file {}: {e}", path.display()))?;

    let _ = DIAGNOSE_STATE.set(DiagnoseState {
        file: Mutex::new(file),
    });

    log(if append {
        "diagnostic logging enabled (append)"
    } else {
        "diagnostic logging enabled"
    });
    log(format!(
        "app version={} executable={}",
        env!("CARGO_PKG_VERSION"),
        std::env::current_exe().unwrap_or_default().display()
    ));
    Ok(path)
}

fn open_log(path: &std::path::Path, append: bool) -> std::io::Result<File> {
    // Both processes must always append, even after a --diagnose reset.
    // A plain write handle would overwrite lines appended by the dashboard.
    if !append {
        File::create(path)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
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

    let line = format!(
        "[{timestamp}] [pid {}] {}\n",
        std::process::id(),
        message.as_ref()
    );
    if let Ok(mut file) = state.file.lock() {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

pub fn log_error(context: &str, error: impl std::fmt::Display) {
    log(format!("{context}: {error}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_tail_tracks_appends_and_truncation_without_partial_utf8_lines() {
        let path = std::env::temp_dir().join(format!(
            "ccum-log-tail-{}-{}.log",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "old line\n✓ latest\n").unwrap();
        assert_eq!(read_tail(&path, 128).unwrap(), "old line\n✓ latest\n");
        assert_eq!(read_tail(&path, 12).unwrap(), "✓ latest\n");
        // Cut through the multi-byte character: discard that incomplete line.
        assert_eq!(read_tail(&path, 9).unwrap(), "");
        writeln!(
            OpenOptions::new().append(true).open(&path).unwrap(),
            "new result"
        )
        .unwrap();
        assert!(read_tail(&path, 128).unwrap().ends_with("new result\n"));
        std::fs::write(&path, "restarted\n").unwrap();
        assert_eq!(read_tail(&path, 128).unwrap(), "restarted\n");
        let mut monitor = open_log(&path, false).unwrap();
        monitor.write_all(b"monitor\n").unwrap();
        let mut dashboard = open_log(&path, true).unwrap();
        dashboard.write_all(b"dashboard\n").unwrap();
        monitor.write_all(b"poll complete\n").unwrap();
        assert_eq!(
            read_tail(&path, 128).unwrap(),
            "monitor\ndashboard\npoll complete\n"
        );
        drop(dashboard);
        drop(monitor);
        std::fs::remove_file(path).unwrap();
    }
}
