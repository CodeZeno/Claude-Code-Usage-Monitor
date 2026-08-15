use eframe::egui;

use crate::ui::theme::{accent, anchor_idle_fill, anchor_outline};
use crate::ui::tokens::{ANCHOR_CELL_GAP, ANCHOR_CELL_SIZE, CONTROL_HEIGHT};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AnchorPoint {
    pub(crate) column: usize,
    pub(crate) row: usize,
}

impl AnchorPoint {
    pub(crate) const fn new(column: usize, row: usize) -> Self {
        Self { column, row }
    }
}

/// A domain-neutral three-by-three anchor selector.
pub(crate) struct AnchorPointPicker<'a> {
    selected: &'a mut AnchorPoint,
    width: f32,
}

impl<'a> AnchorPointPicker<'a> {
    pub(crate) fn new(selected: &'a mut AnchorPoint) -> Self {
        Self {
            selected,
            width: control_size(),
        }
    }

    pub(crate) fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub(crate) fn show(self, ui: &mut egui::Ui) -> egui::Response {
        ui.allocate_ui_with_layout(
            egui::vec2(self.width, CONTROL_HEIGHT),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| show_grid(ui, self.selected),
        )
        .inner
    }
}

fn show_grid(ui: &mut egui::Ui, selected: &mut AnchorPoint) -> egui::Response {
    let size = control_size();
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let response_hovered = response.hovered();
    let hovered = ui
        .input(|input| input.pointer.hover_pos())
        .filter(|position| response_hovered && rect.contains(*position))
        .map(|position| point_cell(rect, position));

    let cell_rect = |column: usize, row: usize| {
        egui::Rect::from_min_size(
            egui::pos2(
                rect.left() + column as f32 * (ANCHOR_CELL_SIZE + ANCHOR_CELL_GAP),
                rect.top() + row as f32 * (ANCHOR_CELL_SIZE + ANCHOR_CELL_GAP),
            ),
            egui::vec2(ANCHOR_CELL_SIZE, ANCHOR_CELL_SIZE),
        )
    };
    let outline = anchor_outline();
    let connector = egui::Stroke::new(1.0, outline);
    for row in [0, 2] {
        for column in 0..2 {
            ui.painter().line_segment(
                [
                    cell_rect(column, row).center(),
                    cell_rect(column + 1, row).center(),
                ],
                connector,
            );
        }
    }
    for column in [0, 2] {
        for row in 0..2 {
            ui.painter().line_segment(
                [
                    cell_rect(column, row).center(),
                    cell_rect(column, row + 1).center(),
                ],
                connector,
            );
        }
    }
    for row in 0..3 {
        for column in 0..3 {
            let cell = cell_rect(column, row);
            let is_selected = *selected == AnchorPoint::new(column, row);
            let is_hovered = hovered == Some(AnchorPoint::new(column, row));
            ui.painter().rect_filled(
                cell,
                0.0,
                if is_selected {
                    accent()
                } else if is_hovered {
                    outline
                } else {
                    anchor_idle_fill()
                },
            );
            ui.painter().rect_stroke(
                cell,
                0.0,
                egui::Stroke::new(
                    1.0,
                    if is_selected {
                        egui::Color32::WHITE
                    } else {
                        outline
                    },
                ),
                egui::StrokeKind::Inside,
            );
        }
    }

    if let Some(point) = response
        .interact_pointer_pos()
        .filter(|_| response.clicked())
        .map(|position| point_cell(rect, position))
    {
        if *selected != point {
            *selected = point;
            response.mark_changed();
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn control_size() -> f32 {
    ANCHOR_CELL_SIZE * 3.0 + ANCHOR_CELL_GAP * 2.0
}

fn point_cell(rect: egui::Rect, position: egui::Pos2) -> AnchorPoint {
    let column = (((position.x - rect.left()) / rect.width()) * 3.0)
        .floor()
        .clamp(0.0, 2.0) as usize;
    let row = (((position.y - rect.top()) / rect.height()) * 3.0)
        .floor()
        .clamp(0.0, 2.0) as usize;
    AnchorPoint::new(column, row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_positions_map_to_the_nine_anchor_points() {
        let rect = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(40.0, 50.0));

        assert_eq!(point_cell(rect, rect.left_top()), AnchorPoint::new(0, 0));
        assert_eq!(point_cell(rect, rect.center()), AnchorPoint::new(1, 1));
        assert_eq!(
            point_cell(rect, rect.right_bottom()),
            AnchorPoint::new(2, 2)
        );
    }
}
