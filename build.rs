use epaint_default_fonts::UBUNTU_LIGHT;
use heck::ToKebabCase;
use lucide_icons::{Icon as LucideIcon, LUCIDE_FONT_BYTES};
use oxifont_subset::{subset_font_with_options, SubsetOptions};
use proc_macro2::{TokenStream, TokenTree};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use winres::{VersionInfo, WindowsResource};

fn main() {
    build_lucide_subset();
    build_ui_fallback_subset();

    let version = env!("CARGO_PKG_VERSION");

    // Embed the icon and richer PE version metadata into the executable.
    let mut res = WindowsResource::new();
    let numeric_version = pack_version(version);

    res.set_icon("src/icons/icon.ico")
        // Declaring modern Windows compatibility enables WS_EX_LAYERED child
        // windows, which the Windows 11 raised desktop requires.
        // Keep the manifest in a file so winres asks the resource compiler to
        // embed it verbatim. `set_manifest` wraps every line in spaces, which
        // puts whitespace before the XML declaration and breaks parsers such
        // as wingetcreate's Vestris.ResourceLib.
        .set_manifest_file("src/app.manifest")
        .set("FileVersion", version)
        .set("ProductVersion", version)
        .set_version_info(VersionInfo::FILEVERSION, numeric_version)
        .set_version_info(VersionInfo::PRODUCTVERSION, numeric_version);

    res.compile().expect("Failed to compile Windows resources");

    println!("cargo:rerun-if-changed=src/app.manifest");
}

fn build_ui_fallback_subset() {
    let codepoints = (' '..='~')
        .chain(['\u{2014}', '\u{2018}', '\u{2019}'])
        .collect::<BTreeSet<_>>();
    let options = SubsetOptions::default()
        .strip_hints(true)
        .retain_layout_tables(false)
        .retain_names(false)
        .drop_variations(true);
    let (subset, stats) = subset_font_with_options(UBUNTU_LIGHT, &codepoints, &options)
        .expect("Failed to build the UI fallback font subset");

    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR was not set"))
        .join("ui-fallback.ttf");
    write_if_changed(&output, &subset)
        .unwrap_or_else(|error| panic!("Failed to write {}: {error}", output.display()));

    println!(
        "cargo:warning=UI fallback font subset: {} characters, {} -> {} bytes",
        codepoints.len(),
        stats.original_size,
        stats.subset_size
    );
}

fn build_lucide_subset() {
    println!("cargo:rerun-if-changed=src");

    let mut source_files = Vec::new();
    collect_rust_sources(Path::new("src"), &mut source_files)
        .expect("Failed to enumerate Rust source files for Lucide icons");

    let mut variants = BTreeSet::new();
    for path in source_files {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("Failed to read {}: {error}", path.display()));
        let tokens = source
            .parse::<TokenStream>()
            .unwrap_or_else(|error| panic!("Failed to tokenize {}: {error}", path.display()));
        collect_lucide_variants(tokens, &mut variants);
    }

    assert!(
        !variants.is_empty(),
        "No LucideIcon::Variant references were found under src"
    );

    let codepoints = variants
        .iter()
        .map(|variant| {
            let icon_name = variant.to_kebab_case();
            LucideIcon::try_from(icon_name.as_str())
                .unwrap_or_else(|error| panic!("Unknown Lucide icon variant {variant}: {error}"))
                .unicode()
        })
        .collect::<BTreeSet<_>>();

    let options = SubsetOptions::default()
        .strip_hints(true)
        .retain_layout_tables(false)
        .retain_names(false)
        .drop_variations(true);
    let (subset, stats) = subset_font_with_options(LUCIDE_FONT_BYTES, &codepoints, &options)
        .expect("Failed to build the Lucide icon font subset");

    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR was not set"))
        .join("lucide-subset.ttf");
    write_if_changed(&output, &subset)
        .unwrap_or_else(|error| panic!("Failed to write {}: {error}", output.display()));

    println!(
        "cargo:warning=Lucide font subset: {} icons, {} -> {} bytes",
        codepoints.len(),
        stats.original_size,
        stats.subset_size
    );
}

fn collect_rust_sources(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

fn write_if_changed(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if fs::read(path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    fs::write(path, contents)
}

fn collect_lucide_variants(tokens: TokenStream, variants: &mut BTreeSet<String>) {
    let tokens = tokens.into_iter().collect::<Vec<_>>();
    for window in tokens.windows(4) {
        if let (
            TokenTree::Ident(alias),
            TokenTree::Punct(first_colon),
            TokenTree::Punct(second_colon),
            TokenTree::Ident(variant),
        ) = (&window[0], &window[1], &window[2], &window[3])
        {
            if alias == "LucideIcon"
                && first_colon.as_char() == ':'
                && second_colon.as_char() == ':'
            {
                variants.insert(variant.to_string());
            }
        }
    }

    for token in tokens {
        if let TokenTree::Group(group) = token {
            collect_lucide_variants(group.stream(), variants);
        }
    }
}

fn pack_version(version: &str) -> u64 {
    let core = version.split('-').next().unwrap_or(version);
    let mut parts = core.split('.').map(|part| part.parse::<u64>().unwrap_or(0));

    let major = parts.next().unwrap_or(0).min(u16::MAX as u64);
    let minor = parts.next().unwrap_or(0).min(u16::MAX as u64);
    let patch = parts.next().unwrap_or(0).min(u16::MAX as u64);

    (major << 48) | (minor << 32) | (patch << 16)
}
