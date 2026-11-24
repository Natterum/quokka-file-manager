//! Draft column configuration with mouse drag and drop; persisted only on Apply.
use super::*;
use crate::config::{ColumnLayout, COLUMN_COUNT};

const COLUMN_ROW_H: f32 = 48.0;
const COLUMN_ROW_GAP: f32 = 6.0;

#[derive(PartialEq)]
pub(super) enum Outcome { Open, Apply, Cancel }

pub(super) struct ColumnsDialog {
    pub pane: usize,
    pub layout: ColumnLayout,
    pub from_settings: bool,
    dragging: Option<usize>,
}

impl ColumnsDialog {
    pub fn new(pane: usize, layout: ColumnLayout, from_settings: bool) -> Self {
        Self { pane, layout, from_settings, dragging: None }
    }

    pub fn show(&mut self, ctx: &egui::Context, esc: bool, enter: bool) -> Outcome {
        let mut outcome = if esc { Outcome::Cancel } else { Outcome::Open };
        let width = 820.0_f32.min(ctx.content_rect().width() - 64.0);
        let body_h = (ctx.content_rect().height() - 210.0).clamp(260.0, 520.0);
        flat_dialog(ctx, "columns-edit", width, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let (header, _) = ui.allocate_exact_size(vec2(width, 82.0), Sense::hover());
            let icon = Rect::from_min_size(header.min + vec2(24.0, 21.0), vec2(40.0, 40.0));
            ui.painter().rect_filled(icon, rad(12.0), BLUE_SOFT());
            icons::glyph(ui.painter(), icon.shrink(10.0), Glyph::Columns, BLUE);
            ui.painter().text(header.min + vec2(78.0, 29.0), Align2::LEFT_CENTER,
                tr!("Настройка колонок", "Configure columns"), semibold(19.0), TEXT());
            ui.painter().text(header.min + vec2(78.0, 53.0), Align2::LEFT_CENTER,
                if self.pane == 0 { tr!("Левая панель · состав и порядок", "Left pane · visibility and order") }
                else { tr!("Правая панель · состав и порядок", "Right pane · visibility and order") }, regular(12.0), MUTED());
            ui.painter().hline(header.x_range(), header.bottom(), Stroke::new(1.0, BORDER()));

            let (body, _) = ui.allocate_exact_size(vec2(width, body_h), Sense::hover());
            let left = Rect::from_min_size(body.min, vec2(width * 0.56, body_h));
            let right = Rect::from_min_max(pos2(left.right(), body.top()), body.max);
            ui.painter().rect_filled(right, 0.0, BG());
            ui.painter().vline(left.right(), body.y_range(), Stroke::new(1.0, BORDER()));
            place(ui, egui::UiBuilder::new().max_rect(left.shrink2(vec2(24.0, 18.0))), |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr!("Колонки", "Columns")).font(semibold(13.5)).color(TEXT()));
                    let count = self.layout.visible.iter().filter(|v| **v).count();
                    egui::Frame::NONE.fill(BLUE_SOFT()).corner_radius(rad(5.0)).inner_margin(Margin::symmetric(7, 2)).show(ui, |ui| {
                        ui.label(RichText::new(format!("{count} / {COLUMN_COUNT}")).size(10.5).color(BLUE));
                    });
                });
                ui.label(RichText::new(tr!("Перетащите строки, чтобы изменить порядок", "Drag rows to change the order")).size(11.0).color(MUTED()));
                ui.add_space(6.0);
                egui::ScrollArea::vertical().id_salt("columns-list")
                    .max_height((body_h - 90.0).max(100.0)).auto_shrink([false, false])
                    .show(ui, |ui| { self.rows(ui); });
            });
            place(ui, egui::UiBuilder::new().max_rect(right.shrink2(vec2(22.0, 20.0))), |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                ui.label(RichText::new(tr!("Предпросмотр", "Preview")).font(semibold(13.5)).color(TEXT()));
                ui.label(RichText::new(tr!("Так колонки расположатся в панели", "How columns will appear in the pane")).size(11.0).color(MUTED()));
                ui.add_space(6.0);
                self.preview(ui);
                ui.add_space(10.0);
                egui::Frame::NONE.fill(BLUE_SOFT()).corner_radius(rad(9.0)).inner_margin(Margin::same(12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(tr!("Настройте под себя", "Make it yours")).font(semibold(11.5)).color(BLUE));
                    ui.add_space(5.0);
                    ui.label(RichText::new(tr!("Флажок включает колонку. Порядок строк сверху вниз задаёт порядок слева направо.", "Check a column to show it. The top-to-bottom order becomes left to right.")).size(11.0).color(TEXT_SOFT()));
                });
                ui.add_space(4.0);
                ui.label(RichText::new(tr!("«Имя» всегда включено. При сужении панели меняется только его ширина.", "Name is always enabled. Only its width changes when the pane gets narrower.")).size(11.0).color(MUTED()));
            });

            let (footer, _) = ui.allocate_exact_size(vec2(width, 66.0), Sense::hover());
            ui.painter().hline(footer.x_range(), footer.top(), Stroke::new(1.0, BORDER()));
            place(ui, egui::UiBuilder::new().max_rect(footer.shrink2(vec2(24.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)), |ui| {
                if jb_btn(ui, tr!("Сбросить", "Reset"), false).on_hover_text(tr!("Вернуть стандартные колонки и их порядок", "Restore the default columns and order")).clicked() {
                    self.layout = ColumnLayout::default();
                    self.dragging = None;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if jb_btn(ui, tr!("Применить", "Apply"), true).clicked() { outcome = Outcome::Apply; }
                    ui.add_space(8.0);
                    if jb_btn(ui, tr!("Отмена", "Cancel"), false).clicked() { outcome = Outcome::Cancel; }
                });
            });
        });
        if esc { Outcome::Cancel }
        else if enter && self.dragging.is_none() { Outcome::Apply }
        else { outcome }
    }

    fn preview(&self, ui: &mut Ui) {
        let width = ui.available_width();
        egui::Frame::NONE.fill(CARD()).stroke(Stroke::new(1.0, BORDER())).corner_radius(rad(8.0))
            .inner_margin(Margin::same(1)).show(ui, |ui| {
            ui.set_width(width - 2.0);
            egui::ScrollArea::horizontal().id_salt("columns-preview").max_height(132.0)
                .auto_shrink([false, false]).show(ui, |ui| {
                let enabled: Vec<_> = self.layout.order.into_iter().filter(|id| self.layout.visible[*id]).collect();
                let total: f32 = enabled.iter().map(|id| if *id == 0 { 140.0 } else { 118.0 }).sum();
                let (rect, _) = ui.allocate_exact_size(vec2(total, 118.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(Rect::from_min_size(rect.min, vec2(total, 30.0)), 0.0, HEADER_BG());
                for row in 0..3 {
                    let y = rect.top() + 30.0 + row as f32 * 29.0;
                    if row == 1 { p.rect_filled(Rect::from_min_size(pos2(rect.left(), y), vec2(total, 29.0)), 0.0, HOVER()); }
                    p.hline(rect.x_range(), y, Stroke::new(1.0, BORDER()));
                }
                let mut x = rect.left();
                for id in enabled {
                    let w = if id == 0 { 140.0 } else { 118.0 };
                    let hp = p.with_clip_rect(p.clip_rect().intersect(Rect::from_min_max(pos2(x, rect.top()), pos2(x + w, rect.bottom()))));
                    let title = elide_with_suffix(&hp, column_label(id), &regular(10.0), w - 20.0, "...");
                    hp.text(pos2(x + 10.0, rect.top() + 15.0), Align2::LEFT_CENTER, title, regular(10.0), TEXT_SOFT());
                    for row in 0..3 {
                        let value = Self::sample(id, row);
                        let value = elide_right(&hp, value, &regular(10.0), w - 20.0);
                        hp.text(pos2(x + 10.0, rect.top() + 44.5 + row as f32 * 29.0), Align2::LEFT_CENTER, value, regular(10.0), if id == 0 { TEXT() } else { MUTED() });
                    }
                    x += w;
                    p.vline(x, rect.y_range(), Stroke::new(1.0, BORDER()));
                }
            });
        });
        ui.label(RichText::new(tr!("Пример списка файлов · прокрутка по горизонтали", "Sample file list · scroll horizontally")).size(10.0).color(MUTED()));
    }

    fn sample(id: usize, row: usize) -> &'static str {
        match id {
            0 => ["Documents", "Report.pdf", "Photo.jpg"][row],
            1 | 4 | 5 => ["18.11.2026  09:41", "19.11.2026  14:20", "20.11.2026  12:08"][row],
            2 => ["—", tr!("2,4 МБ", "2.4 MB"), tr!("840 КБ", "840 KB")][row],
            3 => [tr!("Папка", "Folder"), "PDF", "JPEG"][row],
            6 => ["—", "pdf", "jpg"][row],
            _ => ["D", "A", "A"][row],
        }
    }

    fn rows(&mut self, ui: &mut Ui) {
        let mut rects = Vec::with_capacity(COLUMN_COUNT);
        ui.spacing_mut().item_spacing.y = COLUMN_ROW_GAP;
        for (position, id) in self.layout.order.into_iter().enumerate() {
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), COLUMN_ROW_H), Sense::hover());
            let response = ui.interact(row, Id::unique(("column-row", id)), Sense::drag());
            let selected = self.dragging == Some(position);
            let enabled = self.layout.visible[id];
            let p = ui.painter();
            p.rect(row, rad(8.0), if selected { BLUE_SOFT() } else if response.hovered() { HOVER() } else { CARD() },
                Stroke::new(1.0, if selected { BLUE } else { BORDER() }), eframe::egui::StrokeKind::Middle);
            // Six dots stay crisp with every font and make the drag handle explicit.
            for dx in [-3.0, 3.0] {
                for dy in [-6.0, 0.0, 6.0] {
                    p.circle_filled(pos2(row.left() + 16.0 + dx, row.center().y + dy), 1.25, if selected { BLUE } else { MUTED() });
                }
            }
            let tile = Rect::from_center_size(pos2(row.left() + 48.0, row.center().y), vec2(28.0, 28.0));
            let color = match id { 0 => BLUE, 1 | 4 | 5 => PURPLE, 2 => TEAL, _ => SLATE() };
            p.rect_filled(tile, rad(7.0), if enabled { mix(color, CARD(), 0.9) } else { INPUT_BG() });
            let glyph = match id { 0 | 6 => Glyph::Document, 2 => Glyph::Drive, 3 => Glyph::Columns, 7 => Glyph::Gear, _ => Glyph::Document };
            let ink = if enabled { color } else { MUTED() };
            if matches!(id, 1 | 4 | 5) {
                let calendar = tile.shrink(7.0);
                p.rect_stroke(calendar, rad(2.0), Stroke::new(1.2, ink), eframe::egui::StrokeKind::Middle);
                p.hline(calendar.x_range(), calendar.top() + 4.0, Stroke::new(1.2, ink));
                for x in [calendar.left() + 3.5, calendar.right() - 3.5] {
                    p.vline(x, (calendar.top() - 2.0)..=(calendar.top() + 1.5), Stroke::new(1.2, ink));
                }
            } else { icons::glyph(p, tile.shrink(7.0), glyph, ink); }
            p.text(pos2(row.left() + 72.0, row.center().y - 7.0), Align2::LEFT_CENTER,
                column_label(id), semibold(12.0), if enabled { TEXT() } else { TEXT_SOFT() });
            p.text(pos2(row.left() + 72.0, row.center().y + 10.0), Align2::LEFT_CENTER,
                if id == 0 { tr!("Обязательная колонка", "Always visible") } else { Self::sample(id, 1) }, regular(10.0), MUTED());
            if response.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::Grab); }
            if response.drag_started_by(egui::PointerButton::Primary) { self.dragging = Some(position); }
            let check = Rect::from_center_size(pos2(row.right() - 24.0, row.center().y), vec2(20.0, 20.0));
            let mut toggle = ui.interact(check.expand(4.0), Id::unique(("column-visible", id)), if id == 0 { Sense::hover() } else { Sense::click() });
            if toggle.clicked() {
                self.layout.visible[id] = !self.layout.visible[id];
                toggle.mark_changed();
            }
            toggle.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, id != 0, self.layout.visible[id], column_label(id)));
            let on = self.layout.visible[id];
            p.rect(check, rad(5.0), if on { if id == 0 { mix(BLUE, CARD(), 0.55) } else { BLUE } } else { CARD() },
                Stroke::new(1.0, if on { Color32::TRANSPARENT } else if toggle.hovered() { BLUE } else { FIELD_BORDER() }), eframe::egui::StrokeKind::Middle);
            if on {
                p.add(egui::Shape::line(vec![check.center() + vec2(-4.5, 0.0), check.center() + vec2(-1.0, 3.5), check.center() + vec2(5.0, -3.5)], Stroke::new(1.8, CARD())));
            }
            if id != 0 { toggle = toggle.on_hover_cursor(egui::CursorIcon::PointingHand); }
            toggle.on_hover_text(if id == 0 { tr!("Колонка «Имя» всегда включена", "Name is always enabled") } else { column_label(id) });
            if id == 7 {
                response.on_hover_text(tr!("R — только чтение · H — скрытый · S — системный · A — архивный · D — папка · L — ссылка · C — сжатый · E — зашифрованный", "R — read-only · H — hidden · S — system · A — archive · D — directory · L — link · C — compressed · E — encrypted"));
            }
            rects.push(row);
        }
        if let Some(from) = self.dragging {
            let (down, released, pointer) = ui.input(|i| (i.pointer.primary_down(), i.pointer.button_released(egui::PointerButton::Primary), i.pointer.interact_pos()));
            let bounds = rects[0].union(rects[COLUMN_COUNT - 1]);
            let target = pointer.filter(|p| bounds.expand(8.0).intersect(ui.clip_rect()).contains(*p))
                .map(|p| rects.iter().position(|r| p.y < r.center().y).unwrap_or(COLUMN_COUNT));
            if down {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                if let Some(target) = target {
                    let y = if target < COLUMN_COUNT { rects[target].top() - 2.0 } else { bounds.bottom() + 2.0 };
                    ui.painter().hline(bounds.x_range(), y, Stroke::new(2.5, BLUE));
                }
                if let Some(pointer) = pointer {
                    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("column-row-ghost")));
                    let ghost = Rect::from_min_size(pointer + vec2(14.0, 10.0), vec2(210.0, 40.0));
                    painter.rect(ghost, rad(9.0), CARD(), Stroke::new(1.0, BLUE), eframe::egui::StrokeKind::Middle);
                    painter.text(ghost.center(), Align2::CENTER_CENTER, column_label(self.layout.order[from]), regular(13.0), TEXT());
                }
                // Keep the destination reachable when the dialog is shorter than the list.
                if let Some(pointer) = pointer {
                    let clip = ui.clip_rect();
                    let delta = if pointer.y < clip.top() + 24.0 { 8.0 }
                        else if pointer.y > clip.bottom() - 24.0 { -8.0 } else { 0.0 };
                    ui.scroll_with_delta(vec2(0.0, delta));
                }
                ui.ctx().request_repaint();
            } else {
                if released {
                    if let Some(target) = target { self.layout.move_column(from, target); }
                }
                self.dragging = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, editor: &mut ColumnsDialog, events: Vec<egui::Event>) -> egui::Pos2 {
        let mut origin = egui::Pos2::ZERO;
        ctx.run_ui(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(600.0, 600.0))),
            events, ..Default::default()
        }, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                origin = ui.cursor().min;
                editor.rows(ui);
            });
        }).textures_delta.clear();
        origin
    }

    #[test]
    fn modal_fits_minimum_window_and_escape_cancels() {
        for size in [vec2(960.0, 600.0), vec2(1400.0, 860.0)] {
            let ctx = egui::Context::default();
            theme::apply(&ctx, &theme::Appearance::default());
            let mut editor = ColumnsDialog::new(1, ColumnLayout::default(), true);
            editor.layout.visible[4] = true;
            editor.layout.move_column(4, 0);
            let mut outcome = Outcome::Open;
            for frame in 0..4 {
                ctx.run_ui(egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                    time: Some(frame as f64), ..Default::default()
                }, |ui| { let ctx = &ui.ctx().clone(); outcome = editor.show(ctx, false, false); }).textures_delta.clear();
            }
            assert!(outcome == Outcome::Open);
            let bounds = ctx.memory(|m| m.area_rect(Id::unique("columns-edit"))).unwrap();
            assert!(bounds.width() < size.x && bounds.height() < size.y,
                "dialog {bounds:?} must fit viewport {size:?}");
            ctx.run_ui(egui::RawInput::default(), |ui| { let ctx = &ui.ctx().clone(); outcome = editor.show(ctx, true, false); }).textures_delta.clear();
            assert!(outcome == Outcome::Cancel);
        }
    }

    #[test]
    fn checkboxes_toggle_new_columns_without_starting_drag() {
        let ctx = egui::Context::default();
        theme::apply(&ctx, &theme::Appearance::default());
        let mut editor = ColumnsDialog::new(0, ColumnLayout::default(), false);
        let origin = frame(&ctx, &mut editor, vec![]);
        let click = |pos, pressed| egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE,
        };
        let created = origin + vec2(562.0, 4.0 * (COLUMN_ROW_H + COLUMN_ROW_GAP) + COLUMN_ROW_H / 2.0);
        frame(&ctx, &mut editor, vec![egui::Event::PointerMoved(created), click(created, true)]);
        frame(&ctx, &mut editor, vec![click(created, false)]);
        assert!(editor.layout.visible[4]);
        assert!(editor.dragging.is_none());
        let name = origin + vec2(562.0, COLUMN_ROW_H / 2.0);
        frame(&ctx, &mut editor, vec![egui::Event::PointerMoved(name), click(name, true)]);
        frame(&ctx, &mut editor, vec![click(name, false)]);
        assert!(editor.layout.visible[0]);
    }

    #[test]
    fn mouse_drag_reorders_rows_and_outside_drop_cancels() {
        let ctx = egui::Context::default();
        theme::apply(&ctx, &theme::Appearance::default());
        let mut editor = ColumnsDialog::new(0, ColumnLayout::default(), false);
        let origin = frame(&ctx, &mut editor, vec![]);
        let start = origin + vec2(15.0, COLUMN_ROW_H / 2.0);
        let end = origin + vec2(15.0, 7.0 * (COLUMN_ROW_H + COLUMN_ROW_GAP) + COLUMN_ROW_H - 2.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE,
        };
        frame(&ctx, &mut editor, vec![egui::Event::PointerMoved(start), button(start, true)]);
        frame(&ctx, &mut editor, vec![egui::Event::PointerMoved(end)]);
        frame(&ctx, &mut editor, vec![button(end, false)]);
        assert_eq!(editor.layout.order, [1, 2, 3, 4, 5, 6, 7, 0]);
        assert!(editor.dragging.is_none());
        let before = editor.layout.clone();
        let outside = origin + vec2(15.0, 500.0);
        frame(&ctx, &mut editor, vec![egui::Event::PointerMoved(start), button(start, true)]);
        frame(&ctx, &mut editor, vec![egui::Event::PointerMoved(outside)]);
        frame(&ctx, &mut editor, vec![button(outside, false)]);
        assert_eq!(editor.layout, before);
    }
}

