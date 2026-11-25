#![allow(unsafe_code, reason = "calls Win32 APIs through the windows crate")]
#![allow(
    clippy::undocumented_unsafe_blocks,
    reason = "ratchet: add // SAFETY: comments to the existing unsafe blocks, then remove this allow"
)]

use super::*;

pub(super) fn languages(language: LanguageId) -> Vec<(&'static str, &'static str)> {
    std::iter::once(("system", language.text("System default")))
        .chain(
            LanguageId::ALL
                .into_iter()
                .map(|language| (language.code(), language.native_name())),
        )
        .collect()
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

#[derive(Clone)]
pub(super) struct AvailableTheme {
    pub(super) path: PathBuf,
    pub(super) name: String,
    pub(super) label: String,
    pub(super) read_only: bool,
}

#[derive(PartialEq, Eq)]
struct ThemeFileStamp {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
}

pub(super) struct AvailableThemesCache {
    directory: PathBuf,
    files: Vec<ThemeFileStamp>,
    themes: Arc<Vec<AvailableTheme>>,
    next_check: Option<Instant>,
    needs_reload: bool,
}

impl AvailableThemesCache {
    pub(super) fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            files: Vec::new(),
            themes: Arc::new(Vec::new()),
            next_check: None,
            needs_reload: true,
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.next_check = None;
        self.needs_reload = true;
    }

    fn file_stamps(&self) -> std::io::Result<Vec<ThemeFileStamp>> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                continue;
            }
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                files.push(ThemeFileStamp {
                    path,
                    modified: metadata.modified().ok(),
                    len: metadata.len(),
                });
            }
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(files)
    }

    fn snapshot(&mut self, now: Instant) -> Arc<Vec<AvailableTheme>> {
        // Keep filesystem work out of the repaint hot path. Poll at most once a
        // second; file stamps also detect external edits made without a rename.
        if self.next_check.is_some_and(|next_check| now < next_check) {
            return Arc::clone(&self.themes);
        }
        self.next_check = Some(now + Duration::from_secs(1));
        let Ok(files) = self.file_stamps() else {
            // Keep the last good list on transient directory/metadata errors.
            return Arc::clone(&self.themes);
        };
        if self.needs_reload || files != self.files {
            let mut themes = files
                .iter()
                .filter_map(|file| {
                    let path = file.path.clone();
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
                .collect::<Vec<_>>();
            sort_themes(&mut themes);
            self.themes = Arc::new(themes);
            self.files = files;
            self.needs_reload = false;
        }
        Arc::clone(&self.themes)
    }
}

impl StudioApp {
    pub(super) fn available_themes(&mut self) -> Arc<Vec<AvailableTheme>> {
        let mut themes = self.available_themes.snapshot(Instant::now());
        if let Some(path) = &self.theme_path {
            if !themes.iter().any(|theme| theme.path == *path) {
                // An active theme outside the managed library remains selectable.
                // Keep this fallback separate so unsaved renames and switches are
                // reflected immediately without rebuilding the disk cache.
                let read_only = self.theme.is_builtin();
                let themes = Arc::make_mut(&mut themes);
                themes.push(AvailableTheme {
                    path: path.clone(),
                    name: self.theme.name.clone(),
                    label: if read_only {
                        format!("{} (built-in)", self.theme.name)
                    } else {
                        self.theme.name.clone()
                    },
                    read_only,
                });
                sort_themes(themes);
            }
        }
        themes
    }
}

fn sort_themes(themes: &mut [AvailableTheme]) {
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
}

#[cfg(test)]
mod theme_cache_tests {
    use super::*;

    fn write_theme(path: &Path, name: &str) {
        let theme = blank_theme(name);
        app_settings::write_json_atomic(path, &theme).unwrap();
    }

    #[test]
    fn theme_cache_reuses_snapshot_across_repaints_and_unchanged_polls() {
        let directory = tempfile::tempdir().unwrap();
        write_theme(&directory.path().join("theme.json"), "Cached theme");
        let mut cache = AvailableThemesCache::new(directory.path().into());
        let now = Instant::now();
        let initial = cache.snapshot(now);
        assert_eq!(initial.len(), 1);
        for frame in 1..=120 {
            assert!(Arc::ptr_eq(
                &initial,
                &cache.snapshot(now + Duration::from_millis(frame * 16))
            ));
        }
    }

    #[test]
    fn theme_cache_polls_external_additions_edits_and_deletions() {
        let directory = tempfile::tempdir().unwrap();
        let first_path = directory.path().join("first.json");
        write_theme(&first_path, "First");
        let mut cache = AvailableThemesCache::new(directory.path().into());
        let now = Instant::now();
        let initial = cache.snapshot(now);

        let second_path = directory.path().join("second.JSON");
        write_theme(&second_path, "Second");
        assert!(Arc::ptr_eq(
            &initial,
            &cache.snapshot(now + Duration::from_millis(16))
        ));
        let added = cache.snapshot(now + Duration::from_secs(1));
        assert_eq!(added.len(), 2);

        // Same-sized in-place edit, with a deterministic modified-time change.
        let mut theme = theme_engine::load_theme(&first_path).unwrap();
        theme.name = "Third".into();
        let modified = std::fs::metadata(&first_path).unwrap().modified().unwrap();
        std::fs::write(&first_path, serde_json::to_vec_pretty(&theme).unwrap()).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&first_path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified + Duration::from_secs(2)))
            .unwrap();
        let edited = cache.snapshot(now + Duration::from_secs(2));
        assert_eq!(
            edited
                .iter()
                .map(|theme| theme.name.as_str())
                .collect::<Vec<_>>(),
            ["Second", "Third"]
        );

        std::fs::remove_file(second_path).unwrap();
        let deleted = cache.snapshot(now + Duration::from_secs(3));
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].name, "Third");
    }

    #[test]
    fn theme_cache_invalidation_reloads_even_with_identical_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("theme.json");
        write_theme(&path, "First");
        let mut cache = AvailableThemesCache::new(directory.path().into());
        let now = Instant::now();
        let initial = cache.snapshot(now);
        let metadata = std::fs::metadata(&path).unwrap();
        let mut theme = theme_engine::load_theme(&path).unwrap();
        theme.name = "Other".into();
        std::fs::write(&path, serde_json::to_vec_pretty(&theme).unwrap()).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
            .unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), metadata.len());
        assert!(Arc::ptr_eq(
            &initial,
            &cache.snapshot(now + Duration::from_secs(1))
        ));
        cache.invalidate();
        let refreshed = cache.snapshot(now + Duration::from_secs(1));
        assert_eq!(refreshed[0].name, "Other");
        assert_eq!(initial[0].name, "First");
    }

    #[test]
    fn theme_cache_preserves_ordering_and_skips_invalid_files() {
        let directory = tempfile::tempdir().unwrap();
        write_theme(&directory.path().join("z.json"), "alpha");
        write_theme(&directory.path().join("a.json"), "Alpha");
        app_settings::write_json_atomic(
            &directory.path().join("builtin.json"),
            &ThemeDocument::starter(),
        )
        .unwrap();
        std::fs::write(directory.path().join("invalid.json"), "{}").unwrap();
        write_theme(&directory.path().join("ignored.txt"), "Ignored");
        std::fs::create_dir(directory.path().join("folder.json")).unwrap();
        let mut cache = AvailableThemesCache::new(directory.path().into());
        let themes = cache.snapshot(Instant::now());
        assert_eq!(themes.len(), 3);
        assert!(themes[0].read_only);
        assert_eq!(themes[0].label, format!("{} (built-in)", themes[0].name));
        assert_eq!(themes[1].path.file_name().unwrap(), "a.json");
        assert_eq!(themes[2].path.file_name().unwrap(), "z.json");
    }

    #[test]
    fn theme_cache_detects_library_creation_and_removal() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("themes");
        let mut cache = AvailableThemesCache::new(library.clone());
        let now = Instant::now();
        assert!(cache.snapshot(now).is_empty());
        let path = library.join("theme.json");
        write_theme(&path, "Created");
        assert_eq!(cache.snapshot(now + Duration::from_secs(1)).len(), 1);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(library).unwrap();
        assert!(cache.snapshot(now + Duration::from_secs(2)).is_empty());
    }
}
