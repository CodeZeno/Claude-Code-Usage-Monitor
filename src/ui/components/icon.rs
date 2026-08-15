use eframe::egui;
use lucide_icons::Icon as LucideIcon;

use crate::ui::theme::{accent, accent_hover_border};
use crate::ui::tokens::{CONTROL_CORNER_RADIUS, CONTROL_HEIGHT};

pub(crate) fn icon_text(icon: LucideIcon, size: f32) -> egui::RichText {
    egui::RichText::new(icon.unicode().to_string())
        .family(egui::FontFamily::Name("lucide".into()))
        .size(size)
}

pub(crate) fn labeled_icon_button<'a>(icon: LucideIcon, label: &'a str) -> egui::Button<'a> {
    egui::Button::new((icon_text(icon, 16.0), label)).min_size(egui::vec2(0.0, CONTROL_HEIGHT))
}

pub(crate) fn icon_only_button(icon: LucideIcon) -> egui::Button<'static> {
    egui::Button::new(icon_text(icon, 16.0)).min_size(egui::vec2(CONTROL_HEIGHT, CONTROL_HEIGHT))
}

pub(crate) fn paint_centered_icon(
    ui: &egui::Ui,
    rect: egui::Rect,
    icon: LucideIcon,
    size: f32,
    color: egui::Color32,
) {
    ui.painter().text(
        rect.center() + egui::vec2(0.0, 0.5),
        egui::Align2::CENTER_CENTER,
        icon.unicode().to_string(),
        egui::FontId::new(size, egui::FontFamily::Name("lucide".into())),
        color,
    );
}

pub(crate) fn icon_button(ui: &mut egui::Ui, icon: LucideIcon, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(CONTROL_HEIGHT, CONTROL_HEIGHT),
        egui::Sense::click(),
    );
    let visuals = ui.style().interact(&response);
    let fill = if selected {
        accent()
    } else {
        visuals.weak_bg_fill
    };
    let stroke = if selected && response.hovered() {
        egui::Stroke::new(1.0, accent_hover_border())
    } else {
        visuals.bg_stroke
    };
    ui.painter().rect(
        rect.expand(visuals.expansion),
        egui::CornerRadius::same(CONTROL_CORNER_RADIUS),
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );
    paint_centered_icon(
        ui,
        rect,
        icon,
        16.0,
        if selected {
            egui::Color32::WHITE
        } else {
            visuals.text_color()
        },
    );
    response
}

pub(crate) fn leading_icon_button(
    ui: &mut egui::Ui,
    icon: LucideIcon,
    label: &str,
    width: f32,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, CONTROL_HEIGHT), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    ui.painter().rect(
        rect.expand(visuals.expansion),
        egui::CornerRadius::same(CONTROL_CORNER_RADIUS),
        visuals.weak_bg_fill,
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );

    let padding = ui.spacing().button_padding.x;
    let icon_width = 16.0;
    let icon_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + padding, rect.top()),
        egui::pos2(rect.left() + padding + icon_width, rect.bottom()),
    );
    let icon_galley = egui::WidgetText::from(icon_text(icon, icon_width)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        icon_width,
        egui::TextStyle::Button,
    );
    let icon_position =
        egui::Align2::CENTER_CENTER.align_size_within_rect(icon_galley.size(), icon_rect);
    ui.painter()
        .galley(icon_position.min, icon_galley, visuals.text_color());

    let text_rect = egui::Rect::from_min_max(
        egui::pos2(icon_rect.right() + ui.spacing().icon_spacing, rect.top()),
        egui::pos2(rect.right() - padding, rect.bottom()),
    );
    let galley = egui::WidgetText::from(label).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        text_rect.width().max(1.0),
        egui::TextStyle::Button,
    );
    let text_position = egui::Align2::LEFT_CENTER.align_size_within_rect(galley.size(), text_rect);
    ui.painter()
        .galley(text_position.min, galley, visuals.text_color());
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::components::dropdown::Dropdown;
    use crate::ui::components::text_field::singleline;

    #[test]
    fn standard_controls_share_one_height() {
        let context = egui::Context::default();
        crate::ui::theme::configure_style(&context, crate::localization::LanguageId::English);
        let mut heights = [0.0; 6];
        let _ = context.run_ui(egui::RawInput::default(), |ui| {
            ui.horizontal(|ui| {
                heights[0] = Dropdown::from_id_salt("probe")
                    .selected_text("Theme")
                    .show_ui(ui, |_| {})
                    .response
                    .rect
                    .height();
                heights[1] = ui
                    .add(labeled_icon_button(LucideIcon::Save, "Apply"))
                    .rect
                    .height();
                heights[2] = ui.add(icon_only_button(LucideIcon::X)).rect.height();
                heights[3] = ui.button("Plain").rect.height();
                let mut text = String::new();
                heights[4] = ui.add(singleline(&mut text)).rect.height();
                heights[5] = ui
                    .add(egui::Button::new((
                        "Discard",
                        icon_text(LucideIcon::X, 16.0),
                    )))
                    .rect
                    .height();
            });
        });
        assert_eq!(heights, [CONTROL_HEIGHT; 6]);
    }
}
