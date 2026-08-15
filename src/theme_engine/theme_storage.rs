use super::*;

pub fn themes_directory() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata)
        .join("ClaudeCodeUsageMonitor")
        .join("themes")
}

#[derive(Clone, Debug)]
pub struct ManagedAsset {
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

pub fn assets_directory() -> PathBuf {
    themes_directory().join("assets")
}

pub(super) fn supported_asset_extension(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
    )
    .then_some(extension)
}

pub(super) fn managed_asset_relative_path(file_name: &str) -> String {
    format!("assets/{file_name}")
}

pub fn list_assets() -> Result<Vec<ManagedAsset>, String> {
    let directory = assets_directory();
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut assets = std::fs::read_dir(&directory)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let absolute_path = entry.path();
            supported_asset_extension(&absolute_path)?;
            let (width, height) = image::image_dimensions(&absolute_path).ok()?;
            let metadata = entry.metadata().ok()?;
            let name = absolute_path.file_name()?.to_str()?.to_string();
            Some(ManagedAsset {
                relative_path: managed_asset_relative_path(&name),
                absolute_path,
                name,
                width,
                height,
                bytes: metadata.len(),
            })
        })
        .collect::<Vec<_>>();
    assets.sort_by_key(|asset| asset.name.to_ascii_lowercase());
    Ok(assets)
}

pub fn import_asset(source: &Path) -> Result<ManagedAsset, String> {
    if !source.is_file() {
        return Err("Choose an image file to add to the asset library".into());
    }
    let source_bytes = std::fs::read(source).map_err(|error| error.to_string())?;
    let source_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "The selected image needs a valid file name".to_string())?;
    import_asset_bytes(source_name, &source_bytes)
}

pub fn import_asset_bytes(file_name: &str, source_bytes: &[u8]) -> Result<ManagedAsset, String> {
    let source_path = Path::new(file_name);
    if source_path.file_name().and_then(|name| name.to_str()) != Some(file_name) {
        return Err("The selected image needs a valid file name".into());
    }
    let extension = supported_asset_extension(source_path)
        .ok_or_else(|| "Supported images are PNG, JPEG, GIF, BMP, and WebP".to_string())?;
    let image = image::load_from_memory(source_bytes)
        .map_err(|error| format!("Unable to read the selected image: {error}"))?;
    let (width, height) = (image.width(), image.height());
    let directory = assets_directory();
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let source_stem = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(safe_file_stem)
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "image".into());

    let mut suffix = 1usize;
    let (file_name, target) = loop {
        let file_name = if suffix == 1 {
            format!("{source_stem}.{extension}")
        } else {
            format!("{source_stem}-{suffix}.{extension}")
        };
        let target = directory.join(&file_name);
        if !target.exists() {
            break (file_name, target);
        }
        if std::fs::read(&target).ok().as_deref() == Some(source_bytes) {
            let bytes = std::fs::metadata(&target)
                .map(|metadata| metadata.len())
                .unwrap_or(source_bytes.len() as u64);
            return Ok(ManagedAsset {
                relative_path: managed_asset_relative_path(&file_name),
                absolute_path: target,
                name: file_name,
                width,
                height,
                bytes,
            });
        }
        suffix += 1;
    };

    let temporary = directory.join(format!(".{file_name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, source_bytes).map_err(|error| error.to_string())?;
    if let Err(error) = std::fs::rename(&temporary, &target) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(ManagedAsset {
        relative_path: managed_asset_relative_path(&file_name),
        absolute_path: target,
        name: file_name,
        width,
        height,
        bytes: source_bytes.len() as u64,
    })
}

pub(super) fn managed_asset_file_name(path: &str) -> Option<&str> {
    let normalized = path.strip_prefix("assets/")?;
    (!normalized.is_empty()
        && !normalized.contains('/')
        && !normalized.contains('\\')
        && Path::new(normalized)
            .file_name()
            .and_then(|name| name.to_str())
            == Some(normalized))
    .then_some(normalized)
}

pub fn delete_asset(path: &str) -> Result<(), String> {
    let file_name = managed_asset_file_name(path)
        .ok_or_else(|| "The selected file is not a managed asset".to_string())?;
    let target = assets_directory().join(file_name);
    remove_asset_references_from_saved_themes(path)?;
    std::fs::remove_file(target).map_err(|error| error.to_string())
}

pub(super) fn remove_asset_references_from_saved_themes(path: &str) -> Result<(), String> {
    let mut updates = Vec::new();
    for entry in std::fs::read_dir(themes_directory())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
    {
        let theme_path = entry.path();
        if !theme_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            continue;
        }
        let source = std::fs::read_to_string(&theme_path).map_err(|error| error.to_string())?;
        let mut theme: ThemeDocument =
            serde_json::from_str(&source).map_err(|error| error.to_string())?;
        if remove_asset_references(&mut theme, path) > 0 {
            theme.prepare_runtime();
            let errors = theme.validate();
            if !errors.is_empty() {
                return Err(errors.join("\n"));
            }
            updates.push((theme_path, theme));
        }
    }
    for (theme_path, theme) in updates {
        crate::app_settings::write_json_atomic(&theme_path, &theme)?;
    }
    Ok(())
}

pub fn remove_asset_references(theme: &mut ThemeDocument, path: &str) -> usize {
    let mut removed = 0;
    for surface in &mut theme.surfaces {
        if matches!(
            &surface.background,
            LayerBackground::Image { path: image, .. } if image == path
        ) {
            surface.background = LayerBackground::None;
            removed += 1;
        }
        for object in &mut surface.children {
            if matches!(
                &object.background,
                LayerBackground::Image { path: image, .. } if image == path
            ) {
                object.background = LayerBackground::None;
                removed += 1;
            }
        }
    }
    removed
}

pub fn theme_asset_usage(theme: &ThemeDocument, path: &str) -> usize {
    theme
        .surfaces
        .iter()
        .map(|surface| {
            usize::from(matches!(
                &surface.background,
                LayerBackground::Image { path: image, .. } if image == path
            )) + surface
                .children
                .iter()
                .filter(|object| {
                    matches!(
                        &object.background,
                        LayerBackground::Image { path: image, .. } if image == path
                    )
                })
                .count()
        })
        .sum()
}

pub fn save_theme(theme: &ThemeDocument) -> Result<PathBuf, String> {
    let mut theme = theme.clone();
    theme.prepare_runtime();
    if theme.is_builtin() {
        return Err(format!(
            "{} is read-only; duplicate it to make changes",
            theme.name
        ));
    }
    let errors = theme.validate();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let directory = themes_directory();
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let path = directory.join(format!("{}.json", safe_file_stem(&theme.id)));
    crate::app_settings::write_json_atomic(&path, &theme)?;
    Ok(path)
}

pub fn is_managed_theme_path(path: &Path) -> bool {
    let directory = themes_directory();
    path.parent().is_some_and(|parent| {
        parent == directory
            || std::fs::canonicalize(parent)
                .ok()
                .zip(std::fs::canonicalize(&directory).ok())
                .is_some_and(|(parent, directory)| parent == directory)
    })
}

pub fn delete_theme(path: &Path) -> Result<(), String> {
    if !is_managed_theme_path(path) {
        return Err("Only themes saved in Theme Studio can be deleted".into());
    }
    let directory = std::fs::canonicalize(themes_directory()).map_err(|error| error.to_string())?;
    let path = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    if path.parent() != Some(directory.as_path()) {
        return Err("Theme path is outside the managed themes folder".into());
    }
    let theme = load_theme(&path)?;
    if theme.is_builtin() {
        return Err(format!("{} is built-in and cannot be deleted", theme.name));
    }
    std::fs::remove_file(path).map_err(|error| error.to_string())
}

pub fn load_theme(path: &Path) -> Result<ThemeDocument, String> {
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut theme: ThemeDocument = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    theme.prepare_runtime();
    let errors = theme.validate();
    if errors.is_empty() {
        Ok(theme)
    } else {
        Err(errors.join("\n"))
    }
}

pub fn ensure_starter_theme() -> Result<PathBuf, String> {
    let directory = themes_directory();
    for (expected_id, source) in BUILTIN_THEME_SOURCES {
        let mut theme: ThemeDocument = serde_json::from_str(source)
            .map_err(|error| format!("Built-in theme '{expected_id}' is invalid JSON: {error}"))?;
        if theme.id != *expected_id {
            return Err(format!(
                "Built-in theme id '{}' does not match '{expected_id}'",
                theme.id
            ));
        }
        theme.prepare_runtime();
        let errors = theme.validate();
        if !errors.is_empty() {
            return Err(format!(
                "Built-in theme '{}' is invalid:\n{}",
                theme.name,
                errors.join("\n")
            ));
        }
        let path = directory.join(format!("{expected_id}.json"));
        let canonical = serde_json::to_vec_pretty(&theme).map_err(|error| error.to_string())?;
        let current = std::fs::read(&path).ok();
        if current.as_deref() != Some(canonical.as_slice()) {
            crate::app_settings::write_json_atomic(&path, &theme)?;
        }
    }
    ensure_bundled_editable_themes(&directory, &assets_directory())?;
    for removed_id in REMOVED_BUILTIN_THEME_IDS {
        let path = directory.join(format!("{removed_id}.json"));
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(directory.join(format!("{CLASSIC_THEME_ID}.json")))
}

pub(super) fn ensure_bundled_editable_themes(
    directory: &Path,
    asset_directory: &Path,
) -> Result<(), String> {
    let install_marker = directory.join(BUNDLED_EDITABLE_INSTALL_MARKER);
    if install_marker.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(asset_directory).map_err(|error| error.to_string())?;
    for (file_name, source) in BUNDLED_THEME_ASSETS {
        let path = asset_directory.join(file_name);
        if !path.exists() {
            std::fs::write(path, source).map_err(|error| error.to_string())?;
        }
    }

    for (expected_id, source) in BUNDLED_EDITABLE_THEME_SOURCES {
        let mut bundled: ThemeDocument = serde_json::from_str(source).map_err(|error| {
            format!("Bundled editable theme '{expected_id}' is invalid JSON: {error}")
        })?;
        if bundled.id != *expected_id {
            return Err(format!(
                "Bundled editable theme id '{}' does not match '{expected_id}'",
                bundled.id
            ));
        }
        bundled.prepare_runtime();
        let errors = bundled.validate();
        if !errors.is_empty() {
            return Err(format!(
                "Bundled editable theme '{}' is invalid:\n{}",
                bundled.name,
                errors.join("\n")
            ));
        }

        let path = directory.join(format!("{expected_id}.json"));
        if !path.exists() {
            crate::app_settings::write_json_atomic(&path, &bundled)?;
            continue;
        }

        // Upgrade the original locally-created Minecraft theme without
        // replacing any other user edits. Once changed, later menu choices are
        // preserved because only the old prototype reference is recognized.
        let Ok(mut installed) = load_theme(&path) else {
            continue;
        };
        if migrate_minecraft_context_menu(&mut installed) {
            crate::app_settings::write_json_atomic(&path, &installed)?;
        }
    }
    std::fs::write(install_marker, b"1").map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) fn migrate_minecraft_context_menu(theme: &mut ThemeDocument) -> bool {
    if theme.id != MINECRAFT_THEME_ID {
        return false;
    }
    const LEGACY_ACTION: &str = "show_context_menu(\"classic-test\")";
    const DASHBOARD_V2_ACTION: &str = "show_context_menu(\"dashboard-v2\")";
    fn migrate_object(object: &mut SceneObject) -> bool {
        let mut changed = false;
        if let Some(events) = object.mouse_events.as_mut() {
            if events.right_click.trim() == LEGACY_ACTION {
                events.right_click = DASHBOARD_V2_ACTION.into();
                changed = true;
            }
        }
        for child in &mut object.children {
            changed |= migrate_object(child);
        }
        changed
    }
    theme
        .surfaces
        .iter_mut()
        .fold(false, |changed, surface| migrate_object(surface) || changed)
}
