//! Shared quota-fetching core, reused by both the GUI monitor binary and the
//! headless `aum-quota` CLI.
//!
//! Only the modules needed to poll Claude Code / Codex quota and turn that
//! into routing decisions live here. GUI-only concerns (window, tray icon,
//! theming, snapshot history, self-update) stay owned by `src/main.rs` and
//! are not part of this crate, so this library carries none of their state
//! or windowing dependencies.
//!
//! `main.rs` does not depend on this crate — it keeps its own `mod`
//! declarations for `poller`/`models`/etc. so the existing GUI binary's
//! compilation and behavior are completely unaffected by this crate's
//! existence. Both binaries compile the same on-disk source files, so there
//! is exactly one implementation of quota polling, never two.

#[cfg(feature = "diagnose")]
pub mod diagnose;
#[cfg(not(feature = "diagnose"))]
#[path = "diagnose_disabled.rs"]
pub mod diagnose;

pub mod antigravity_statusline;
pub mod localization;
pub mod models;
pub mod poller;

pub mod dispatcher;
pub mod quota_health;
pub mod quota_report;
