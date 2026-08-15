use eframe::egui;
use lucide_icons::Icon as LucideIcon;

use crate::ui::theme::{
    accent, asset_card_border, asset_card_selected, asset_card_surface, asset_preview_surface,
    muted,
};

pub(crate) fn asset_card(
    ui: &mut egui::Ui,
    name: &str,
    details: &str,
    hover_text: &str,
    texture: Option<&egui::TextureHandle>,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(176.0, 158.0), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let fill = if selected {
        asset_card_selected()
    } else if response.hovered() {
        visuals.weak_bg_fill
    } else {
        asset_card_surface()
    };
    ui.painter().rect(
        rect,
        6.0,
        fill,
        egui::Stroke::new(
            if selected { 2.0 } else { 1.0 },
            if selected {
                accent()
            } else {
                asset_card_border()
            },
        ),
        egui::StrokeKind::Inside,
    );

    let image_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(8.0, 8.0),
        egui::pos2(rect.right() - 8.0, rect.top() + 106.0),
    );
    ui.painter()
        .rect_filled(image_rect, 3.0, asset_preview_surface());
    if let Some(texture) = texture {
        let source_size = texture.size_vec2();
        let scale = (image_rect.width() / source_size.x)
            .min(image_rect.height() / source_size.y)
            .min(1.0);
        let destination = egui::Rect::from_center_size(image_rect.center(), source_size * scale);
        ui.painter().image(
            texture.id(),
            destination,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        ui.painter().text(
            image_rect.center(),
            egui::Align2::CENTER_CENTER,
            LucideIcon::Image.unicode().to_string(),
            egui::FontId::new(28.0, egui::FontFamily::Name("lucide".into())),
            muted(),
        );
    }

    let name_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 9.0, image_rect.bottom() + 7.0),
        egui::pos2(rect.right() - 9.0, image_rect.bottom() + 26.0),
    );
    let galley = egui::WidgetText::from(egui::RichText::new(name).strong()).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        name_rect.width(),
        egui::TextStyle::Body,
    );
    ui.painter().galley(
        egui::Align2::LEFT_CENTER
            .align_size_within_rect(galley.size(), name_rect)
            .min,
        galley,
        ui.visuals().text_color(),
    );
    ui.painter().text(
        egui::pos2(rect.left() + 9.0, rect.bottom() - 9.0),
        egui::Align2::LEFT_BOTTOM,
        details,
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
        muted(),
    );
    response.on_hover_text(hover_text)
}
