use eframe::egui;

use crate::ui::theme::{accent, menu_hover, menu_text, selected_menu_fill, GITHUB_MARK_GLYPH};

pub(crate) const ITEM_HEIGHT: f32 = 36.0;

pub(crate) fn navigation_item(ui: &mut egui::Ui, selected: bool, title: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ITEM_HEIGHT),
        egui::Sense::click(),
    );
    let fill = if selected {
        selected_menu_fill()
    } else if response.hovered() {
        menu_hover()
    } else {
        egui::Color32::TRANSPARENT
    };
    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    if selected {
        let marker_clip =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 6.0, rect.bottom()));
        ui.painter()
            .with_clip_rect(marker_clip)
            .rect_filled(rect, 4.0, accent());
    }
    ui.painter().text(
        egui::pos2(rect.left() + 18.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(16.0),
        menu_text(),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(crate) fn github_link(ui: &mut egui::Ui, url: &str) -> egui::Response {
    github_link_with(ui, url, |url| {
        crate::native_interop::open_web_url(None, url)
    })
}

fn github_link_with(
    ui: &mut egui::Ui,
    url: &str,
    open: impl FnOnce(&str) -> bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ITEM_HEIGHT, ITEM_HEIGHT), egui::Sense::click());
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        GITHUB_MARK_GLYPH,
        egui::FontId::new(ITEM_HEIGHT, egui::FontFamily::Name("lucide".into())),
        menu_text(),
    );
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(url);
    if response.clicked() && !open(url) {
        crate::diagnose::log("unable to open dashboard GitHub link");
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_click_calls_the_browser_opener_directly() {
        let context = egui::Context::default();
        crate::ui::theme::configure_style(&context, crate::localization::LanguageId::English);
        let url = "https://github.com/CodeZeno/Claude-Code-Usage-Monitor";
        let mut center = egui::Pos2::ZERO;
        let mut opened = Vec::new();
        for pressed in [None, Some(true), Some(false)] {
            let mut input = egui::RawInput::default();
            if let Some(pressed) = pressed {
                input.events = vec![
                    egui::Event::PointerMoved(center),
                    egui::Event::PointerButton {
                        pos: center,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ];
            }
            let mut output = context.run_ui(input, |ui| {
                center = github_link_with(ui, url, |url| {
                    opened.push(url.to_string());
                    true
                })
                .rect
                .center();
            });
            // This route must not depend on egui's optional OpenUrl backend.
            assert!(output.platform_output.commands.is_empty());
            output.textures_delta.clear();
        }
        assert_eq!(opened, [url]);
    }

    #[test]
    fn github_link_matches_navigation_item_height() {
        let context = egui::Context::default();
        crate::ui::theme::configure_style(&context, crate::localization::LanguageId::English);
        let mut heights = [0.0; 2];
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.set_width(134.0);
            heights[0] = navigation_item(ui, false, "Settings").rect.height();
            heights[1] = github_link(ui, "https://github.com/example/repository")
                .rect
                .height();
        });
        output.textures_delta.clear();

        assert_eq!(heights, [ITEM_HEIGHT; 2]);
    }

    /// The GitHub mark font was assembled by hand, so guard the checked-in
    /// artifact against resolving to a glyph that rasterizes to nothing.
    #[test]
    fn github_mark_glyph_rasterizes_from_the_lucide_family() {
        let context = egui::Context::default();
        crate::ui::theme::configure_style(&context, crate::localization::LanguageId::English);
        let font = egui::FontId::new(ITEM_HEIGHT, egui::FontFamily::Name("lucide".into()));

        let mut glyph = None;
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let galley = ui.ctx().fonts_mut(|fonts| {
                fonts.layout_no_wrap(GITHUB_MARK_GLYPH.to_string(), font.clone(), menu_text())
            });
            glyph = galley.rows[0].glyphs.first().copied();
        });
        output.textures_delta.clear();

        let glyph = glyph.expect("the lucide family produced no glyph for the GitHub mark");
        assert!(glyph.advance_width > 0.0, "the mark has no advance width");
        assert!(!glyph.uv_rect.is_nothing(), "the mark rasterized blank");
    }
}
