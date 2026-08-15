use eframe::egui;

use crate::ui::components::expression_button::expression_button;
use crate::ui::components::helper_field::helper_preview_field;
use crate::ui::tokens::CONTROL_HEIGHT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompoundFieldAction {
    None,
    Open,
    Remove,
}

pub(crate) fn expression_or_value(
    ui: &mut egui::Ui,
    id: egui::Id,
    available_width: f32,
    expression_preview: Option<(&str, egui::Align)>,
    helper_name: &str,
    render_value: impl FnOnce(&mut egui::Ui, f32),
) -> CompoundFieldAction {
    if let Some((preview, align)) = expression_preview {
        let action = helper_preview_field(
            ui,
            id.with("preview"),
            preview,
            available_width,
            true,
            helper_name,
            align,
        );
        if action.remove {
            CompoundFieldAction::Remove
        } else if action.open {
            CompoundFieldAction::Open
        } else {
            CompoundFieldAction::None
        }
    } else {
        let width = (available_width - CONTROL_HEIGHT - ui.spacing().item_spacing.x).max(64.0);
        render_value(ui, width);
        if expression_button(ui, false)
            .on_hover_text(format!("Use {helper_name}"))
            .clicked()
        {
            CompoundFieldAction::Open
        } else {
            CompoundFieldAction::None
        }
    }
}
