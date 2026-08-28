use eframe::egui;

pub(crate) struct Modal<'a> {
    title: &'a str,
    id: egui::Id,
    width: f32,
    fixed_height: Option<f32>,
}

impl<'a> Modal<'a> {
    pub(crate) fn new(title: &'a str, id: impl std::hash::Hash + std::fmt::Debug) -> Self {
        Self {
            title,
            id: egui::Id::new(id),
            width: 360.0,
            fixed_height: None,
        }
    }

    pub(crate) fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub(crate) fn fixed_height(mut self, height: f32) -> Self {
        self.fixed_height = Some(height);
        self
    }

    pub(crate) fn show<R>(self, context: &egui::Context, body: impl FnOnce(&mut egui::Ui) -> R) {
        let mut window = egui::Window::new(self.title)
            .id(self.id)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .collapsible(false)
            .resizable(false)
            .default_width(self.width)
            .min_width(self.width)
            .max_width(self.width);
        if let Some(height) = self.fixed_height {
            window = window.fixed_size(egui::vec2(self.width, height));
        }
        let _ = window.show(context, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_modal_content_cannot_expand_past_its_title_bar() {
        let context = egui::Context::default();
        let mut modal_rect = None;
        for time in [0.0, 1.0] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                time: Some(time),
                ..Default::default()
            };
            context.begin_pass(input);
            Modal::new("Dialog", "fixed-modal-size-test")
                .width(310.0)
                .fixed_height(110.0)
                .show(&context, |ui| {
                    ui.label("Content");
                });
            modal_rect = context.memory(|memory| memory.area_rect("fixed-modal-size-test"));
            let mut output = context.end_pass();
            output.textures_delta.clear();
        }

        let modal_rect = modal_rect.expect("modal should be visible");
        assert_eq!(modal_rect.width(), 310.0);
    }
}
