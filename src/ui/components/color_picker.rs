use eframe::egui;

use crate::ui::components::text_field::singleline;
use crate::ui::theme::{checkerboard_dark, checkerboard_light};
use crate::ui::tokens::{CONTROL_CORNER_RADIUS, CONTROL_HEIGHT};

const PICKER_WIDTH: f32 = 320.0;
const PICKER_OUTER_PADDING: f32 = 9.0;
const GRADIENT_STEPS: u32 = 36;

pub(crate) fn color_button(ui: &mut egui::Ui, color: &mut [u8; 4]) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(
        egui::vec2(ui.spacing().interact_size.x, CONTROL_HEIGHT),
        egui::Sense::click(),
    );
    let popup_id = response.id.with("popup");

    if ui.is_rect_visible(rect) {
        let radius = CONTROL_CORNER_RADIUS;
        let left = egui::Rect::from_min_max(rect.left_top(), rect.center_bottom());
        let right = egui::Rect::from_min_max(rect.center_top(), rect.right_bottom());
        ui.painter().rect_filled(
            left,
            egui::CornerRadius {
                nw: radius,
                sw: radius,
                ..Default::default()
            },
            checkerboard_dark(),
        );
        ui.painter().rect_filled(
            right,
            egui::CornerRadius {
                ne: radius,
                se: radius,
                ..Default::default()
            },
            checkerboard_light(),
        );
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(radius),
            egui::Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]),
        );
    }

    egui::Popup::menu(&response)
        .id(popup_id)
        .width(PICKER_WIDTH)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            if rgba_color_picker(ui, color) {
                response.mark_changed();
            }
        });
    response
}

pub(crate) fn color_string_field(
    ui: &mut egui::Ui,
    value: &mut String,
    width: f32,
) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, CONTROL_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let mut color = parse_color(value).unwrap_or([255, 0, 255, 255]);
            let button_width = ui.spacing().interact_size.x;
            let text_width = (width - button_width - ui.spacing().item_spacing.x).max(1.0);
            let button = color_button(ui, &mut color);
            if button.changed() {
                *value = format_color(color);
            }
            button.union(ui.add(singleline(value).desired_width(text_width)))
        },
    )
    .inner
}

fn format_color([red, green, blue, alpha]: [u8; 4]) -> String {
    format!("#{red:02X}{green:02X}{blue:02X}{alpha:02X}")
}

fn parse_color(value: &str) -> Option<[u8; 4]> {
    let raw = value.trim().trim_start_matches('#');
    match raw.len() {
        8 => u32::from_str_radix(raw, 16).ok().map(|value| {
            [
                ((value >> 24) & 255) as u8,
                ((value >> 16) & 255) as u8,
                ((value >> 8) & 255) as u8,
                (value & 255) as u8,
            ]
        }),
        6 => u32::from_str_radix(raw, 16).ok().map(|value| {
            [
                ((value >> 16) & 255) as u8,
                ((value >> 8) & 255) as u8,
                (value & 255) as u8,
                255,
            ]
        }),
        _ => None,
    }
}

fn rgba_color_picker(ui: &mut egui::Ui, color: &mut [u8; 4]) -> bool {
    let original = *color;
    ui.set_min_width(PICKER_WIDTH);
    ui.horizontal(|ui| {
        ui.add_space(PICKER_OUTER_PADDING);
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - PICKER_OUTER_PADDING).max(1.0));

            let mut channels = *color;
            let mut numeric_changed = false;
            ui.horizontal(|ui| {
                let gap = ui.spacing().item_spacing.x * 3.0;
                let component_width = ((ui.available_width() - gap) / 4.0).max(1.0);
                for (prefix, channel) in ["R ", "G ", "B ", "A "]
                    .into_iter()
                    .zip(channels.iter_mut())
                {
                    numeric_changed |= ui
                        .add_sized(
                            [component_width, CONTROL_HEIGHT],
                            egui::DragValue::new(channel).speed(0.5).prefix(prefix),
                        )
                        .changed();
                }
            });
            if numeric_changed {
                *color = channels;
            }

            let mut hsva = egui::ecolor::Hsva::from(egui::Color32::from_rgba_unmultiplied(
                color[0], color[1], color[2], color[3],
            ));
            let control_width = ui.available_width();
            let hue = hsva.h;
            let color_area = color_slider_2d(
                ui,
                egui::vec2(control_width, control_width),
                &mut hsva.s,
                &mut hsva.v,
                |s, v| {
                    egui::Color32::from(egui::ecolor::Hsva {
                        h: hue,
                        s,
                        v,
                        a: 1.0,
                    })
                },
            );
            let hue_slider = color_slider_1d(ui, control_width, &mut hsva.h, |h| {
                egui::Color32::from(egui::ecolor::Hsva {
                    h,
                    s: 1.0,
                    v: 1.0,
                    a: 1.0,
                })
            });
            let [red, green, blue, _] = hsva.to_srgba_unmultiplied();
            let alpha_slider = color_slider_1d(ui, control_width, &mut hsva.a, |alpha| {
                egui::Color32::from_rgba_unmultiplied(
                    red,
                    green,
                    blue,
                    (alpha * 255.0).round() as u8,
                )
            });
            if color_area.changed() || hue_slider.changed() || alpha_slider.changed() {
                *color = egui::Color32::from(hsva).to_srgba_unmultiplied();
            }
        });
    });
    *color != original
}

fn color_slider_1d(
    ui: &mut egui::Ui,
    width: f32,
    value: &mut f32,
    color_at: impl Fn(f32) -> egui::Color32,
) -> egui::Response {
    let size = egui::vec2(width, CONTROL_HEIGHT);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    if let Some(pointer) = response.interact_pointer_pos() {
        let previous = *value;
        *value = egui::remap_clamp(pointer.x, rect.left()..=rect.right(), 0.0..=1.0);
        if *value != previous {
            response.mark_changed();
        }
    }
    if ui.is_rect_visible(rect) {
        paint_checkers(ui.painter(), rect);
        let mut mesh = egui::Mesh::default();
        for index in 0..=GRADIENT_STEPS {
            let amount = index as f32 / GRADIENT_STEPS as f32;
            let x = egui::lerp(rect.left()..=rect.right(), amount);
            mesh.colored_vertex(egui::pos2(x, rect.top()), color_at(amount));
            mesh.colored_vertex(egui::pos2(x, rect.bottom()), color_at(amount));
            if index < GRADIENT_STEPS {
                mesh.add_triangle(index * 2, index * 2 + 1, index * 2 + 2);
                mesh.add_triangle(index * 2 + 1, index * 2 + 2, index * 2 + 3);
            }
        }
        ui.painter().add(egui::Shape::mesh(mesh));
        ui.painter().rect_stroke(
            rect,
            0.0,
            ui.style().interact(&response).bg_stroke,
            egui::StrokeKind::Inside,
        );

        let x = egui::lerp(rect.left()..=rect.right(), *value);
        let radius = rect.height() / 4.0;
        let picked = color_at(*value);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(x, rect.center().y),
                egui::pos2(x + radius, rect.bottom()),
                egui::pos2(x - radius, rect.bottom()),
            ],
            picked,
            egui::Stroke::new(
                ui.style().interact(&response).fg_stroke.width,
                contrast_color(picked),
            ),
        ));
    }
    response
}

fn color_slider_2d(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    horizontal: &mut f32,
    vertical: &mut f32,
    color_at: impl Fn(f32, f32) -> egui::Color32,
) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    if let Some(pointer) = response.interact_pointer_pos() {
        let previous = (*horizontal, *vertical);
        *horizontal = egui::remap_clamp(pointer.x, rect.left()..=rect.right(), 0.0..=1.0);
        *vertical = egui::remap_clamp(pointer.y, rect.bottom()..=rect.top(), 0.0..=1.0);
        if (*horizontal, *vertical) != previous {
            response.mark_changed();
        }
    }
    if ui.is_rect_visible(rect) {
        let mut mesh = egui::Mesh::default();
        for horizontal_index in 0..=GRADIENT_STEPS {
            for vertical_index in 0..=GRADIENT_STEPS {
                let x_amount = horizontal_index as f32 / GRADIENT_STEPS as f32;
                let y_amount = vertical_index as f32 / GRADIENT_STEPS as f32;
                let x = egui::lerp(rect.left()..=rect.right(), x_amount);
                let y = egui::lerp(rect.bottom()..=rect.top(), y_amount);
                mesh.colored_vertex(egui::pos2(x, y), color_at(x_amount, y_amount));
                if horizontal_index < GRADIENT_STEPS && vertical_index < GRADIENT_STEPS {
                    let row = GRADIENT_STEPS + 1;
                    let top_left = vertical_index * row + horizontal_index;
                    mesh.add_triangle(top_left, top_left + 1, top_left + row);
                    mesh.add_triangle(top_left + 1, top_left + row, top_left + row + 1);
                }
            }
        }
        ui.painter().add(egui::Shape::mesh(mesh));
        let visuals = ui.style().interact(&response);
        ui.painter()
            .rect_stroke(rect, 0.0, visuals.bg_stroke, egui::StrokeKind::Inside);
        let picked = color_at(*horizontal, *vertical);
        ui.painter().add(egui::epaint::CircleShape {
            center: egui::pos2(
                egui::lerp(rect.left()..=rect.right(), *horizontal),
                egui::lerp(rect.bottom()..=rect.top(), *vertical),
            ),
            radius: 8.0,
            fill: picked,
            stroke: egui::Stroke::new(visuals.fg_stroke.width, contrast_color(picked)),
        });
    }
    response
}

fn paint_checkers(painter: &egui::Painter, rect: egui::Rect) {
    let checker_size = rect.height() / 2.0;
    painter.rect_filled(rect, 0.0, checkerboard_dark());
    let columns = (rect.width() / checker_size).ceil() as usize;
    for column in 0..columns {
        let top = column % 2 == 0;
        let tile = egui::Rect::from_min_size(
            egui::pos2(
                rect.left() + column as f32 * checker_size,
                if top { rect.top() } else { rect.center().y },
            ),
            egui::vec2(checker_size, checker_size),
        )
        .intersect(rect);
        painter.rect_filled(tile, 0.0, checkerboard_light());
    }
}

fn contrast_color(color: egui::Color32) -> egui::Color32 {
    let brightness =
        (u16::from(color.r()) * 3 + u16::from(color.g()) * 6 + u16::from(color.b())) / 10;
    if brightness < 128 {
        egui::Color32::WHITE
    } else {
        egui::Color32::BLACK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_field_respects_its_total_width_and_control_height() {
        let context = egui::Context::default();
        crate::ui::theme::configure_style(&context, crate::localization::LanguageId::English);
        let mut size = egui::Vec2::ZERO;
        let _ = context.run_ui(egui::RawInput::default(), |ui| {
            let mut value = "#FFFFFFFF".to_owned();
            size = color_string_field(ui, &mut value, 240.0).rect.size();
        });

        assert_eq!(size, egui::vec2(240.0, CONTROL_HEIGHT));
    }

    #[test]
    fn parses_rgb_and_rgba_strings() {
        assert_eq!(parse_color("#112233"), Some([0x11, 0x22, 0x33, 0xFF]));
        assert_eq!(parse_color("#11223380"), Some([0x11, 0x22, 0x33, 0x80]));
        assert_eq!(parse_color("invalid"), None);
        assert_eq!(format_color([0x11, 0x22, 0x33, 0x80]), "#11223380");
    }
}
