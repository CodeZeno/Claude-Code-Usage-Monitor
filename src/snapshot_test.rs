//! Reference-image comparison shared by the theme and dashboard snapshot tests.

use std::path::{Path, PathBuf};

/// Set to `1` to write the current renders as the new reference images.
pub(crate) const UPDATE_SNAPSHOTS: &str = "CCUM_UPDATE_SNAPSHOTS";
/// A channel that moves by more than this counts as a changed pixel.
const CHANNEL_TOLERANCE: u8 = 48;
/// Share of a theme surface's pixels allowed to change, for small
/// anti-aliasing differences between Windows builds. A changed label or bar
/// is well above this.
pub(crate) const THEME_CHANGED_PIXEL_RATIO: f64 = 0.005;
/// The same for a whole dashboard page, which is much larger than a surface:
/// about 800 pixels of an 1100 x 760 page. A changed label is above this; the
/// version number in the navigation changing with each release is below it.
pub(crate) const PAGE_CHANGED_PIXEL_RATIO: f64 = 0.001;

pub(crate) fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Whether this run should rewrite the reference images.
pub(crate) fn update_requested() -> bool {
    std::env::var(UPDATE_SNAPSHOTS).is_ok_and(|value| value == "1")
}

/// Where a suite's reference images and renders live.
pub(crate) struct Snapshots<'a> {
    /// Committed reference images.
    pub(crate) references: &'a Path,
    /// Renders written for review, under `target/`.
    pub(crate) actual_root: &'a Path,
    /// Replace the references instead of comparing.
    pub(crate) update: bool,
    /// Share of pixels allowed to change.
    pub(crate) max_changed_ratio: f64,
}

/// Writes `image` to `actual_root/relative` for review, then compares it with
/// `references/relative`, or replaces that reference when `update` is set.
/// Returns why the images differ; a diff image is written next to the render.
pub(crate) fn check(
    image: &image::RgbaImage,
    relative: &Path,
    snapshots: &Snapshots<'_>,
) -> Option<String> {
    let Snapshots {
        references,
        actual_root,
        update,
        max_changed_ratio,
    } = *snapshots;
    let actual_path = actual_root.join(relative);
    std::fs::create_dir_all(actual_path.parent().unwrap()).unwrap();
    image.save(&actual_path).unwrap();

    let reference_path = references.join(relative);
    if update {
        std::fs::create_dir_all(reference_path.parent().unwrap()).unwrap();
        image.save(&reference_path).unwrap();
        return None;
    }
    let Ok(reference) = image::open(&reference_path) else {
        return Some("no reference image".into());
    };
    let stem = actual_path.file_stem().unwrap().to_string_lossy();
    let diff_path = actual_path.with_file_name(format!("{stem}-diff.png"));
    compare(image, &reference.to_rgba8(), &diff_path, max_changed_ratio)
}

/// Returns why `actual` does not match `reference`, and writes a diff image
/// that shows changed pixels in red over a faded copy of the reference.
pub(crate) fn compare(
    actual: &image::RgbaImage,
    reference: &image::RgbaImage,
    diff_path: &Path,
    max_changed_ratio: f64,
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
    if ratio <= max_changed_ratio {
        return None;
    }
    diff.save(diff_path).unwrap();
    Some(format!(
        "{changed} of {total} pixels changed ({:.2}%); see {}",
        ratio * 100.0,
        diff_path.display()
    ))
}
