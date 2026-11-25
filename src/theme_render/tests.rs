use super::*;

use std::collections::BTreeSet;

/// Set to `1` to write the current renders as the new reference images.
const UPDATE_SNAPSHOTS: &str = "CCUM_UPDATE_SNAPSHOTS";
/// A channel that moves by more than this counts as a changed pixel.
const CHANNEL_TOLERANCE: u8 = 48;
/// Share of pixels allowed to change, for small anti-aliasing differences
/// between Windows builds. A changed label or bar is well above this.
const CHANGED_PIXEL_RATIO: f64 = 0.005;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn theme_files(folder: &Path) -> Vec<PathBuf> {
    let mut files = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn reference_images(root: &Path) -> BTreeSet<PathBuf> {
    let mut images = BTreeSet::new();
    let Ok(themes) = std::fs::read_dir(root) else {
        return images;
    };
    for theme in themes {
        let theme = theme.unwrap().path();
        if !theme.is_dir() {
            continue;
        }
        for image in std::fs::read_dir(&theme).unwrap() {
            let image = image.unwrap().path();
            if image
                .extension()
                .is_some_and(|extension| extension == "png")
            {
                images.insert(image.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    images
}

/// Returns why `actual` does not match `reference`, and writes a diff image
/// that shows changed pixels in red over a faded copy of the reference.
fn compare(
    actual: &image::RgbaImage,
    reference: &image::RgbaImage,
    diff_path: &Path,
) -> Option<String> {
    if actual.dimensions() != reference.dimensions() {
        return Some(format!(
            "size changed from {:?} to {:?}",
            reference.dimensions(),
            actual.dimensions()
        ));
    }
    let mut diff = image::RgbaImage::new(actual.width(), actual.height());
    let mut changed = 0_usize;
    for ((actual, reference), out) in actual
        .pixels()
        .zip(reference.pixels())
        .zip(diff.pixels_mut())
    {
        let moved = actual
            .0
            .iter()
            .zip(reference.0)
            .any(|(a, r)| a.abs_diff(r) > CHANNEL_TOLERANCE);
        if moved {
            changed += 1;
            *out = image::Rgba([255, 0, 0, 255]);
        } else {
            let [red, green, blue, alpha] = reference.0;
            let grey = ((u16::from(red) + u16::from(green) + u16::from(blue)) / 3) as u8;
            *out = image::Rgba([grey, grey, grey, alpha / 3]);
        }
    }
    let total = actual.pixels().len().max(1);
    let ratio = changed as f64 / total as f64;
    if ratio <= CHANGED_PIXEL_RATIO {
        return None;
    }
    diff.save(diff_path).unwrap();
    Some(format!(
        "{changed} of {total} pixels changed ({:.2}%); see {}",
        ratio * 100.0,
        diff_path.display()
    ))
}

/// Renders every surface of every theme in `src/themes`, in dark and light
/// mode, with the sample usage fixture, and compares it with
/// `tests/snapshots/themes/<theme file>/<surface>-<appearance>.png`.
///
/// Every render is also written to `target/theme-snapshots`, which CI uploads
/// as an artifact, so a theme pull request can be reviewed as pictures.
#[test]
fn themes_match_reference_snapshots() {
    let root = manifest_dir();
    let themes = root.join("src/themes");
    let references = root.join("tests/snapshots/themes");
    let actual_root = root.join("target/theme-snapshots");
    let update = std::env::var(UPDATE_SNAPSHOTS).is_ok_and(|value| value == "1");
    let usage = parse_usage(SAMPLE_USAGE).unwrap();

    let mut failures = Vec::new();
    let mut expected = BTreeSet::new();
    for theme_path in theme_files(&themes) {
        let stem = theme_path
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let theme = load_theme(&theme_path)
            .unwrap_or_else(|error| panic!("{} does not load: {error}", theme_path.display()));
        for (index, surface) in theme.surfaces.iter().enumerate() {
            for appearance in Appearance::BOTH {
                let settings = RenderSettings {
                    appearance,
                    asset_root: Some(themes.clone()),
                    ..RenderSettings::default()
                };
                let rendered = render_surface(&theme, index, &usage, &settings);
                let relative =
                    Path::new(&stem).join(format!("{}-{}.png", surface.id, appearance.name()));
                for warning in &rendered.warnings {
                    failures.push(format!("{}: warning: {warning}", relative.display()));
                }
                let image = to_rgba_image(&rendered);
                let actual_path = actual_root.join(&relative);
                std::fs::create_dir_all(actual_path.parent().unwrap()).unwrap();
                image.save(&actual_path).unwrap();
                expected.insert(relative.clone());

                let reference_path = references.join(&relative);
                if update {
                    std::fs::create_dir_all(reference_path.parent().unwrap()).unwrap();
                    image.save(&reference_path).unwrap();
                    continue;
                }
                let Ok(reference) = image::open(&reference_path) else {
                    failures.push(format!("{}: no reference image", relative.display()));
                    continue;
                };
                let diff_path = actual_path.with_file_name(format!(
                    "{}-{}-diff.png",
                    surface.id,
                    appearance.name()
                ));
                if let Some(problem) = compare(&image, &reference.to_rgba8(), &diff_path) {
                    failures.push(format!("{}: {problem}", relative.display()));
                }
            }
        }
    }
    for stale in reference_images(&references).difference(&expected) {
        if update {
            std::fs::remove_file(references.join(stale)).unwrap();
        } else {
            failures.push(format!(
                "{}: reference image has no matching theme surface",
                stale.display()
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "theme snapshots differ:\n  {}\n\nRenders are in {}. If the change is intended, \
         run `$env:{UPDATE_SNAPSHOTS}=1; cargo test --locked theme_render` (PowerShell) or \
         `{UPDATE_SNAPSHOTS}=1 cargo test --locked theme_render` (bash), check the new images \
         in tests/snapshots/themes, and commit them.",
        failures.join("\n  "),
        actual_root.display()
    );
}

#[test]
fn renders_are_reproducible() {
    let usage = parse_usage(SAMPLE_USAGE).unwrap();
    let theme = bundled_theme("classic-usage-widget").unwrap().unwrap();
    let settings = RenderSettings::default();
    let first = render_surface(&theme, 0, &usage, &settings);
    let second = render_surface(&theme, 0, &usage, &settings);
    assert_eq!(first.pixels, second.pixels);
    assert!(first.width > 1 && first.height > 1);
}

#[test]
fn appearance_reaches_the_theme() {
    let usage = parse_usage(SAMPLE_USAGE).unwrap();
    let theme = bundled_theme("classic-usage-widget").unwrap().unwrap();
    let dark = render_surface(&theme, 0, &usage, &RenderSettings::default());
    let light = render_surface(
        &theme,
        0,
        &usage,
        &RenderSettings {
            appearance: Appearance::Light,
            ..RenderSettings::default()
        },
    );
    assert_ne!(dark.pixels, light.pixels);
}

#[test]
fn premultiplied_pixels_become_straight_alpha() {
    let rendered = RenderedTheme {
        width: 3,
        height: 1,
        pixels: vec![0x0000_0000, 0xFF10_2030, 0x8040_2000],
        warnings: Vec::new(),
    };
    let image = to_rgba_image(&rendered);
    assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 0]);
    assert_eq!(image.get_pixel(1, 0).0, [0x10, 0x20, 0x30, 0xFF]);
    assert_eq!(image.get_pixel(2, 0).0, [0x80, 0x40, 0x00, 0x80]);
}

fn args(values: &[&str]) -> Vec<String> {
    std::iter::once("claude-code-usage-monitor")
        .chain(values.iter().copied())
        .map(String::from)
        .collect()
}

#[test]
fn cli_is_ignored_without_the_flag() {
    assert_eq!(handle_cli_mode(&args(&["--studio"])), None);
}

#[test]
fn cli_arguments_parse_with_defaults() {
    let request = parse_args(&args(&[
        "--render-theme",
        "classic-usage-widget",
        "--out",
        "out",
    ]))
    .unwrap();
    assert_eq!(request.theme, "classic-usage-widget");
    assert_eq!(request.out, PathBuf::from("out"));
    assert_eq!(request.appearances, Appearance::BOTH.to_vec());
    assert_eq!(request.surface, None);
    assert_eq!(request.now_unix, DEFAULT_NOW_UNIX);

    let request = parse_args(&args(&[
        "--render-theme",
        "theme.json",
        "--out",
        "widget.png",
        "--surface",
        "main",
        "--appearance",
        "light",
        "--fixture",
        "usage.json",
        "--now",
        "1700000000",
        "--utc-offset",
        "-300",
        "--scale",
        "2",
    ]))
    .unwrap();
    assert_eq!(request.surface.as_deref(), Some("main"));
    assert_eq!(request.appearances, vec![Appearance::Light]);
    assert_eq!(request.fixture, Some(PathBuf::from("usage.json")));
    assert_eq!(request.now_unix, 1_700_000_000);
    assert_eq!(request.utc_offset_minutes, -300);
    assert_eq!(request.scale, 2.0);
}

#[test]
fn cli_rejects_bad_arguments() {
    for bad in [
        &["--render-theme"][..],
        &["--render-theme", "classic-usage-widget"],
        &[
            "--render-theme",
            "classic-usage-widget",
            "--out",
            "x",
            "--appearance",
            "dim",
        ],
        &[
            "--render-theme",
            "classic-usage-widget",
            "--out",
            "x",
            "--scale",
            "9",
        ],
        &[
            "--render-theme",
            "classic-usage-widget",
            "--out",
            "x",
            "--utc-offset",
            "2000",
        ],
        &[
            "--render-theme",
            "classic-usage-widget",
            "--out",
            "x",
            "--colour",
        ],
    ] {
        assert!(
            parse_args(&args(bad)).is_err(),
            "{bad:?} should be rejected"
        );
    }
}

#[test]
fn cli_writes_one_png_per_surface_and_appearance() {
    let folder = tempfile::tempdir().unwrap();
    let request = parse_args(&args(&[
        "--render-theme",
        "stacked-rows",
        "--out",
        folder.path().to_str().unwrap(),
    ]))
    .unwrap();
    let written = run(&request).unwrap();
    let theme = bundled_theme("stacked-rows").unwrap().unwrap();
    assert_eq!(written.len(), theme.surfaces.len() * 2);
    let first = image::open(&written[0]).unwrap();
    assert!(first.width() > 1);
    assert!(written[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with(&format!("{}-{}-dark", theme.id, theme.surfaces[0].id)));

    let single = folder.path().join("one.png");
    let request = RenderRequest {
        out: single.clone(),
        ..request
    };
    assert!(run(&request).is_err(), "many images cannot go to one file");
    let request = RenderRequest {
        surface: Some("0".into()),
        appearances: vec![Appearance::Light],
        ..request
    };
    assert_eq!(run(&request).unwrap(), vec![single]);
}

#[test]
fn unknown_theme_lists_the_built_in_ids() {
    let error = load_requested_theme("no-such-theme").unwrap_err();
    assert!(error.contains("classic-usage-widget"), "{error}");
}
