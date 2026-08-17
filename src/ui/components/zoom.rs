use eframe::egui;

use crate::localization::LanguageId;
use crate::ui::tokens::CONTROL_HEIGHT;

pub(crate) fn zoom_control(
    ui: &mut egui::Ui,
    zoom: &mut f32,
    levels: &[f32],
    reset_value: f32,
    language: LanguageId,
) -> bool {
    debug_assert!(!levels.is_empty());
    let mut level = nearest_zoom_level_index(*zoom, levels) as u32;
    let slider = ui
        .add_sized(
            [120.0, CONTROL_HEIGHT],
            egui::Slider::new(&mut level, 0..=levels.len().saturating_sub(1) as u32)
                .integer()
                .show_value(false),
        )
        .on_hover_text(language.text("Use the mouse wheel over the canvas to zoom"));
    if slider.changed() {
        *zoom = levels[level as usize];
    }
    let reset = ui
        .add_sized(
            [54.0, CONTROL_HEIGHT],
            egui::Button::new(zoom_multiplier_label(*zoom)),
        )
        .on_hover_text(language.text("Reset zoom to 1x"));
    if reset.clicked() && *zoom != reset_value {
        *zoom = reset_value;
    }
    reset.clicked()
}

pub(crate) fn step_zoom(zoom: &mut f32, levels: &[f32], direction: f32) -> bool {
    if levels.is_empty() || direction == 0.0 {
        return false;
    }
    let current = nearest_zoom_level_index(*zoom, levels);
    let next = if direction > 0.0 {
        (current + 1).min(levels.len() - 1)
    } else {
        current.saturating_sub(1)
    };
    if next == current {
        return false;
    }
    *zoom = levels[next];
    true
}

fn nearest_zoom_level_index(zoom: f32, levels: &[f32]) -> usize {
    levels
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| (*left - zoom).abs().total_cmp(&(*right - zoom).abs()))
        .map_or(0, |(index, _)| index)
}

fn zoom_multiplier_label(zoom: f32) -> String {
    let value = format!("{zoom:.2}");
    format!("{}x", value.trim_end_matches('0').trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVELS: &[f32] = &[0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];

    #[test]
    fn zoom_steps_move_between_discrete_multipliers() {
        let mut zoom = 1.0;
        assert!(step_zoom(&mut zoom, LEVELS, 1.0));
        assert_eq!(zoom, 2.0);
        assert!(step_zoom(&mut zoom, LEVELS, -1.0));
        assert_eq!(zoom, 1.0);
    }

    #[test]
    fn zoom_steps_stop_at_the_supported_extremes() {
        let mut zoom = 0.25;
        assert!(!step_zoom(&mut zoom, LEVELS, -1.0));
        assert_eq!(zoom, 0.25);

        zoom = 16.0;
        assert!(!step_zoom(&mut zoom, LEVELS, 1.0));
        assert_eq!(zoom, 16.0);
    }

    #[test]
    fn zoom_labels_use_multiplier_notation() {
        assert_eq!(zoom_multiplier_label(0.25), "0.25x");
        assert_eq!(zoom_multiplier_label(0.5), "0.5x");
        assert_eq!(zoom_multiplier_label(1.0), "1x");
        assert_eq!(zoom_multiplier_label(16.0), "16x");
    }
}
