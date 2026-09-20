//! Shared, atomically persisted state used by the widget and studio processes.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

use crate::models::{AppUsageData, CodexCreditsState};
use crate::providers::{ProviderId, ProviderSet};

pub const POLL_1_MIN_SECONDS: u32 = 60;
pub const POLL_5_MIN_SECONDS: u32 = 300;
pub const POLL_15_MIN_SECONDS: u32 = 900;
pub const POLL_1_HOUR_SECONDS: u32 = 3_600;
pub const POLL_1_MIN: u32 = POLL_1_MIN_SECONDS * 1_000;
pub const POLL_5_MIN: u32 = POLL_5_MIN_SECONDS * 1_000;
pub const POLL_15_MIN: u32 = POLL_15_MIN_SECONDS * 1_000;
pub const POLL_1_HOUR: u32 = POLL_1_HOUR_SECONDS * 1_000;
// SetTimer clamps longer intervals to USER_TIMER_MAXIMUM (i32::MAX ms).
pub const MAX_POLL_MINUTES: u32 = i32::MAX as u32 / POLL_1_MIN;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingsFile {
    #[serde(default)]
    pub accounts: crate::accounts::AccountSettings,
    #[serde(default, skip_serializing)]
    pub tray_offset: i32,
    #[serde(default, skip_serializing)]
    pub taskbar_index: usize,
    /// True only when the settings file still contains the pre-theme placement
    /// fields. While this remains true, ordinary settings saves preserve those
    /// fields so only the startup migration can consume them.
    #[serde(skip)]
    pub legacy_placement_pending: bool,
    #[serde(default = "default_true", skip_serializing)]
    pub widget_visible: bool,
    /// True only while the pre-theme `widget_visible` value still needs to be
    /// transferred to the main root's Render expression.
    #[serde(skip)]
    pub legacy_visibility_pending: bool,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update_check_unix: Option<u64>,
    #[serde(default = "default_true")]
    show_claude_code: bool,
    #[serde(default)]
    show_codex: bool,
    #[serde(default)]
    show_antigravity: bool,
    #[serde(default)]
    show_opencode: bool,
    #[serde(default)]
    show_cursor: bool,
    #[serde(default = "default_true")]
    pub custom_theme_enabled: bool,
    /// Show what is left of each allowance instead of what has been spent, so
    /// the widget counts down towards a limit rather than up from zero.
    #[serde(default)]
    pub usage_countdown: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_theme_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floating_card_opacity: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_override: Option<PlacementOverride>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementOverride {
    pub nest: String,
    #[serde(default)]
    pub monitor_index: usize,
    #[serde(default)]
    pub screen_x: i32,
    #[serde(default)]
    pub screen_y: i32,
    #[serde(default)]
    pub tray_offset: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floating_host: Option<FloatingHost>,
}

/// The unpadded, logical host size retained when a taskbar root is undocked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloatingHost {
    pub theme_id: String,
    pub surface_id: String,
    pub width: u32,
    pub height: u32,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            accounts: Default::default(),
            tray_offset: 0,
            taskbar_index: 0,
            legacy_placement_pending: false,
            widget_visible: true,
            legacy_visibility_pending: false,
            poll_interval_ms: default_poll_interval(),
            language: None,
            last_update_check_unix: None,
            show_claude_code: true,
            show_codex: false,
            show_antigravity: false,
            show_opencode: false,
            show_cursor: false,
            custom_theme_enabled: true,
            usage_countdown: false,
            active_theme_path: None,
            dashboard_width: None,
            dashboard_height: None,
            floating_card_opacity: None,
            placement_override: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyPlacement {
    pub tray_offset: i32,
    pub taskbar_index: usize,
}

impl SettingsFile {
    pub fn normalize(&mut self) {
        self.accounts.claude.normalize();
        self.accounts.codex.normalize();
        if !(POLL_1_MIN..=MAX_POLL_MINUTES * POLL_1_MIN).contains(&self.poll_interval_ms)
            || !self.poll_interval_ms.is_multiple_of(POLL_1_MIN)
        {
            self.poll_interval_ms = default_poll_interval();
        }
        if self.enabled_providers().is_empty() {
            self.set_enabled_providers(ProviderSet::default());
        }
        if let Some(opacity) = self.floating_card_opacity {
            self.floating_card_opacity = Some(opacity.min(100));
        }
        // The widget and Theme Studio are now one system. Keep accepting this
        // legacy setting so older settings files migrate cleanly.
        self.custom_theme_enabled = true;
        self.dashboard_width = valid_dashboard_dimension(self.dashboard_width);
        self.dashboard_height = valid_dashboard_dimension(self.dashboard_height);
    }

    pub fn legacy_placement(&self) -> Option<LegacyPlacement> {
        self.legacy_placement_pending.then_some(LegacyPlacement {
            tray_offset: self.tray_offset,
            taskbar_index: self.taskbar_index,
        })
    }

    pub fn consume_legacy_placement(&mut self) -> Option<LegacyPlacement> {
        let placement = self.legacy_placement()?;
        self.legacy_placement_pending = false;
        self.tray_offset = 0;
        self.taskbar_index = 0;
        Some(placement)
    }

    pub fn legacy_widget_visibility(&self) -> Option<bool> {
        self.legacy_visibility_pending
            .then_some(self.widget_visible)
    }

    pub fn consume_legacy_widget_visibility(&mut self) -> Option<bool> {
        let visible = self.legacy_widget_visibility()?;
        self.legacy_visibility_pending = false;
        self.widget_visible = true;
        Some(visible)
    }

    pub fn enabled_providers(&self) -> ProviderSet {
        ProviderSet::from_enabled(
            ProviderId::ALL
                .into_iter()
                .filter(|provider| self.provider_enabled(*provider)),
        )
    }

    pub fn provider_enabled(&self, provider: ProviderId) -> bool {
        match provider {
            ProviderId::Claude => self.show_claude_code,
            ProviderId::Codex => self.show_codex,
            ProviderId::Antigravity => self.show_antigravity,
            ProviderId::OpenCode => self.show_opencode,
            ProviderId::Cursor => self.show_cursor,
        }
    }

    pub fn set_provider_enabled(&mut self, provider: ProviderId, enabled: bool) {
        match provider {
            ProviderId::Claude => self.show_claude_code = enabled,
            ProviderId::Codex => self.show_codex = enabled,
            ProviderId::Antigravity => self.show_antigravity = enabled,
            ProviderId::OpenCode => self.show_opencode = enabled,
            ProviderId::Cursor => self.show_cursor = enabled,
        }
    }

    pub fn set_enabled_providers(&mut self, providers: ProviderSet) {
        for provider in ProviderId::ALL {
            self.set_provider_enabled(provider, providers.contains(provider));
        }
    }

    pub fn toggle_provider(&mut self, provider: ProviderId) -> bool {
        let mut providers = self.enabled_providers();
        if !providers.toggle(provider) {
            return false;
        }
        self.set_enabled_providers(providers);
        true
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UsageCache {
    pub updated_unix: u64,
    pub poll_ok: bool,
    pub data: AppUsageData,
}

#[cfg(not(test))]
pub fn app_data_directory() -> PathBuf {
    let root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    root.join("ClaudeCodeUsageMonitor")
}

/// Test threads get independent settings, themes, menus, and caches. Do not
/// change APPDATA: provider discovery and parallel tests also read it.
#[cfg(test)]
pub fn app_data_directory() -> PathBuf {
    thread_local! {
        static DIRECTORY: TestAppData = TestAppData::new();
    }
    DIRECTORY.with(|directory| directory.0.clone())
}

#[cfg(test)]
struct TestAppData(PathBuf);

#[cfg(test)]
impl TestAppData {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        loop {
            let path = std::env::temp_dir().join(format!(
                "ccum-test-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test settings directory: {error}"),
            }
        }
    }
}

#[cfg(test)]
impl Drop for TestAppData {
    fn drop(&mut self) {
        // Only remove the directory this thread successfully created.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn settings_path() -> PathBuf {
    app_data_directory().join("settings.json")
}
pub fn usage_cache_path() -> PathBuf {
    app_data_directory().join("usage-cache.json")
}

pub fn load_settings() -> SettingsFile {
    let mut settings = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|content| decode_settings(&content))
        .unwrap_or_default();
    settings.normalize();
    settings
}

pub fn save_settings(settings: &SettingsFile) -> Result<(), String> {
    let mut normalized = settings.clone();
    normalized.normalize();
    write_json_atomic(&settings_path(), &settings_json(&normalized))
}

fn decode_settings(content: &str) -> Option<SettingsFile> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    let legacy_placement_pending = value.as_object().is_some_and(|object| {
        object.contains_key("tray_offset") || object.contains_key("taskbar_index")
    });
    let legacy_visibility_pending = value
        .as_object()
        .is_some_and(|object| object.contains_key("widget_visible"));
    let mut settings: SettingsFile = serde_json::from_value(value).ok()?;
    settings.legacy_placement_pending = legacy_placement_pending;
    settings.legacy_visibility_pending = legacy_visibility_pending;
    Some(settings)
}

fn settings_json(settings: &SettingsFile) -> serde_json::Value {
    let mut value = serde_json::to_value(settings).unwrap_or_default();
    if settings.legacy_placement_pending {
        if let Some(object) = value.as_object_mut() {
            object.insert("tray_offset".into(), settings.tray_offset.into());
            object.insert("taskbar_index".into(), settings.taskbar_index.into());
        }
    }
    if settings.legacy_visibility_pending {
        if let Some(object) = value.as_object_mut() {
            object.insert("widget_visible".into(), settings.widget_visible.into());
        }
    }
    value
}

pub fn codex_credits_path() -> PathBuf {
    app_data_directory().join("codex-credits.json")
}

pub fn load_codex_credits() -> Option<CodexCreditsState> {
    read_json(&codex_credits_path())
}

pub fn save_codex_credits(state: &CodexCreditsState) -> Result<(), String> {
    write_json_atomic(&codex_credits_path(), state)
}

pub fn load_usage_cache() -> Option<UsageCache> {
    let mut cache: UsageCache = read_json(&usage_cache_path())?;
    cache.data.invalidate_changed_credentials();
    Some(cache)
}

pub fn save_usage_cache(data: &AppUsageData, poll_ok: bool) -> Result<(), String> {
    write_json_atomic(
        &usage_cache_path(),
        &UsageCache {
            updated_unix: now_unix(),
            poll_ok,
            data: data.clone(),
        },
    )
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid settings path")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state.json");
    let temporary = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let json = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary).map_err(|error| error.to_string())?;
        file.write_all(&json).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    let source = wide_path(&temporary);
    let destination = wide_path(path);
    let moved = unsafe {
        MoveFileExW(
            PCWSTR::from_raw(source.as_ptr()),
            PCWSTR::from_raw(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return Err("Unable to replace the settings file".into());
    }
    Ok(())
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn default_poll_interval() -> u32 {
    POLL_15_MIN
}
fn default_true() -> bool {
    true
}
fn valid_dashboard_dimension(value: Option<f32>) -> Option<f32> {
    value.filter(|value| value.is_finite() && (64.0..=16_384.0).contains(value))
}
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_files_stay_inside_the_test_directory() {
        let root = app_data_directory();
        assert!(root.starts_with(std::env::temp_dir()));
        if let Some(real) = std::env::var_os("APPDATA") {
            assert!(!root.starts_with(PathBuf::from(real).join("ClaudeCodeUsageMonitor")));
        }
        for path in [
            settings_path(),
            usage_cache_path(),
            codex_credits_path(),
            crate::theme_engine::themes_directory(),
            crate::theme_engine::assets_directory(),
            crate::context_menu::context_menus_directory(),
            crate::theme_engine::ensure_starter_theme().unwrap(),
            crate::context_menu::ensure_builtin_context_menus().unwrap(),
        ] {
            assert!(path.starts_with(&root), "{}", path.display());
        }
        save_settings(&SettingsFile::default()).unwrap();
        assert!(settings_path().is_file());
    }

    #[test]
    fn parallel_test_threads_have_independent_settings_and_clean_up() {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let workers: Vec<_> = [7, 11]
            .into_iter()
            .map(|minutes| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let settings = SettingsFile {
                        poll_interval_ms: minutes * POLL_1_MIN,
                        ..Default::default()
                    };
                    save_settings(&settings).unwrap();
                    barrier.wait();
                    assert_eq!(load_settings().poll_interval_ms, minutes * POLL_1_MIN);
                    app_data_directory()
                })
            })
            .collect();
        let paths: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_ne!(paths[0], paths[1]);
        assert!(paths.iter().all(|path| !path.exists()));
    }

    #[test]
    fn custom_poll_minutes_and_presets_survive_settings_round_trip() {
        for minutes in [1, 2, 5, 7, 15, 60, 120, 1_440, MAX_POLL_MINUTES] {
            let interval = minutes * POLL_1_MIN;
            let mut decoded =
                decode_settings(&format!(r#"{{"poll_interval_ms":{interval}}}"#)).unwrap();
            decoded.normalize();
            assert_eq!(decoded.poll_interval_ms, interval);
            let mut reloaded = decode_settings(&settings_json(&decoded).to_string()).unwrap();
            reloaded.normalize();
            assert_eq!(reloaded.poll_interval_ms, interval);
        }
    }

    #[test]
    fn invalid_poll_intervals_fall_back_to_the_default() {
        for interval in [
            0,
            POLL_1_MIN - 1,
            POLL_1_MIN + 1,
            (MAX_POLL_MINUTES + 1) * POLL_1_MIN,
            u32::MAX,
        ] {
            let mut decoded =
                decode_settings(&format!(r#"{{"poll_interval_ms":{interval}}}"#)).unwrap();
            decoded.normalize();
            assert_eq!(decoded.poll_interval_ms, default_poll_interval());
        }
    }

    #[test]
    fn named_accounts_round_trip_without_changing_legacy_provider_preferences() {
        let old = decode_settings(r#"{"show_claude_code":true,"show_codex":true}"#).unwrap();
        assert_eq!(old.accounts, crate::accounts::AccountSettings::default());
        let mut settings = old;
        settings.accounts.codex.add();
        settings.accounts.codex.profiles[1].config_dir = "C:\\Users\\Test\\.codex-work".into();
        settings.accounts.codex.profiles[1].enabled = true;
        settings.accounts.codex.selected = "account_1".into();
        let decoded = decode_settings(&settings_json(&settings).to_string()).unwrap();
        assert_eq!(decoded.accounts, settings.accounts);
        assert_eq!(decoded.enabled_providers(), settings.enabled_providers());
    }

    #[test]
    fn settings_never_disable_every_provider() {
        let mut settings = SettingsFile {
            show_claude_code: false,
            show_codex: false,
            show_antigravity: false,
            ..Default::default()
        };
        settings.normalize();
        assert_eq!(settings.enabled_providers(), ProviderSet::default());
    }

    #[test]
    fn provider_selection_keeps_the_existing_settings_keys() {
        let mut settings = SettingsFile::default();
        settings.set_enabled_providers(ProviderSet::from_enabled([
            ProviderId::Codex,
            ProviderId::Antigravity,
            ProviderId::OpenCode,
            ProviderId::Cursor,
        ]));

        let json = settings_json(&settings);
        assert_eq!(json["show_claude_code"], false);
        assert_eq!(json["show_codex"], true);
        assert_eq!(json["show_antigravity"], true);
        assert_eq!(json["show_opencode"], true);
        assert_eq!(json["show_cursor"], true);

        let decoded = decode_settings(&json.to_string()).unwrap();
        assert_eq!(decoded.enabled_providers(), settings.enabled_providers());
    }

    #[test]
    fn provider_toggle_keeps_the_last_provider_enabled() {
        let mut settings = SettingsFile::default();
        assert!(!settings.toggle_provider(ProviderId::Claude));
        assert_eq!(settings.enabled_providers(), ProviderSet::default());
    }

    #[test]
    fn usage_direction_defaults_to_counting_up_and_round_trips() {
        let settings = SettingsFile::default();
        assert!(!settings.usage_countdown);
        assert_eq!(settings_json(&settings)["usage_countdown"], false);

        let counting_up = decode_settings(r#"{"poll_interval_ms":900000}"#).unwrap();
        assert!(!counting_up.usage_countdown);

        let counting_down = decode_settings(r#"{"usage_countdown":true}"#).unwrap();
        assert!(counting_down.usage_countdown);
        assert_eq!(settings_json(&counting_down)["usage_countdown"], true);
    }

    #[test]
    fn settings_always_use_the_theme_widget() {
        let mut settings = SettingsFile {
            custom_theme_enabled: false,
            ..Default::default()
        };
        settings.normalize();
        assert!(settings.custom_theme_enabled);
    }

    #[test]
    fn legacy_widget_visibility_is_preserved_until_migration_consumes_it() {
        let mut settings = decode_settings(r#"{"widget_visible":false}"#).unwrap();
        assert_eq!(settings.legacy_widget_visibility(), Some(false));
        assert_eq!(settings_json(&settings)["widget_visible"], false);

        assert_eq!(settings.consume_legacy_widget_visibility(), Some(false));
        assert_eq!(settings.legacy_widget_visibility(), None);
        assert!(settings_json(&settings).get("widget_visible").is_none());
    }

    #[test]
    fn legacy_placement_is_preserved_until_the_migration_consumes_it() {
        let mut settings = decode_settings(
            r#"{
                "tray_offset": 144,
                "taskbar_index": 2,
                "poll_interval_ms": 60000,
                "show_claude_code": true
            }"#,
        )
        .unwrap();

        assert_eq!(
            settings.legacy_placement(),
            Some(LegacyPlacement {
                tray_offset: 144,
                taskbar_index: 2,
            })
        );
        let pending = settings_json(&settings);
        assert_eq!(pending["tray_offset"], 144);
        assert_eq!(pending["taskbar_index"], 2);

        settings.consume_legacy_placement();
        let migrated = settings_json(&settings);
        assert!(migrated.get("tray_offset").is_none());
        assert!(migrated.get("taskbar_index").is_none());
        assert_eq!(migrated["poll_interval_ms"], 60000);
    }

    #[test]
    fn modern_settings_do_not_request_legacy_migration() {
        let settings = decode_settings(
            r#"{
                "poll_interval_ms": 900000,
                "active_theme_path": "migrated-theme.json"
            }"#,
        )
        .unwrap();
        assert_eq!(settings.legacy_placement(), None);
        assert_eq!(settings.legacy_widget_visibility(), None);
    }

    #[test]
    fn dashboard_dimensions_are_preserved_and_validated() {
        let settings = decode_settings(
            r#"{
                "dashboard_width": 1280.5,
                "dashboard_height": 760.0
            }"#,
        )
        .unwrap();
        assert_eq!(settings.dashboard_width, Some(1280.5));
        assert_eq!(settings.dashboard_height, Some(760.0));

        let mut invalid = SettingsFile {
            dashboard_width: Some(0.0),
            dashboard_height: Some(20_000.0),
            ..Default::default()
        };
        invalid.normalize();
        assert_eq!(invalid.dashboard_width, None);
        assert_eq!(invalid.dashboard_height, None);
    }
}
