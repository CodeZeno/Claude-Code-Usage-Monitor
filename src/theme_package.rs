//! Portable Theme Studio packages containing a theme, its menu, and used assets.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::context_menu::{self, ContextMenuDocument};
use crate::theme_engine::{self, LayerBackground, ThemeDocument};

const THEME_ENTRY: &str = "theme.json";
const CONTEXT_MENU_ENTRY: &str = "context-menu.json";
const ASSET_PREFIX: &str = "assets/";
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_024;

#[derive(Clone, Debug)]
pub struct ImportedTheme {
    pub theme: ThemeDocument,
    pub theme_path: PathBuf,
    pub context_menu: Option<(ContextMenuDocument, PathBuf)>,
    pub imported_assets: usize,
}

#[derive(Debug)]
struct DecodedPackage {
    theme: ThemeDocument,
    context_menu: Option<ContextMenuDocument>,
    assets: HashMap<String, Vec<u8>>,
}

pub fn is_theme_package(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

pub fn is_theme_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

pub fn export_package(
    destination: &Path,
    theme: &ThemeDocument,
    context_menu: &ContextMenuDocument,
) -> Result<usize, String> {
    validate_theme(theme)?;
    validate_context_menu(context_menu)?;

    let asset_paths = theme_asset_paths(theme);
    let theme_bytes = serde_json::to_vec_pretty(theme).map_err(|error| error.to_string())?;
    let context_menu_bytes =
        serde_json::to_vec_pretty(context_menu).map_err(|error| error.to_string())?;
    if theme_bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err("theme.json exceeds the 64 MB package limit".into());
    }
    if context_menu_bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err("context-menu.json exceeds the 64 MB package limit".into());
    }
    let mut package_bytes = theme_bytes.len() as u64 + context_menu_bytes.len() as u64;
    let mut assets = Vec::with_capacity(asset_paths.len());
    for relative_path in &asset_paths {
        let file_name = managed_asset_name(relative_path)?;
        let source = theme_engine::assets_directory().join(file_name);
        let bytes = std::fs::read(&source).map_err(|error| {
            format!(
                "Theme asset '{}' could not be read: {error}",
                source.display()
            )
        })?;
        if bytes.len() as u64 > MAX_ENTRY_BYTES {
            return Err(format!(
                "Theme asset '{}' exceeds the 64 MB package limit",
                source.display()
            ));
        }
        package_bytes = package_bytes.saturating_add(bytes.len() as u64);
        if package_bytes > MAX_PACKAGE_BYTES {
            return Err("Theme package expands beyond the 256 MB limit".into());
        }
        assets.push((relative_path, bytes));
    }

    let file = File::create(destination).map_err(|error| {
        format!(
            "Unable to create theme package '{}': {error}",
            destination.display()
        )
    })?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);

    archive
        .start_file(THEME_ENTRY, options)
        .map_err(|error| error.to_string())?;
    archive
        .write_all(&theme_bytes)
        .map_err(|error| error.to_string())?;

    archive
        .start_file(CONTEXT_MENU_ENTRY, options)
        .map_err(|error| error.to_string())?;
    archive
        .write_all(&context_menu_bytes)
        .map_err(|error| error.to_string())?;

    for (relative_path, bytes) in assets {
        archive
            .start_file(relative_path, options)
            .map_err(|error| error.to_string())?;
        archive
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
    }

    let mut file = archive.finish().map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    Ok(asset_paths.len())
}

pub fn import_path(source: &Path) -> Result<ImportedTheme, String> {
    if is_theme_package(source) {
        let file = File::open(source)
            .map_err(|error| format!("Unable to open '{}': {error}", source.display()))?;
        import_decoded(decode_package(file)?)
    } else if is_theme_file(source) {
        let bytes = std::fs::read(source)
            .map_err(|error| format!("Unable to open '{}': {error}", source.display()))?;
        let theme = decode_theme(&bytes)?;
        let (theme, theme_path) = persist_theme(theme)?;
        Ok(ImportedTheme {
            theme,
            theme_path,
            context_menu: None,
            imported_assets: 0,
        })
    } else {
        Err("Choose a Theme Studio package (.zip) or theme file (.json)".into())
    }
}

fn import_decoded(mut package: DecodedPackage) -> Result<ImportedTheme, String> {
    let used_assets = theme_asset_paths(&package.theme);
    for relative_path in &used_assets {
        if !package.assets.contains_key(relative_path) {
            return Err(format!(
                "Theme package is missing the used asset '{relative_path}'"
            ));
        }
    }

    let mut imported_assets = 0;
    for (relative_path, bytes) in package.assets {
        let file_name = managed_asset_name(&relative_path)?;
        let imported = theme_engine::import_asset_bytes(file_name, &bytes)?;
        if imported.relative_path != relative_path {
            replace_theme_asset_path(&mut package.theme, &relative_path, &imported.relative_path);
        }
        imported_assets += 1;
    }

    let imported_menu = package.context_menu.map(persist_context_menu).transpose()?;
    let (theme, theme_path) = persist_theme(package.theme)?;
    Ok(ImportedTheme {
        theme,
        theme_path,
        context_menu: imported_menu,
        imported_assets,
    })
}

fn decode_package<R: Read + Seek>(reader: R) -> Result<DecodedPackage, String> {
    let mut archive = ZipArchive::new(reader).map_err(|error| error.to_string())?;
    if archive.len() > MAX_ENTRIES {
        return Err(format!(
            "Theme package contains too many entries (maximum {MAX_ENTRIES})"
        ));
    }

    let mut names = HashSet::new();
    let mut total_bytes = 0u64;
    let mut theme = None;
    let mut context_menu = None;
    let mut assets = HashMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let name = safe_entry_name(entry.name())?;
        if !names.insert(name.clone()) {
            return Err(format!("Theme package contains duplicate entry '{name}'"));
        }
        if entry.size() > MAX_ENTRY_BYTES {
            return Err(format!("Theme package entry '{name}' exceeds 64 MB"));
        }
        total_bytes = total_bytes.saturating_add(entry.size());
        if total_bytes > MAX_PACKAGE_BYTES {
            return Err("Theme package expands beyond the 256 MB limit".into());
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .by_ref()
            .take(MAX_ENTRY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_ENTRY_BYTES {
            return Err(format!("Theme package entry '{name}' exceeds 64 MB"));
        }
        match name.as_str() {
            THEME_ENTRY => theme = Some(decode_theme(&bytes)?),
            CONTEXT_MENU_ENTRY => context_menu = Some(decode_context_menu(&bytes)?),
            _ if name.starts_with(ASSET_PREFIX) => {
                managed_asset_name(&name)?;
                assets.insert(name, bytes);
            }
            _ => return Err(format!("Unsupported theme package entry '{name}'")),
        }
    }
    let theme = theme.ok_or("Theme package does not contain theme.json")?;
    Ok(DecodedPackage {
        theme,
        context_menu,
        assets,
    })
}

fn decode_theme(bytes: &[u8]) -> Result<ThemeDocument, String> {
    let mut theme: ThemeDocument =
        serde_json::from_slice(bytes).map_err(|error| format!("Invalid theme JSON: {error}"))?;
    theme.prepare_runtime();
    validate_theme(&theme)?;
    Ok(theme)
}

fn decode_context_menu(bytes: &[u8]) -> Result<ContextMenuDocument, String> {
    let menu: ContextMenuDocument = serde_json::from_slice(bytes)
        .map_err(|error| format!("Invalid context menu JSON: {error}"))?;
    validate_context_menu(&menu)?;
    Ok(menu)
}

fn validate_theme(theme: &ThemeDocument) -> Result<(), String> {
    let errors = theme.validate();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn validate_context_menu(menu: &ContextMenuDocument) -> Result<(), String> {
    let errors = menu.validate();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn persist_theme(mut theme: ThemeDocument) -> Result<(ThemeDocument, PathBuf), String> {
    if theme.is_builtin() {
        let path = theme_engine::ensure_starter_theme()?;
        let existing = theme_engine::load_theme(&path)?;
        if !themes_match(&existing, &theme) {
            return Err("An imported package cannot replace a built-in theme".into());
        }
        return Ok((existing, path));
    }
    theme.prepare_runtime();
    let path = theme_engine::save_theme(&theme)?;
    Ok((theme, path))
}

fn themes_match(left: &ThemeDocument, right: &ThemeDocument) -> bool {
    serde_json::to_value(left).ok() == serde_json::to_value(right).ok()
}

fn persist_context_menu(
    menu: ContextMenuDocument,
) -> Result<(ContextMenuDocument, PathBuf), String> {
    if menu.is_builtin() {
        context_menu::ensure_builtin_context_menus()?;
        let existing = context_menu::resolve_context_menu(Some(&menu.id))?;
        if existing != menu {
            return Err("An imported package cannot replace a built-in context menu".into());
        }
        let path = context_menu::context_menus_directory().join(format!("{}.json", menu.id));
        return Ok((existing, path));
    }
    let path = context_menu::save_context_menu(&menu)?;
    Ok((menu, path))
}

fn safe_entry_name(name: &str) -> Result<String, String> {
    if name.contains('\\') || name.starts_with('/') || name.contains(':') {
        return Err(format!("Unsafe theme package path '{name}'"));
    }
    let path = Path::new(name);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("Unsafe theme package path '{name}'"));
    }
    Ok(name.to_string())
}

fn managed_asset_name(relative_path: &str) -> Result<&str, String> {
    let file_name = relative_path
        .strip_prefix(ASSET_PREFIX)
        .ok_or_else(|| format!("Asset path '{relative_path}' must start with assets/"))?;
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || Path::new(file_name)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(file_name)
    {
        return Err(format!("Unsafe asset path '{relative_path}'"));
    }
    Ok(file_name)
}

fn theme_asset_paths(theme: &ThemeDocument) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for surface in &theme.surfaces {
        collect_background_asset(&surface.background, &mut paths);
        for object in &surface.children {
            collect_background_asset(&object.background, &mut paths);
        }
    }
    paths
}

fn collect_background_asset(background: &LayerBackground, paths: &mut BTreeSet<String>) {
    if let LayerBackground::Image { path, .. } = background {
        paths.insert(path.clone());
    }
}

fn replace_theme_asset_path(theme: &mut ThemeDocument, old: &str, new: &str) {
    for surface in &mut theme.surfaces {
        replace_background_asset(&mut surface.background, old, new);
        for object in &mut surface.children {
            replace_background_asset(&mut object.background, old, new);
        }
    }
}

fn replace_background_asset(background: &mut LayerBackground, old: &str, new: &str) {
    if let LayerBackground::Image { path, .. } = background {
        if path == old {
            *path = new.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn package_decoder_accepts_theme_menu_and_assets() {
        let theme = ThemeDocument::starter();
        let menu = context_menu::classic_context_menu();
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut bytes);
            let options = SimpleFileOptions::default();
            writer.start_file(THEME_ENTRY, options).unwrap();
            writer
                .write_all(&serde_json::to_vec(&theme).unwrap())
                .unwrap();
            writer.start_file(CONTEXT_MENU_ENTRY, options).unwrap();
            writer
                .write_all(&serde_json::to_vec(&menu).unwrap())
                .unwrap();
            writer.finish().unwrap();
        }
        bytes.set_position(0);
        let decoded = decode_package(bytes).unwrap();
        assert_eq!(decoded.theme.id, theme.id);
        assert_eq!(decoded.context_menu.unwrap().id, menu.id);
    }

    #[test]
    fn package_decoder_rejects_unsafe_paths() {
        assert!(safe_entry_name("../theme.json").is_err());
        assert!(safe_entry_name("assets\\logo.png").is_err());
        assert!(safe_entry_name("C:/theme.json").is_err());
        assert_eq!(
            safe_entry_name("assets/logo.png").unwrap(),
            "assets/logo.png"
        );
    }
}
