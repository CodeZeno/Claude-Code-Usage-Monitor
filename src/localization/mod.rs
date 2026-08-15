// Keep the complete translation catalogue while the GPU dashboard progressively
// adopts the legacy widget strings.
#![allow(dead_code)]

mod dutch;
mod english;
mod french;
mod german;
mod helper_translations;
mod japanese;
mod korean;
mod portuguese_brazil;
mod russian;
mod simplified_chinese;
mod spanish;
mod traditional_chinese;

use windows::core::PWSTR;
use windows::Win32::Globalization::{
    GetUserDefaultLocaleName, GetUserDefaultUILanguage, GetUserPreferredUILanguages,
    LCIDToLocaleName, LOCALE_ALLOW_NEUTRAL_NAMES, MAX_LOCALE_NAME, MUI_LANGUAGE_NAME,
};

use crate::providers::ProviderId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageId {
    English,
    Dutch,
    Spanish,
    French,
    German,
    Japanese,
    Korean,
    TraditionalChinese,
    SimplifiedChinese,
    Russian,
    PortugueseBrazil,
}

impl LanguageId {
    pub const ALL: [LanguageId; 11] = [
        LanguageId::English,
        LanguageId::Dutch,
        LanguageId::Spanish,
        LanguageId::French,
        LanguageId::German,
        LanguageId::Japanese,
        LanguageId::Korean,
        LanguageId::TraditionalChinese,
        LanguageId::SimplifiedChinese,
        LanguageId::Russian,
        LanguageId::PortugueseBrazil,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Dutch => "nl",
            Self::Spanish => "es",
            Self::French => "fr",
            Self::German => "de",
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::TraditionalChinese => "zh-TW",
            Self::SimplifiedChinese => "zh-CN",
            Self::Russian => "ru",
            Self::PortugueseBrazil => "pt-BR",
        }
    }

    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Dutch => "Nederlands",
            Self::Spanish => "Español",
            Self::French => "Français",
            Self::German => "Deutsch",
            Self::Japanese => "日本語",
            Self::Korean => "한국어",
            Self::TraditionalChinese => "繁體中文",
            Self::SimplifiedChinese => "简体中文",
            Self::Russian => "Русский",
            Self::PortugueseBrazil => "Português (Brasil)",
        }
    }

    pub fn strings(self) -> Strings {
        match self {
            Self::English => english::STRINGS,
            Self::Dutch => dutch::STRINGS,
            Self::Spanish => spanish::STRINGS,
            Self::French => french::STRINGS,
            Self::German => german::STRINGS,
            Self::Japanese => japanese::STRINGS,
            Self::Korean => korean::STRINGS,
            Self::TraditionalChinese => traditional_chinese::STRINGS,
            Self::SimplifiedChinese => simplified_chinese::STRINGS,
            Self::Russian => russian::STRINGS,
            Self::PortugueseBrazil => portuguese_brazil::STRINGS,
        }
    }

    /// Translate user-interface text introduced by the dashboard and Theme Studio.
    ///
    /// English text is used as the stable catalogue key. Locale modules may
    /// deliberately fall back to that key while a specialist term is awaiting
    /// a reviewed translation.
    pub fn text(self, english: &'static str) -> &'static str {
        let strings = self.strings();
        match english {
            "Settings" => return strings.settings,
            "Update frequency" => return strings.update_frequency,
            "Start with Windows" => return strings.start_with_windows,
            "Language" => return strings.language,
            "System default" => return strings.system_default,
            "Refresh" => return strings.refresh,
            "Check for updates" => return strings.check_for_updates,
            "Exit" => return strings.exit,
            "Claude Code" => return strings.claude_code_model,
            "Codex" => return strings.codex_model,
            "Antigravity" => return strings.antigravity_model,
            _ => {}
        }
        if let Some(translation) = helper_translations::text(self, english) {
            return translation;
        }
        match self {
            Self::English => english::text(english),
            Self::Dutch => dutch::text(english),
            Self::Spanish => spanish::text(english),
            Self::French => french::text(english),
            Self::German => german::text(english),
            Self::Japanese => japanese::text(english),
            Self::Korean => korean::text(english),
            Self::TraditionalChinese => traditional_chinese::text(english),
            Self::SimplifiedChinese => simplified_chinese::text(english),
            Self::Russian => russian::text(english),
            Self::PortugueseBrazil => portuguese_brazil::text(english),
        }
    }

    pub fn update_via_winget_label(self) -> &'static str {
        match self {
            Self::English => english::UPDATE_VIA_WINGET_LABEL,
            Self::Dutch => dutch::UPDATE_VIA_WINGET_LABEL,
            Self::Spanish => spanish::UPDATE_VIA_WINGET_LABEL,
            Self::French => french::UPDATE_VIA_WINGET_LABEL,
            Self::German => german::UPDATE_VIA_WINGET_LABEL,
            Self::Japanese => japanese::UPDATE_VIA_WINGET_LABEL,
            Self::Korean => korean::UPDATE_VIA_WINGET_LABEL,
            Self::TraditionalChinese => traditional_chinese::UPDATE_VIA_WINGET_LABEL,
            Self::SimplifiedChinese => simplified_chinese::UPDATE_VIA_WINGET_LABEL,
            Self::Russian => russian::UPDATE_VIA_WINGET_LABEL,
            Self::PortugueseBrazil => portuguese_brazil::UPDATE_VIA_WINGET_LABEL,
        }
    }

    pub fn provider_auth_error(self, provider: ProviderId) -> (&'static str, &'static str) {
        let strings = self.strings();
        match provider {
            ProviderId::Claude => (strings.token_expired_title, strings.token_expired_body),
            ProviderId::Codex => (
                strings.codex_token_expired_title,
                strings.codex_token_expired_body,
            ),
            ProviderId::Antigravity => (
                strings.antigravity_token_expired_title,
                strings.antigravity_token_expired_body,
            ),
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        let normalized = code.trim().replace('_', "-").to_ascii_lowercase();
        if normalized.is_empty() || normalized == "system" {
            return None;
        }

        let prefix = normalized.split('-').next().unwrap_or_default();
        match prefix {
            "en" => Some(Self::English),
            "nl" => Some(Self::Dutch),
            "es" => Some(Self::Spanish),
            "fr" => Some(Self::French),
            "de" => Some(Self::German),
            "ja" => Some(Self::Japanese),
            "ko" => Some(Self::Korean),
            "zh" => {
                if normalized.contains("tw")
                    || normalized.contains("hk")
                    || normalized.contains("mo")
                    || normalized.contains("hant")
                {
                    Some(Self::TraditionalChinese)
                } else {
                    Some(Self::SimplifiedChinese)
                }
            }
            "ru" => Some(Self::Russian),
            "pt" => Some(Self::PortugueseBrazil),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Strings {
    pub window_title: &'static str,
    pub refresh: &'static str,
    pub update_frequency: &'static str,
    pub one_minute: &'static str,
    pub five_minutes: &'static str,
    pub fifteen_minutes: &'static str,
    pub one_hour: &'static str,
    pub models: &'static str,
    pub claude_code_model: &'static str,
    pub codex_model: &'static str,
    pub antigravity_model: &'static str,
    pub settings: &'static str,
    pub start_with_windows: &'static str,
    pub language: &'static str,
    pub system_default: &'static str,
    pub check_for_updates: &'static str,
    pub checking_for_updates: &'static str,
    pub updates: &'static str,
    pub update_in_progress: &'static str,
    pub up_to_date: &'static str,
    pub up_to_date_short: &'static str,
    pub update_failed: &'static str,
    pub applying_update: &'static str,
    pub update_to: &'static str,
    pub update_available: &'static str,
    pub update_prompt_now: &'static str,
    pub exit: &'static str,
    pub session_window: &'static str,
    pub weekly_window: &'static str,
    pub now: &'static str,
    pub day_suffix: &'static str,
    pub hour_suffix: &'static str,
    pub minute_suffix: &'static str,
    pub second_suffix: &'static str,
    pub token_expired_title: &'static str,
    pub token_expired_body: &'static str,
    pub codex_token_expired_title: &'static str,
    pub codex_token_expired_body: &'static str,
    pub antigravity_token_expired_title: &'static str,
    pub antigravity_token_expired_body: &'static str,
    pub codex_window_title: &'static str,
    pub antigravity_window_title: &'static str,
}

pub fn resolve_language(language_override: Option<LanguageId>) -> LanguageId {
    language_override.unwrap_or_else(detect_system_language)
}

pub fn detect_system_language() -> LanguageId {
    preferred_ui_languages()
        .into_iter()
        .find_map(|locale| LanguageId::from_code(&locale))
        .or_else(default_ui_locale)
        .or_else(default_locale_name)
        .unwrap_or(LanguageId::English)
}

pub fn update_via_winget(language: LanguageId) -> &'static str {
    language.update_via_winget_label()
}

fn preferred_ui_languages() -> Vec<String> {
    unsafe {
        let mut num_languages = 0u32;
        let mut buffer_len = 0u32;
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut num_languages,
            PWSTR::null(),
            &mut buffer_len,
        )
        .is_err()
            || buffer_len == 0
        {
            return Vec::new();
        }

        let mut buffer = vec![0u16; buffer_len as usize];
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut num_languages,
            PWSTR(buffer.as_mut_ptr()),
            &mut buffer_len,
        )
        .is_err()
        {
            return Vec::new();
        }

        buffer
            .split(|unit| *unit == 0)
            .filter(|part| !part.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
}

fn default_ui_locale() -> Option<LanguageId> {
    unsafe {
        let lang_id = GetUserDefaultUILanguage();
        let mut buffer = [0u16; MAX_LOCALE_NAME as usize];
        let len = LCIDToLocaleName(
            lang_id as u32,
            Some(&mut buffer),
            LOCALE_ALLOW_NEUTRAL_NAMES,
        );
        if len <= 1 {
            return None;
        }
        let locale = String::from_utf16_lossy(&buffer[..(len as usize - 1)]);
        LanguageId::from_code(&locale)
    }
}

fn default_locale_name() -> Option<LanguageId> {
    unsafe {
        let mut buffer = [0u16; MAX_LOCALE_NAME as usize];
        let len = GetUserDefaultLocaleName(&mut buffer);
        if len <= 1 {
            return None;
        }
        let locale = String::from_utf16_lossy(&buffer[..(len as usize - 1)]);
        LanguageId::from_code(&locale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_non_english_locale_translates_the_primary_dashboard_workflow() {
        let keys = [
            "Open Dashboard",
            "Theme Studio",
            "Assets",
            "Every 5 minutes",
            "Enabled",
            "Active theme",
            "Save changes?",
            "Save and continue",
            "Discard changes",
            "Cancel",
            "New theme",
            "Create",
            "Duplicate theme",
            "Create copy",
            "Delete theme?",
            "Delete theme",
            "Delete asset?",
            "Delete",
            "Scene",
            "Add layer",
            "Background",
            "Content type",
            "Apply",
            "Import...",
            "Export...",
            "Import a theme or package",
            "Export theme package",
            "Unable to import theme",
            "Unable to export theme package",
            "Save or discard changes before importing",
            "Add images once, reuse them across themes, or drop image files here to import them.",
            "Theme Studio packages and themes",
            "Theme packages",
            "Theme files",
            "All files",
            "Theme Studio packages",
            "Images",
            "Action helper",
            "Build safe mouse actions that affect layers at runtime.",
            "Choose one action for this context menu item.",
            "Enter actions...",
            "Show dashboard",
            "Toggle dashboard",
            "Show context menu",
            "Set property",
            "Reset property",
            "Increase value",
            "Decrease value",
            "Run layer actions",
            "Show widget",
            "Check for updates",
            "Name the new theme",
            "Theme name",
            "Name the editable copy",
            "Are you sure you want to delete {name}?",
            "Delete context menu?",
            "Delete context menu",
            "Are you sure you want to delete {name} from the asset library and all themes using it?",
        ];

        for language in LanguageId::ALL
            .into_iter()
            .filter(|language| *language != LanguageId::English)
        {
            for key in keys {
                assert_ne!(
                    language.text(key),
                    key,
                    "{} is missing the essential translation for {key:?}",
                    language.code()
                );
            }
        }
    }

    #[test]
    fn untranslated_specialist_text_falls_back_to_english() {
        assert_eq!(
            LanguageId::Japanese.text("A future specialist label"),
            "A future specialist label"
        );
    }
}
