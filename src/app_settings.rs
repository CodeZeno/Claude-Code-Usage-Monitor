//! Shared, atomically persisted state used by the widget and studio processes.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

use crate::models::AppUsageData;
use crate::providers::{ProviderId, ProviderSet};

pub const POLL_1_MIN_SECONDS: u32 = 60;
pub const POLL_5_MIN_SECONDS: u32 = 300;
pub const POLL_15_MIN_SECONDS: u32 = 900;
pub const POLL_1_HOUR_SECONDS: u32 = 3_600;
pub const POLL_1_MIN: u32 = POLL_1_MIN_SECONDS * 1_000;
pub const POLL_5_MIN: u32 = POLL_5_MIN_SECONDS * 1_000;
pub const POLL_15_MIN: u32 = POLL_15_MIN_SECONDS * 1_000;
pub const POLL_1_HOUR: u32 = POLL_1_HOUR_SECONDS * 1_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SettingsFile {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_theme_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_width: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_height: Option<f32>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
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
            active_theme_path: None,
            dashboard_width: None,
            dashboard_height: None,
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
        if !matches!(
            self.poll_interval_ms,
            POLL_1_MIN | POLL_5_MIN | POLL_15_MIN | POLL_1_HOUR
        ) {
            self.poll_interval_ms = default_poll_interval();
        }
        if self.enabled_providers().is_empty() {
            self.set_enabled_providers(ProviderSet::default());
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

pub fn app_data_directory() -> PathBuf {
    let root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    root.join("ClaudeCodeUsageMonitor")
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

pub fn load_usage_cache() -> Option<UsageCache> {
    read_json(&usage_cache_path())
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
