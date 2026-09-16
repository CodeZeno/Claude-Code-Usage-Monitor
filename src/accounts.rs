//! Named credential sources. Settings contain paths and labels, never tokens.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::providers::ProviderId;

/// Stable non-secret key for source paths and file metadata (not token contents).
pub fn fingerprint(value: &str) -> String {
    let hash = value.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

pub fn file_signature(path: &Path) -> String {
    let metadata = std::fs::metadata(path).ok();
    fingerprint(&format!(
        "{}|{:?}|{:?}",
        source_key(path),
        metadata.as_ref().map(|m| m.len()),
        metadata.and_then(|m| m.modified().ok())
    ))
}

pub fn source_key(path: &Path) -> String {
    match std::fs::canonicalize(path) {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().to_lowercase(),
    }
}

pub fn default_credential_path(provider: ProviderId) -> Option<PathBuf> {
    let directory = environment_directory(provider).or_else(|| {
        dirs::home_dir().map(|home| {
            home.join(if provider == ProviderId::Claude {
                ".claude"
            } else {
                ".codex"
            })
        })
    })?;
    Some(directory.join(if provider == ProviderId::Claude {
        ".credentials.json"
    } else {
        "auth.json"
    }))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountProfile {
    pub id: String,
    pub name: String,
    pub config_dir: String,
    pub credentials_path: String,
    pub enabled: bool,
}

impl Default for AccountProfile {
    fn default() -> Self {
        Self {
            id: "default".into(),
            name: "Default".into(),
            config_dir: String::new(),
            credentials_path: String::new(),
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderAccounts {
    pub profiles: Vec<AccountProfile>,
    pub selected: String,
    /// Retain retired IDs so existing theme bindings never target a new account.
    pub used_ids: std::collections::BTreeSet<String>,
}

impl Default for ProviderAccounts {
    fn default() -> Self {
        Self {
            profiles: vec![AccountProfile::default()],
            selected: "default".into(),
            used_ids: ["default".into()].into(),
        }
    }
}

impl ProviderAccounts {
    pub fn selected(&self) -> Option<&AccountProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.enabled && profile.id == self.selected)
            .or_else(|| self.profiles.iter().find(|profile| profile.enabled))
    }

    pub fn add(&mut self) {
        self.normalize();
        let (id, number) = self.allocate_id();
        self.profiles.push(AccountProfile {
            id,
            name: format!("Account {number}"),
            enabled: false,
            ..Default::default()
        });
    }

    fn allocate_id(&mut self) -> (String, usize) {
        let mut number = 1;
        while self.used_ids.contains(&format!("account_{number}")) {
            number += 1;
        }
        let id = format!("account_{number}");
        self.used_ids.insert(id.clone());
        (id, number)
    }

    pub fn normalize(&mut self) {
        let selected_index = self
            .profiles
            .iter()
            .position(|profile| profile.id == self.selected)
            .or_else(|| {
                self.profiles
                    .iter()
                    .position(|profile| profile.id.eq_ignore_ascii_case(&self.selected))
            });
        self.used_ids = std::mem::take(&mut self.used_ids)
            .into_iter()
            .map(|id| id.to_ascii_lowercase())
            .collect();
        // Reserve every existing ID before allocating replacements. An earlier
        // invalid profile must not steal a later valid profile's binding.
        self.used_ids.extend(
            self.profiles
                .iter()
                .map(|profile| profile.id.to_ascii_lowercase()),
        );
        let mut seen = std::collections::HashSet::new();
        for index in 0..self.profiles.len() {
            let profile = &self.profiles[index];
            if profile.id.is_empty()
                || !profile
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || !seen.insert(profile.id.to_ascii_lowercase())
            {
                let (id, _) = self.allocate_id();
                seen.insert(id.to_ascii_lowercase());
                self.profiles[index].id = id;
            }
            let profile = &mut self.profiles[index];
            if profile.name.trim().is_empty() {
                profile.name = profile.id.clone();
            }
        }
        if let Some(index) = selected_index {
            self.selected = self.profiles[index].id.clone();
        }
        self.selected = self.selected().map(|p| p.id.clone()).unwrap_or_default();
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountSettings {
    pub claude: ProviderAccounts,
    pub codex: ProviderAccounts,
}

impl AccountSettings {
    pub fn get(&self, provider: ProviderId) -> Option<&ProviderAccounts> {
        match provider {
            ProviderId::Claude => Some(&self.claude),
            ProviderId::Codex => Some(&self.codex),
            _ => None,
        }
    }
}

pub fn environment_directory(provider: ProviderId) -> Option<PathBuf> {
    let variable = match provider {
        ProviderId::Claude => "CLAUDE_CONFIG_DIR",
        ProviderId::Codex => "CODEX_HOME",
        _ => return None,
    };
    environment_directory_value(std::env::var_os(variable))
}

fn environment_directory_value(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).and_then(|value| {
        let path = Path::new(&value);
        expand_path(path).or_else(|| std::env::current_dir().ok().map(|cwd| cwd.join(path)))
    })
}

pub fn expand_path(path: &Path) -> Option<PathBuf> {
    let text = path.to_string_lossy();
    if text == "~" {
        return dirs::home_dir();
    }
    if let Some(tail) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        return Some(dirs::home_dir()?.join(tail));
    }
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        // Settings must behave the same when launched from a terminal or at login.
        None
    }
}

impl AccountProfile {
    pub fn same_source(&self, other: &Self) -> bool {
        self.id == other.id
            && self.config_dir == other.config_dir
            && self.credentials_path == other.credentials_path
    }

    pub fn credential_path(&self, provider: ProviderId) -> Result<Option<PathBuf>, String> {
        if !self.credentials_path.trim().is_empty() {
            return expand_path(Path::new(self.credentials_path.trim()))
                .map(Some)
                .ok_or_else(|| "Use an absolute credentials file path or ~/path".into());
        }
        let directory = if !self.config_dir.trim().is_empty() {
            expand_path(Path::new(self.config_dir.trim()))
                .ok_or("Use an absolute config directory or ~/path")?
        } else if self.id == "default" {
            return Ok(None);
        } else {
            return Err(
                "Set a config directory or credentials file before enabling this account".into(),
            );
        };
        Ok(Some(directory.join(match provider {
            ProviderId::Claude => ".credentials.json",
            ProviderId::Codex => "auth.json",
            _ => return Err("This provider does not support account profiles".into()),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleted_ids_are_not_reused_after_saving_and_restarting() {
        let mut accounts = ProviderAccounts::default();
        accounts.add();
        let removed_id = accounts.profiles.pop().unwrap().id;
        let json = serde_json::to_string(&accounts).unwrap();
        let mut reloaded: ProviderAccounts = serde_json::from_str(&json).unwrap();
        reloaded.add();
        assert_ne!(reloaded.profiles.last().unwrap().id, removed_id);
        assert!(reloaded.used_ids.contains(&removed_id));
    }

    #[test]
    fn legacy_ids_are_reserved_and_case_collisions_keep_the_selected_account() {
        let mut accounts: ProviderAccounts = serde_json::from_str(
            r#"{
            "profiles": [
                {"id":"Work","name":"First","config_dir":"C:/first"},
                {"id":"work","name":"Second","config_dir":"C:/second"},
                {"id":"account_1","name":"Existing"}
            ], "selected":"work"
        }"#,
        )
        .unwrap();
        accounts.normalize();
        assert_eq!(accounts.profiles[0].id, "Work");
        assert_eq!(accounts.profiles[2].id, "account_1");
        assert_ne!(accounts.profiles[1].id.to_ascii_lowercase(), "work");
        assert_eq!(accounts.selected().unwrap().name, "Second");
        let normalized = accounts.clone();
        accounts.normalize();
        assert_eq!(accounts, normalized);
        accounts.profiles.clear();
        accounts.add();
        assert!(!normalized
            .profiles
            .iter()
            .any(|old| old.id.eq_ignore_ascii_case(&accounts.profiles[0].id)));
    }

    #[test]
    fn environment_overrides_handle_empty_tilde_and_spaces_without_global_mutation() {
        assert_eq!(environment_directory_value(None), None);
        assert_eq!(environment_directory_value(Some("".into())), None);
        assert_eq!(
            environment_directory_value(Some("C:\\Users\\Two Words\\.claude-work".into())),
            Some(PathBuf::from("C:\\Users\\Two Words\\.claude-work"))
        );
        assert_eq!(
            environment_directory_value(Some("~/.codex-work".into())),
            dirs::home_dir().map(|home| home.join(".codex-work"))
        );
    }

    #[test]
    fn explicit_file_overrides_directory_and_invalid_profiles_never_use_default() {
        let profile = AccountProfile {
            id: "work".into(),
            config_dir: "C:\\work".into(),
            credentials_path: "C:\\private\\work.json".into(),
            ..Default::default()
        };
        assert_eq!(
            profile.credential_path(ProviderId::Claude).unwrap(),
            Some(PathBuf::from("C:\\private\\work.json"))
        );
        let empty = AccountProfile {
            id: "work".into(),
            ..Default::default()
        };
        assert!(empty.credential_path(ProviderId::Codex).is_err());
        assert!(expand_path(Path::new("relative/path")).is_none());
        assert_eq!(
            AccountProfile::default()
                .credential_path(ProviderId::Claude)
                .unwrap(),
            None
        );
    }

    #[test]
    fn disabled_or_removed_selection_uses_first_enabled_account() {
        let mut accounts = ProviderAccounts::default();
        accounts.add();
        accounts.profiles[1].enabled = true;
        accounts.profiles[0].enabled = false;
        accounts.normalize();
        assert_eq!(accounts.selected, "account_1");
        accounts.profiles.clear();
        accounts.normalize();
        assert!(accounts.selected().is_none());
    }
}
