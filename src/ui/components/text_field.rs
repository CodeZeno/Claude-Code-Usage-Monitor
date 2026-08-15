use eframe::egui;
use lucide_icons::Icon as LucideIcon;

use crate::localization::LanguageId;
use crate::ui::components::icon::icon_button;
use crate::ui::tokens::CONTROL_HEIGHT;

/// A single-line text field that always occupies the shared control height.
pub(crate) struct SinglelineField<'a> {
    widget: egui::TextEdit<'a>,
    desired_width: f32,
}

pub(crate) fn singleline<'a>(text: &'a mut dyn egui::TextBuffer) -> SinglelineField<'a> {
    SinglelineField {
        widget: egui::TextEdit::singleline(text)
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(8, 6)),
        desired_width: f32::INFINITY,
    }
}

impl<'a> SinglelineField<'a> {
    pub(crate) fn desired_width(mut self, width: f32) -> Self {
        self.desired_width = width;
        self.widget = self.widget.desired_width(width);
        self
    }

    pub(crate) fn id(mut self, id: egui::Id) -> Self {
        self.widget = self.widget.id(id);
        self
    }

    pub(crate) fn hint_text(mut self, hint_text: impl Into<egui::WidgetText>) -> Self {
        self.widget = self.widget.hint_text(hint_text);
        self
    }

    pub(crate) fn horizontal_align(mut self, align: egui::Align) -> Self {
        self.widget = self.widget.horizontal_align(align);
        self
    }

    pub(crate) fn show(self, ui: &mut egui::Ui) -> egui::text_edit::TextEditOutput {
        let width = self.desired_width.min(ui.available_width()).max(1.0);
        let layout = egui::Layout::centered_and_justified(ui.layout().main_dir());
        ui.allocate_ui_with_layout(egui::vec2(width, CONTROL_HEIGHT), layout, |ui| {
            self.widget.show(ui)
        })
        .inner
    }
}

impl egui::Widget for SinglelineField<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        self.show(ui).response.response
    }
}

pub(crate) fn name_editor(
    ui: &mut egui::Ui,
    name: &mut String,
    id: egui::Id,
    language: LanguageId,
) {
    name_editor_with_prefix(ui, name, id, "", language);
}

pub(crate) fn name_editor_with_prefix(
    ui: &mut egui::Ui,
    name: &mut String,
    id: egui::Id,
    prefix: &str,
    language: LanguageId,
) {
    let editing_id = id.with("active");
    let focus_id = id.with("focus");
    let edit_id = id.with("text");
    let draft_id = id.with("draft");
    let editing = ui.data_mut(|data| data.get_temp::<bool>(editing_id).unwrap_or(false));
    let width = ui.available_width().max(1.0);

    if editing {
        let mut draft = ui
            .data_mut(|data| data.get_temp::<String>(draft_id))
            .unwrap_or_else(|| name.clone());
        let (response, save_clicked) = ui
            .horizontal(|ui| {
                if !prefix.is_empty() {
                    ui.label(
                        egui::RichText::new(prefix)
                            .size(16.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                }
                let gap = ui.spacing().item_spacing.x;
                let text_width = (ui.available_width() - CONTROL_HEIGHT - gap).max(1.0);
                let response = ui.add_sized(
                    [text_width, CONTROL_HEIGHT],
                    singleline(&mut draft).id(edit_id),
                );
                let save_clicked = icon_button(ui, LucideIcon::Save, false)
                    .on_hover_text(language.text("Save name"))
                    .clicked();
                (response, save_clicked)
            })
            .inner;
        if ui.data_mut(|data| data.remove_temp::<bool>(focus_id).is_some()) {
            response.request_focus();
        }
        let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
        if save_clicked || enter_pressed {
            *name = draft;
            ui.data_mut(|data| {
                data.remove::<bool>(editing_id);
                data.remove::<String>(draft_id);
            });
            ui.memory_mut(|memory| memory.surrender_focus(edit_id));
        } else {
            ui.data_mut(|data| data.insert_temp(draft_id, draft));
        }
        return;
    }

    let (rect, row_response) =
        ui.allocate_exact_size(egui::vec2(width, CONTROL_HEIGHT), egui::Sense::hover());
    let gap = ui.spacing().item_spacing.x;
    let text_width = (rect.width() - CONTROL_HEIGHT - gap).max(1.0);
    let heading_color = egui::Color32::WHITE;
    let display_name = format!("{prefix}{name}");
    let galley = egui::WidgetText::from(
        egui::RichText::new(display_name)
            .size(16.0)
            .strong()
            .color(heading_color),
    )
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        text_width,
        egui::TextStyle::Body,
    );
    let text_rect = egui::Align2::LEFT_CENTER.align_size_within_rect(
        galley.size(),
        egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.left() + text_width, rect.bottom()),
        ),
    );
    ui.painter().galley(text_rect.min, galley, heading_color);

    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(
            (text_rect.right() + gap).min(rect.right() - CONTROL_HEIGHT),
            rect.top(),
        ),
        egui::vec2(CONTROL_HEIGHT, CONTROL_HEIGHT),
    );
    let text_response = ui
        .interact(
            egui::Rect::from_min_max(rect.left_top(), egui::pos2(icon_rect.left(), rect.bottom())),
            id.with("name"),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::Text)
        .on_hover_text(language.text("Edit name"));
    let show_icon =
        (row_response.hovered() || text_response.hovered() || ui.rect_contains_pointer(icon_rect))
            && ui.is_enabled();
    let edit_clicked = if show_icon {
        let response = ui
            .interact(icon_rect, id.with("button"), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(language.text("Edit name"));
        crate::ui::components::icon::paint_centered_icon(
            ui,
            icon_rect,
            LucideIcon::SquarePen,
            16.0,
            ui.style().interact(&response).text_color(),
        );
        response.clicked()
    } else {
        false
    };
    if text_response.clicked() || edit_clicked {
        ui.data_mut(|data| {
            data.insert_temp(editing_id, true);
            data.insert_temp(focus_id, true);
            data.insert_temp(draft_id, name.clone());
        });
        ui.ctx().request_repaint();
    }
}

pub(crate) fn inline_rename(
    ui: &mut egui::Ui,
    name: &mut String,
    id: egui::Id,
    width: f32,
    editable: bool,
    language: LanguageId,
) -> (egui::Response, bool) {
    if !editable {
        return (name_label(ui, name, width, egui::Sense::click()), false);
    }
    let editing_id = id.with("active");
    let select_all_id = id.with("select-all");
    let editing = ui.data_mut(|data| data.get_temp::<bool>(editing_id).unwrap_or(false));
    if editing {
        let mut output = singleline(name)
            .id(id)
            .desired_width(width)
            .horizontal_align(egui::Align::Min)
            .show(ui);
        if ui.data_mut(|data| data.remove_temp::<bool>(select_all_id).is_some()) {
            output.response.request_focus();
            let end = egui::text::CCursor::new(name.chars().count());
            output
                .state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    end,
                )));
            output.state.store(ui.ctx(), id);
        }
        let finish =
            output.response.lost_focus() || ui.input(|input| input.key_pressed(egui::Key::Enter));
        if finish {
            ui.data_mut(|data| data.remove::<bool>(editing_id));
            ui.memory_mut(|memory| memory.surrender_focus(id));
        }
        let response = output.response.response;
        let changed = response.changed();
        (response, changed)
    } else {
        let response = name_label(ui, name, width, egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::Text)
            .on_hover_text(language.text("Double-click to rename"));
        if response.double_clicked() {
            ui.data_mut(|data| {
                data.insert_temp(editing_id, true);
                data.insert_temp(select_all_id, true);
            });
            ui.ctx().request_repaint();
        }
        (response, false)
    }
}

fn name_label(ui: &mut egui::Ui, name: &str, width: f32, sense: egui::Sense) -> egui::Response {
    let width = width.max(1.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, CONTROL_HEIGHT), sense);
    let galley = egui::WidgetText::from(name.to_owned()).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        width,
        egui::TextStyle::Body,
    );
    let text_rect = egui::Align2::LEFT_CENTER.align_size_within_rect(galley.size(), rect);
    ui.painter().galley(
        text_rect.min,
        galley,
        ui.style().interact(&response).text_color(),
    );
    response
}
