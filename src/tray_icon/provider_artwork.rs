//! Rasterize the dashboard's existing provider marks once, without a GPU or
//! another window. These images are for the Windows Settings cache only;
//! the live notification-area artwork remains controlled by the theme.
use std::sync::OnceLock;

use eframe::egui;

use crate::providers::ProviderId;
use crate::theme_engine::RenderedTheme;
use crate::ui::theme::{provider_mark_glyph, BRAND_MARK_FONT_BYTES};

const SIZE: usize = 64;

pub(crate) fn provider_snapshot(provider: ProviderId) -> Option<&'static [u8]> {
    static SNAPSHOTS: OnceLock<Vec<Option<Vec<u8>>>> = OnceLock::new();
    SNAPSHOTS.get_or_init(|| {
        ProviderId::ALL
            .into_iter()
            .map(|provider| {
                let artwork = provider_artwork(provider)?;
                // A neutral tile keeps the mark legible regardless of Settings' theme,
                // which can differ from the taskbar theme. Source pixels are premultiplied.
                let image = image::RgbaImage::from_fn(artwork.width, artwork.height, |x, y| {
                    let pixel = artwork.pixels[(y * artwork.width + x) as usize];
                    let alpha = pixel >> 24;
                    let rgb = [16, 8, 0]
                        .map(|shift| (((pixel >> shift) & 255) + 32 * (255 - alpha) / 255) as u8);
                    image::Rgba([rgb[0], rgb[1], rgb[2], 255])
                });
                let mut output = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(image)
                    .write_to(&mut output, image::ImageFormat::Png)
                    .ok()?;
                Some(output.into_inner())
            })
            .collect()
    })[provider as usize]
        .as_deref()
}

fn provider_artwork(provider: ProviderId) -> Option<RenderedTheme> {
    static MARKS: OnceLock<Vec<Option<RenderedTheme>>> = OnceLock::new();
    let mut image = MARKS.get_or_init(rasterize_marks)[provider as usize].clone()?;
    // Use light marks on the Settings snapshot's neutral background.
    let rgb = if provider == ProviderId::Claude {
        [217u32, 119, 87]
    } else {
        [245, 245, 245]
    };
    for pixel in &mut image.pixels {
        let alpha = *pixel >> 24;
        *pixel = (alpha << 24)
            | ((rgb[0] * alpha / 255) << 16)
            | ((rgb[1] * alpha / 255) << 8)
            | (rgb[2] * alpha / 255);
    }
    Some(image)
}

fn rasterize_marks() -> Vec<Option<RenderedTheme>> {
    let context = egui::Context::default();
    let mut fonts = egui::FontDefinitions::empty();
    fonts.font_data.insert(
        "provider-marks".into(),
        egui::FontData::from_static(BRAND_MARK_FONT_BYTES).into(),
    );
    fonts.families.insert(
        egui::FontFamily::Proportional,
        vec!["provider-marks".into()],
    );
    context.set_fonts(fonts);
    ProviderId::ALL
        .into_iter()
        .map(|provider| {
            let mut image = None;
            let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                ui.ctx().fonts_mut(|fonts| {
                    let galley = fonts.layout_no_wrap(
                        provider_mark_glyph(provider).to_string(),
                        egui::FontId::proportional(54.0),
                        egui::Color32::WHITE,
                    );
                    let Some(glyph) = galley.rows.first().and_then(|row| row.glyphs.first()) else {
                        return;
                    };
                    let uv = glyph.uv_rect;
                    let width = usize::from(uv.max[0] - uv.min[0]);
                    let height = usize::from(uv.max[1] - uv.min[1]);
                    if width == 0 || height == 0 || width > SIZE || height > SIZE {
                        return;
                    }
                    let atlas = fonts.image();
                    let mut pixels = vec![0; SIZE * SIZE];
                    let left = (SIZE - width) / 2;
                    let top = (SIZE - height) / 2;
                    for y in 0..height {
                        for x in 0..width {
                            let alpha = u32::from(
                                atlas[(usize::from(uv.min[0]) + x, usize::from(uv.min[1]) + y)].a(),
                            );
                            pixels[(top + y) * SIZE + left + x] = alpha * 0x01010101;
                        }
                    }
                    image = Some(RenderedTheme {
                        width: SIZE as u32,
                        height: SIZE as u32,
                        pixels,
                        warnings: Vec::new(),
                    });
                });
            });
            // We consume the CPU atlas directly; no renderer needs these GPU
            // upload instructions, and egui requires they be handled explicitly.
            output.textures_delta.clear();
            image
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_snapshots_are_distinct_valid_pngs_with_readable_backgrounds() {
        let mut snapshots = Vec::new();
        for provider in ProviderId::ALL {
            let bytes = provider_snapshot(provider).unwrap();
            let decoded = image::load_from_memory(bytes).unwrap().to_rgba8();
            assert_eq!(decoded.dimensions(), (64, 64));
            assert!(decoded.pixels().all(|pixel| pixel[3] == 255));
            assert!(!snapshots.contains(&bytes));
            snapshots.push(bytes);
        }
    }

    #[test]
    fn every_provider_has_distinct_visible_artwork() {
        let mut masks = Vec::new();
        for provider in ProviderId::ALL {
            let dark = provider_artwork(provider).expect("missing provider mark");
            assert_eq!((dark.width, dark.height), (64, 64));
            assert!(dark.pixels.iter().filter(|pixel| **pixel >> 24 > 0).count() > 50);
            assert_eq!(
                dark.pixels[0], 0,
                "the icon background must stay transparent"
            );
            let mask: Vec<_> = dark.pixels.iter().map(|pixel| pixel >> 24).collect();
            assert!(!masks.contains(&mask), "two providers have the same mark");
            masks.push(mask);
            for pixel in &dark.pixels {
                let alpha = pixel >> 24;
                assert!([pixel & 255, (pixel >> 8) & 255, (pixel >> 16) & 255]
                    .into_iter()
                    .all(|channel| channel <= alpha));
            }
        }
    }
}
