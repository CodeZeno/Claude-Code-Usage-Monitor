use serde::{Deserialize, Serialize};

/// Stable identity shared by settings, polling, themes, and context menus.
///
/// Adding a provider starts here: register its descriptor, implement its poller,
/// and connect any provider-specific settings persistence that older versions
/// need to understand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ProviderId {
    Claude = 0,
    Codex = 1,
    Antigravity = 2,
    OpenCode = 3,
    Cursor = 4,
    Grok = 5,
    Copilot = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    /// Stable key used by theme expressions and context-menu documents.
    pub key: &'static str,
    /// Stable key used by the persisted usage cache.
    pub cache_key: &'static str,
    /// English catalogue key resolved through the localization layer.
    pub display_name: &'static str,
    /// English catalogue key for the provider setting description.
    pub settings_description: &'static str,
    /// Stable Win32 command id used by native provider menu items.
    pub native_menu_command_id: u16,
    pub default_enabled: bool,
    /// Where the poller finds the user's sign-in, most preferred first.
    pub credentials: &'static [CredentialSource],
}

/// A place a provider reads the user's sign-in from.
///
/// Providers use only credentials that the provider's own CLI, desktop app or
/// IDE has already stored on this machine. The monitor never asks for an API
/// key, token or cookie, and never stores or writes one. `UserSupplied` exists
/// only for the providers that predate this rule; the provider contract test
/// rejects it anywhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    /// A file the provider's own client writes when the user signs in.
    SignInFile(&'static str),
    /// A Windows Credential Manager entry the provider's own client writes.
    CredentialManager(&'static str),
    /// An environment variable that the provider's own tools already read.
    ToolEnvironment(&'static str),
    /// The provider's CLI, run so that it refreshes its own stored sign-in.
    CliRefresh(&'static str),
    /// A credential the user copies in by hand. Not allowed for new providers.
    UserSupplied(&'static str),
}

impl CredentialSource {
    pub const fn name(self) -> &'static str {
        match self {
            Self::SignInFile(name)
            | Self::CredentialManager(name)
            | Self::ToolEnvironment(name)
            | Self::CliRefresh(name)
            | Self::UserSupplied(name) => name,
        }
    }

    pub fn describe(self) -> String {
        let kind = match self {
            Self::SignInFile(_) => "sign-in file",
            Self::CredentialManager(_) => "Windows Credential Manager",
            Self::ToolEnvironment(_) => "environment variable",
            Self::CliRefresh(_) => "CLI refresh",
            Self::UserSupplied(_) => "user-supplied",
        };
        format!("{kind}: {}", self.name())
    }
}

pub const PROVIDER_DESCRIPTORS: [ProviderDescriptor; 7] = [
    ProviderDescriptor {
        id: ProviderId::Claude,
        key: "claude",
        cache_key: "claude_code",
        display_name: "Claude Code",
        settings_description: "Collect usage from Anthropic",
        native_menu_command_id: 60,
        default_enabled: true,
        credentials: &[
            CredentialSource::SignInFile("~/.claude/.credentials.json (Claude Code)"),
            CredentialSource::SignInFile("Claude desktop app token cache (config.json)"),
            CredentialSource::SignInFile("~/.claude/.credentials.json in WSL (Claude Code)"),
            CredentialSource::CliRefresh("claude"),
        ],
    },
    ProviderDescriptor {
        id: ProviderId::Codex,
        key: "codex",
        cache_key: "codex",
        display_name: "Codex",
        settings_description: "Collect usage from OpenAI",
        native_menu_command_id: 61,
        default_enabled: false,
        credentials: &[
            CredentialSource::SignInFile("$CODEX_HOME/auth.json or ~/.codex/auth.json (Codex CLI)"),
            CredentialSource::CliRefresh("codex"),
        ],
    },
    ProviderDescriptor {
        id: ProviderId::Antigravity,
        key: "antigravity",
        cache_key: "antigravity",
        display_name: "Antigravity",
        settings_description: "Collect usage from Google",
        native_menu_command_id: 62,
        default_enabled: false,
        credentials: &[CredentialSource::CredentialManager(
            "gemini:antigravity (Antigravity)",
        )],
    },
    ProviderDescriptor {
        id: ProviderId::OpenCode,
        key: "opencode",
        cache_key: "opencode",
        display_name: "OpenCode",
        settings_description: "Collect usage from OpenCode Go",
        native_menu_command_id: 63,
        default_enabled: false,
        credentials: &[
            CredentialSource::UserSupplied("OPENCODE_GO_AUTH_COOKIE"),
            CredentialSource::UserSupplied("OPENCODE_GO_WORKSPACE_ID"),
            CredentialSource::UserSupplied(
                "%APPDATA%/opencode-go/config.json or OPENCODE_GO_CONFIG_FILE",
            ),
        ],
    },
    ProviderDescriptor {
        id: ProviderId::Cursor,
        key: "cursor",
        cache_key: "cursor",
        display_name: "Cursor",
        settings_description: "Collect usage from Cursor",
        native_menu_command_id: 64,
        default_enabled: false,
        credentials: &[
            CredentialSource::SignInFile(
                "%APPDATA%/Cursor/User/globalStorage/state.vscdb (Cursor)",
            ),
            CredentialSource::UserSupplied("CURSOR_SESSION_TOKEN"),
        ],
    },
    ProviderDescriptor {
        id: ProviderId::Grok,
        key: "grok",
        cache_key: "grok",
        display_name: "Grok",
        settings_description: "Collect usage from xAI",
        native_menu_command_id: 65,
        default_enabled: false,
        credentials: &[
            CredentialSource::SignInFile("$GROK_HOME/auth.json or ~/.grok/auth.json (Grok CLI)"),
            CredentialSource::CliRefresh("grok"),
        ],
    },
    ProviderDescriptor {
        id: ProviderId::Copilot,
        key: "copilot",
        cache_key: "copilot",
        display_name: "Copilot",
        settings_description: "Collect usage from GitHub",
        native_menu_command_id: 66,
        default_enabled: false,
        credentials: &[
            CredentialSource::CredentialManager("Copilot CLI login"),
            CredentialSource::CredentialManager("GitHub CLI login (gh)"),
            CredentialSource::ToolEnvironment("COPILOT_GITHUB_TOKEN"),
            CredentialSource::ToolEnvironment("GH_TOKEN"),
            CredentialSource::ToolEnvironment("GITHUB_TOKEN"),
        ],
    },
];

impl ProviderId {
    pub const ALL: [Self; 7] = [
        Self::Claude,
        Self::Codex,
        Self::Antigravity,
        Self::OpenCode,
        Self::Cursor,
        Self::Grok,
        Self::Copilot,
    ];

    pub const fn descriptor(self) -> &'static ProviderDescriptor {
        &PROVIDER_DESCRIPTORS[self as usize]
    }

    pub fn from_key(key: &str) -> Option<Self> {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.key == key)
            .map(|descriptor| descriptor.id)
    }

    pub fn from_cache_key(key: &str) -> Option<Self> {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.cache_key == key)
            .map(|descriptor| descriptor.id)
    }

    pub fn from_native_menu_command_id(command_id: u16) -> Option<Self> {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.native_menu_command_id == command_id)
            .map(|descriptor| descriptor.id)
    }
}

impl Default for ProviderId {
    fn default() -> Self {
        PROVIDER_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.default_enabled)
            .map(|descriptor| descriptor.id)
            .unwrap_or(Self::Claude)
    }
}

/// Compact, copyable selection passed between settings, UI, and poll workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderSet(u64);

impl ProviderSet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub fn from_enabled(enabled: impl IntoIterator<Item = ProviderId>) -> Self {
        let mut providers = Self::empty();
        for provider in enabled {
            providers.set(provider, true);
        }
        providers
    }

    pub const fn contains(self, provider: ProviderId) -> bool {
        self.0 & provider.bit() != 0
    }

    pub fn set(&mut self, provider: ProviderId, enabled: bool) {
        if enabled {
            self.0 |= provider.bit();
        } else {
            self.0 &= !provider.bit();
        }
    }

    /// Toggle a provider while preserving the application invariant that at
    /// least one provider remains enabled.
    pub fn toggle(&mut self, provider: ProviderId) -> bool {
        let enabled = self.contains(provider);
        if enabled && self.len() == 1 {
            return false;
        }
        self.set(provider, !enabled);
        true
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    pub fn first(self) -> Option<ProviderId> {
        self.iter().next()
    }

    pub fn iter(self) -> impl Iterator<Item = ProviderId> {
        ProviderId::ALL
            .into_iter()
            .filter(move |provider| self.contains(*provider))
    }
}

impl Default for ProviderSet {
    fn default() -> Self {
        Self::from_enabled(
            PROVIDER_DESCRIPTORS
                .iter()
                .filter(|descriptor| descriptor.default_enabled)
                .map(|descriptor| descriptor.id),
        )
    }
}

impl ProviderId {
    const fn bit(self) -> u64 {
        1 << self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_provider_set_comes_from_descriptors() {
        assert_eq!(
            ProviderSet::default(),
            ProviderSet::from_enabled([ProviderId::Claude])
        );
    }

    #[test]
    fn provider_set_refuses_to_toggle_off_its_last_provider() {
        let mut providers = ProviderSet::from_enabled([ProviderId::Codex]);
        assert!(!providers.toggle(ProviderId::Codex));
        assert!(providers.contains(ProviderId::Codex));
    }

    #[test]
    fn provider_keys_round_trip_through_the_registry() {
        for descriptor in PROVIDER_DESCRIPTORS {
            assert_eq!(ProviderId::from_key(descriptor.key), Some(descriptor.id));
            assert_eq!(
                ProviderId::from_cache_key(descriptor.cache_key),
                Some(descriptor.id)
            );
            assert_eq!(
                ProviderId::from_native_menu_command_id(descriptor.native_menu_command_id),
                Some(descriptor.id)
            );
        }
    }
}
