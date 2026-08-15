use super::*;

pub(super) fn languages(language: LanguageId) -> [(&'static str, &'static str); 12] {
    [
        ("system", language.text("System default")),
        ("en", LanguageId::English.native_name()),
        ("nl", LanguageId::Dutch.native_name()),
        ("es", LanguageId::Spanish.native_name()),
        ("fr", LanguageId::French.native_name()),
        ("de", LanguageId::German.native_name()),
        ("ja", LanguageId::Japanese.native_name()),
        ("ko", LanguageId::Korean.native_name()),
        ("zh-TW", LanguageId::TraditionalChinese.native_name()),
        ("zh-CN", LanguageId::SimplifiedChinese.native_name()),
        ("ru", LanguageId::Russian.native_name()),
        ("pt-BR", LanguageId::PortugueseBrazil.native_name()),
    ]
}
pub(super) fn language_name(language: LanguageId, code: &str) -> &str {
    languages(language)
        .into_iter()
        .find(|(value, _)| *value == code)
        .map(|(_, name)| name)
        .unwrap_or_else(|| language.text("System default"))
}
pub(super) fn unique_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "{}-{}-{}",
        prefix,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

pub(super) fn blank_theme(name: &str) -> ThemeDocument {
    let mut theme = ThemeDocument::starter();
    theme.id = unique_id("theme");
    theme.name = name.to_string();
    theme.surfaces = vec![SceneObject::root(
        unique_id("surface"),
        "Surface",
        320.0.into(),
        100.0.into(),
        Placement::default(),
    )];
    theme.prepare_runtime();
    theme
}

pub(super) fn choose_file(owner: isize, title: &str, filter: &str) -> Option<PathBuf> {
    let mut file = [0u16; 32768];
    let filter: Vec<u16> = filter.encode_utf16().collect();
    let title: Vec<u16> = format!("{title}\0").encode_utf16().collect();
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(owner as *mut _),
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    if unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
        let len = file
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(file.len());
        Some(Path::new(&String::from_utf16_lossy(&file[..len])).to_path_buf())
    } else {
        None
    }
}

pub(super) fn choose_save_file(
    owner: isize,
    title: &str,
    filter: &str,
    default_name: &str,
    default_extension: &str,
) -> Option<PathBuf> {
    let mut file = [0u16; 32768];
    let default_name: Vec<u16> = default_name.encode_utf16().collect();
    let copy_len = default_name.len().min(file.len().saturating_sub(1));
    file[..copy_len].copy_from_slice(&default_name[..copy_len]);
    let filter: Vec<u16> = filter.encode_utf16().collect();
    let title: Vec<u16> = format!("{title}\0").encode_utf16().collect();
    let default_extension: Vec<u16> = format!("{default_extension}\0").encode_utf16().collect();
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(owner as *mut _),
        lpstrFilter: PCWSTR(filter.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        lpstrDefExt: PCWSTR(default_extension.as_ptr()),
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    if unsafe { GetSaveFileNameW(&mut dialog) }.as_bool() {
        let len = file
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(file.len());
        Some(Path::new(&String::from_utf16_lossy(&file[..len])).to_path_buf())
    } else {
        None
    }
}

pub(super) fn safe_file_name(value: &str, fallback: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for character in value.trim().chars() {
        if character.is_alphanumeric() || matches!(character, '-' | '_') {
            result.push(character);
            separator = false;
        } else if !separator && !result.is_empty() {
            result.push('-');
            separator = true;
        }
    }
    let result = result.trim_matches('-');
    if result.is_empty() {
        fallback.into()
    } else {
        result.into()
    }
}

pub(super) struct AvailableTheme {
    pub(super) path: PathBuf,
    pub(super) name: String,
    pub(super) label: String,
    pub(super) read_only: bool,
}

pub(super) fn available_themes(
    current_path: Option<&Path>,
    current_theme: &ThemeDocument,
) -> Vec<AvailableTheme> {
    let _ = theme_engine::ensure_starter_theme();
    let mut themes: Vec<AvailableTheme> = std::fs::read_dir(theme_engine::themes_directory())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .filter_map(|path| {
            theme_engine::load_theme(&path).ok().map(|theme| {
                let read_only = theme.is_builtin();
                let label = if read_only {
                    format!("{} (built-in)", theme.name)
                } else {
                    theme.name.clone()
                };
                AvailableTheme {
                    path,
                    name: theme.name,
                    label,
                    read_only,
                }
            })
        })
        .collect();
    if let Some(path) = current_path {
        if !themes.iter().any(|theme| theme.path == path) {
            let read_only = current_theme.is_builtin();
            themes.push(AvailableTheme {
                path: path.to_path_buf(),
                name: current_theme.name.clone(),
                label: if read_only {
                    format!("{} (built-in)", current_theme.name)
                } else {
                    current_theme.name.clone()
                },
                read_only,
            });
        }
    }
    themes.sort_by(|left, right| {
        right
            .read_only
            .cmp(&left.read_only)
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
            .then_with(|| left.path.cmp(&right.path))
    });
    themes
}
