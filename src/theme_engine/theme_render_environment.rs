//! A pinned environment for reproducible renders.
//!
//! The live widget reads the wall clock, the machine's time zone, the Windows
//! appearance and the managed themes folder. Snapshot tests and
//! `--render-theme` pin all four, so one theme and one usage fixture always
//! produce the same pixels on any machine.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq)]
pub struct FixedRenderEnvironment {
    /// Seconds since the Unix epoch for `time.now.*` and reset countdowns.
    pub now_unix: u64,
    /// Offset from UTC used for local times, so output does not depend on the
    /// machine's time zone.
    pub utc_offset_minutes: i32,
    /// Windows appearance reported to themes (`system.dark`) and floating cards.
    pub dark: bool,
    /// Folder that relative image paths resolve against instead of the managed
    /// themes folder, normally the folder holding the theme file.
    pub asset_root: Option<PathBuf>,
}

thread_local! {
    static FIXED_ENVIRONMENT: RefCell<Option<FixedRenderEnvironment>> = const { RefCell::new(None) };
}

/// Run `f` with this thread's theme renders using `environment` instead of
/// the live clock, time zone, appearance and themes folder.
pub fn with_fixed_render_environment<T>(
    environment: FixedRenderEnvironment,
    f: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<FixedRenderEnvironment>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            FIXED_ENVIRONMENT.with(|cell| *cell.borrow_mut() = previous);
        }
    }
    let _restore = Restore(FIXED_ENVIRONMENT.with(|cell| cell.replace(Some(environment))));
    f()
}

fn fixed<T>(read: impl FnOnce(&FixedRenderEnvironment) -> T) -> Option<T> {
    FIXED_ENVIRONMENT.with(|cell| cell.borrow().as_ref().map(read))
}

pub(super) fn render_now() -> SystemTime {
    fixed(|environment| UNIX_EPOCH + Duration::from_secs(environment.now_unix))
        .unwrap_or_else(SystemTime::now)
}

pub(super) fn render_now_unix() -> f64 {
    render_now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

pub(super) fn fixed_utc_offset_seconds() -> Option<i64> {
    fixed(|environment| i64::from(environment.utc_offset_minutes) * 60)
}

pub(super) fn render_dark_mode() -> bool {
    fixed(|environment| environment.dark).unwrap_or_else(crate::theme::is_dark_mode)
}

pub(super) fn fixed_asset_root() -> Option<PathBuf> {
    fixed(|environment| environment.asset_root.clone()).flatten()
}
